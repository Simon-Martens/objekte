//! JSON filter execution across memory, segments, grep, and source files.
use super::*;
use crate::{
    JsonFilter, JsonQuery, JsonQueryBackend, JsonQueryDependencies, JsonQueryResult,
    json_filter::TypedPlan,
};
use grep_searcher::{MmapChoice, SearcherBuilder, Sink, SinkMatch};
use std::io::{Seek, SeekFrom};


/// Search sink that stops after one regex match.
#[derive(Default)]
struct FirstMatch(bool);


impl Sink for FirstMatch {
    type Error = std::io::Error;

    fn matched(&mut self, _: &grep_searcher::Searcher, _: &SinkMatch<'_>) -> std::io::Result<bool> {
        self.0 = true;
        Ok(false)
    }
}


/// Per-file verification result and phase timings.
#[derive(Default)]
struct ScanOutcome {
    candidate: bool,
    matches: bool,
    source_bytes: u64,
    parse_nanos: u64,
    query_nanos: u64,
    file_open_nanos: u64,
    read_nanos: u64,
    regex_nanos: u64,
}


// An unsupported OR child can match outside every known posting, so its whole
// union is unbounded. An AND may still use the indexed children it knows.
fn segment_typed_ids(
    snapshot: &segment_cache::Snapshot,
    plan: &TypedPlan,
) -> Option<BTreeSet<String>> {
    match plan {
        TypedPlan::Term(field, operator, value) => {
            snapshot.json_filter_ids(field, *operator, value)
        }
        TypedPlan::And(parts) => {
            let mut combined: Option<BTreeSet<String>> = None;
            for part in parts {
                if let Some(ids) = segment_typed_ids(snapshot, part) {
                    combined = Some(match combined {
                        Some(old) => old.intersection(&ids).cloned().collect(),
                        None => ids,
                    });
                }
            }
            combined
        }
        TypedPlan::Or(parts) => {
            let mut combined = BTreeSet::new();
            for part in parts {
                combined.extend(segment_typed_ids(snapshot, part)?);
            }
            Some(combined)
        }
    }
}


