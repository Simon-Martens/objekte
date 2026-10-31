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
            for (id, stamp) in &files {
                let raw = self.raw_from(&source, id, Some(stamp))?;
                let doc = self.parse(c, id, &raw.json, true);
                let t = Instant::now();
                if doc.state.status != RecordStatus::IncorrectJson && filter.matches(&doc.data)? {
                    if matched >= request.offset && result.returned() < request.limit {
                        result.push(Record {
                            collection: c.name.clone(),
                            id: id.clone(),
                            path: self.path(c, id),
                            size_bytes: raw.stamp.size,
                            modified_unix_nanos: raw.stamp.modified.clone(),
                            hash: raw.hash,
                            data: doc.data,
                            state: doc.state,
                            raw: None,
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
            let entries = files.iter().collect::<Vec<_>>();
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
                    }
                    if outcome.matches {
                        if matched >= request.offset && result.returned() < request.limit {
                            let raw = self.raw_from(&source, id, Some(stamp))?;
                            let doc = self.parse(c, id, &raw.json, true);
                            if doc.state.status == RecordStatus::IncorrectJson
                                || !filter.matches(&doc.data)?
                            {
                                return Err(Error::Unstable(
                                    source.path.join(format!("{id}.json")),
                                ));
                            }
                            result.push(Record {
                                collection: c.name.clone(),
                                id: (*id).clone(),
                                path: self.path(c, id),
                                size_bytes: raw.stamp.size,
                                modified_unix_nanos: raw.stamp.modified.clone(),
                                hash: raw.hash,
                                data: doc.data,
                                state: doc.state,
                                raw: None,
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
