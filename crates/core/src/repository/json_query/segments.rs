//! Persistent segment candidate selection and JSON verification.

use super::*;
use crate::json_filter::TypedPlan;


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
    pub(super) fn query_json_segments(
        &mut self,
        request: &JsonQuery,
        filter: &JsonFilter,
        collections: &[Collection],
        result: &mut QueryOutput<'_>,
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
            if result.streaming
                && !request.count
                && !cardinality_risk
                && !actual_array_risk
                && result.returned() == request.limit
            {
                continue;
            }
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
            let batch_size = if result.streaming {
                rayon::current_num_threads()
                    .saturating_mul(8)
                    .clamp(64, 512)
            } else {
                candidates.len().max(1)
            };
            for batch in candidates.chunks(batch_size) {
                let verify_started = Instant::now();
                let outcomes = batch
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
                for (id, (matches, metrics)) in batch.iter().zip(outcomes) {
                    self.metrics.merge(&metrics);
                    if matches {
                        if matched >= request.offset && result.returned() < request.limit {
                            let entry = snapshot
                                .entry(id)
                                .ok_or_else(|| Error::Cache(format!("segment ID {id} missing")))?;
                            let raw = self.raw_from(&source, id, Some(&entry.stamp))?;
                            let doc = self.parse(c, id, &raw.json, true);
                            if doc.state.status == RecordStatus::IncorrectJson
                                || !filter.matches(&doc.data)?
                            {
                                return Err(Error::Unstable(c.path.join(format!("{id}.json"))));
                            }
                            result.push(Record {
                                collection: c.name.clone(),
                                id: (*id).clone(),
                                path: self.path(c, id),
                                hash: raw.hash,
                                data: doc.data,
                                state: doc.state,
                                raw: None,
                            })?;
                        }
                        matched += 1;
                    }
                }
                self.metrics.segment_materialize_nanos += nanos(materialize_started);
                if result.streaming
                    && !request.count
                    && !cardinality_risk
                    && !actual_array_risk
                    && result.returned() == request.limit
                {
                    break;
                }
            }
            source.finish()?;
        }
        result.total = request.count.then_some(matched);
        Ok(())
    }
}
