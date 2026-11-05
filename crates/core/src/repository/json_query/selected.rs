//! Ordinary JSON queries over a pinned mixed-format source inventory.
//! JSON-only scopes keep the concrete picker path in the parent module.

use super::*;
use crate::{
    repository::{
        extension_query::source_location, ordered_result_cache::OrderedResultKey,
        predicate_cache::PredicateCacheKey,
    },
    sorting::{SortBuffer, SortRecord},
};


impl Repository {
    pub(super) fn query_json_selected(
        &mut self,
        request: &JsonQuery,
        selected: Vec<(Collection, CustomSourceSnapshot)>,
        cache_allowed: bool,
    ) -> Result<(JsonQueryResult, Option<PendingSelectedOrder>)> {
        let started = Instant::now();
        let before = self.metrics.clone();
        if request.limit == 0
            || request.limit > i64::MAX as usize
            || request.offset > i64::MAX as usize
            || self.options.total_limit == 0
        {
            return Err(Error::Request(
                "invalid JSON pagination or total_limit".into(),
            ));
        }
        let filter =
            JsonFilter::compile_with_fts(&request.filter, &request.parameters, request.fts)?;
        let mut bound = Vec::with_capacity(selected.len());
        let mut plans = BTreeMap::new();
        for (collection, _) in &selected {
            bound.push(self.bind_filter(&filter, collection)?);
            if !request.sort.is_empty() {
                plans.insert(
                    collection.name.clone(),
                    self.sort_plan(collection, &request.sort)?,
                );
            }
        }
        let mut offsets = Vec::with_capacity(selected.len() + 1);
        offsets.push(0_usize);
        for (_, snapshot) in &selected {
            let next = offsets
                .last()
                .unwrap()
                .checked_add(snapshot.sources.len())
                .ok_or_else(|| Error::Request("selected source table exceeds 2^32 slots".into()))?;
            if next > u32::MAX as usize {
                return Err(Error::Request(
                    "selected source table exceeds 2^32 slots".into(),
                ));
            }
            offsets.push(next);
        }
        let names = selected
            .iter()
            .map(|(collection, _)| collection.name.clone())
            .collect::<Vec<_>>();
        let ordered_key = OrderedResultKey {
            predicate: PredicateCacheKey {
                collection: format!("@frontend:{}", names.join("\0")),
                expression: serde_json::to_vec(&(
                    "ordinary-selected-json/v1",
                    filter.cache_expression(),
                    selected
                        .iter()
                        .map(|(collection, snapshot)| {
                            (
                                &collection.mapping_hash,
                                &collection.schema_hash,
                                &snapshot.definitions,
                                &snapshot.source_generation,
                            )
                        })
                        .collect::<Vec<_>>(),
                ))
                .expect("selected JSON identity serializes"),
            },
            sort: serde_json::to_vec(&request.sort).expect("sort identity serializes"),
        };
        let can_cache = cache_allowed
            && self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest
            && self.query_cache.enabled()
            && !filter.has_relations()
            && !plans.values().any(crate::sorting::SortPlan::has_relations)
            && selected.iter().all(|(_, snapshot)| snapshot.cacheable);
        let cached_page = can_cache
            .then(|| {
                self.query_cache
                    .completed_ordered_page(&ordered_key, request.offset, request.limit)
            })
            .flatten();
        let cache_hit = cached_page.is_some();
        let mut matches = 0_usize;
        let mut ordered_slots = Vec::new();
        if let Some((total, _)) = &cached_page {
            self.metrics.ordered_result_hits += 1;
            matches = *total;
        } else {
            let cap = self.options.total_limit;
            let mut sort_buffer =
                (!request.sort.is_empty()).then(|| SortBuffer::<usize>::new(plans, 0, cap, cap));
            let needs_status =
                filter.uses_status() || sort_buffer.as_ref().is_some_and(SortBuffer::uses_status);
            let mut bytes = Vec::new();
            for (collection_index, (collection, snapshot)) in selected.iter().enumerate() {
                let mut positions = (0..snapshot.sources.len()).collect::<Vec<_>>();
                if sort_buffer.is_none() {
                    positions.sort_unstable_by(|&left, &right| {
                        let a = &snapshot.sources[left];
                        let b = &snapshot.sources[right];
                        let modified = |source: &crate::extensions::SourceDescriptor| {
                            source
                                .metadata
                                .get("modified_unix_nanos")
                                .and_then(|value| value.parse::<i128>().ok())
                                .unwrap_or(0)
                        };
                        modified(b)
                            .cmp(&modified(a))
                            .then_with(|| a.record_id.cmp(&b.record_id))
                    });
                }
                for index in positions {
                    let source = &snapshot.sources[index];
                    let record =
                        self.selected_record(collection, source, &mut bytes, needs_status)?;
                    let stamp = Stamp {
                        size: record.size_bytes,
                        modified: record.modified_unix_nanos.clone(),
                        identity: source.revision.clone(),
                    };
                    let created = source
                        .metadata
                        .get("created_unix_nanos")
                        .and_then(|value| value.parse::<i128>().ok());
                    if !bound[collection_index].matches_source(
                        &record.data,
                        &record.id,
                        &record.path,
                        &stamp,
                        created,
                        record.state.status,
                    )? {
                        continue;
                    }
                    matches += 1;
                    let slot = offsets[collection_index] + index;
                    if let Some(buffer) = &mut sort_buffer {
                        let sort_record = SortRecord {
                            id: &record.id,
                            collection_path: &collection.path,
                            root: &self.model.root,
                            size: record.size_bytes,
                            modified: &record.modified_unix_nanos,
                            status: record.state.status,
                            locator: Some(&source.locator),
                            created,
                        };
                        let keys = buffer.project(&collection.name, &sort_record, &record.data)?;
                        buffer.insert_keys(&collection.name, &record.id, keys, || slot);
                    } else if ordered_slots.len() < cap {
                        ordered_slots.push(u32::try_from(slot).expect("checked source slots"));
                    }
                }
            }
            if let Some(buffer) = sort_buffer {
                ordered_slots = buffer
                    .into_page()
                    .into_iter()
                    .map(|slot| u32::try_from(slot).expect("checked source slots"))
                    .collect();
            }
        }
        let end = ordered_slots
            .len()
            .min(request.offset.saturating_add(request.limit));
        let page = cached_page.as_ref().map_or_else(
            || ordered_slots.get(request.offset..end).unwrap_or(&[]),
            |(_, page)| page.as_slice(),
        );
        let mut records = Vec::with_capacity(page.len());
        let mut bytes = Vec::new();
        for &slot in page {
            let (collection_index, local) = source_location(&offsets, slot)
                .ok_or_else(|| Error::Cache("selected source slot is unavailable".into()))?;
            let (collection, snapshot) = &selected[collection_index];
            let source = &snapshot.sources[local];
            let record = self.selected_record(collection, source, &mut bytes, true)?;
            let stamp = Stamp {
                size: record.size_bytes,
                modified: record.modified_unix_nanos.clone(),
                identity: source.revision.clone(),
            };
            let created = source
                .metadata
                .get("created_unix_nanos")
                .and_then(|value| value.parse::<i128>().ok());
            if !bound[collection_index].matches_source(
                &record.data,
                &record.id,
                &record.path,
                &stamp,
                created,
                record.state.status,
            )? {
                return Err(Error::Unstable(source.locator.clone()));
            }
            records.push(record);
        }
        if self.options.freshness != Freshness::MetadataOnOpen {
            for (collection, snapshot) in &selected {
                let latest = self.source_descriptors(&collection.name)?;
                self.metrics.enumerations += 1;
                if latest != *snapshot.sources {
                    return Err(Error::Unstable(collection.path.clone()));
                }
            }
        }
        let pending = (can_cache && !cache_hit && matches <= self.options.total_limit)
            .then_some((ordered_key, ordered_slots));
        Ok((
            JsonQueryResult {
                records,
                total: request
                    .count
                    .then_some(matches.min(self.options.total_limit)),
                truncated: matches > self.options.total_limit,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                dependencies: JsonQueryDependencies {
                    collections: names,
                    paths: filter.paths().to_vec(),
                    metadata_fields: filter.metadata_fields().to_vec(),
                },
                filter_hash: filter.hash().into(),
                metrics: self.metrics.since(&before),
                query_plan: vec![
                    if cache_hit {
                        "selected-ordered-cache"
                    } else {
                        "selected-exact-scan"
                    }
                    .into(),
                ],
            },
            pending,
        ))
    }
}
