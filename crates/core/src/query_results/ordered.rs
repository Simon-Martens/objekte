//! Stored and inferred ordered traversal policies.

use super::*;
use crate::query_results::ranking::RankKey;

pub(super) struct LifecycleOrderCursor {
    pub(super) key: RankKey,
    pub(super) bytes: usize,
    pub(super) collection_index: usize,
    pub(super) segment_index: usize,
    pub(super) position: usize,
    pub(super) slot: RecordSlot,
}

impl PartialEq for LifecycleOrderCursor {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for LifecycleOrderCursor {}

impl PartialOrd for LifecycleOrderCursor {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for LifecycleOrderCursor {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

pub(super) fn read_lifecycle_ordered_record(
    repo: &Repository,
    _catalog: &FileCatalog,
    file: &FileEntry,
    record: &RecordEntry,
    slot: RecordSlot,
    reader: &mut Option<(crate::model::FileId, RecordRangeReader)>,
    max_bytes: usize,
    context: &ExecutionContext,
) -> Result<(Vec<u8>, bool), String> {
    let opened = reader.as_ref().is_none_or(|(id, _)| *id != file.id);
    if opened {
        let path = repo.files.root.join(&file.relative_path);
        *reader = Some((
            file.id,
            RecordRangeReader::open_for_search(&path, file.revision, Some(context))?,
        ));
    }
    let (_, source) = reader.as_mut().ok_or("ordered source reader missing")?;
    let (mut rows, _) = source.read_bounded(&[(slot.0, record.range)], max_bytes)?;
    Ok((rows.pop().ok_or("ordered source row missing")?.1, opened))
}

pub(super) struct StoredCursor<'a> {
    columns: &'a [(&'a SortColumn, bool)],
    order: &'a [crate::model::RecordSlot],
    record: &'a RecordEntry,
    collection_name: &'a str,
    collection_index: usize,
    plan_index: usize,
    position: usize,
}

impl PartialEq for StoredCursor<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for StoredCursor<'_> {}

impl PartialOrd for StoredCursor<'_> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for StoredCursor<'_> {
    fn cmp(&self, other: &Self) -> Ordering {
        let mut keys = Ordering::Equal;
        for ((a, descending), (b, _)) in self.columns.iter().zip(other.columns) {
            keys = a.keys[self.record.slot.0 as usize].compare_with(
                &b.keys[other.record.slot.0 as usize],
                *descending,
                a.options,
            );
            if !keys.is_eq() {
                break;
            }
        }
        keys.then_with(|| {
            (self.collection_name, &self.record.id).cmp(&(other.collection_name, &other.record.id))
        })
    }
}

pub(super) fn stored_cursor<'a>(
    repo: &'a Repository,
    columns: &'a [(&'a SortColumn, bool)],
    order: &'a [crate::model::RecordSlot],
    collection_index: usize,
    plan_index: usize,
    position: usize,
) -> Option<StoredCursor<'a>> {
    let slot = order.get(position)?;
    let collection = &repo.collections[collection_index];
    Some(StoredCursor {
        columns,
        order,
        record: &collection.generation.records[slot.0 as usize],
        collection_name: &collection.definition.name,
        collection_index,
        plan_index,
        position,
    })
}

struct OrderedPlan<'a> {
    collection_index: usize,
    filter: Option<&'a BoundExpr>,
    fts: Option<&'a BoundFts>,
    plan: Plan,
    candidates: RoaringBitmap,
}

/// Membership already prepared for a stored-order attempt whose selective
/// result is cheaper to rank with the general top-k collector.
pub(super) struct RejectedOrderPlans {
    pub(super) entries: Vec<(Plan, RoaringBitmap)>,
    pub(super) planning_ns: u64,
}

pub(crate) fn direct_payload(
    repo: &Repository,
    query: &Query,
    collection_index: usize,
    record: &RecordEntry,
    counters: &mut WorkCounters,
    actual: &mut PlanActual,
    source_access: &mut SourceAccessSession<'_>,
    stream_limit: Option<usize>,
    execution: Option<&ExecutionContext>,
) -> Result<Option<Payload>, String> {
    if query.options.return_mode == ReturnMode::Locator {
        return Ok(None);
    }
    if query.options.return_mode == ReturnMode::Logical && record.class == RecordClass::RawFallback
    {
        return Ok(Some(Payload::LogicalUnavailable {
            diagnostic_ref: record.diagnostic_ref,
        }));
    }
    let collection = &repo.collections[collection_index];
    let mut read = if let Some(limit) = stream_limit {
        source_access.read_bounded(record.file, &[(0, record.range)], limit)?
    } else {
        source_access.read(record.file, &[(0, record.range)])?
    };
    let bytes = read.records.pop().ok_or("source record missing")?.1;
    counters.files_opened += read.opened;
    counters.bytes_read += read.bytes_read;
    actual.files_opened += read.opened;
    actual.bytes_read += read.bytes_read;
    let (payload, parses) = payload_from_bytes_with_execution(
        record.class,
        Some(collection),
        &query.options.return_mode,
        bytes,
        execution,
    )?;
    counters.record_parses += parses;
    actual.record_parses += parses;
    Ok(payload)
}

