//! Derived queries on the same selected slots used by ordinary text/JSON APIs.

use super::*;
use crate::repository::extension_derived_index::SelectedDerivedKey;


impl Repository {
    pub(super) fn query_derived_selected(
        &mut self,
        query: &DerivedQuery,
        collection: &Collection,
        snapshot: &CustomSourceSnapshot,
    ) -> Result<DerivedQueryResult> {
        if query.limit == 0 || query.limit > i64::MAX as usize || query.offset > i64::MAX as usize {
            return Err(Error::Request("invalid derived pagination".into()));
        }
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no value extractors are registered".into()))?;
        let binding = extensions.bind_value(
            &query.extractor,
            &query.collection,
            extensions
                .schema(&query.collection)
                .map(std::sync::Arc::as_ref),
        )?;
        let comparison = query.predicate.key()?;
        reduced_key(&[], query.sort)?;
        let use_posting = query.backend == DerivedBackend::Indexed
            && self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest;
        if use_posting {
            self.warm_selected_derived(&query.collection, &query.extractor)?;
        }
        let key = SelectedDerivedKey::new(
            collection,
            &binding.path,
            &snapshot.fingerprint,
            self.extensions.as_ref().expect("extensions exist"),
        );
        let ordered_key = OrderedResultKey {
            predicate: PredicateCacheKey {
                collection: query.collection.clone(),
                expression: serde_json::to_vec(&(
                    "selected-derived/v1",
                    &binding.path,
                    &query.predicate,
                    &snapshot.definitions,
                    &snapshot.source_generation,
                ))
                .expect("derived identity serializes"),
            },
            sort: serde_json::to_vec(&query.sort).expect("derived sort identity serializes"),
        };
        let can_cache = use_posting
            && binding.cacheable
            && snapshot.cacheable
            && self.query_cache.enabled()
            && self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest;
        let cached = can_cache
            .then(|| {
                self.query_cache
                    .completed_ordered_page(&ordered_key, query.offset, query.limit)
            })
            .flatten();
        let mut slots = Vec::<u32>::new();
        let total = if let Some((total, _)) = &cached {
            self.metrics.ordered_result_hits += 1;
            *total
        } else if use_posting {
            let posting = self
                .selected_derived_indexes
                .get(&key)
                .ok_or_else(|| Error::Cache("selected derived posting is unavailable".into()))?;
            let mut membership = vec![false; snapshot.sources.len()];
            let mut sort_keys = vec![None::<TypedKey>; snapshot.sources.len()];
            posting
                .index
                .visit_derived(&binding.path, |slot, value| {
                    membership[slot] |= query.predicate.matches_one(value, comparison.as_ref());
                    if let Some(sort) = query.sort {
                        let current = &mut sort_keys[slot];
                        if current.as_ref().is_none_or(|previous| {
                            if sort.reduce == SortReduce::Min {
                                value < previous
                            } else {
                                value > previous
                            }
                        }) {
                            *current = Some(value.clone());
                        }
                    }
                })
                .ok_or_else(|| Error::Cache("selected derived posting is unavailable".into()))?;
            slots = (0..snapshot.sources.len())
                .filter(|&slot| membership[slot])
                .map(|slot| slot as u32)
                .collect();
            if let Some(sort) = query.sort {
                slots.sort_by(|&left, &right| {
                    compare_sort_keys(
                        sort_keys[left as usize].as_ref(),
                        sort_keys[right as usize].as_ref(),
                        sort.direction,
                    )
                    .then_with(|| {
                        snapshot.sources[left as usize]
                            .record_id
                            .cmp(&snapshot.sources[right as usize].record_id)
                    })
                });
            }
            slots.len()
        } else {
            let mut rows = Vec::new();
            let mut bytes = Vec::new();
            let mut output = Vec::new();
            for (slot, source) in snapshot.sources.iter().enumerate() {
                let record = self.selected_record(collection, source, &mut bytes, false)?;
                if record.state.status.is_malformed() {
                    continue;
                }
                output.clear();
                let state = binding.extractor.extract(&record.data, &mut output)?;
                if state == Extraction::Missing && !output.is_empty() {
                    return Err(Error::Request(format!(
                        "extractor {} emitted values for a missing field",
                        binding.identity,
                    )));
                }
                let keys = output
                    .drain(..)
                    .map(TypedKey::from_extracted)
                    .collect::<Result<Vec<_>>>()?;
                if query.predicate.matches(&keys, comparison.as_ref()) {
                    rows.push((slot as u32, reduced_key(&keys, query.sort)?));
                }
            }
            if let Some(sort) = query.sort {
                rows.sort_by(|(left, a), (right, b)| {
                    compare_sort_keys(a.as_ref(), b.as_ref(), sort.direction).then_with(|| {
                        snapshot.sources[*left as usize]
                            .record_id
                            .cmp(&snapshot.sources[*right as usize].record_id)
                    })
                });
            }
            slots = rows.into_iter().map(|(slot, _)| slot).collect();
            slots.len()
        };
        let end = total.min(self.options.total_limit);
        let page = cached.as_ref().map_or_else(
            || {
                slots
                    .get(query.offset..end.min(query.offset.saturating_add(query.limit)))
                    .unwrap_or(&[])
            },
            |(_, page)| page.as_slice(),
        );
        let mut records = Vec::with_capacity(page.len());
        let mut bytes = Vec::new();
        for &slot in page {
            let source = snapshot
                .sources
                .get(slot as usize)
                .ok_or_else(|| Error::Cache("selected derived slot is unavailable".into()))?;
            let record = self.selected_record(collection, source, &mut bytes, true)?;
            if record.state.status.is_malformed() {
                return Err(Error::Unstable(source.locator.clone()));
            }
            let mut output = Vec::new();
            let state = binding.extractor.extract(&record.data, &mut output)?;
            if state == Extraction::Missing && !output.is_empty() {
                return Err(Error::Request(
                    "extractor emitted values for a missing field".into(),
                ));
            }
            let keys = output
                .into_iter()
                .map(TypedKey::from_extracted)
                .collect::<Result<Vec<_>>>()?;
            if !query.predicate.matches(&keys, comparison.as_ref()) {
                return Err(Error::Unstable(source.locator.clone()));
            }
            records.push(record);
        }
        if self.options.freshness != Freshness::MetadataOnOpen {
            let latest = self.source_descriptors(&query.collection)?;
            self.metrics.enumerations += 1;
            if latest != *snapshot.sources {
                return Err(Error::Unstable(collection.path.clone()));
            }
        }
        if can_cache && cached.is_none() && total <= self.options.total_limit {
            self.query_cache.insert_ordered(ordered_key, slots);
        }
        Ok(DerivedQueryResult {
            records,
            total: query.count.then_some(total),
            truncated: total > self.options.total_limit,
        })
    }
}
