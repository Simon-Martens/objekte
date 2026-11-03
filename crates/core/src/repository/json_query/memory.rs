//! In-memory bigram and typed-index JSON query execution.

use super::*;
use crate::{
    Field,
    file_picker::SlotSelection,
    json_filter::{FileMetadata, FtsPlan, MetadataDecision, MetadataPathHint},
    repository::{
        ordered_result_cache::{OrderedCoverage, OrderedResultKey},
        predicate_cache::PredicateCacheKey,
    },
};
use grep_searcher::{Searcher, SearcherBuilder};
use regex::bytes::Regex;
use std::sync::OnceLock;

static ESCAPED_KEY: OnceLock<Regex> = OnceLock::new();

thread_local! {
    static SCAN_SCRATCH: std::cell::RefCell<Option<(Searcher, Vec<u8>)>> =
        const { std::cell::RefCell::new(None) };
}


/// Per-thread searcher and read buffer. Rayon runs `map_init` for every split
/// job, and building a `Searcher` zero-fills about 72 KiB of line buffers.
struct ScanScratch(Option<(Searcher, Vec<u8>)>);


impl ScanScratch {
    fn take() -> Self {
        Self(Some(
            SCAN_SCRATCH
                .with_borrow_mut(Option::take)
                .unwrap_or_else(|| {
                    (
                        SearcherBuilder::new()
                            .line_number(false)
                            .bom_sniffing(false)
                            .build(),
                        Vec::with_capacity(4096),
                    )
                }),
        ))
    }

    fn parts(&mut self) -> (&mut Searcher, &mut Vec<u8>) {
        let (searcher, bytes) = self.0.as_mut().expect("scratch is returned only on drop");
        (searcher, bytes)
    }
}


impl Drop for ScanScratch {
    fn drop(&mut self) {
        let scratch = self.0.take();
        let _ = SCAN_SCRATCH.try_with(|slot| {
            if let Ok(mut slot) = slot.try_borrow_mut() {
                *slot = scratch;
            }
        });
    }
}

fn known_truth(
    entry: &crate::repository::predicate_cache::VerifiedPredicateEntry,
    slot: usize,
) -> Option<bool> {
    entry.known(slot)
}

/// A verified conjunction plus exact scalar postings can prove a larger
/// conjunction without opening its non-page source files.
fn complete_cached_conjunction(
    cache: &mut crate::repository::predicate_cache::VerifiedPredicateCache,
    filter: &JsonFilter,
    collection: &str,
    picker: &crate::file_picker::FilePicker,
) -> Option<Vec<u64>> {
    let terms = filter.cache_conjuncts();
    if !(2..=8).contains(&terms.len()) {
        return None;
    }
    let full_mask = (1_usize << terms.len()) - 1;
    let mut best = None::<(usize, Vec<u64>)>;
    for mask in 1..full_mask {
        let expression = if mask.count_ones() == 1 {
            terms[mask.trailing_zeros() as usize].cache_expression()
        } else {
            let indices = (0..terms.len())
                .filter(|index| mask & (1 << index) != 0)
                .collect::<Vec<_>>();
            filter.cache_subset(&indices).cache_expression()
        };
        let key = PredicateCacheKey {
            collection: collection.to_owned(),
            expression,
        };
        if let Some(entry) = cache.get(&key).filter(|entry| entry.complete)
            && best
                .as_ref()
                .is_none_or(|(previous, _)| mask.count_ones() > previous.count_ones())
        {
            best = Some((mask, entry.matched.clone()));
        }
    }
    let (mask, mut matches) = best?;
    for (index, term) in terms.iter().enumerate() {
        if mask & (1 << index) != 0 {
            continue;
        }
        let posting = picker.exact_match_bits(&term.exact_index_plan()?)?;
        for (word, next) in matches.iter_mut().zip(posting) {
            *word &= next;
        }
    }
    Some(matches)
}

