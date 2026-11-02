//! Direct JSON scanning and ripgrep-backed source-file queries.

use super::*;
use grep_searcher::{MmapChoice, SearcherBuilder};
use std::io::{Seek, SeekFrom};


impl Repository {
    pub(super) fn query_json_direct(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut QueryOutput<'_>,
    ) -> Result<()> {
        let mut matched = 0;
        for c in collections {
            let filter = filter.for_fields(&c.fields);
            let (source, files) = self.inventory(c)?;
            let mut entries = files.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|a, b| {
                let a_time = a.1.modified.parse::<i128>().expect("validated timestamp");
                let b_time = b.1.modified.parse::<i128>().expect("validated timestamp");
                b_time.cmp(&a_time).then_with(|| a.0.cmp(b.0))
            });
            for (id, stamp) in entries {
                let raw = self.raw_from(&source, id, Some(stamp))?;
                let doc = self.parse(c, id, &raw.json, true);
                let t = Instant::now();
                let metadata_path = filter.uses_source_path().then(|| self.path(c, id));
                if filter.matches_source(
                    &doc.data,
                    id,
                    metadata_path.as_deref().unwrap_or(""),
                    &raw.stamp,
                    raw.created,
                    doc.state.status,
                )? {
                    if matched >= request.offset && result.returned() < request.limit {
                        let malformed = doc.state.status == RecordStatus::IncorrectJson;
                        result.push(Record {
                            collection: c.name.clone(),
                            id: id.clone(),
                            path: metadata_path.unwrap_or_else(|| self.path(c, id)),
                            size_bytes: raw.stamp.size,
                            modified_unix_nanos: raw.stamp.modified.clone(),
                            hash: raw.hash,
                            data: doc.data,
                            state: doc.state,
                            raw: malformed.then_some(raw.json),
                        })?;
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

    pub(super) fn query_json_grep(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut QueryOutput<'_>,
    ) -> Result<()> {
        result.query_plan =
            vec!["ripgrep source prefilter where safe; Sonic JSON verification".into()];
        let mut matched = 0;
        for c in collections {
            let filter = filter.for_fields(&c.fields);
            let prefilter = filter.source_prefilter()?;
            let (source, files) = self.inventory(c)?;
            let relative_collection = c
                .path
                .strip_prefix(&self.model.root)
                .expect("scoped collection")
                .to_path_buf();
            let mut entries = files.iter().collect::<Vec<_>>();
            entries.sort_unstable_by(|a, b| {
                let a_time = a.1.modified.parse::<i128>().expect("validated timestamp");
                let b_time = b.1.modified.parse::<i128>().expect("validated timestamp");
                b_time.cmp(&a_time).then_with(|| a.0.cmp(b.0))
            });
            let batch_size = if result.streaming {
                rayon::current_num_threads()
                    .saturating_mul(8)
                    .clamp(64, 512)
            } else {
                entries.len().max(1)
            };
            for batch in entries.chunks(batch_size) {
                let scan_started = Instant::now();
                let outcomes = batch
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
                            let before_metadata = file.metadata().map_err(|e| io(&path, e))?;
                            let before = Stamp::from_metadata(&before_metadata, &path)?;
                            let created = crate::source::created_timestamp(&before_metadata);
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
                                let mut doc = Document::parse(&bytes);
                                outcome.parse_nanos = nanos(t);
                                if filter.uses_status()
                                    && doc.state.status != RecordStatus::IncorrectJson
                                {
                                    let started = Instant::now();
                                    doc.validate(c);
                                    outcome.validation_nanos = nanos(started);
                                    outcome.validated = c.schema.is_some();
                                }
                                let t = Instant::now();
                                let metadata_path = filter.uses_source_path().then(|| {
                                    relative_collection
                                        .join(format!("{id}.json"))
                                        .to_string_lossy()
                                        .into_owned()
                                });
                                outcome.matches = filter.matches_source(
                                    &doc.data,
                                    id,
                                    metadata_path.as_deref().unwrap_or(""),
                                    &before,
                                    created,
                                    doc.state.status,
                                )?;
                                outcome.query_nanos = nanos(t);
                            }
                            let after = Stamp::from_metadata(
                                &file.metadata().map_err(|e| io(&path, e))?,
                                &path,
                            )?;
                            if before != after || (candidate && outcome.source_bytes != after.size)
                            {
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
                    if outcome.candidate {
                        self.metrics.grep_candidates += 1;
                        self.metrics.source_reads += 1;
                        self.metrics.source_bytes += outcome.source_bytes;
                        self.metrics.parses += 1;
                        self.metrics.parse_nanos += outcome.parse_nanos;
                        self.metrics.json_decode_nanos += outcome.parse_nanos;
                        self.metrics.query_nanos += outcome.query_nanos;
                        self.metrics.validation_nanos += outcome.validation_nanos;
                        self.metrics.validations += u64::from(outcome.validated);
                    }
                    if outcome.matches {
                        if matched >= request.offset && result.returned() < request.limit {
                            let raw = self.raw_from(&source, id, Some(stamp))?;
                            let doc = self.parse(c, id, &raw.json, true);
                            let metadata_path = filter.uses_source_path().then(|| self.path(c, id));
                            if !filter.matches_source(
                                &doc.data,
                                id,
                                metadata_path.as_deref().unwrap_or(""),
                                &raw.stamp,
                                raw.created,
                                doc.state.status,
                            )? {
                                return Err(Error::Unstable(
                                    source.path.join(format!("{id}.json")),
                                ));
                            }
                            let malformed = doc.state.status == RecordStatus::IncorrectJson;
                            result.push(Record {
                                collection: c.name.clone(),
                                id: (*id).clone(),
                                path: metadata_path.unwrap_or_else(|| self.path(c, id)),
                                size_bytes: raw.stamp.size,
                                modified_unix_nanos: raw.stamp.modified.clone(),
                                hash: raw.hash,
                                data: doc.data,
                                state: doc.state,
                                raw: malformed.then_some(raw.json),
                            })?;
                        }
                        matched += 1;
                    }
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
}