pub(super) fn check_stream_page_read(
    delivery: &mut Delivery<'_>,
    query: &Query,
    page: &[SearchHit],
    record: &RecordEntry,
) -> Result<(), String> {
    let source_bytes = usize::try_from(record.range.len()).unwrap_or(usize::MAX);
    delivery.check_record_bytes(source_bytes)?;
    let payload_bytes = match query.options.return_mode {
        ReturnMode::Locator => 0,
        ReturnMode::Bytes => source_bytes,
        ReturnMode::Logical => source_bytes.saturating_mul(4),
    };
    let current = page.iter().fold(0usize, |total, hit| {
        total.saturating_add(crate::streaming::hit_bytes(hit))
    });
    delivery.check_buffer(current.saturating_add(payload_bytes))
}

pub(super) fn prepare_all_batch(
    delivery: &mut Delivery<'_>,
    page: &mut Vec<SearchHit>,
    counters: &WorkCounters,
    record: &RecordEntry,
) -> Result<(), String> {
    let current = page.iter().fold(0usize, |total, hit| {
        total.saturating_add(crate::streaming::hit_bytes(hit))
    });
    let next = usize::try_from(record.range.len()).unwrap_or(usize::MAX);
    if !page.is_empty()
        && (page.len() >= delivery.options().batch_rows
            || current.saturating_add(next) >= delivery.options().max_event_bytes
            || current.saturating_add(next) >= delivery.max_buffered_bytes())
    {
        delivery.ordered_batch(page, counters)?;
        page.clear();
    }
    Ok(())
}

pub(super) fn flush_all_batch_if_full(
    delivery: &mut Delivery<'_>,
    batch: &mut Vec<SearchHit>,
    counters: &WorkCounters,
) -> Result<(), String> {
    let mut bytes = batch.iter().fold(0usize, |total, hit| {
        total.saturating_add(crate::streaming::hit_bytes(hit))
    });
    if bytes > delivery.options().max_event_bytes && batch.len() > 1 {
        let last = batch.pop().ok_or("all-results batch missing final row")?;
        delivery.ordered_batch(batch, counters)?;
        batch.clear();
        bytes = crate::streaming::hit_bytes(&last);
        batch.push(last);
    }
    if batch.len() >= delivery.options().batch_rows || bytes > delivery.options().max_event_bytes {
        delivery.ordered_batch(batch, counters)?;
        batch.clear();
    }
    Ok(())
}

pub(super) fn inferred_order_index<'a>(
    repo: &'a Repository,
    query: &Query,
    segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
) -> Option<(usize, FieldPath, &'a PostingIndex)> {
    if query.options.sort.len() != 1 {
        return None;
    }
    let part = simple_composite_part(&query.options.sort[0])?;
    if part.path.root != PathRoot::Data {
        return None;
    }
    let mut selected = repo
        .collections
        .iter()
        .enumerate()
        .filter(|(_, collection)| {
            query.collections.is_empty() || query.collections.contains(&collection.definition.name)
        });
    let (collection_index, collection) = selected.next()?;
    if selected.next().is_some()
        || collection.sorting.columns.contains_key(&part.path)
        || collection
            .definition
            .schema
            .defaults
            .get(&part.path.segments)
            .is_some_and(|name| name != "default")
    {
        return None;
    }
    let mut selected = None;
    for segment in segments.get(collection_index)?.iter() {
        if segment.active.len() == 0 {
            continue;
        }
        let catalog = crate::index_catalog::IndexCatalog::segment(&segment.index, collection);
        let index = catalog.sortable_scalar(&part.path.segments)?;
        if selected.is_none() {
            selected = Some(index);
        }
    }
    selected.map(|index| (collection_index, part.path, index))
}

/// A posting order is useful for an unrestricted page or a filter whose
/// selected access already proves membership. With a residual FILTER/FTS,
/// filtering first preserves the verified membership cache and avoids an
/// ordered source scan across candidates that will later be rejected.
pub(super) fn posting_order_eligible(
    repo: &Repository,
    query: &Query,
    bound: Option<&crate::query::prepared::PreparedQuery>,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
) -> bool {
    let Some((collection_index, _, _)) = inferred_order_index(repo, query, segments) else {
        return false;
    };
    if query.fts.is_some() {
        return false;
    }
    if query.filter.is_none() {
        return true;
    }
    let owned = if bound.is_none() {
        crate::query::prepared::prepare(repo, query).ok()
    } else {
        None
    };
    let Some(prepared) = bound.or(owned.as_ref()) else {
        return false;
    };
    let Some(filter) = prepared
        .collections
        .iter()
        .find(|entry| entry.collection_index == collection_index)
        .and_then(|entry| entry.filter.as_ref())
    else {
        return false;
    };
    let collection = &repo.collections[collection_index];
    let mut saw_segment = false;
    for segment in segments[collection_index].iter() {
        if segment.active.len() == 0 {
            continue;
        }
        saw_segment = true;
        let logical = &segment.active.bitmap() & &segment.index.logical_records;
        let Ok(bounds) = segment_filter_bounds_with_catalog(
            &segment.index,
            &segment.active,
            filter,
            collection,
            Some(catalog),
        ) else {
            return false;
        };
        if !excludes_raw_fallback(filter)
            || (bounds.may.as_ref() & &logical) != (bounds.must.as_ref() & &logical)
        {
            return false;
        }
    }
    saw_segment
}