fn required_key_prefilters(required: &[String], fields: &BTreeMap<String, Field>) -> Vec<Regex> {
    required
        .iter()
        .filter(|name| !fields.contains_key(*name))
        .filter(|name| {
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
        .map(|name| {
            Regex::new(&regex::escape(&format!("\"{name}\"")))
                .expect("escaped ASCII key is a valid regex")
        })
        .collect()
}

fn fts_candidate_bits(picker: &crate::file_picker::FilePicker, plan: &FtsPlan) -> Option<Vec<u64>> {
    match plan {
        FtsPlan::Literal(literal, path) => {
            picker.text_field_candidate_bits(literal, path, true, true)
        }
        FtsPlan::And(children) => {
            let mut children = children.iter();
            let mut bits = children
                .next()
                .map(|child| fts_candidate_bits(picker, child))
                .unwrap_or_else(|| Some(vec![0; picker.slot_count().div_ceil(64)]));
            for child in children {
                let next = fts_candidate_bits(picker, child);
                bits = match (bits, next) {
                    (Some(mut left), Some(right)) => {
                        for (word, next) in left.iter_mut().zip(right) {
                            *word &= next;
                        }
                        Some(left)
                    }
                    (left @ Some(_), None) => left,
                    (None, right) => right,
                };
            }
            bits
        }
        FtsPlan::Or(children) => {
            let mut bits = Some(vec![0; picker.slot_count().div_ceil(64)]);
            for child in children {
                let next = fts_candidate_bits(picker, child);
                bits = match (bits, next) {
                    (Some(mut left), Some(right)) => {
                        for (word, next) in left.iter_mut().zip(right) {
                            *word |= next;
                        }
                        Some(left)
                    }
                    _ => None,
                };
            }
            bits
        }
    }
}


impl Repository {
    /// Keep the picker available for memory queries, views, and reconciliation.
    pub(in crate::repository) fn ensure_picker(&mut self, c: &Collection) -> Result<()> {
        let rebuild = !self.file_pickers.contains_key(&c.name)
            || matches!(self.options.freshness, Freshness::HashOnRequest);
        if rebuild {
            self.query_cache.invalidate(&c.name);
            let started = Instant::now();
            let cache_path = self.bigram_cache_path(&c.name)?;
            let cache_present = cache_path.is_file();
            let snapshot = cache_present
                .then(|| BigramSnapshot::load_scoped(&cache_path, &c.path, c.recursive).ok())
                .flatten();
            let mut picker = crate::file_picker::FilePicker::open_with_indexes(
                &c.path,
                c.recursive,
                &c.indexes,
                &c.advanced_index_paths,
                &c.sort_indexes,
                &c.fields,
                c.schema
                    .as_ref()
                    .and_then(|schema| schema.field.language.as_deref()),
                snapshot,
                cache_present.then_some(cache_path.as_path()),
                c.bigram_encoding.unwrap_or(self.options.bigram_encoding),
            )?;
            // A saved overlay of many changed files would disable indexed lookups.
            let compacted = picker.compact_large_overlay()?;
            if cache_present && (compacted || !picker.loaded_snapshot() || picker.snapshot_dirty())
            {
                let _ = picker.save_bigram_snapshot(&cache_path);
            } else if cache_present && !picker.secondary_loaded() {
                let _ = picker.save_secondary_snapshot(&cache_path);
            }
            if picker.loaded_snapshot() {
                self.metrics.bigram_snapshot_loads += 1;
            }
            if picker.secondary_loaded() {
                self.metrics.secondary_snapshot_loads += 1;
            }
            if let Some(paths) = self.requested_postings.get(&c.name) {
                let additional = paths
                    .iter()
                    .filter(|path| !c.advanced_index_paths.contains(path))
                    .cloned()
                    .collect::<Vec<_>>();
                picker.build_postings(&additional)?;
            }
            if let Some(paths) = self.auto_postings.get(&c.name) {
                let inferred_only = paths
                    .iter()
                    .filter(|path| !c.advanced_index_paths.contains(path))
                    .cloned()
                    .collect();
                picker.restore_inferred_paths(&inferred_only);
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
                self.metrics.bigram_rendered_compress_nanos += timings.rendered_compress.wall_nanos;
                self.metrics.bigram_field_compress_nanos += timings.field_compress.wall_nanos;
                self.metrics.bigram_typed_nanos += timings.typed_nanos;
                self.metrics.bigram_view_nanos += timings.view_nanos;
            }
        } else if self.options.freshness == Freshness::MetadataOnRequest {
            let started = Instant::now();
            let changed = self
                .file_pickers
                .get_mut(&c.name)
                .expect("picker exists")
                .refresh()?;
            if changed > 0 {
                self.query_cache.invalidate(&c.name);
                self.autosave_loaded_bigram_cache(&c.name)?;
            }
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
        stop_after: Option<usize>,
    ) -> Result<()> {
        result.query_plan =
            vec!["in-memory bigram, typed, and presence candidates; source verification".into()];
        let mut matched = 0;
        let mut pending_cache = Vec::new();
        let mut pending_ordered = Vec::new();
        let evictions_before = self.query_cache.evictions();
        let escaped_key =
            ESCAPED_KEY.get_or_init(|| Regex::new(r"\\u").expect("literal Unicode escape regex"));
        for c in collections {
            if stop_after.is_some_and(|limit| matched >= limit) {
                break;
            }
            let filter = self.bind_filter(filter, c)?;
            let relative_collection = c
                .path
                .strip_prefix(&self.model.root)
                .expect("scoped collection")
                .to_path_buf();
            let values_prefilter = filter.values_prefilter_with_path();
            let source_pattern = filter.source_prefilter()?;
            let scalar_fields = filter.top_level_scalar_fields();
            let required_fields = filter.required_top_level_fields();
            let exact_plan = filter.exact_index_plan();
            let fts_plan = filter.fts_plan();
            self.ensure_picker(c)?;
            if fts_plan.is_some() {
                let started = Instant::now();
                if self
                    .file_pickers
                    .get_mut(&c.name)
                    .expect("picker exists")
                    .ensure_text_index()?
                {
                    self.query_cache.invalidate(&c.name);
                    self.metrics.fts_open_nanos += nanos(started);
                }
            }
            let candidate_started = Instant::now();
            let picker = self.file_pickers.get(&c.name).unwrap();
            let values_prefilter = (!filter.references_indexed_field(picker.typed_fields())
                || values_prefilter
                    .as_ref()
                    .and_then(|(_, _, path)| path.as_ref())
                    .is_some_and(|path| picker.has_text_field_path(path)))
            .then_some(values_prefilter)
            .flatten();
            let optimistic = !picker.verified();
            // Undeclared fields can have any JSON shape. Scan that collection
            // for scalar cardinality errors without slowing indexed collections.
            let cardinality_risk =
                filter.needs_cardinality_check_in(|path| picker.typed_scalar_safe(path));
            let array_risk = scalar_fields
                .iter()
                .any(|field| picker.has_array_value(field));
            let risk = cardinality_risk || array_risk;
            // This summary was built from decoded keys. It can only reject a
            // collection when every indexed file lacks a mandatory key.
            if !risk && picker.definitely_lacks_key(&required_fields) {
                continue;
            }
            let key_prefilters = if risk {
                Vec::new()
            } else {
                required_key_prefilters(&required_fields, &c.fields)
            };
            // A later collection can contain an invalid array in a scalar field,
            // so build its picker and check that risk even when the page is full.
            if !request.count && !risk && result.returned() == request.limit {
                continue;
            }
            let cache_key = (self.options.cache == CacheMode::Memory
                && self.options.freshness != Freshness::HashOnRequest
                && self.query_cache.enabled()
                && (!result.streaming || self.options.stream_query_cache)
                && picker.verified()
                && picker.slot_count() <= u32::MAX as usize
                && !risk
                && !filter.has_metadata()
                // Linked records can change without this collection changing.
                && !filter.has_relations())
            .then(|| PredicateCacheKey {
                collection: c.name.clone(),
                expression: filter.cache_expression(),
            });
            if let Some(key) = cache_key.as_ref()
                && !self
                    .query_cache
                    .get(key)
                    .is_some_and(|entry| entry.complete)
            {
                let branches = filter.cache_disjuncts();
                if branches.len() > 1 {
                    let mut union = None::<Vec<u64>>;
                    for branch in branches {
                        let branch_key = PredicateCacheKey {
                            collection: c.name.clone(),
                            expression: branch.cache_expression(),
                        };
                        let Some(entry) = self
                            .query_cache
                            .get(&branch_key)
                            .filter(|entry| entry.complete)
                        else {
                            union = None;


                            break;
                        };
                        if let Some(matched) = &mut union {
                            for (word, next) in matched.iter_mut().zip(&entry.matched) {
                                *word |= next;
                            }
                        } else {
                            union = Some(entry.matched.clone());
                        }
                    }
                    if let Some(matched) = union {
                        self.query_cache.record_bits(
                            key.clone(),
                            vec![u64::MAX; matched.len()],
                            matched,
                            true,
                        );
                    }
                }
            }
            if let Some(key) = cache_key.as_ref()
                && result.on_candidate.is_none()
                && !self
                    .query_cache
                    .get(key)
                    .is_some_and(|entry| entry.complete)
                && let Some(matches) =
                    complete_cached_conjunction(&mut self.query_cache, &filter, &c.name, picker)
            {
                self.query_cache.record_bits(
                    key.clone(),
                    vec![u64::MAX; matches.len()],
                    matches,
                    true,
                );
            }
            if let Some(key) = cache_key.as_ref()
                && result.on_candidate.is_none()
                && (request.sort.is_empty() || picker.has_sort_index(&request.sort))
            {
                let order_key = OrderedResultKey {
                    predicate: key.clone(),
                    sort: serde_json::to_vec(&request.sort).expect("sort serializes"),
                };
                if let Some((slots, coverage)) = self.query_cache.ordered(&order_key)
                    && coverage != OrderedCoverage::Complete
                {
                    let capped_count = request.count
                        && coverage == OrderedCoverage::CappedPrefix
                        && collections.len() == 1
                        && stop_after.is_some();
                    let start = request.offset.saturating_sub(matched).min(slots.len());
                    let remaining = request.limit.saturating_sub(result.returned());
                    if (!request.count || capped_count)
                        && remaining > 0
                        && start.saturating_add(remaining) <= slots.len()
                    {
                        let page = slots[start..start + remaining]
                            .iter()
                            .map(|&slot| picker.view_record(slot as usize))
                            .collect::<Vec<_>>();
                        Self::query_json_metadata_records(
                            &mut self.metrics,
                            super::exact::ExactQuery {
                                filter: &filter,
                                collection: c,
                                root: &self.model.root,
                            },
                            &page,
                            result,
                        )?;
                        self.metrics.ordered_result_hits += 1;
                        matched += if capped_count {
                            self.options.total_limit.saturating_add(1)
                        } else {
                            slots.len()
                        };
                        self.metrics.bigram_candidates_nanos += nanos(candidate_started);
                        continue;
                    }
                }
            }
            if let Some(key) = cache_key.as_ref()
                && result.on_candidate.is_none()
                && (request.sort.is_empty() || picker.has_sort_index(&request.sort))
                && let Some(bits) = self
                    .query_cache
                    .get(key)
                    .and_then(|entry| entry.complete.then(|| entry.matched.clone()))
            {
                let order_key = OrderedResultKey {
                    predicate: key.clone(),
                    sort: serde_json::to_vec(&request.sort).expect("sort serializes"),
                };
                let (count, page_slots) = if let Some((slots, OrderedCoverage::Complete)) =
                    self.query_cache.ordered(&order_key)
                {
                    self.metrics.ordered_result_hits += 1;
                    let start = request.offset.saturating_sub(matched).min(slots.len());
                    let end = start
                        .saturating_add(request.limit.saturating_sub(result.returned()))
                        .min(slots.len());
                    (slots.len(), slots[start..end].to_vec())
                } else {
                    let mut selection = SlotSelection::from_bits(picker, Some(bits));
                    selection.order_sparse(picker, request.sort.is_empty(), &request.sort);
                    let slots = selection
                        .ordered(picker, request.sort.is_empty(), &request.sort)
                        .map(|slot| u32::try_from(slot).expect("picker slots fit u32"))
                        .collect::<Vec<_>>();
                    let start = request.offset.saturating_sub(matched).min(slots.len());
                    let end = start
                        .saturating_add(request.limit.saturating_sub(result.returned()))
                        .min(slots.len());
                    let page = slots[start..end].to_vec();
                    let count = slots.len();
                    self.query_cache.insert_ordered(order_key, slots);
                    (count, page)
                };
                self.metrics.predicate_cache_hits += 1;
                let page = page_slots
                    .iter()
                    .map(|&slot| picker.view_record(slot as usize))
                    .collect::<Vec<_>>();
                Self::query_json_metadata_records(
                    &mut self.metrics,
                    super::exact::ExactQuery {
                        filter: &filter,
                        collection: c,
                        root: &self.model.root,
                    },
                    &page,
                    result,
                )?;
                matched += count;
                if !request.sort.is_empty() {
                    result
                        .query_plan
                        .push("schema-declared ordered sort index".into());
                }
                self.metrics.bigram_candidates_nanos += nanos(candidate_started);
                continue;
            }
            if request.sort.is_empty()
                && !optimistic
                && !risk
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
                        filter: &filter,
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
            let mut selection = picker.candidate_slot_selection_for_path(
                (!risk)
                    .then(|| {
                        values_prefilter
                            .as_ref()
                            .map(|(_, pattern, _)| pattern.as_str())
                    })
                    .flatten(),
                (!risk)
                    .then(|| {
                        values_prefilter
                            .as_ref()
                            .and_then(|(_, _, path)| path.as_ref())
                    })
                    .flatten(),
                typed_plan.as_ref(),
                if risk { &[] } else { &required_fields },
            );
            if filter.has_metadata() && !risk {
                let relative = relative_collection.to_string_lossy().replace('\\', "/");
                let root = if relative.is_empty() {
                    String::new()
                } else {
                    format!("{relative}/")
                };
                if let Some(hint) = filter.metadata_path_hint() {
                    let disjoint = match hint {
                        MetadataPathHint::Exact(path) => !path.starts_with(&root),
                        MetadataPathHint::Prefix(prefix) => {
                            !prefix.starts_with(&root) && !root.starts_with(&prefix)
                        }
                    };
                    if disjoint {
                        selection.clear();
                    }
                }
                if let Some(hint) = filter.metadata_id_hint()
                    && let Some(ids) = picker.metadata_id_selection(&hint)
                {
                    selection.intersect(picker, ids);
                }
                if selection.len() > 0 && !filter.metadata_path_covers_root(&root) {
                    let needs_path = filter.uses_source_path();
                    // One buffer serves every slot's repository-relative path.
                    let mut path = String::new();
                    selection.retain(picker, |slot| {
                        let (id, stamp) = picker.view_record(slot);
                        path.clear();
                        if needs_path {
                            if !relative.is_empty() {
                                path.push_str(&relative);
                                path.push('/');
                            }
                            path.push_str(id);
                            path.push_str(".json");
                        }
                        filter.metadata_decision(&FileMetadata {
                            id,
                            path: &path,
                            size: stamp.size,
                            modified: stamp.modified.parse().expect("validated timestamp"),
                            created: None,
                            status: RecordStatus::Pending,
                        }) != MetadataDecision::False
                    });
                }
            }
            if !risk && let Some(plan) = &fts_plan {
                let bits = fts_candidate_bits(picker, plan);
                self.metrics.fts_candidates += bits
                    .as_ref()
                    .map_or(picker.len(), |bits| picker.count_live_bits(bits))
                    as u64;
                selection.intersect_bits(picker, bits);
                if !result
                    .query_plan
                    .iter()
                    .any(|step| step.contains("full-text"))
                {
                    result
                        .query_plan
                        .push("rendered full-text bigram candidates".into());
                }
            }
            if !risk && filter.has_relations() {
                // Semi-join: the linked collection was queried first; keep
                // records whose relation values include a matching ID.
                for (path, ids) in filter.relation_semijoins(|path| {
                    picker.typed_scalar_safe(path)
                        || matches!(path.as_slice(), [crate::JsonPathStep::Key(field)]
                            if !picker.has_array_value(field))
                }) {
                    selection.intersect_bits(picker, picker.relation_candidate_bits(&path, &ids));
                    result
                        .query_plan
                        .push(format!("relation semi-join: {} linked IDs", ids.len()));
                }
            }
            let mut conjuncts = Vec::new();
            let mut term_keys = Vec::new();
            let mut term_snapshots = Vec::new();
            let mut omitted_terms = Vec::new();
            let mut full_snapshot = None;
            let mut disjunction = false;
            if let Some(key) = cache_key.as_ref()
                && result.on_candidate.is_none()
            {
                full_snapshot = self.query_cache.get(key).cloned();
                let disjuncts = filter.cache_disjuncts();
                disjunction = disjuncts.len() > 1;
                conjuncts = if disjunction {
                    disjuncts
                } else {
                    filter.cache_conjuncts()
                };
                omitted_terms = vec![false; conjuncts.len()];
                if !disjunction && (2..=8).contains(&conjuncts.len()) {
                    let mut best = None;
                    for mask in 1_usize..(1_usize << conjuncts.len()) - 1 {
                        if mask.count_ones() < 2 {
                            continue;
                        }
                        let indices = (0..conjuncts.len())
                            .filter(|index| mask & (1 << index) != 0)
                            .collect::<Vec<_>>();
                        let subset = filter.cache_subset(&indices);
                        let key = PredicateCacheKey {
                            collection: c.name.clone(),
                            expression: subset.cache_expression(),
                        };
                        if let Some(entry) = self.query_cache.get(&key)
                            && entry.complete
                            && best.as_ref().is_none_or(
                                |(_, size, _): &(Vec<usize>, usize, Vec<u64>)| {
                                    indices.len() > *size
                                },
                            )
                        {
                            best =
                                Some((indices, mask.count_ones() as usize, entry.matched.clone()));
                        }
                    }
                    if let Some((indices, _, bits)) = best {
                        selection.intersect_bits(picker, Some(bits));
                        for index in indices {
                            omitted_terms[index] = true;
                        }
                        self.metrics.predicate_cache_hits += 1;
                    }
                }
                for (index, term) in conjuncts.iter().enumerate() {
                    if omitted_terms[index] || term.exact_index_plan().is_some() {
                        term_keys.push(None);
                        term_snapshots.push(None);
                        continue;
                    }
                    let term_key = PredicateCacheKey {
                        collection: c.name.clone(),
                        expression: term.cache_expression(),
                    };
                    let snapshot = self.query_cache.get(&term_key).cloned();
                    if !disjunction
                        && let Some(entry) = snapshot.as_ref().filter(|entry| entry.complete)
                    {
                        selection.intersect_bits(picker, Some(entry.matched.clone()));
                        omitted_terms[index] = true;
                        self.metrics.predicate_cache_hits += 1;
                        term_keys.push(None);
                        term_snapshots.push(None);
                        continue;
                    }
                    term_snapshots.push(snapshot);
                    term_keys.push(Some(term_key));
                }
            }
            let indexed_sort =
                selection.order_sparse(picker, request.sort.is_empty(), &request.sort);
            if !request.sort.is_empty() && indexed_sort {
                result
                    .query_plan
                    .push("schema-declared ordered sort index".into());
            }
            self.metrics.bigram_candidates_nanos += nanos(candidate_started);
            if filter.metadata_only()
                && picker.metadata_exact_ready()
                && !risk
                && result.on_candidate.is_none()
                && (request.sort.is_empty() || indexed_sort)
            {
                result
                    .query_plan
                    .push("exact inventory metadata; source page verification".into());
                let count = selection.len();
                let page = selection
                    .ordered(picker, request.sort.is_empty(), &request.sort)
                    .skip(request.offset.saturating_sub(matched))
                    .take(request.limit.saturating_sub(result.returned()))
                    .map(|slot| picker.view_record(slot))
                    .collect::<Vec<_>>();
                Self::query_json_metadata_records(
                    &mut self.metrics,
                    super::exact::ExactQuery {
                        filter: &filter,
                        collection: c,
                        root: &self.model.root,
                    },
                    &page,
                    result,
                )?;
                matched += count;
                continue;
            }
            let prefilter = if risk { None } else { source_pattern.as_ref() };
            let source = Source::open_scoped(&c.path, c.recursive)?;
            // Counted queries and global-cardinality checks scan every
            // candidate unless an indexed sort reaches its total cap. For
            // capped pages, grow batches across misses to reduce barriers
            // and reset after matches when the index was selective.
            let full_scan = (request.count || risk) && stop_after.is_none();
            let base_chunk = rayon::current_num_threads()
                .saturating_mul(4)
                .clamp(32, 256);
            let strong_prefilter = selection.len().saturating_mul(2) < picker.len();
            let max_chunk = if strong_prefilter {
                base_chunk.saturating_mul(8)
            } else {
                base_chunk.saturating_mul(256).max(8192)
            };
            let mut chunk_size = if full_scan {
                // Count-only scans carry small outcomes and benefit from fewer
                // parallel barriers. Keep streaming and retained-document
                // batches smaller for callback progression and bounded memory.
                if result.on_candidate.is_some() || result.streaming {
                    rayon::current_num_threads()
                        .saturating_mul(8)
                        .clamp(64, 512)
                } else {
                    4096
                }
            } else {
                base_chunk
            };
            let mut entries = selection.ordered(picker, request.sort.is_empty(), &request.sort);
            let mut observed = cache_key.as_ref().map(|_| {
                (
                    vec![0_u64; picker.slot_count().div_ceil(64)],
                    vec![0_u64; picker.slot_count().div_ceil(64)],
                )
            });
            let mut ordered_matches = Vec::new();
            let max_ordered_slots = self.options.query_cache_bytes / 8;
            let mut ordered_truncated = false;
            let mut term_observed = term_keys
                .iter()
                .map(|key| {
                    key.as_ref().map(|_| {
                        (
                            vec![0_u64; picker.slot_count().div_ceil(64)],
                            vec![0_u64; picker.slot_count().div_ceil(64)],
                        )
                    })
                })
                .collect::<Vec<_>>();
            let mut complete = false;
            loop {
                let batch = entries.by_ref().take(chunk_size).collect::<Vec<_>>();
                if batch.is_empty() {
                    complete = true;
                    break;
                }
                let mut page_matches = Vec::new();
                let capture_candidate = result.on_candidate.is_some();
                // Captured documents and @status validation need every member.
                let prune_keys = (!capture_candidate && !filter.uses_status())
                    .then(|| filter.top_level_keys())
                    .flatten();
                let matched_before = matched;
                let scan_started = Instant::now();
                let outcomes = batch
                    .par_iter()
                    .map_init(ScanScratch::take, |scratch, &slot| -> Result<ScanOutcome> {
                        let (searcher, bytes) = scratch.parts();
                        if let Some(matches) = full_snapshot
                            .as_ref()
                            .and_then(|entry| known_truth(entry, slot))
                        {
                            return Ok(ScanOutcome {
                                matches,
                                cache_skipped: true,
                                ..Default::default()
                            });
                        }
                        let mut cached_terms = 0;
                        let mut known_result = None;
                        for (index, snapshot) in term_snapshots.iter().enumerate() {
                            if omitted_terms[index] {
                                continue;
                            }
                            if let Some(matches) =
                                snapshot.as_ref().and_then(|entry| known_truth(entry, slot))
                            {
                                cached_terms += 1;
                                if matches == disjunction {
                                    known_result = Some(matches);
                                    break;
                                }
                            }
                        }
                        if let Some(matches) = known_result {
                            return Ok(ScanOutcome {
                                matches,
                                cache_skipped: true,
                                cached_terms,
                                ..Default::default()
                            });
                        }
                        let (id, stamp) = picker.view_record(slot);
                        let open_started = Instant::now();
                        let path = source.path.join(format!("{id}.json"));
                        let mut file = match source.file(id) {
                            Ok(file) => file,
                            Err(Error::Io { source: error, .. })
                                if optimistic && error.kind() == std::io::ErrorKind::NotFound =>
                            {
                                return Ok(ScanOutcome::default());
                            }
                            Err(error) => return Err(error),
                        };
                        let before_metadata = file.metadata().map_err(|e| io(&path, e))?;
                        let before = Stamp::from_metadata(&before_metadata, &path)?;
                        let created = crate::source::created_timestamp(&before_metadata);
                        if !optimistic && &before != stamp {
                            return Err(Error::Unstable(path));
                        }
                        let file_open_nanos = nanos(open_started);
                        let read_started = Instant::now();
                        bytes.clear();
                        file.read_to_end(bytes).map_err(|e| io(&path, e))?;
                        let read_nanos = nanos(read_started);
                        let regex_started = Instant::now();
                        let source_candidate = if let Some((matcher, _)) = prefilter {
                            let mut sink = FirstMatch::default();
                            searcher
                                .search_slice(matcher, bytes, &mut sink)
                                .map_err(|e| io(&path, e))?;
                            sink.0
                        } else {
                            true
                        };
                        // A missing literal top-level key cannot match. Any
                        // Unicode escape may encode a key, so keep those files.
                        let candidate = source_candidate
                            && (key_prefilters.is_empty()
                                || escaped_key.is_match(bytes)
                                || key_prefilters.iter().all(|regex| regex.is_match(bytes)));
                        let regex_nanos = nanos(regex_started);
                        let mut outcome = ScanOutcome {
                            candidate,
                            file_open_nanos,
                            read_nanos,
                            regex_nanos,
                            cached_terms,
                            ..Default::default()
                        };
                        let mut matched_document = None;
                        if candidate {
                            outcome.source_bytes = bytes.len() as u64;
                            let t = Instant::now();
                            let mut doc = match &prune_keys {
                                Some(keys) => Document::parse_pruned(bytes, keys),
                                None => Document::parse(bytes),
                            };
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
                            if conjuncts.is_empty() {
                                outcome.matches = filter.matches_source(
                                    &doc.data,
                                    id,
                                    metadata_path.as_deref().unwrap_or(""),
                                    &before,
                                    created,
                                    doc.state.status,
                                )?;
                            } else if doc.state.status != RecordStatus::IncorrectJson {
                                filter.check_cardinality(&doc.data)?;
                                outcome.matches = !disjunction;
                                outcome.term_results =
                                    smallvec::SmallVec::from_elem(0_u8, conjuncts.len());
                                for (index, term) in conjuncts.iter().enumerate() {
                                    if omitted_terms[index] {
                                        continue;
                                    }
                                    let matches = if let Some(known) = term_snapshots[index]
                                        .as_ref()
                                        .and_then(|entry| known_truth(entry, slot))
                                    {
                                        known
                                    } else {
                                        let matches = term.matches(&doc.data)?;
                                        outcome.term_results[index] = if matches { 2 } else { 1 };
                                        matches
                                    };
                                    if matches == disjunction {
                                        outcome.matches = matches;
                                        break;
                                    }
                                }
                            }
                            outcome.query_nanos = nanos(t);
                            if outcome.matches && capture_candidate {
                                matched_document = Some(doc);
                            }
                        }
                        if bytes.len() as u64 != before.size {
                            return Err(Error::Unstable(path));
                        }
                        // Uncaptured page records are read and checked again
                        // before they are returned, so only a captured document
                        // needs a second stat against an edit during this read.
                        if let Some(doc) = matched_document {
                            let after = Stamp::from_metadata(
                                &file.metadata().map_err(|e| io(&path, e))?,
                                &path,
                            )?;
                            if before != after {
                                return Err(Error::Unstable(path));
                            }
                            let started = Instant::now();
                            let hash = digest(bytes);
                            outcome.hash_nanos = nanos(started);
                            // Only metadata predicates can match malformed JSON;
                            // its record returns the source bytes.
                            let raw = (doc.state.status == RecordStatus::IncorrectJson)
                                .then(|| bytes.clone());
                            outcome.projected = Some((doc, hash, after, raw));
                        }
                        Ok(outcome)
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.metrics.grep_scans += outcomes
                    .iter()
                    .filter(|outcome| !outcome.cache_skipped)
                    .count() as u64;
                self.metrics.grep_bytes += batch
                    .iter()
                    .zip(&outcomes)
                    .filter(|(_, outcome)| !outcome.cache_skipped)
                    .map(|(&slot, _)| picker.view_record(slot).1.size)
                    .sum::<u64>();
                self.metrics.grep_nanos += nanos(scan_started);
                for (&slot, mut outcome) in batch.iter().zip(outcomes) {
                    self.metrics.predicate_cache_hits += u64::from(outcome.cache_skipped);
                    self.metrics.partial_evaluation_hits += outcome.cached_terms;
                    for (index, &value) in outcome.term_results.iter().enumerate() {
                        if value == 0 {
                            continue;
                        }
                        if let Some((evaluated, matched)) = term_observed[index].as_mut() {
                            let bit = 1_u64 << (slot % 64);
                            evaluated[slot / 64] |= bit;
                            if value == 2 {
                                matched[slot / 64] |= bit;
                            }
                        }
                    }
                    if let Some((evaluated, matches)) = observed.as_mut() {
                        let bit = 1_u64 << (slot % 64);
                        evaluated[slot / 64] |= bit;
                        if outcome.matches {
                            matches[slot / 64] |= bit;
                        }
                    }
                    let (id, stamp) = picker.view_record(slot);
                    self.metrics.bigram_file_open_nanos += outcome.file_open_nanos;
                    self.metrics.bigram_read_nanos += outcome.read_nanos;
                    self.metrics.bigram_regex_nanos += outcome.regex_nanos;
                    if outcome.projected.is_some() {
                        self.metrics.hash_nanos += outcome.hash_nanos;
                        self.metrics.hashes += 1;
                    }
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
                        if cache_key.is_some() {
                            if ordered_matches.len() < max_ordered_slots {
                                ordered_matches
                                    .push(u32::try_from(slot).expect("picker slots fit u32"));
                            } else {
                                ordered_truncated = true;
                            }
                        }
                        if matched >= request.offset
                            && result.returned() + page_matches.len() < request.limit
                        {
                            if capture_candidate {
                                let (document, hash, actual_stamp, raw) = outcome
                                    .projected
                                    .take()
                                    .expect("sorted match has parsed document");
                                result.push_candidate(c, id, &actual_stamp, document, hash, raw)?;
                            } else {
                                page_matches.push((id.to_owned(), stamp.clone()));
                            }
                        }
                        matched += 1;
                    }
                }
                let materialize_started = Instant::now();
                for (id, stamp) in page_matches {
                    let raw = match Self::read_raw(
                        &mut self.metrics,
                        &source,
                        &id,
                        (!optimistic).then_some(&stamp),
                    ) {
                        Ok(raw) => raw,
                        Err(Error::Io { source: error, .. })
                            if optimistic && error.kind() == std::io::ErrorKind::NotFound =>
                        {
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    let doc = Self::parse_value(&mut self.metrics, c, &id, &raw.json, true);
                    let metadata_path = filter.uses_source_path().then(|| self.path(c, &id));
                    if !filter.matches_source(
                        &doc.data,
                        &id,
                        metadata_path.as_deref().unwrap_or(""),
                        &raw.stamp,
                        raw.created,
                        doc.state.status,
                    )? {
                        if optimistic {
                            continue;
                        }
                        return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                    }
                    let malformed = doc.state.status == RecordStatus::IncorrectJson;
                    result.push(Record {
                        collection: c.name.clone(),
                        id: id.clone(),
                        path: metadata_path.unwrap_or_else(|| self.path(c, &id)),
                        size_bytes: raw.stamp.size,
                        modified_unix_nanos: raw.stamp.modified.clone(),
                        hash: raw.hash,
                        data: doc.data,
                        state: doc.state,
                        raw: malformed.then_some(raw.json),
                    })?;
                }
                self.metrics.bigram_materialize_nanos += nanos(materialize_started);
                if stop_after.is_some_and(|limit| matched >= limit) {
                    break;
                }
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
            source.finish()?;
            if let (Some(key), Some((evaluated, matches))) = (cache_key, observed) {
                if request.sort.is_empty() || indexed_sort {
                    let coverage = if ordered_truncated {
                        OrderedCoverage::Prefix
                    } else if complete {
                        OrderedCoverage::Complete
                    } else if stop_after.is_some_and(|limit| matched >= limit) {
                        OrderedCoverage::CappedPrefix
                    } else {
                        OrderedCoverage::Prefix
                    };
                    pending_ordered.push((
                        OrderedResultKey {
                            predicate: key.clone(),
                            sort: serde_json::to_vec(&request.sort).expect("sort serializes"),
                        },
                        ordered_matches,
                        coverage,
                    ));
                }
                pending_cache.push((key, evaluated, matches, complete));
            }
            for (key, observed) in term_keys.into_iter().zip(term_observed) {
                if let (Some(key), Some((evaluated, matches))) = (key, observed) {
                    pending_cache.push((key, evaluated, matches, false));
                }
            }
        }
        for (key, evaluated, matches, complete) in pending_cache {
            self.query_cache
                .record_bits(key, evaluated, matches, complete);
        }
        for (key, slots, coverage) in pending_ordered {
            self.query_cache
                .insert_ordered_with_coverage(key, slots, coverage);
        }
        self.metrics.cache_evictions += self.query_cache.evictions() - evictions_before;
        result.total = request.count.then_some(matched);
        Ok(())
    }
}
