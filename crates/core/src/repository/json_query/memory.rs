//! In-memory bigram and typed-index JSON query execution.

use super::*;
use grep_searcher::SearcherBuilder;


impl Repository {
    /// Keep the picker available for memory queries, views, and reconciliation.
    pub(in crate::repository) fn ensure_picker(&mut self, c: &Collection) -> Result<()> {
        let rebuild = !self.file_pickers.contains_key(&c.name)
            || matches!(self.options.freshness, Freshness::HashOnRequest);
        if rebuild {
            let started = Instant::now();
            let mut picker =
                crate::file_picker::FilePicker::open_with_indexes(&c.path, &c.indexes, &c.fields)?;
            if let Some(paths) = self.requested_postings.get(&c.name) {
                for path in paths {
                    picker.build_posting(path)?;
                }
            }

            #[cfg(feature = "index-timings")]
            let timings = picker.build_timings();
            self.file_pickers.insert(c.name.clone(), picker);
            self.metrics.bigram_open_nanos += nanos(started);
            #[cfg(feature = "index-timings")]
            {
                self.metrics.bigram_inventory_nanos += timings.inventory_nanos;
                self.metrics.bigram_slots_nanos += timings.slots_nanos;
                self.metrics.bigram_scan_nanos += timings.scan_nanos;
                self.metrics.bigram_project_worker_nanos += timings.project_worker_nanos;
                self.metrics.bigram_add_worker_nanos += timings.add_worker_nanos;
                self.metrics.bigram_compress_nanos += timings.compress_nanos;
                self.metrics.bigram_typed_nanos += timings.typed_nanos;
                self.metrics.bigram_view_nanos += timings.view_nanos;
            }
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

    pub(super) fn query_json_bigram(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut QueryOutput<'_>,
    ) -> Result<()> {
        let values_pattern = filter.values_prefilter().map(|(_, pattern)| pattern);
        let source_pattern = filter.source_prefilter()?;
        let scalar_fields = filter.top_level_scalar_fields();
        let required_fields = filter.required_top_level_fields();
        let exact_plan = filter.exact_index_plan();
        result.query_plan =
            vec!["in-memory bigram, typed, and presence candidates; source verification".into()];
        let mut matched = 0;
        for c in collections {
            self.ensure_picker(c)?;
            let candidate_started = Instant::now();
            let picker = self.file_pickers.get(&c.name).unwrap();
            // Undeclared fields can have any JSON shape. Scan that collection
            // for scalar cardinality errors without slowing indexed collections.
            let cardinality_risk =
                filter.needs_cardinality_check_in(c, |path| picker.typed_scalar_safe(path));
            let array_risk = scalar_fields
                .iter()
                .any(|field| picker.has_array_value(field));
            let risk = cardinality_risk || array_risk;
            // A later collection can contain an invalid array in a scalar field,
            // so build its picker and check that risk even when the page is full.
            if !request.count && !risk && result.returned() == request.limit {
                continue;
            }
            if !risk
                && let Some(selection) = exact_plan.as_ref().and_then(|plan| {
                    picker.exact_selection(
                        plan,
                        request.offset.saturating_sub(matched),
                        request.limit.saturating_sub(result.returned()),
                    )
                })
            {
                self.metrics.bigram_candidates_nanos += nanos(candidate_started);
                if result.query_plan.len() == 1 {
                    result.query_plan.push(
                        "exact scalar/presence index matches; source page verification".into(),
                    );
                }
                Self::query_json_exact_records(
                    &mut self.metrics,
                    super::exact::ExactQuery {
                        filter,
                        collection: c,
                        root: &self.model.root,
                    },
                    &selection,
                    result,
                    &mut matched,
                )?;
                continue;
            }
            let typed_plan = (!risk).then(|| filter.typed_plan()).flatten();
            let entries = picker.candidate_records(
                (!risk).then_some(values_pattern.as_deref()).flatten(),
                typed_plan.as_ref(),
                if risk { &[] } else { &required_fields },
            );
            self.metrics.bigram_candidates_nanos += nanos(candidate_started);
            let prefilter = if risk { None } else { source_pattern.as_ref() };
            let source = Source::open(&c.path)?;
            // Counted queries and global-cardinality checks keep their full
            // scan behavior. For pages, grow batches across misses to reduce
            // barriers and reset after matches when the index was selective.
            // Cap the first page batch on large machines to limit over-scan.
            let full_scan = request.count || risk;
            let base_chunk = rayon::current_num_threads()
                .saturating_mul(4)
                .clamp(32, 256);
            let strong_prefilter = entries.len().saturating_mul(2) < picker.len();
            let max_chunk = if strong_prefilter {
                base_chunk.saturating_mul(8)
            } else {
                base_chunk.saturating_mul(256).max(8192)
            };
            let mut chunk_size = if full_scan {
                if result.streaming {
                    rayon::current_num_threads()
                        .saturating_mul(8)
                        .clamp(64, 512)
                } else {
                    entries.len().max(1)
                }
            } else {
                base_chunk
            };
            let mut batch_start = 0;
            while batch_start < entries.len() {
                let batch_end = batch_start.saturating_add(chunk_size).min(entries.len());
                let batch = &entries[batch_start..batch_end];
                batch_start = batch_end;
                let mut page_matches = Vec::new();
                let matched_before = matched;
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
                            && result.returned() + page_matches.len() < request.limit
                        {
                            page_matches.push(((*id).to_owned(), (*stamp).clone()));
                        }
                        matched += 1;
                    }
                }
                let materialize_started = Instant::now();
                for (id, stamp) in page_matches {
                    let raw = Self::read_raw(&mut self.metrics, &source, &id, Some(&stamp))?;
                    let doc = Self::parse_value(&mut self.metrics, c, &id, &raw.json, true);
                    if doc.state.status == RecordStatus::IncorrectJson
                        || !filter.matches(&doc.data)?
                    {
                        return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                    }
                    result.push(Record {
                        collection: c.name.clone(),
                        id: id.clone(),
                        path: self.path(c, &id),
                        hash: raw.hash,
                        data: doc.data,
                        state: doc.state,
                        raw: None,
                    })?;
                }
                self.metrics.bigram_materialize_nanos += nanos(materialize_started);
                if !request.count && !risk && result.returned() == request.limit {
                    break;
                }
                if !full_scan {
                    if strong_prefilter && matched > matched_before {
                        chunk_size = base_chunk;
                    } else {
                        chunk_size = chunk_size.saturating_mul(2).min(max_chunk);
                    }
                }
            }
            drop(entries);
            source.finish()?;
        }
        result.total = request.count.then_some(matched);
        Ok(())
    }
}