impl Repository {
    /// Query schema-declared JSON paths in all registered collections, or an explicit scope.
    /// Ordering is (collection, id).
    pub fn query_json(&mut self, request: &JsonQuery) -> Result<JsonQueryResult> {
        let before = self.metrics.clone();
        let t = Instant::now();
        if request.limit == 0
            || request.limit > i64::MAX as usize
            || request.offset > i64::MAX as usize
        {
            return Err(Error::Request(
                "query limit must be positive and pagination must fit i64".into(),
            ));
        }
        let filter = JsonFilter::compile(&request.filter, &request.parameters)?;
        self.metrics.filter_compile_nanos += nanos(t);
        self.reload()?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect())
            .into_iter()
            .collect::<BTreeSet<_>>();
        let collections = names
            .iter()
            .map(|name| self.collection(name))
            .collect::<Result<Vec<_>>>()?;
        if request.collections.is_some()
            && let Some(c) = collections.iter().find(|c| !filter.follows_schema(c))
        {
            return Err(Error::Request(format!(
                "filter does not follow the current schema of collection {}",
                c.name
            )));
        }
        let searchable = collections
            .iter()
            .filter(|c| filter.follows_schema(c))
            .cloned()
            .collect::<Vec<_>>();
        let mut result = JsonQueryResult {
            records: Vec::new(),
            total: request.count.then_some(0),
            dependencies: JsonQueryDependencies {
                collections: names.into_iter().collect(),
                paths: filter.paths().to_vec(),
                revisions: BTreeMap::new(),
            },
            filter_hash: filter.hash().into(),
            metrics: Metrics::default(),
            query_plan: Vec::new(),
        };
        if !collections.is_empty() {
            match request.backend {
                JsonQueryBackend::Segments if self.options.cache != CacheMode::Segments => {
                    return Err(Error::Request(
                        "segment query backend requires the segment cache".into(),
                    ));
                }
                JsonQueryBackend::Segments if !searchable.is_empty() => {
                    self.query_json_segments(request, &filter, &searchable, &mut result)?
                }
                JsonQueryBackend::Grep if !searchable.is_empty() => {
                    self.query_json_grep(request, &filter, &searchable, &mut result)?
                }
                JsonQueryBackend::Auto | JsonQueryBackend::Bigram if !searchable.is_empty() => {
                    self.query_json_bigram(request, &filter, &searchable, &mut result)?
                }
                JsonQueryBackend::Direct if !searchable.is_empty() => {
                    self.query_json_direct(request, &filter, &searchable, &mut result)?
                }
                _ => {}
            }
        }
        result.metrics = self.metrics.since(&before);
        Ok(result)
    }

    fn query_json_segments(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut JsonQueryResult,
    ) -> Result<()> {
        let cardinality_risk = filter.needs_global_cardinality_check(collections);
        let prefilter = if cardinality_risk {
            None
        } else {
            filter.values_prefilter()
        };
        result.query_plan = vec![match &prefilter {
            Some((_, pattern)) => format!("parallel segment search {pattern}; Sonic verification"),
            None => "segment ID scan; Sonic verification".into(),
        }];
        let mut matched = 0;
        for c in collections {
            let sync_started = Instant::now();
            self.sync_segments(c, false)?;
            self.metrics.segment_sync_nanos += nanos(sync_started);
            let snapshot_started = Instant::now();
            let snapshot = self
                .segment_snapshot(c)?
                .ok_or_else(|| Error::Cache("segment snapshot disappeared".into()))?;
            self.metrics.segment_snapshot_nanos += nanos(snapshot_started);
            result
                .dependencies
                .revisions
                .insert(c.name.clone(), snapshot.manifest.revision);
            let actual_array_risk = filter
                .top_level_scalar_fields()
                .iter()
                .any(|field| snapshot.has_array_value(field));
            let index_started = Instant::now();
            let indexed = if cardinality_risk || actual_array_risk {
                None
            } else {
                filter
                    .typed_plan()
                    .as_ref()
                    .and_then(|plan| segment_typed_ids(&snapshot, plan))
            };
            self.metrics.segment_index_nanos += nanos(index_started);
            let mut candidates =
                if let Some((matcher, _)) = prefilter.as_ref().filter(|_| !actual_array_risk) {
                    let t = Instant::now();
                    let ids = snapshot.text_candidates(matcher)?;
                    self.metrics.grep_scans += snapshot.shards.len() as u64;
                    self.metrics.grep_candidates += ids.len() as u64;
                    self.metrics.grep_nanos += nanos(t);
                    ids
                } else {
                    let mut ids = indexed.as_ref().map_or_else(
                        || {
                            snapshot
                                .entries()
                                .map(|(id, _)| id.clone())
                                .collect::<Vec<_>>()
                        },
                        |ids| ids.iter().cloned().collect::<Vec<_>>(),
                    );
                    ids.sort();
                    ids
                };
            if let Some(indexed) = &indexed {
                candidates.retain(|id| indexed.contains(id));
            }
            let source = Source::open(&c.path)?;
            let verify_started = Instant::now();
            let outcomes = candidates
                .par_iter()
                .map(|id| -> Result<(bool, Metrics)> {
                    let entry = snapshot
                        .entry(id)
                        .ok_or_else(|| Error::Cache(format!("segment ID {id} missing")))?;
                    let mut metrics = Metrics::default();
                    let raw = Self::read_raw(&mut metrics, &source, id, Some(&entry.stamp))?;
                    let doc = Self::parse_value(&mut metrics, c, id, &raw.json, false);
                    let matches = if doc.state.status == RecordStatus::IncorrectJson {
                        false
                    } else {
                        let t = Instant::now();
                        let matches = filter.matches(&doc.data)?;
                        metrics.query_nanos += nanos(t);
                        matches
                    };
                    Ok((matches, metrics))
                })
                .collect::<Result<Vec<_>>>()?;
            self.metrics.segment_verify_nanos += nanos(verify_started);
            let materialize_started = Instant::now();
            for (id, (matches, metrics)) in candidates.into_iter().zip(outcomes) {
                self.metrics.merge(&metrics);
                if matches {
                    if matched >= request.offset && result.records.len() < request.limit {
                        let entry = snapshot
                            .entry(&id)
                            .ok_or_else(|| Error::Cache(format!("segment ID {id} missing")))?;
                        let raw = self.raw_from(&source, &id, Some(&entry.stamp))?;
                        let doc = self.parse(c, &id, &raw.json, true);
                        if doc.state.status == RecordStatus::IncorrectJson
                            || !filter.matches(&doc.data)?
                        {
                            return Err(Error::Unstable(c.path.join(format!("{id}.json"))));
                        }
                        result.records.push(Record {
                            collection: c.name.clone(),
                            id: id.clone(),
                            path: self.path(c, &id),
                            hash: raw.hash,
                            data: doc.data,
                            state: doc.state,
                            raw: None,
                        });
                    }
                    matched += 1;
                }
            }
            self.metrics.segment_materialize_nanos += nanos(materialize_started);
            source.finish()?;
        }
        result.total = request.count.then_some(matched);
        Ok(())
    }

    fn query_json_direct(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut JsonQueryResult,
    ) -> Result<()> {
        let mut matched = 0;
        for c in collections {
            let (source, files) = self.inventory(c)?;
            for (id, stamp) in &files {
                let raw = self.raw_from(&source, id, Some(stamp))?;
                let doc = self.parse(c, id, &raw.json, true);
                let t = Instant::now();
                if doc.state.status != RecordStatus::IncorrectJson && filter.matches(&doc.data)? {
                    if matched >= request.offset && result.records.len() < request.limit {
                        result.records.push(Record {
                            collection: c.name.clone(),
                            id: id.clone(),
                            path: self.path(c, id),
                            hash: raw.hash,
                            data: doc.data,
                            state: doc.state,
                            raw: None,
                        });
                    }
                    matched += 1;
                }
                self.metrics.query_nanos += nanos(t);
            }
            source.finish()?;
            if self.inventory(c)?.1 != files {
                return Err(Error::Unstable(c.path.clone()));
            }
        }
        result.total = request.count.then_some(matched);
        result.query_plan = vec!["direct source scan; compiled JSON filter".into()];
        Ok(())
    }

    fn query_json_grep(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut JsonQueryResult,
    ) -> Result<()> {
        let prefilter = filter.source_prefilter()?;
        result.query_plan = vec![match &prefilter {
            Some((_, pattern)) => format!(
                "ripgrep parallel buffered file search ({}) with regex {pattern}; Sonic JSON verification",
                rayon::current_num_threads()
            ),
            None => "source scan (no usable ripgrep prefilter); Sonic JSON verification".into(),
        }];
        let mut matched = 0;
        for c in collections {
            let (source, files) = self.inventory(c)?;
            let entries = files.iter().collect::<Vec<_>>();
            let scan_started = Instant::now();
            let outcomes = entries
                .par_iter()
                .map_init(
                    || {
                        SearcherBuilder::new()
                            .line_number(false)
                            .bom_sniffing(false)
                            .memory_map(MmapChoice::never())
                            .build()
                    },
                    |searcher, entry| -> Result<ScanOutcome> {
                        let (id, stamp) = *entry;
                        let path = source.path.join(format!("{id}.json"));
                        let mut file = source.file(id)?;
                        let before = Stamp::from_metadata(
                            &file.metadata().map_err(|e| io(&path, e))?,
                            &path,
                        )?;
                        if &before != stamp {
                            return Err(Error::Unstable(path));
                        }
                        let candidate = if let Some((matcher, _)) = &prefilter {
                            let mut sink = FirstMatch::default();
                            searcher
                                .search_file(matcher, &file, &mut sink)
                                .map_err(|e| io(&path, e))?;
                            sink.0
                        } else {
                            true
                        };
                        let mut outcome = ScanOutcome {
                            candidate,
                            ..Default::default()
                        };
                        if candidate {
                            file.seek(SeekFrom::Start(0)).map_err(|e| io(&path, e))?;
                            let mut bytes = Vec::new();
                            file.read_to_end(&mut bytes).map_err(|e| io(&path, e))?;
                            outcome.source_bytes = bytes.len() as u64;
                            let t = Instant::now();
                            let doc = Document::parse(&bytes);
                            outcome.parse_nanos = nanos(t);
                            if doc.state.status != RecordStatus::IncorrectJson {
                                let t = Instant::now();
                                outcome.matches = filter.matches(&doc.data)?;
                                outcome.query_nanos = nanos(t);
                            }
                        }
                        let after = Stamp::from_metadata(
                            &file.metadata().map_err(|e| io(&path, e))?,
                            &path,
                        )?;
                        if before != after || (candidate && outcome.source_bytes != after.size) {
                            return Err(Error::Unstable(path));
                        }
                        Ok(outcome)
                    },
                )
                .collect::<Result<Vec<_>>>()?;
            self.metrics.grep_scans += entries.len() as u64;
            self.metrics.grep_bytes += entries.iter().map(|(_, stamp)| stamp.size).sum::<u64>();
            self.metrics.grep_nanos += nanos(scan_started);
            for ((id, stamp), outcome) in entries.into_iter().zip(outcomes) {
                if outcome.candidate {
                    self.metrics.grep_candidates += 1;
                    self.metrics.source_reads += 1;
                    self.metrics.source_bytes += outcome.source_bytes;
                    self.metrics.parses += 1;
                    self.metrics.parse_nanos += outcome.parse_nanos;
                    self.metrics.json_decode_nanos += outcome.parse_nanos;
                    self.metrics.query_nanos += outcome.query_nanos;
                }
                if outcome.matches {
                    if matched >= request.offset && result.records.len() < request.limit {
                        let raw = self.raw_from(&source, id, Some(stamp))?;
                        let doc = self.parse(c, id, &raw.json, true);
                        if doc.state.status == RecordStatus::IncorrectJson
                            || !filter.matches(&doc.data)?
                        {
                            return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                        }
                        result.records.push(Record {
                            collection: c.name.clone(),
                            id: id.clone(),
                            path: self.path(c, id),
                            hash: raw.hash,
                            data: doc.data,
                            state: doc.state,
                            raw: None,
                        });
                    }
                    matched += 1;
                }
            }
            source.finish()?;
            if self.inventory(c)?.1 != files {
                return Err(Error::Unstable(c.path.clone()));
            }
        }
        result.total = request.count.then_some(matched);
        Ok(())
    }

    pub(super) fn ensure_picker(&mut self, c: &Collection) -> Result<()> {
        let rebuild = !self.file_pickers.contains_key(&c.name)
            || matches!(self.options.freshness, Freshness::HashOnRequest);
        if rebuild {
            let started = Instant::now();
            self.file_pickers.insert(
                c.name.clone(),
                crate::file_picker::FilePicker::open_with_indexes(&c.path, &c.indexes, &c.fields)?,
            );
            self.metrics.bigram_open_nanos += nanos(started);
        } else if self.options.freshness == Freshness::MetadataOnRequest {
            let started = Instant::now();
            self.file_pickers
                .get_mut(&c.name)
                .expect("picker exists")
                .refresh()?;
            self.metrics.bigram_refresh_nanos += nanos(started);
        }
        Ok(())
    }

    fn query_json_bigram(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut JsonQueryResult,
    ) -> Result<()> {
        let values_pattern = filter.values_prefilter().map(|(_, pattern)| pattern);
        let source_pattern = filter.source_prefilter()?;
        let cardinality_risk = filter.needs_global_cardinality_check(collections);
        let scalar_fields = filter.top_level_scalar_fields();
        result.query_plan =
            vec!["FFF values bigram candidates; source regex scan; Sonic verification".into()];
        let mut matched = 0;
        for c in collections {
            self.ensure_picker(c)?;
            let candidate_started = Instant::now();
            let picker = self.file_pickers.get(&c.name).unwrap();
            let array_risk = scalar_fields
                .iter()
                .any(|field| picker.has_array_value(field));
            let risk = cardinality_risk || array_risk;
            // A later collection can contain an invalid array in a scalar field,
            // so build its picker and check that risk even when the page is full.
            if !request.count && !risk && result.records.len() == request.limit {
                continue;
            }
            let typed_plan = (!risk).then(|| filter.typed_plan()).flatten();
            let entries = picker.candidate_records(
                (!risk).then_some(values_pattern.as_deref()).flatten(),
                typed_plan.as_ref(),
            );
            self.metrics.bigram_candidates_nanos += nanos(candidate_started);
            let prefilter = if risk { None } else { source_pattern.as_ref() };
            let source = Source::open(&c.path)?;
            let mut page_matches = Vec::new();
            // Keep counted queries in one parallel batch. For an uncounted page,
            // use bounded parallel batches and stop opening files once it fills.
            let batch_size = if request.count || risk {
                entries.len().max(1)
            } else {
                rayon::current_num_threads()
                    .saturating_mul(8)
                    .clamp(64, 512)
            };
            for batch in entries.chunks(batch_size) {
                let scan_started = Instant::now();
                let outcomes = batch
                    .par_iter()
                    .map_init(
                        || {
                            (
                                SearcherBuilder::new()
                                    .line_number(false)
                                    .bom_sniffing(false)
                                    .build(),
                                Vec::with_capacity(4096),
                            )
                        },
                        |(searcher, bytes), (id, stamp)| -> Result<ScanOutcome> {
                            let open_started = Instant::now();
                            let path = source.path.join(format!("{id}.json"));
                            let mut file = source.file(id)?;
                            let before = Stamp::from_metadata(
                                &file.metadata().map_err(|e| io(&path, e))?,
                                &path,
                            )?;
                            if &before != *stamp {
                                return Err(Error::Unstable(path));
                            }
                            let file_open_nanos = nanos(open_started);
                            let read_started = Instant::now();
                            bytes.clear();
                            file.read_to_end(bytes).map_err(|e| io(&path, e))?;
                            let read_nanos = nanos(read_started);
                            let (candidate, regex_nanos) = if let Some((matcher, _)) = prefilter {
                                let regex_started = Instant::now();
                                let mut sink = FirstMatch::default();
                                searcher
                                    .search_slice(matcher, bytes, &mut sink)
                                    .map_err(|e| io(&path, e))?;
                                (sink.0, nanos(regex_started))
                            } else {
                                (true, 0)
                            };
                            let mut outcome = ScanOutcome {
                                candidate,
                                file_open_nanos,
                                read_nanos,
                                regex_nanos,
                                ..Default::default()
                            };
                            if candidate {
                                outcome.source_bytes = bytes.len() as u64;
                                let t = Instant::now();
                                let doc = Document::parse(bytes);
                                outcome.parse_nanos = nanos(t);
                                if doc.state.status != RecordStatus::IncorrectJson {
                                    let t = Instant::now();
                                    outcome.matches = filter.matches(&doc.data)?;
                                    outcome.query_nanos = nanos(t);
                                }
                            }
                            let after = Stamp::from_metadata(
                                &file.metadata().map_err(|e| io(&path, e))?,
                                &path,
                            )?;
                            if before != after || bytes.len() as u64 != after.size {
                                return Err(Error::Unstable(path));
                            }
                            Ok(outcome)
                        },
                    )
                    .collect::<Result<Vec<_>>>()?;
                self.metrics.grep_scans += batch.len() as u64;
                self.metrics.grep_bytes += batch.iter().map(|(_, stamp)| stamp.size).sum::<u64>();
                self.metrics.grep_nanos += nanos(scan_started);
                for ((id, stamp), outcome) in batch.iter().zip(outcomes) {
                    self.metrics.bigram_file_open_nanos += outcome.file_open_nanos;
                    self.metrics.bigram_read_nanos += outcome.read_nanos;
                    self.metrics.bigram_regex_nanos += outcome.regex_nanos;
                    if outcome.candidate {
                        self.metrics.grep_candidates += 1;
                        self.metrics.source_reads += 1;
                        self.metrics.source_bytes += outcome.source_bytes;
                        self.metrics.parses += 1;
                        self.metrics.parse_nanos += outcome.parse_nanos;
                        self.metrics.json_decode_nanos += outcome.parse_nanos;
                        self.metrics.query_nanos += outcome.query_nanos;
                    }
                    if outcome.matches {
                        if matched >= request.offset
                            && result.records.len() + page_matches.len() < request.limit
                        {
                            page_matches.push(((*id).to_owned(), (*stamp).clone()));
                        }
                        matched += 1;
                    }
                }
                if !request.count
                    && !risk
                    && result.records.len() + page_matches.len() == request.limit
                {
                    break;
                }
            }
            drop(entries);
            let materialize_started = Instant::now();
            for (id, stamp) in page_matches {
                let raw = self.raw_from(&source, &id, Some(&stamp))?;
                let doc = self.parse(c, &id, &raw.json, true);
                if doc.state.status == RecordStatus::IncorrectJson || !filter.matches(&doc.data)? {
                    return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                }
                result.records.push(Record {
                    collection: c.name.clone(),
                    id: id.clone(),
                    path: self.path(c, &id),
                    hash: raw.hash,
                    data: doc.data,
                    state: doc.state,
                    raw: None,
                });
            }
            self.metrics.bigram_materialize_nanos += nanos(materialize_started);
            source.finish()?;
        }
        result.total = request.count.then_some(matched);
        Ok(())
    }
}
