//! Ordinary JSON queries over a pinned mixed-format source inventory.
//! JSON-only scopes keep the concrete picker path in the parent module.

use super::*;
use crate::{
    json_filter::TypedPlan,
    repository::{
        extension_query::source_location, ordered_result_cache::OrderedResultKey,
        predicate_cache::PredicateCacheKey,
    },
    sorting::{SortBuffer, SortRecord},
};

type SelectedRecordCallback<'a> = dyn FnMut(usize, &Record) -> Result<()> + 'a;


impl Repository {
    /// Estimate selected typed posting work without allocating candidate
    /// words. This lets a selective text posting avoid a redundant broad
    /// scalar probe.
    pub(in crate::repository) fn selected_field_candidate_estimate(
        &self,
        collection: &Collection,
        snapshot: &CustomSourceSnapshot,
        filter: &JsonFilter,
    ) -> Option<usize> {
        if filter.has_relations()
            || self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return None;
        }
        let extensions = self.extensions.as_ref()?;
        let posting = |path: &crate::JsonPath| {
            let key = crate::repository::extension_derived_index::SelectedDerivedKey::new(
                collection,
                path,
                &snapshot.fingerprint,
                extensions,
            );
            self.selected_derived_indexes.get(&key)
        };
        if filter.needs_selected_cardinality_check_in(|path| {
            posting(path).is_some_and(|index| index.index.scalar_safe(path))
        }) {
            return None;
        }