pub(super) fn inferred_order_column(
    repo: &Repository,
    query: &Query,
    prepared: &crate::query::prepared::PreparedQuery,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    execution: Option<&ExecutionContext>,
) -> Result<Option<(usize, Arc<SortColumn>)>, String> {
    let Some((collection_index, path, _index)) = inferred_order_index(repo, query, segments) else {
        return Ok(None);
    };
    if prepared.collections.len() != 1
        || prepared.collections[0].collection_index != collection_index
    {
        return Ok(None);
    }
    let live = segments[collection_index]
        .iter()
        .filter(|segment| segment.active.len() != 0)
        .collect::<Vec<_>>();
    let [segment] = live.as_slice() else {
        return Ok(None);
    };
    inferred_order_column_for_segment(repo, collection_index, query, &path, segment, execution)
        .map(|column| column.map(|column| (collection_index, column)))
}

pub(super) fn inferred_order_column_for_segment(
    repo: &Repository,
    collection_index: usize,
    query: &Query,
    path: &FieldPath,
    segment: &PublishedSegment,
    execution: Option<&ExecutionContext>,
) -> Result<Option<Arc<SortColumn>>, String> {
    let collection = &repo.collections[collection_index];
    let segment_index = crate::index_catalog::IndexCatalog::segment(&segment.index, collection);
    let index = segment_index
        .sortable_scalar(&path.segments)
        .ok_or("selected inferred segment lost scalar coverage")?;
    let key = InferredOrderKey::new(
        collection_index,
        collection.generation.generation,
        segment.id,
        path.clone(),
        &collection.definition.sort_locale,
        collection.definition.sort_options,
        query.options.sort[0].descending,
    );
    let column = cached_inferred_order(&repo.inferred_orders, key, || {
        SortColumn::from_inferred_posting(
            index,
            &segment.index.records,
            path.clone(),
            &collection.definition.sort_locale,
            collection.definition.sort_options,
            query.options.sort[0].descending,
            execution,
        )
    })?;
    if let (Some(column), Some(execution)) = (&column, execution) {
        execution.check_string()?;
        if column.retained_bytes() > execution.limits.inferred_order_bytes {
            return Err(execution.fail(
                ExecutionErrorKind::Resource,
                "inferred order exceeds retained-byte limit",
            ));
        }
    }
    Ok(column)
}

pub(super) fn cached_inferred_order(
    cache: &Mutex<crate::index::sorting::InferredOrderCache>,
    key: InferredOrderKey,
    build: impl FnOnce() -> Result<Option<SortColumn>, String>,
) -> Result<Option<Arc<SortColumn>>, String> {
    if let Some(column) = cache.lock().map_err(|error| error.to_string())?.get(&key) {
        return Ok(Some(column));
    }
    let Some(column) = build()? else {
        return Ok(None);
    };
    let column = cache
        .lock()
        .map_err(|error| error.to_string())?
        .insert_if_absent(key, Arc::new(column));
    Ok(Some(column))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn try_inferred_order_page(
    repo: &Repository,
    query: &Query,
    prepared: &crate::query::prepared::PreparedQuery,
    preparation_ns: u64,
    traces: Option<&mut Vec<QueryPlanTrace>>,
    mut delivery: Option<&mut Delivery<'_>>,
    all_offset: Option<usize>,
    execution: Option<&ExecutionContext>,
    active: Option<&[ActiveRecords]>,
) -> Result<Option<SearchResult>, String> {
    if !posting_order_eligible(repo, query, Some(prepared), &repo.segments, &repo.catalog) {
        return Ok(None);
    }
    if active.is_some() {
        // A static inferred column may read retired source records while it is built.
        return Ok(None);
    }
    let Some((collection_index, column)) =
        inferred_order_column(repo, query, prepared, &repo.segments, execution)?
    else {
        return Ok(None);
    };
    let terms = (0..repo.collections.len())
        .map(|position| {
            (position == collection_index).then(|| {
                vec![BoundSortTerm {
                    source: BoundSortSource::Stored(column.as_ref()),
                    descending: query.options.sort[0].descending,
                    options: column.options,
                }]
            })
        })
        .collect();
    let sort = ResolvedSort::from_bound(
        repo,
        query,
        BoundSort {
            terms,
            has_computed: false,
            needs_source: false,
        },
    );
    let mut local_traces = Vec::new();
    let result = try_stored_order_page(
        repo,
        query,
        prepared,
        &sort,
        preparation_ns,
        traces.as_ref().map(|_| &mut local_traces),
        delivery.as_deref_mut(),
        all_offset,
        false,
        None,
        execution,
        active,
    )?;
    if result.is_some() {
        if let Some(traces) = traces {
            for trace in &mut local_traces {
                trace.sort_strategy = "inferred_posting_order_page".into();
            }
            traces.extend(local_traces);
        }
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn try_inferred_segment_order_page(
    repo: &Repository,
    query: &Query,
    prepared: &crate::query::prepared::PreparedQuery,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    traces: Option<&mut Vec<QueryPlanTrace>>,
    execution: Option<&ExecutionContext>,
) -> Result<Option<SearchResult>, String> {
    if query.options.sort.len() != 1
        || query.fts.is_some()
        || base_active.iter().any(|active| active.len() != 0)
        || prepared.collections.len() != 1
        || (query.options.return_mode != ReturnMode::Locator && execution.is_some())
    {
        return Ok(None);
    }
    if !posting_order_eligible(repo, query, Some(prepared), segments, catalog) {
        return Ok(None);
    }
    let collection_index = prepared.collections[0].collection_index;
    let filter = prepared.collections[0].filter.as_ref();
    let Some((_, path, _)) = inferred_order_index(repo, query, segments) else {
        return Ok(None);
    };
    let collection = &repo.collections[collection_index];
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("page size overflow")?;
    let mut cursors = Vec::new();
    let mut candidate_count = 0u64;
    let mut input_records = 0u64;
    for segment in segments[collection_index]
        .iter()
        .filter(|segment| segment.active.len() != 0)
    {
        let Some(column) = inferred_order_column_for_segment(
            repo,
            collection_index,
            query,
            &path,
            segment,
            execution,
        )?
        else {
            return Ok(None);
        };
        let candidates = if let Some(filter) = filter {
            if !excludes_raw_fallback(filter) {
                return Ok(None);
            }
            let bounds = segment_filter_bounds_with_catalog(
                &segment.index,
                &segment.active,
                filter,
                collection,
                Some(catalog),
            )?;
            let logical = &segment.active.bitmap() & &segment.index.logical_records;
            if (bounds.may.as_ref() & &logical) != (bounds.must.as_ref() & &logical) {
                return Ok(None);
            }
            bounds.must.as_ref() & &logical
        } else {
            segment.active.bitmap().clone()
        };
        candidate_count = candidate_count.saturating_add(candidates.len());
        input_records = input_records.saturating_add(segment.active.len());
        cursors.push((segment, column, candidates, 0usize));
    }
    let evidence = if query.options.evidence {
        filter
            .and_then(segment_static_evidence_fields)
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let descending = query.options.sort[0].descending;
    let mut ranked = Vec::with_capacity(query.options.offset.saturating_add(query.options.limit));
    let mut rank = 0usize;
    while rank < cap && rank < candidate_count as usize {
        for (_, column, candidates, position) in &mut cursors {
            let order = if descending {
                &column.descending
            } else {
                &column.ascending
            };
            while let Some(slot) = order.get(*position) {
                if candidates.contains(slot.0) {
                    break;
                }
                *position += 1;
            }
        }
        let Some(best) = cursors
            .iter()
            .enumerate()
            .filter_map(|(index, cursor)| {
                let order = if descending {
                    &cursor.1.descending
                } else {
                    &cursor.1.ascending
                };
                order.get(cursor.3).map(|slot| (index, *slot))
            })
            .min_by(|(left_index, left_slot), (right_index, right_slot)| {
                let left = &cursors[*left_index];
                let right = &cursors[*right_index];
                let left_record = left
                    .0
                    .index
                    .record(*left_slot)
                    .expect("inferred cursor record");
                let right_record = right
                    .0
                    .index
                    .record(*right_slot)
                    .expect("inferred cursor record");
                left.1.keys[left_slot.0 as usize]
                    .compare_with(
                        &right.1.keys[right_slot.0 as usize],
                        descending,
                        left.1.options,
                    )
                    .then_with(|| left_record.id.cmp(&right_record.id))
            })
        else {
            break;
        };
        cursors[best.0].3 += 1;
        if rank >= query.options.offset {
            ranked.push((best.0, best.1));
        }
        rank += 1;
    }
    let mut page = ranked
        .into_iter()
        .map(|(cursor, slot)| {
            let (segment, _, _, _) = &cursors[cursor];
            let resolved = segment_record_ref(&segment.index, &segment.active, catalog, slot)?
                .ok_or("inferred ordered segment record retired")?;
            Ok((
                cursor,
                segment_locator_hit(collection, resolved, revision, evidence.clone()),
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let mut counters = WorkCounters {
        candidates: candidate_count,
        exact_posting_matches: candidate_count,
        total_matches: candidate_count,
        match_count_complete: true,
        returned_records: page.len() as u64,
        ..WorkCounters::default()
    };
    if query.options.return_mode != ReturnMode::Locator {
        let mut selected_files = HashSet::new();
        let mut logical_parses = 0u64;
        let mut retained = Vec::with_capacity(page.len());
        for (cursor, mut hit) in page {
            let segment = cursors[cursor].0;
            let source_bytes = hit.byte_end.saturating_sub(hit.byte_start);
            if let Some(execution) = execution {
                if source_bytes > execution.limits.source_record_bytes {
                    return Err(execution.fail(
                        ExecutionErrorKind::Resource,
                        "selected source record exceeds read allowance",
                    ));
                }
                execution.charge_reads(1)?;
                execution.charge_bytes(source_bytes)?;
            }
            let materialized = get_segment_record_by_id(
                repo,
                collection_index,
                &segment.index,
                &segment.active,
                catalog,
                revision,
                &hit.record_id,
                query.options.return_mode.clone(),
            );
            let mut materialized = match materialized {
                Ok(Some(materialized)) => materialized,
                Ok(None) => return Err("selected segment record retired".into()),
                Err(message) => {
                    if record_unavailable_hit(
                        repo,
                        catalog,
                        SourceHitLocator::from(&hit),
                        message.clone(),
                        &mut counters,
                    )? {
                        continue;
                    }
                    return Err(message);
                }
            };
            if query.options.return_mode == ReturnMode::Bytes
                || hit.record_class != RecordClass::RawFallback
            {
                selected_files.insert(hit.file_id);
                counters.selected_payload_reads += 1;
                counters.selected_payload_bytes += source_bytes;
            }
            if query.options.return_mode == ReturnMode::Logical
                && hit.record_class == RecordClass::Logical
            {
                logical_parses += 1;
            }
            materialized.matched_fields = std::mem::take(&mut hit.matched_fields);
            retained.push(materialized);
        }
        page = retained
            .into_iter()
            .enumerate()
            .map(|(index, hit)| (index, hit))
            .collect();
        counters.files_opened = selected_files.len() as u64;
        counters.distinct_files = selected_files.len() as u64;
        counters.record_parses = logical_parses;
        counters.returned_records = page.len() as u64;
    }
    if counters.source_unavailable_events > 0 {
        counters.match_count_complete = false;
    }
    if let Some(traces) = traces {
        traces.push(QueryPlanTrace {
            collection: collection.definition.name.clone(),
            generation: revision.0,
            bigram_alphabet: collection.raw.alphabet,
            folded_representation: crate::index::bigram::FOLDED_REPRESENTATION,
            fts_case_sensitivity: None,
            fts_mode: None,
            fts_exact_literal: None,
            fts_prefilter_reason: None,
            fts_covered_target: "none".into(),
            fts_estimated_candidates: None,
            remaining_verification: false,
            input_records,
            fts_strategy: "absent".into(),
            normalized_sort: query.options.sort.clone(),
            sort_strategy: "inferred_posting_order_page".into(),
            sort_strategy_estimated: false,
            filter_exact: true,
            terms: Vec::new(),
            planning: PlanningCounters::default(),
            bound_terms: bound_term_diagnostics(repo, filter),
            candidates: candidate_count,
            candidates_estimated: false,
            actual: None,
            observations: Vec::new(),
        });
    }
    Ok(Some(SearchResult {
        count: (query.options.count && counters.match_count_complete)
            .then_some(candidate_count as usize),
        hits: page.into_iter().map(|(_, hit)| hit).collect(),
        counters,
    }))
}

pub(super) fn try_stored_order_page(
    repo: &Repository,
    query: &Query,
    prepared: &crate::query::prepared::PreparedQuery,
    sort: &ResolvedSort<'_>,
    preparation_ns: u64,
    mut traces: Option<&mut Vec<QueryPlanTrace>>,
    mut delivery: Option<&mut Delivery<'_>>,
    all_offset: Option<usize>,
    force_order: bool,
    mut rejected: Option<&mut RejectedOrderPlans>,
    execution: Option<&ExecutionContext>,
    active: Option<&[ActiveRecords]>,
) -> Result<Option<SearchResult>, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    if sort.bound.has_computed || (query.options.count && delivery.is_none() && !force_order) {
        return Ok(None);
    }
    if query.options.sort.len() > 1
        && prepared.collections.iter().any(|entry| {
            repo.collections[entry.collection_index]
                .sorting
                .composite(&query.options.sort)
                .is_none()
        })
    {
        return Ok(None);
    }
    if all_offset.is_none()
        && (prepared.collections.iter().any(|entry| {
            entry
                .fts
                .as_ref()
                .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
        }) || prepared.collections.iter().any(|entry| {
            entry
                .filter
                .as_ref()
                .is_some_and(crate::relation_target_cache::has_source_dependent)
        }))
    {
        return Ok(None);
    }
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let planning_started = DetailedInstant::now();
    let diagnostics = traces.is_some();
    let mut plans = Vec::new();
    let mut candidate_total = 0u64;
    for entry in &prepared.collections {
        let collection_index = entry.collection_index;
        let filter = entry.filter.as_ref();
        let fts = entry.fts.as_ref();
        let collection = &repo.collections[collection_index];
        let domain = collection_domain(collection, collection_index, active);
        let mut plan = filter
            .as_ref()
            .map(|expr| {
                plan_expr_restricted_with_execution(
                    repo,
                    collection,
                    &repo.segments,
                    expr,
                    &domain,
                    &domain,
                    query.options.evidence,
                    diagnostics,
                    execution,
                    Some(Arc::clone(&prepared.predicates)),
                )
            })
            .transpose()?
            .unwrap_or(Plan {
                candidates: None,
                exact: true,
                proven_hits: RoaringBitmap::new(),
                proofs: Vec::new(),
                choices: Vec::new(),
                semijoin_work: Default::default(),
                planning: PlanningCounters::default(),
            });
        let mut candidates = plan.candidates.take().unwrap_or_else(|| domain.clone());
        candidates &= &domain;
        if let Some(fts_candidates) = fts.as_ref().and_then(|fts| fts_candidates(collection, fts)) {
            candidates &= fts_candidates;
        }
        candidate_total += candidates.len();
        plans.push(OrderedPlan {
            collection_index,
            filter,
            fts,
            plan,
            candidates,
        });
    }
    // For a selective membership set, ranking the matches is cheaper than
    // traversing the full stored list. Zero matches need no traversal.
    if !force_order
        && delivery.is_none()
        && cap > 0
        && candidate_total > 0
        && candidate_total < (cap as u64).saturating_mul(4)
    {
        if let Some(rejected) = rejected.as_deref_mut() {
            rejected.entries = plans
                .into_iter()
                .map(|entry| (entry.plan, entry.candidates))
                .collect();
            rejected.planning_ns = planning_started.elapsed().as_nanos() as u64;
        }
        return Ok(None);
    }
    let residuals = plans
        .iter()
        .map(|entry| {
            entry.filter.map(|expr| {
                prepare_residual(
                    expr,
                    &entry.plan.proofs,
                    &repo.collections[entry.collection_index],
                    entry.candidates.len(),
                    &prepared.predicates,
                )
            })
        })
        .collect::<Vec<_>>();
    let projections = plans
        .iter()
        .zip(&residuals)
        .map(|(entry, residual)| {
            let mut projection =
                scan_projection(query, residual.as_ref(), &entry.candidates, false);
            prepare_value_text_projection(
                &mut projection,
                borrowed_value_text(&repo.collections[entry.collection_index], entry.fts)
                    || borrowed_relation_source_text(
                        &repo.collections[entry.collection_index],
                        entry.fts,
                    ),
            );
            projection
        })
        .collect::<Vec<_>>();
    let direct_plans = plans
        .iter()
        .zip(&residuals)
        .map(|(entry, residual)| {
            DirectSourcePlan::prepare(
                &repo.collections[entry.collection_index],
                sort,
                entry.fts,
                residual.as_ref(),
            )
        })
        .collect::<Vec<_>>();
    if let Some(traces) = traces.as_deref_mut() {
        for entry in &plans {
            let collection = &repo.collections[entry.collection_index];
            traces.push(QueryPlanTrace {
                collection: collection.definition.name.clone(),
                generation: collection.generation.generation,
                bigram_alphabet: collection.raw.alphabet,
                folded_representation: crate::index::bigram::FOLDED_REPRESENTATION,
                fts_case_sensitivity: entry.fts.map(|fts| fts.case_sensitivity),
                fts_mode: entry.fts.map(|fts| fts.mode),
                fts_exact_literal: entry.fts.map(|fts| fts.source_literal.clone()),
                fts_prefilter_reason: regex_prefilter_reason(collection, entry.fts),
                fts_covered_target: fts_covered_target(entry.fts),
                fts_estimated_candidates: entry.fts.and_then(|fts| {
                    estimate_fts_candidates(collection, fts).map(|estimate| estimate.0)
                }),
                remaining_verification: entry.fts.is_some() || !entry.plan.exact,
                input_records: collection_domain(collection, entry.collection_index, active).len(),
                fts_strategy: if collection.logical_records.len()
                    != collection.generation.records.len() as u64
                    && entry.fts.is_some_and(|fts| fts.fields.is_empty())
                {
                    "stored order: logical text plus fallback source text"
                } else {
                    "stored order page traversal"
                }
                .into(),
                normalized_sort: query.options.sort.clone(),
                sort_strategy: if query.options.sort.len() > 1 {
                    "stored_composite_order_page"
                } else {
                    "stored_order_page"
                }
                .into(),
                sort_strategy_estimated: false,
                filter_exact: entry.plan.exact,
                terms: entry.plan.choices.clone(),
                planning: entry.plan.planning,
                bound_terms: bound_term_diagnostics(repo, entry.filter),
                candidates: entry.candidates.len(),
                candidates_estimated: false,
                actual: None,
                observations: Vec::new(),
            });
        }
    }
    let mut counters = WorkCounters {
        candidates: candidate_total,
        #[cfg(feature = "detailed-profiling")]
        preparation_ns,
        #[cfg(feature = "detailed-profiling")]
        planning_ns: planning_started.elapsed().as_nanos() as u64,
        ..WorkCounters::default()
    };
    #[cfg(not(feature = "detailed-profiling"))]
    let _ = preparation_ns;
    let mut actuals = (0..plans.len())
        .map(|_| PlanActual::default())
        .collect::<Vec<_>>();
    for (entry, actual) in plans.iter().zip(&mut actuals) {
        let work = entry.plan.semijoin_work;
        counters.add_semijoin(work)?;
        actual.files_opened += work.target_file_opens;
        actual.bytes_read += work.target_bytes_read;
        actual.record_parses += work.target_reads;
        actual.relation_target_candidates += work.target_candidates;
        actual.relation_target_matches += work.target_matches;
        actual.relation_target_reads += work.target_reads;
        actual.relation_target_file_opens += work.target_file_opens;
        actual.relation_edge_visits += work.edge_visits;
        actual.relation_reverse_postings_visited += work.reverse_postings_visited;
        actual.relation_reverse_members_visited += work.reverse_members_visited;
        actual.relation_verified_targets_reused += work.verified_targets_reused;
    }
    let mut heap = BinaryHeap::new();
    for (plan_index, entry) in plans.iter().enumerate() {
        let columns = sort.columns[entry.collection_index]
            .as_ref()
            .expect("selected collection");
        let order = if query.options.sort.len() > 1 {
            &repo.collections[entry.collection_index]
                .sorting
                .composite(&query.options.sort)
                .expect("composite checked")
                .order
        } else if let Some(field) = query.options.sort.first() {
            let column = columns[0].0;
            if field.descending {
                &column.descending
            } else {
                &column.ascending
            }
        } else {
            &repo.collections[entry.collection_index].identity_order
        };
        if let Some(cursor) = stored_cursor(
            repo,
            columns,
            order.as_slice(),
            entry.collection_index,
            plan_index,
            0,
        ) {
            heap.push(Reverse(cursor));
        }
    }
    let mut page = Vec::new();
    let mut stopped_early = false;
    let mut locator_query = query.clone();
    locator_query.options.return_mode = ReturnMode::Locator;
    // A one-record residual check still needs to retain its match even when
    // the requested page has zero rows or this record lies before the offset.
    locator_query.options.limit = 1;
    locator_query.options.offset = 0;
    let mut source_access = SourceAccessSession::new(&repo.files);
    if let Some(execution) = execution {
        source_access.configure_execution(execution);
    }
    if cap > 0 || query.options.count {
        loop {
            if let Some(execution) = execution {
                execution.check_string()?;
            }
            if let Some(delivery) = delivery.as_deref_mut() {
                delivery.check()?;
            }
            let Some(Reverse(cursor)) = heap.pop() else {
                break;
            };
            let entry = &plans[cursor.plan_index];
            let actual = &mut actuals[cursor.plan_index];
            let slot = cursor.record.slot.0;
            if entry.candidates.contains(slot) {
                let direct = (entry.plan.exact && entry.fts.is_none())
                    || (!query.options.evidence
                        && entry.fts.is_none()
                        && entry.plan.proven_hits.contains(slot));
                let mut matched = false;
                let mut evidence = HashSet::new();
                let mut payload = None;
                if direct {
                    matched = true;
                    counters.exact_posting_matches += 1;
                    actual.index_proven_matches += 1;
                    if query.options.evidence {
                        evidence = entry
                            .filter
                            .map(|expr| {
                                proven_evidence(
                                    repo,
                                    &repo.collections[entry.collection_index],
                                    cursor.record,
                                    expr,
                                    &entry.plan.proofs,
                                )
                            })
                            .transpose()?
                            .unwrap_or_default();
                    }
                    let position = counters.total_matches as usize;
                    if position >= all_offset.unwrap_or(query.options.offset)
                        && (all_offset.is_some() || position < cap)
                    {
                        if let Some(delivery) = delivery.as_deref_mut() {
                            if all_offset.is_some() {
                                prepare_all_batch(delivery, &mut page, &counters, cursor.record)?;
                            }
                            check_stream_page_read(delivery, query, &page, cursor.record)?;
                        }
                        payload = direct_payload(
                            repo,
                            query,
                            entry.collection_index,
                            cursor.record,
                            &mut counters,
                            actual,
                            &mut source_access,
                            delivery
                                .as_ref()
                                .map(|delivery| delivery.max_buffered_bytes()),
                            execution,
                        )?;
                    }
                } else {
                    let position = counters.total_matches as usize;
                    let scan_query = if position >= all_offset.unwrap_or(query.options.offset)
                        && (all_offset.is_some() || position < cap)
                    {
                        query
                    } else {
                        &locator_query
                    };
                    if std::ptr::eq(scan_query, query) {
                        if let Some(delivery) = delivery.as_deref_mut() {
                            if all_offset.is_some() {
                                prepare_all_batch(delivery, &mut page, &counters, cursor.record)?;
                            }
                            check_stream_page_read(delivery, query, &page, cursor.record)?;
                        }
                    }
                    let scanned = scan_file(
                        repo,
                        scan_query,
                        sort,
                        entry.collection_index,
                        residuals[cursor.plan_index].as_ref(),
                        entry.fts,
                        None,
                        direct_plans[cursor.plan_index].as_ref(),
                        None,
                        &projections[cursor.plan_index],
                        false,
                        None,
                        None,
                        cursor.record.file.0,
                        &[(slot, cursor.record.range)],
                        Some(&mut source_access),
                        delivery.as_ref().map(|delivery| delivery.token()),
                        delivery
                            .as_ref()
                            .map(|delivery| delivery.max_buffered_bytes() as u64),
                        false,
                        execution,
                    )?;
                    counters.add_scan(
                        ScanWork {
                            sort_key_evaluations: 0,
                            ..ScanWork::from(&scanned)
                        },
                        &scanned.relation_work,
                    )?;
                    actual.files_opened += scanned.files_opened;
                    actual.bytes_read += scanned.bytes_read;
                    actual.record_parses += scanned.record_parses;
                    actual.raw_text_verifications += scanned.raw_text_verifications;
                    actual.residual_evaluations += scanned.residual_evaluations;
                    actual.files_opened +=
                        scanned.relation_work.source_reads + scanned.relation_work.target_reads;
                    actual.bytes_read += scanned.relation_work.bytes_read;
                    actual.record_parses +=
                        scanned.relation_work.source_reads + scanned.relation_work.target_reads;
                    actual.relation_target_reads += scanned.relation_work.target_reads;
                    actual.relation_verified_targets_reused +=
                        scanned.relation_work.target_cache_hits;
                    actual.relation_target_file_opens += scanned.relation_work.target_reads;
                    actual.relation_edge_visits += scanned.relation_work.edge_visits;
                    actual.relation_backlink_probe_visits +=
                        scanned.relation_work.backlink_probe_visits;
                    actual.relation_bytes_read += scanned.relation_work.bytes_read;
                    if let Some(found) = scanned.matches.into_iter().next() {
                        matched = true;
                        evidence = found.evidence.iter().cloned().collect();
                        payload = found.payload;
                    }
                }
                if matched {
                    let position = counters.total_matches as usize;
                    if position >= all_offset.unwrap_or(query.options.offset)
                        && (all_offset.is_some() || position < cap)
                    {
                        let matched_fields = if query.options.evidence {
                            evidence.into_iter().collect()
                        } else {
                            Vec::new()
                        };
                        page.push(make_hit(
                            repo,
                            entry.collection_index,
                            cursor.record,
                            matched_fields,
                            payload,
                        ));
                    }
                    counters.total_matches += 1;
                    if let Some(delivery) = delivery.as_deref_mut() {
                        delivery
                            .check_buffer(page.iter().map(crate::streaming::hit_bytes).sum())?;
                        if all_offset.is_some() {
                            flush_all_batch_if_full(delivery, &mut page, &counters)?;
                        } else {
                            delivery.ordered_rows(
                                &page,
                                &counters,
                                page.len() == query.options.limit,
                            )?;
                        }
                        if delivery.options().mode == StreamMode::PageReplacements
                            && page.len() % delivery.options().batch_rows == 0
                        {
                            delivery.replacement(&page, &counters, 0)?;
                        }
                    }
                    if all_offset.is_none()
                        && !query.options.count
                        && counters.total_matches as usize >= cap
                    {
                        stopped_early = true;
                        break;
                    }
                }
            }
            if let Some(next) = stored_cursor(
                repo,
                cursor.columns,
                cursor.order,
                cursor.collection_index,
                cursor.plan_index,
                cursor.position + 1,
            ) {
                heap.push(Reverse(next));
            }
        }
    }
    counters.returned_records = if let Some(delivery) = delivery.as_deref_mut() {
        if all_offset.is_some() {
            delivery.ordered_batch(&page, &counters)?;
            page.clear();
            delivery.progress.emitted_rows
        } else {
            page.len() as u64
        }
    } else {
        page.len() as u64
    };
    counters.distinct_files = source_access.distinct_files();
    counters.match_count_complete = !stopped_early && (cap > 0 || query.options.count);
    if let Some(delivery) = delivery.as_deref_mut() {
        if all_offset.is_none() {
            delivery.ordered_rows(&page, &counters, true)?;
        }
    }
    if let Some(traces) = traces.as_deref_mut() {
        for (trace, actual) in traces.iter_mut().zip(actuals) {
            trace.actual = Some(actual);
        }
    }
    Ok(Some(SearchResult {
        count: query
            .options
            .count
            .then_some(counters.total_matches as usize),
        hits: page,
        counters,
    }))
}

#[allow(clippy::too_many_arguments)]
pub(super) fn offer_cached_key(
    repo: &Repository,
    cached: &CachedResult,
    member_index: usize,
    key: RankKey,
    winners: &mut BinaryHeap<Winner>,
    retained_key_bytes: &mut u64,
    peak_key_bytes: &mut u64,
    cap: usize,
) -> Result<(), String> {
    if cap == 0 || (winners.len() == cap && winners.peek().is_some_and(|worst| key >= worst.key)) {
        return Ok(());
    }
    if winners.len() == cap {
        let old = winners.pop().unwrap();
        *retained_key_bytes -= rank_key_bytes(&old.key);
    }
    *retained_key_bytes = retained_key_bytes
        .checked_add(rank_key_bytes(&key))
        .ok_or("sort key byte count overflow")?;
    if *retained_key_bytes > MAX_SORT_KEY_BYTES {
        return Err("sort key memory budget exceeded".into());
    }
    *peak_key_bytes = (*peak_key_bytes).max(*retained_key_bytes);
    let member = &cached.members[member_index];
    let record =
        &repo.collections[member.collection_index].generation.records[member.slot as usize];
    winners.push(Winner {
        key,
        collection_index: member.collection_index,
        slot: member.slot,
        hit: make_hit(
            repo,
            member.collection_index,
            record,
            member.matched_fields.to_vec(),
            None,
        ),
        retained_bytes: 0,
    });
    Ok(())
}