        fn estimate<'a>(
            plan: &TypedPlan,
            slots: usize,
            posting: &impl Fn(
                &crate::JsonPath,
            ) -> Option<
                &'a crate::repository::extension_derived_index::SelectedDerivedPosting,
            >,
        ) -> Option<usize> {
            match plan {
                TypedPlan::And(parts) => parts
                    .iter()
                    .filter_map(|part| estimate(part, slots, posting))
                    .min(),
                TypedPlan::Or(parts) => {
                    let mut total = 0_usize;
                    for part in parts {
                        total = total.saturating_add(estimate(part, slots, posting)?);
                    }
                    Some(total.min(slots))
                }
                TypedPlan::Term(path, ..)
                | TypedPlan::AnyTerm(path, ..)
                | TypedPlan::Present(path)
                | TypedPlan::Missing(path)
                | TypedPlan::Prefix(path, ..)
                | TypedPlan::Like(path, ..) => {
                    posting(path)?.index.cheap_candidate_estimate(plan, slots)
                }
            }
        }
        estimate(&filter.typed_plan()?, snapshot.sources.len(), &posting)
    }

    /// Evaluate a bound necessary condition against configured logical-field
    /// postings. Unknown AND children are residuals; OR requires coverage in
    /// every branch. The underlying typed index retains exact key semantics.
    pub(in crate::repository) fn selected_field_candidates(
        &mut self,
        collection: &Collection,
        snapshot: &CustomSourceSnapshot,
        filter: &JsonFilter,
    ) -> Option<Vec<u64>> {
        if filter.has_relations()
            || self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return None;
        }
        let extensions = self.extensions.as_ref()?;
        let posting = |path: &crate::JsonPath| {
            let key = crate::repository::extension_derived_index::SelectedDerivedKey::new(
                collection,
                path,
                &snapshot.fingerprint,
                extensions,
            );
            self.selected_derived_indexes.get(&key)
        };
        if filter.needs_selected_cardinality_check_in(|path| {
            posting(path).is_some_and(|index| index.index.scalar_safe(path))
        }) {
            return None;
        }

        fn evaluate<'a>(
            plan: &TypedPlan,
            slots: usize,
            posting: &impl Fn(
                &crate::JsonPath,
            ) -> Option<
                &'a crate::repository::extension_derived_index::SelectedDerivedPosting,
            >,
        ) -> Option<Vec<u64>> {
            match plan {
                TypedPlan::And(parts) => {
                    let mut combined: Option<Vec<u64>> = None;
                    for part in parts {
                        if let Some(next) = evaluate(part, slots, posting) {
                            combined = Some(match combined {
                                Some(mut current) => {
                                    for (left, right) in current.iter_mut().zip(next) {
                                        *left &= right;
                                    }
                                    current
                                }
                                None => next,
                            });
                        }
                    }
                    combined
                }
                TypedPlan::Or(parts) => {
                    let mut combined = vec![0_u64; slots.div_ceil(64)];
                    for part in parts {
                        let next = evaluate(part, slots, posting)?;
                        for (left, right) in combined.iter_mut().zip(next) {
                            *left |= right;
                        }
                    }
                    Some(combined)
                }
                TypedPlan::Term(path, ..)
                | TypedPlan::AnyTerm(path, ..)
                | TypedPlan::Present(path)
                | TypedPlan::Missing(path)
                | TypedPlan::Prefix(path, ..)
                | TypedPlan::Like(path, ..) => {
                    posting(path)?.index.candidate_bits(Some(plan), slots)
                }
            }
        }
        let bits = evaluate(&filter.typed_plan()?, snapshot.sources.len(), &posting)?;
        self.metrics.selected_candidate_bitsets += 1;
        let count = bits
            .iter()
            .map(|word| word.count_ones() as usize)
            .sum::<usize>();
        (count.saturating_mul(4) < snapshot.sources.len().saturating_mul(3)).then_some(bits)
    }

    /// Native postings cover only filesystem JSON slots. The generation-bound
    /// slot map translates those candidates; every custom slot remains in the
    /// fallback domain until a compatible selected posting covers it.
    pub(in crate::repository) fn selected_native_candidates(
        &mut self,
        collection: &Collection,
        snapshot: &CustomSourceSnapshot,
        filter: &JsonFilter,
        backend: JsonQueryBackend,
    ) -> Result<Option<Vec<u64>>> {
        if filter.has_relations()
            || !matches!(backend, JsonQueryBackend::Auto | JsonQueryBackend::Bigram)
            || self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
            || snapshot.native_count == 0
        {
            return Ok(None);
        }
        let typed = filter.typed_plan();
        let required = filter.required_top_level_fields();
        if typed.is_none() && required.is_empty() {
            return Ok(None);
        }
        self.ensure_picker(collection)?;
        let picker = &self.file_pickers[&collection.name];
        let scalar_fields = filter.top_level_scalar_fields();
        let risk = filter.needs_cardinality_check_in(|path| picker.typed_scalar_safe(path))
            || scalar_fields
                .iter()
                .any(|field| picker.has_array_value(field));
        if risk {
            return Ok(None);
        }
        let Some(map) = snapshot.native_slot_map(picker) else {
            return Ok(None);
        };
        let selection =
            picker.candidate_slot_selection_for_path(None, None, typed.as_ref(), &required);
        // A broad posting can cost more to enumerate and map than verification.
        if selection.is_all() || selection.len().saturating_mul(2) >= snapshot.native_count {
            return Ok(None);
        }
        let mut bits = vec![0_u64; snapshot.sources.len().div_ceil(64)];
        for &slot in snapshot.custom_slots.iter() {
            bits[slot as usize / 64] |= 1 << (slot % 64);
        }
        for native in selection.ordered(picker, false, &[]) {
            let selected = map[native].expect("mapped live native slot");
            bits[selected as usize / 64] |= 1 << (selected % 64);
        }
        Ok(Some(bits))
    }

    pub(in crate::repository) fn selected_scalar_candidates(
        &mut self,
        collection: &Collection,
        snapshot: &CustomSourceSnapshot,
        filter: &JsonFilter,
    ) -> Result<Option<Vec<u64>>> {
        if let Some(bits) = self.selected_field_candidates(collection, snapshot, filter) {
            Ok(Some(bits))
        } else {
            self.selected_native_candidates(
                collection,
                snapshot,
                filter,
                crate::JsonQueryBackend::Bigram,
            )
        }
    }

    pub(super) fn query_json_selected(
        &mut self,
        request: &JsonQuery,
        selected: Vec<(Collection, CustomSourceSnapshot)>,
        cache_allowed: bool,
        mut on_record: Option<&mut SelectedRecordCallback<'_>>,
    ) -> Result<(JsonQueryResult, Option<PendingSelectedOrder>, usize)> {
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
        let mut used_native_candidates = false;
        let mut used_field_candidates = false;
        let mut returned = 0_usize;
        let stream_early = on_record.is_some() && request.sort.is_empty() && !cache_hit;
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
                let projection_roots =
                    crate::repository::extension_sources::selected_projection_roots(
                        Some(&bound[collection_index]),
                        None,
                        &request.sort,
                        false,
                    );
                let field_candidates = matches!(
                    request.backend,
                    JsonQueryBackend::Auto | JsonQueryBackend::Bigram
                )
                .then(|| {
                    self.selected_field_candidates(collection, snapshot, &bound[collection_index])
                })
                .flatten();
                used_field_candidates |= field_candidates.is_some();
                let candidates = if field_candidates.is_some() {
                    field_candidates
                } else {
                    let native = self.selected_native_candidates(
                        collection,
                        snapshot,
                        &bound[collection_index],
                        request.backend,
                    )?;
                    used_native_candidates |= native.is_some();
                    native
                };
                for index in snapshot.scan_slots(candidates.as_deref(), sort_buffer.is_none()) {
                    self.metrics.selected_candidate_visits += 1;
                    self.metrics.selected_descriptor_visits += 1;
                    let source = &snapshot.sources[index];
                    let record = self.selected_record_for_evaluation(
                        collection,
                        source,
                        &mut bytes,
                        needs_status,
                        projection_roots.as_ref(),
                    )?;
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
                    if stream_early
                        && matches <= cap
                        && matches > request.offset
                        && matches - request.offset <= request.limit
                    {
                        let checked = self.selected_record(collection, source, &mut bytes, true)?;
                        let checked_stamp = Stamp {
                            size: checked.size_bytes,
                            modified: checked.modified_unix_nanos.clone(),
                            identity: source.revision.clone(),
                        };
                        if !bound[collection_index].matches_source(
                            &checked.data,
                            &checked.id,
                            &checked.path,
                            &checked_stamp,
                            created,
                            checked.state.status,
                        )? {
                            return Err(Error::Unstable(source.locator.clone()));
                        }
                        on_record.as_mut().expect("stream callback")(returned + 1, &checked)?;
                        returned += 1;
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
        for &slot in if stream_early { &[][..] } else { page } {
            let (collection_index, local) = source_location(&offsets, slot)
                .ok_or_else(|| Error::Cache("selected source slot is unavailable".into()))?;
            let (collection, snapshot) = &selected[collection_index];
            self.metrics.selected_descriptor_visits += 1;
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
            if let Some(callback) = on_record.as_mut() {
                callback(returned + 1, &record)?;
                returned += 1;
            } else {
                records.push(record);
                returned += 1;
            }
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
                        if used_field_candidates {
                            "selected-scalar-posting"
                        } else if used_native_candidates {
                            "selected-native-posting+custom-fallback"
                        } else {
                            "selected-exact-scan"
                        }
                    }
                    .into(),
                ],
            },
            pending,
            returned,
        ))
    }
}
