//! Segment relation planning and page execution.

use super::*;
use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::relation::RelationReader;
use crate::index::storage::segment::{
    SegmentSlotFact, any_segment_file, read_slot_members, visit_checked_file_slots,
    visit_record_headers, visit_segment_files,
};
use crate::index::storage::transfer::StorageError;
use crate::model::FileId;
use crate::records::RecordIdSlot;

#[cfg(test)]
thread_local! {
    static REFERENCE_ROW_VISITS: std::cell::RefCell<Option<Vec<String>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) struct ReferenceRowVisitScope {
    armed: bool,
}

#[cfg(test)]
impl ReferenceRowVisitScope {
    pub(crate) fn finish(mut self) -> Vec<String> {
        let visits = take_reference_row_visits();
        self.armed = false;
        visits
    }
}

#[cfg(test)]
impl Drop for ReferenceRowVisitScope {
    fn drop(&mut self) {
        if self.armed {
            let _ = take_reference_row_visits();
        }
    }
}

#[cfg(test)]
pub(crate) fn arm_reference_row_visits() -> ReferenceRowVisitScope {
    REFERENCE_ROW_VISITS.with(|visits| {
        *visits.borrow_mut() = Some(Vec::new());
    });
    ReferenceRowVisitScope { armed: true }
}

#[cfg(test)]
fn take_reference_row_visits() -> Vec<String> {
    REFERENCE_ROW_VISITS.with(|visits| visits.borrow_mut().take().unwrap_or_default())
}

#[cfg(test)]
fn record_reference_row_visit(id: &str) {
    REFERENCE_ROW_VISITS.with(|visits| {
        if let Some(visits) = visits.borrow_mut().as_mut() {
            visits.push(id.to_owned());
        }
    });
}

fn materialize_merged_address(
    repo: &Repository,
    view: &crate::query_execution::relations::view::MergedRelationView<'_>,
    address: crate::query_execution::relations::view::MergedRecordSlot,
    return_mode: ReturnMode,
) -> Result<Option<SearchHit>, String> {
    let Some(resolved) = view.record(address)? else {
        return Ok(None);
    };
    let record = resolved.record;
    let file = &resolved.file;
    let payload = match return_mode {
        ReturnMode::Locator => None,
        ReturnMode::Logical if record.class == RecordClass::RawFallback => {
            Some(Payload::LogicalUnavailable {
                diagnostic_ref: record.diagnostic_ref,
            })
        }
        ReturnMode::Bytes | ReturnMode::Logical => {
            let path = repo.required_file_root().join(&file.relative_path);
            let bytes = crate::source::access::read_record_range_for_search(
                resolved.collection.required_file_reader().reader(),
                &path,
                file.revision,
                record.range,
                None,
            )?;
            super::payload_from_bytes(
                record.class,
                Some(&resolved.collection),
                &return_mode,
                bytes,
            )?
            .0
        }
    };
    Ok(Some(SearchHit {
        collection: resolved.collection.definition.name.clone(),
        record_id: record.id.0.to_string(),
        generation: address.collection.0 as u64,
        file: Some(crate::files::RecordFileInfo::from_entry(
            file,
            Some(record.range),
        )),
        matched_fields: Vec::new(),
        record_class: record.class,
        diagnostic_ref: record.diagnostic_ref,
        payload,
    }))
}

#[derive(Clone)]
struct RelationTargetPrune {
    relation: usize,
    target_ids: HashSet<String>,
    exact: bool,
    evidence: Vec<String>,
}

struct PrunedRelationCandidates {
    candidates: Option<Arc<RoaringBitmap>>,
    relation_available: bool,
}

fn eligible_one_hop_relation_term<'a>(
    repo: &Repository,
    term: &'a BoundTerm,
) -> Option<(
    &'a crate::query::binding::relation::BoundRelationHop,
    &'a BoundTerm,
)> {
    let related = term.related.as_ref()?;
    let [hop] = related.hops.as_slice() else {
        return None;
    };
    if related.source_operand.is_some()
        || hop.direction != crate::query::binding::relation::HopDirection::Forward
        || !hop.selectors.is_empty()
        || !matches!(term.quantifier, Quantifier::Default | Quantifier::Any)
    {
        return None;
    }
    let source = repo.collections.get(hop.from.0 as usize)?;
    let relation = source.relation_declaration(hop.relation.ordinal)?;
    if term.quantifier == Quantifier::Default {
        if relation.cardinality != crate::schema::relations::RelationCardinality::One {
            return None;
        }
        for depth in 1..relation.path.len() {
            let ancestor = source
                .definition
                .schema
                .fields()
                .get(&relation.path[..depth])?;
            if matches!(ancestor.kind, crate::model::LogicalType::Array(_)) {
                return None;
            }
        }
    }
    Some((hop, &related.terminal))
}

fn eligible_direct_relation_term<'a>(
    repo: &Repository,
    term: &'a BoundTerm,
) -> Option<(
    &'a crate::query::binding::relation::BoundRelationHop,
    &'a BoundTerm,
)> {
    if let Some(forward) = eligible_one_hop_relation_term(repo, term) {
        return Some(forward);
    }
    let related = term.related.as_ref()?;
    let [hop] = related.hops.as_slice() else {
        return None;
    };
    (hop.direction == crate::query::binding::relation::HopDirection::Backlink
        && hop.selectors.is_empty()
        && related.source_operand.is_none()
        && term.quantifier == Quantifier::Any)
        .then_some((hop, &related.terminal))
}

fn one_hop_relation_term<'a>(
    repo: &Repository,
    expr: &'a BoundExpr,
) -> Option<(
    &'a crate::query::binding::relation::BoundRelationHop,
    &'a BoundTerm,
)> {
    match expr {
        BoundExpr::Term(term) => eligible_one_hop_relation_term(repo, term),
        BoundExpr::And(parts) => parts
            .iter()
            .find_map(|part| one_hop_relation_term(repo, part)),
        BoundExpr::Or(_) | BoundExpr::Not(_) => None,
    }
}

fn collect_relation_target_ids(
    repo: &Repository,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    hop: &crate::query::binding::relation::BoundRelationHop,
    mut target_candidates: impl FnMut(
        &PublishedSegment,
        &CollectionState,
    ) -> Result<Option<RoaringBitmap>, String>,
) -> Result<Option<RelationTargetPrune>, String> {
    let target_index = hop.to.0 as usize;
    if base_active[target_index].len() != 0 {
        return Ok(None);
    }
    let target = &repo.collections[target_index];
    let mut selections = Vec::with_capacity(segments[target_index].len());
    let mut candidate_count = 0u64;
    let mut active_count = 0u64;
    for published in segments[target_index].iter() {
        let Some(mut may) = target_candidates(published, target)? else {
            return Ok(None);
        };
        may &= published.active.bitmap();
        candidate_count += may.len();
        active_count += published.active.len();
        selections.push(may);
    }
    // A dense target set would spend memory without eliminating enough source
    // reads. Keep this a query-local bounded optimization.
    if candidate_count > 100_000 || candidate_count * 4 > active_count * 3 {
        return Ok(None);
    }
    let mut target_ids = HashSet::new();
    for (published, may) in segments[target_index].iter().zip(selections) {
        for slot in may.iter() {
            let record = published
                .header(RecordSlot(slot))?
                .ok_or("relation target candidate missing")?;
            target_ids.insert(record.id.0.to_string());
        }
    }
    Ok(Some(RelationTargetPrune {
        relation: hop.relation.ordinal as usize,
        target_ids,
        exact: false,
        evidence: Vec::new(),
    }))
}

/// For a positive one-hop term whose terminal predicate is fully covered by
/// resident indexes, reverse exact target IDs to source slots. This remains
/// useful even for a dense target set when an independent FTS condition can
/// substantially narrow the source scan.
fn exact_relation_source_plan(
    repo: &Repository,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    filter: &BoundExpr,
) -> Result<Option<RelationTargetPrune>, String> {
    let BoundExpr::Term(term) = filter else {
        return Ok(None);
    };
    let Some((hop, terminal)) = eligible_one_hop_relation_term(repo, term) else {
        return Ok(None);
    };
    if base_active[hop.from.0 as usize].len() != 0
        || base_active[hop.to.0 as usize].len() != 0
        || term
            .related
            .as_ref()
            .is_some_and(|related| related.source_operand.is_some())
    {
        return Ok(None);
    }
    let target = &repo.collections[hop.to.0 as usize];
    let mut target_ids = HashSet::new();
    for published in segments[hop.to.0 as usize].iter() {
        let bounds = segment_filter_bounds_with_catalog(
            published.id,
            published.as_ref(),
            &published.active,
            &BoundExpr::Term(terminal.clone()),
            target,
            None,
        )?;
        if bounds.may != bounds.must {
            return Ok(None);
        }
        for slot in bounds.must.iter() {
            let record = published
                .header(RecordSlot(slot))?
                .ok_or("exact relation target slot missing")?;
            target_ids.insert(record.id.0.to_string());
            if target_ids.len() > 100_000 {
                return Ok(None);
            }
        }
    }
    let mut evidence = vec![term.query_path.display.clone()];
    evidence.push(term.related.as_ref().unwrap().evidence_path.clone());
    evidence.sort();
    evidence.dedup();
    Ok(Some(RelationTargetPrune {
        relation: hop.relation.ordinal as usize,
        target_ids,
        exact: true,
        evidence,
    }))
}

fn relation_target_prune(
    repo: &Repository,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
) -> Result<Option<RelationTargetPrune>, String> {
    if let Some((hop, terminal)) = filter.and_then(|expr| one_hop_relation_term(repo, expr)) {
        return collect_relation_target_ids(
            repo,
            base_active,
            segments,
            hop,
            |published, collection| {
                segment_filter_bounds(
                    published.id,
                    published,
                    &published.active,
                    &BoundExpr::Term(terminal.clone()),
                    collection,
                )
                .map(|bounds| Some(bounds.may.as_ref().clone()))
            },
        );
    }
    if let Some(fts) = fts
        && let [field] = fts.fields.as_slice()
        && let Some(hops) = field.related.as_ref()
        && let [hop] = hops.as_slice()
        && hop.direction == crate::query::binding::relation::HopDirection::Forward
        && hop.selectors.is_empty()
        && field.query_path.is_none()
    {
        let mut local = fts.clone();
        local.fields[0].related = None;
        return collect_relation_target_ids(
            repo,
            base_active,
            segments,
            hop,
            |published, collection| {
                segment_fts_candidates(published.id, published, collection, &local)
            },
        );
    }
    Ok(None)
}

fn union_relation_matches(
    slots: crate::query_access::membership::SlotSet<'_>,
    output: &mut RoaringBitmap,
    slot_count: usize,
) -> Result<(), String> {
    use crate::query_access::membership::SlotSet;

    let members = slots
        .explicit_view()
        .ok_or("relation result did not supply its exact slots")?;
    if members
        .last()
        .is_some_and(|slot| slot as usize >= slot_count)
    {
        return Err("relation result points outside its segment".into());
    }
    if let SlotSet::Owned(slots) = slots {
        *output |= slots;
    } else {
        members.union_into_bitmap(output);
    }
    Ok(())
}

fn relation_source_candidates(
    published: &PublishedSegment,
    plan: &RelationTargetPrune,
) -> Result<Option<Arc<RoaringBitmap>>, String> {
    let Some(found) = published.reverse_relation_batch(plan.relation, &plan.target_ids)? else {
        return Ok(None);
    };
    let mut may = if plan.exact {
        RoaringBitmap::new()
    } else {
        read_slot_members(published, SegmentSlotFact::Logical)?
            .view()
            .subtract_from(&published.active.bitmap())
    };
    union_relation_matches(found.slots, &mut may, published.descriptor().slots)?;
    may &= published.active.bitmap();
    Ok(Some(Arc::new(may)))
}

#[allow(clippy::too_many_arguments)]
fn prune_relation_candidates(
    repo: &Repository,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    published: &PublishedSegment,
    collection: &CollectionState,
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
    relation_prune: &mut Option<Option<RelationTargetPrune>>,
) -> Result<PrunedRelationCandidates, String> {
    let filter_may = filter
        .map(|filter| {
            segment_filter_bounds_with_catalog(
                published.id,
                published,
                &published.active,
                filter,
                collection,
                Some(catalog),
            )
            .map(|bounds| bounds.may)
        })
        .transpose()?;
    let fts_may = fts
        .map(|fts| segment_fts_candidates(published.id, published, collection, fts))
        .transpose()?
        .flatten();
    let local_candidates = match (filter_may, fts_may) {
        (Some(filter), Some(fts)) => Some(Arc::new(filter.as_ref() & fts)),
        (Some(filter), None) => Some(filter),
        (None, Some(fts)) => Some(Arc::new(fts)),
        (None, None) => None,
    };
    let local_count = local_candidates
        .as_ref()
        .map_or_else(|| published.active.len(), |candidates| candidates.len());
    if local_count > 256 && relation_prune.is_none() {
        *relation_prune = Some(relation_target_prune(
            repo,
            base_active,
            segments,
            filter,
            fts,
        )?);
    }
    let relation_candidates = relation_prune
        .as_ref()
        .and_then(Option::as_ref)
        .map(|plan| relation_source_candidates(published, plan))
        .transpose()?
        .flatten();
    let (candidates, relation_available) =
        combine_relation_candidates(local_candidates, relation_candidates);
    Ok(PrunedRelationCandidates {
        candidates,
        relation_available,
    })
}

fn combine_relation_candidates(
    local: Option<Arc<RoaringBitmap>>,
    relation: Option<Arc<RoaringBitmap>>,
) -> (Option<Arc<RoaringBitmap>>, bool) {
    let relation_available = relation.is_some();
    let candidates = match (local, relation) {
        (Some(local), Some(relation)) => Some(Arc::new(local.as_ref() & relation.as_ref())),
        (Some(local), None) => Some(local),
        (None, Some(relation)) => Some(relation),
        (None, None) => None,
    };
    (candidates, relation_available)
}

enum DirectRelationPredicate<'a> {
    Term(&'a BoundTerm),
    Fts(BoundFts),
}

struct DirectTargetScan {
    matched_id: Option<String>,
    read_bytes: u64,
    read: bool,
    unavailable: bool,
}

fn scan_direct_target_parallel(
    repo: &Repository,
    target_collection: &CollectionState,
    published: &PublishedSegment,
    catalog: &dyn FileCatalogReader,
    predicate: &DirectRelationPredicate<'_>,
    must: Option<&RoaringBitmap>,
    slot: u32,
) -> Result<DirectTargetScan, String> {
    let resolved = segment_record_ref(
        &published.index,
        &published.active,
        catalog,
        RecordSlot(slot),
    )?
    .ok_or("direct relation target retired")?;
    if resolved.record.class == RecordClass::RawFallback {
        return Ok(DirectTargetScan {
            matched_id: None,
            read_bytes: 0,
            read: false,
            unavailable: false,
        });
    }
    if must.is_some_and(|must| must.contains(slot)) {
        return Ok(DirectTargetScan {
            matched_id: Some(resolved.record.id.0.to_string()),
            read_bytes: 0,
            read: false,
            unavailable: false,
        });
    }
    let path = repo.required_file_root().join(&resolved.file.relative_path);
    let bytes = match read_record_range_for_search(
        target_collection.required_file_reader().reader(),
        &path,
        resolved.file.revision,
        resolved.record.range,
        None,
    ) {
        Ok(bytes) => bytes,
        Err(_) => {
            return Ok(DirectTargetScan {
                matched_id: None,
                read_bytes: 0,
                read: false,
                unavailable: true,
            });
        }
    };
    let borrow_value_text = target_collection
        .required_file_reader()
        .scalar_verification()
        == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
        && matches!(predicate, DirectRelationPredicate::Fts(_));
    let extracted = prepare_query_record_with_execution(
        resolved.record.class,
        &target_collection.required_file_reader(),
        &target_collection.definition.schema,
        &bytes,
        &ScanProjection::full(
            matches!(predicate, DirectRelationPredicate::Fts(_)) && !borrow_value_text,
        ),
        false,
        None,
    )?;
    let ctx = RecordContext {
        value: &extracted.value,
        source_bytes: Some(&bytes),
        record: &resolved.record,
        range: (resolved.record).record_range(),
        file: Some(resolved.file.as_ref()),
        collection_name: &target_collection.definition.name,
        repository_root: Some(repo.required_file_root()),
    };
    let matched = match predicate {
        DirectRelationPredicate::Term(term) => eval_term(term, &ctx)?,
        DirectRelationPredicate::Fts(local) => {
            if borrow_value_text {
                crate::reference::fts::eval_borrowed_value_fts(local, &ctx, None)?
            } else {
                eval_bound_fts(local, &extracted.text, &ctx, &mut HashSet::new())?
            }
        }
    };
    Ok(DirectTargetScan {
        matched_id: matched.then(|| resolved.record.id.0.to_string()),
        read_bytes: bytes.len() as u64,
        read: true,
        unavailable: false,
    })
}

/// A single positive forward hop can be answered target first. Verify only
/// possible targets, reverse their IDs through resident source postings, and
/// rank the resulting source locators without parsing every source record.
/// Unsupported shapes and unavailable files use the existing merged scan.
#[allow(clippy::too_many_arguments)]
fn try_direct_relation_page(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[PreparedCollectionQuery],
    context: Option<&ExecutionContext>,
    mut cache_builder: Option<&mut LifecycleCacheBuilder>,
    retained_budget: Option<usize>,
) -> Result<Option<SearchResult>, String> {
    if prepared.len() != 1
        || (query.options.return_mode != ReturnMode::Locator
            && segments
                .iter()
                .flat_map(|group| group.iter())
                .all(|segment| matches!(segment.backing_kind(), RecordBacking::FileRange)))
    {
        return Ok(None);
    }
    let source_index = &prepared[0].collection_index;
    let filter = prepared[0].filter.as_ref();
    let fts = prepared[0].fts.as_ref();
    let (hop, predicate, evidence, local_filter) = if let (Some(filter), None) = (filter, fts) {
        let (term, local_filter) = match filter {
            BoundExpr::Term(term) => (term, None),
            BoundExpr::And(parts) if parts.len() == 2 => {
                let first = &parts[0];
                let second = &parts[1];
                if let BoundExpr::Term(term) = first
                    && eligible_direct_relation_term(repo, term).is_some()
                    && matches!(second, BoundExpr::Term(_))
                {
                    (term, Some(second))
                } else if let BoundExpr::Term(term) = second
                    && eligible_direct_relation_term(repo, term).is_some()
                    && matches!(first, BoundExpr::Term(_))
                {
                    (term, Some(first))
                } else {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        };
        let Some((hop, terminal)) = eligible_direct_relation_term(repo, term) else {
            return Ok(None);
        };
        let mut evidence = if query.options.evidence
            && term.derived == Derived::None
            && !matches!(term.operator, Operator::Ne | Operator::NotContains)
        {
            let mut fields = vec![term.query_path.display.clone()];
            fields.push(term.related.as_ref().unwrap().evidence_path.clone());
            fields.sort();
            fields.dedup();
            fields
        } else {
            Vec::new()
        };
        if query.options.evidence
            && let Some(BoundExpr::Term(local)) = local_filter
            && local.derived == Derived::None
            && !matches!(local.operator, Operator::Ne | Operator::NotContains)
        {
            evidence.push(local.query_path.display.clone());
            evidence.sort();
            evidence.dedup();
        }
        (
            hop,
            DirectRelationPredicate::Term(terminal),
            evidence,
            local_filter,
        )
    } else if let (None, Some(fts)) = (filter, fts)
        && let [field] = fts.fields.as_slice()
        && let Some(hops) = field.related.as_ref()
        && let [hop] = hops.as_slice()
        && hop.direction == crate::query::binding::relation::HopDirection::Forward
        && hop.selectors.is_empty()
        && field.query_path.is_none()
    {
        let mut local = fts.clone();
        local.fields[0].related = None;
        let evidence = if query.options.evidence {
            vec![field.evidence_path.clone()]
        } else {
            Vec::new()
        };
        (hop, DirectRelationPredicate::Fts(local), evidence, None)
    } else {
        return Ok(None);
    };
    if hop.direction == crate::query::binding::relation::HopDirection::Backlink
        && segments
            .iter()
            .flat_map(|group| group.iter())
            .all(|segment| matches!(segment.backing_kind(), RecordBacking::FileRange))
    {
        return Ok(None);
    }
    if hop.from.0 as usize != *source_index
        || base_active[*source_index].len() != 0
        || base_active[hop.to.0 as usize].len() != 0
    {
        return Ok(None);
    }
    let target_index = hop.to.0 as usize;
    let target_collection = &repo.collections[target_index];
    let mut local_proofs = Vec::new();
    if let Some(local_filter) = local_filter {
        for published in segments[*source_index].iter() {
            let bounds = segment_filter_bounds_with_catalog(
                published.id,
                published.as_ref(),
                &published.active,
                local_filter,
                &repo.collections[*source_index],
                Some(catalog),
            )?;
            // Data predicates cannot match a raw fallback. Prove only the
            // logical source domain used by the relation reverse posting.
            let logical = read_slot_members(published, SegmentSlotFact::Logical)?
                .view()
                .intersect_bitmap(&published.active.bitmap());
            let may = bounds.may.as_ref() & &logical;
            let must = bounds.must.as_ref() & &logical;
            if may != must {
                return Ok(None);
            }
            local_proofs.push(must);
        }
    }
    let relation_owner = if hop.direction == crate::query::binding::relation::HopDirection::Forward
    {
        *source_index
    } else {
        target_index
    };
    let source_relation = repo.collections[relation_owner]
        .relation_declaration(hop.relation.ordinal)
        .ok_or("direct relation declaration missing")?;
    let direct_sort_column = if sort.bound.has_computed {
        if hop.direction == crate::query::binding::relation::HopDirection::Backlink {
            return Ok(None);
        }
        let Some([term]) = sort.bound.terms[*source_index].as_deref() else {
            return Ok(None);
        };
        let BoundSortSource::Computed(computed) = &term.source else {
            return Ok(None);
        };
        let Some(path) = computed.direct_related_target_column(hop) else {
            return Ok(None);
        };
        if source_relation.cardinality != crate::schema::relations::RelationCardinality::One {
            return Ok(None);
        }
        let field = FieldPath::parse(path)?;
        if !target_collection
            .definition
            .schema
            .fields()
            .get(&field.segments)
            .is_some_and(|field| !field.multi)
        {
            return Ok(None);
        }
        let config = computed.sort_config();
        for published in segments[target_index].iter() {
            let Some(column) = published.order_column(&field)? else {
                return Ok(None);
            };
            if !crate::query::sort_binding::stored_column_matches_config(column, config) {
                return Ok(None);
            }
        }
        Some(field)
    } else {
        None
    };

    let mut target_ids = HashSet::new();
    let mut target_sort_keys = HashMap::new();
    let mut counters = WorkCounters::default();
    let route_budget = retained_budget
        .unwrap_or(64 * 1024 * 1024)
        .min(64 * 1024 * 1024);
    let mut route_bytes = 0usize;
    let mut reserve_route = |additional: usize| -> bool {
        let Some(next) = route_bytes.checked_add(additional) else {
            return false;
        };
        if next > route_budget {
            return false;
        }
        route_bytes = next;
        true
    };
    for published in segments[target_index].iter() {
        let segment = published.as_ref();
        let (may, must) = match &predicate {
            DirectRelationPredicate::Term(term) => {
                let bounds = segment_filter_bounds_with_catalog(
                    published.id,
                    segment,
                    &published.active,
                    &BoundExpr::Term((*term).clone()),
                    target_collection,
                    Some(catalog),
                )?;
                (
                    bounds.may.as_ref().clone(),
                    Some(bounds.must.as_ref().clone()),
                )
            }
            DirectRelationPredicate::Fts(local) => (
                segment_fts_candidates(published.id, segment, target_collection, local)?
                    .unwrap_or_else(|| published.active.bitmap()),
                None,
            ),
        };
        let may = &may & &published.active.bitmap();
        counters.relation_target_candidates += may.len();
        if let Some(context) = context {
            context.check_slots(counters.relation_target_candidates)?;
        }
        if counters.relation_target_candidates > 100_000 {
            return Ok(None);
        }
        let parallel_targets = context.is_none()
            && cache_builder.is_none()
            && retained_budget.is_none()
            && direct_sort_column.is_none()
            && (2..=32).contains(&repo.options.query_workers)
            && may.len() > 1
            && may
                .iter()
                .map(|slot| {
                    let slot = RecordSlot(slot);
                    Ok::<_, String>(
                        segment
                            .header(slot)?
                            .is_some_and(|record| record.id.0.len() <= 1024)
                            && if let Some(locator) =
                                segment.origin(slot)?.and_then(RecordOrigin::file)
                            {
                                locator.range.is_some_and(|range| range.len() <= 64 * 1024)
                                    && catalog.file(locator.file)?.is_some_and(|file| {
                                        file.relative_path.to_string_lossy().len() <= 4096
                                    })
                            } else {
                                false
                            },
                    )
                })
                .find(|result| !matches!(result, Ok(true)))
                .unwrap_or(Ok(true))?;
        if parallel_targets {
            let slots = may.iter().collect::<Vec<_>>();
            for chunk in slots.chunks(256) {
                let scans = repo.workers.install(|| {
                    chunk
                        .par_iter()
                        .map(|slot| {
                            scan_direct_target_parallel(
                                repo,
                                target_collection,
                                published,
                                catalog,
                                &predicate,
                                must.as_ref(),
                                *slot,
                            )
                        })
                        .collect::<Vec<_>>()
                });
                for scan in scans {
                    let scan = scan?;
                    if scan.unavailable {
                        return Ok(None);
                    }
                    if scan.read {
                        counters.relation_target_reads += 1;
                        counters.relation_target_file_opens += 1;
                        counters.relation_bytes_read += scan.read_bytes;
                    }
                    if let Some(id) = scan.matched_id {
                        counters.relation_target_matches += 1;
                        if !target_ids.contains(&id) {
                            if !reserve_route(id.len().saturating_add(64)) {
                                return Ok(None);
                            }
                            target_ids.insert(id);
                        }
                    }
                }
            }
            continue;
        }
        for slot in may.iter() {
            if let Some(context) = context {
                context.check_string()?;
            }
            let address = RecordSlot(slot);
            let record = segment
                .header(address)?
                .ok_or("direct relation target retired")?;
            if record.class == RecordClass::RawFallback {
                continue;
            }
            let matched = if must.as_ref().is_some_and(|must| must.contains(slot)) {
                // This is an index-only Locator/count proof. Actual payload or
                // residual reads use the checked pinned source identity.
                true
            } else if !matches!(segment.backing_kind(), RecordBacking::FileRange) {
                let provider_body = if segment.backing_kind() == RecordBacking::Provider {
                    if let Some(context) = context {
                        context.charge_reads(1)?;
                    }
                    counters.relation_target_reads += 1;
                    let projection = match &predicate {
                        DirectRelationPredicate::Term(term) => {
                            crate::records::reader::RecordProjection {
                                fields: if term.path.root == crate::model::PathRoot::Data {
                                    vec![term.path.segments.clone()]
                                } else {
                                    Vec::new()
                                },
                                text: false,
                                original_bytes: false,
                            }
                        }
                        DirectRelationPredicate::Fts(_) => {
                            crate::records::reader::RecordProjection {
                                text: true,
                                ..crate::records::reader::RecordProjection::full()
                            }
                        }
                    };
                    Some(read_provider(repo, segment, address, &projection)?)
                } else {
                    None
                };
                let retained_body = segment.retained_body(address)?;
                let value = provider_body
                    .as_ref()
                    .map(|body| &body.value)
                    .or_else(|| retained_body.as_deref().map(|body| &body.value))
                    .ok_or("direct relation target body missing")?;
                let text = provider_body
                    .as_ref()
                    .map(|body| body.text.as_slice())
                    .or_else(|| retained_body.as_deref().map(|body| body.text.as_slice()))
                    .ok_or("direct relation target text missing")?;
                let locator = segment.origin(address)?.and_then(RecordOrigin::file);
                let file = locator
                    .map(|link| catalog.file(link.file))
                    .transpose()?
                    .flatten();
                let ctx = RecordContext {
                    value,
                    source_bytes: None,
                    record: record.as_ref(),
                    range: locator.and_then(|link| link.range),
                    file: file.as_deref(),
                    collection_name: &target_collection.definition.name,
                    repository_root: repo.repository_root(),
                };
                match &predicate {
                    DirectRelationPredicate::Term(term) => eval_term(term, &ctx)?,
                    DirectRelationPredicate::Fts(local) => {
                        if text.is_empty() {
                            crate::reference::fts::eval_borrowed_value_fts(local, &ctx, None)?
                        } else {
                            eval_bound_fts(local, text, &ctx, &mut HashSet::new())?
                        }
                    }
                }
            } else {
                let resolved = segment_record_ref(segment, &published.active, catalog, address)?
                    .ok_or("direct relation target retired")?;
                let path = repo.required_file_root().join(&resolved.file.relative_path);
                if let Some(context) = context {
                    context.charge_reads(1)?;
                    context.charge_bytes(resolved.record.range.len())?;
                }
                let bytes = match read_record_range_for_search(
                    target_collection.required_file_reader().reader(),
                    &path,
                    resolved.file.revision,
                    resolved.record.range,
                    None,
                ) {
                    Ok(bytes) => bytes,
                    Err(_) => return Ok(None),
                };
                counters.relation_target_reads += 1;
                counters.relation_target_file_opens += 1;
                counters.relation_bytes_read += bytes.len() as u64;
                let borrow_value_text = target_collection
                    .required_file_reader()
                    .scalar_verification()
                    == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
                    && matches!(predicate, DirectRelationPredicate::Fts(_));
                let extracted = prepare_query_record_with_execution(
                    resolved.record.class,
                    &target_collection.required_file_reader(),
                    &target_collection.definition.schema,
                    &bytes,
                    &ScanProjection::full(
                        matches!(predicate, DirectRelationPredicate::Fts(_)) && !borrow_value_text,
                    ),
                    false,
                    context,
                )?;
                let ctx = RecordContext {
                    value: &extracted.value,
                    source_bytes: Some(&bytes),
                    record: &resolved.record,
                    range: (resolved.record).record_range(),
                    file: Some(resolved.file.as_ref()),
                    collection_name: &target_collection.definition.name,
                    repository_root: Some(repo.required_file_root()),
                };
                match &predicate {
                    DirectRelationPredicate::Term(term) => eval_term(term, &ctx)?,
                    DirectRelationPredicate::Fts(local) => {
                        if borrow_value_text {
                            crate::reference::fts::eval_borrowed_value_fts(local, &ctx, None)?
                        } else {
                            eval_bound_fts(local, &extracted.text, &ctx, &mut HashSet::new())?
                        }
                    }
                }
            };
            if matched {
                counters.relation_target_matches += 1;
                if hop.direction == crate::query::binding::relation::HopDirection::Backlink {
                    let relation = required_segment_relation(segment, hop.relation.ordinal)?;
                    let mut within_budget = true;
                    relation.visit_targets(RecordSlot(slot), &mut |source_id, _| {
                        if !target_ids.contains(source_id) {
                            if !reserve_route(source_id.len().saturating_add(64)) {
                                within_budget = false;
                                return Ok(true);
                            }
                            target_ids.insert(source_id.to_owned());
                        }
                        Ok(false)
                    })?;
                    if !within_budget {
                        return Ok(None);
                    }
                    continue;
                }
                if !target_ids.contains(record.id.0.as_ref()) {
                    let sort_key = if let Some(path) = direct_sort_column.as_ref() {
                        let key = segment
                            .stored_key(path, RecordSlot(slot))?
                            .ok_or("direct related sort column or slot missing")?;
                        Some(key.into_owned())
                    } else {
                        None
                    };
                    let sort_bytes = sort_key.as_ref().map_or(0, |key| {
                        record
                            .id
                            .0
                            .len()
                            .saturating_add(crate::query::sort_binding::key_bytes(key))
                            .saturating_add(64)
                    });
                    if !reserve_route(
                        record
                            .id
                            .0
                            .len()
                            .saturating_add(64)
                            .saturating_add(sort_bytes),
                    ) {
                        return Ok(None);
                    }
                    if let Some(key) = sort_key {
                        target_sort_keys.insert(record.id.0.clone(), key);
                    }
                    target_ids.insert(record.id.0.to_string());
                }
            }
        }
    }
    let mut source_candidates = Vec::new();
    counters.relation_edge_visits = counters
        .relation_edge_visits
        .saturating_add(target_ids.len() as u64);
    for (segment_index, published) in segments[*source_index].iter().enumerate() {
        let mut may = if hop.direction == crate::query::binding::relation::HopDirection::Backlink {
            let mut matches = RoaringBitmap::new();
            for id in &target_ids {
                if let Some(slot) = published.slot_for_record(id)? {
                    matches.insert(slot.0);
                }
            }
            matches
        } else {
            let Some(found) =
                published.reverse_relation_batch(hop.relation.ordinal as usize, &target_ids)?
            else {
                return Ok(None);
            };
            counters.relation_reverse_postings_visited += found.lists;
            counters.relation_reverse_members_visited += found.members;
            let mut matches = RoaringBitmap::new();
            union_relation_matches(found.slots, &mut matches, published.descriptor().slots)?;
            matches
        };
        may &= published.active.bitmap();
        may = read_slot_members(published, SegmentSlotFact::Logical)?
            .view()
            .intersect_bitmap(&may);
        if local_filter.is_some() {
            may &= &local_proofs[segment_index];
        }
        if !reserve_route(may.serialized_size().saturating_add(64)) {
            return Ok(None);
        }
        source_candidates.push((segment_index, may));
    }
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("direct relation page size overflow")?;
    if query.options.return_mode == ReturnMode::Locator
        && segments[*source_index]
            .iter()
            .all(|segment| matches!(segment.backing_kind(), RecordBacking::FileRange))
        && context.is_none()
        && cache_builder.is_none()
        && retained_budget.is_none()
        && direct_sort_column.is_none()
        && source_candidates.len() <= 4096
    {
        let selected = source_candidates
            .iter()
            .enumerate()
            .map(|(index, (segment, may))| {
                (index == *segment).then(|| (may.clone(), evidence.clone()))
            })
            .collect::<Option<Vec<_>>>();
        if let Some(selected) = selected
            && let Some(result) = exact_ordered_page(
                repo,
                query,
                sort,
                segments,
                catalog,
                revision,
                *source_index,
                &selected,
                None,
                counters.clone(),
            )?
        {
            return Ok(Some(result));
        }
    }
    let mut winners = BinaryHeap::new();
    let mut winner_bytes = 0usize;
    let mut total = 0u64;
    for (segment_index, may) in source_candidates {
        let published = &segments[*source_index][segment_index];
        let stored_order = if cache_builder.is_none()
            && context.is_none()
            && direct_sort_column.is_none()
            && cap < may.len() as usize
        {
            lifecycle_segment_order(published.as_ref(), sort, query, *source_index)?
        } else {
            None
        };
        let ordered = stored_order.is_some();
        if ordered {
            // A global top `cap` cannot include a row ranked below its own
            // shard's top `cap`. Count the full exact bitmap without building
            // rank keys and hit locators for every matching source.
            total += may.len();
            counters.exact_posting_matches += may.len();
        }
        let slots: Box<dyn Iterator<Item = Result<u32, String>> + '_> =
            if let Some(mut order) = stored_order {
                Box::new(
                    std::iter::from_fn(move || order.next_slot().transpose())
                        .filter_map(|slot| match slot {
                            Ok(slot) => may.contains(slot.0).then_some(Ok(slot.0)),
                            Err(error) => Some(Err(error.to_string())),
                        })
                        .take(cap),
                )
            } else {
                Box::new(may.iter().map(Ok))
            };
        for slot in slots {
            let slot = slot?;
            if let Some(context) = context {
                context.check_string()?;
            }
            let record = published
                .header(RecordSlot(slot))?
                .ok_or("direct relation source retired")?;
            let key = if direct_sort_column.is_some() {
                let relation = required_segment_relation(published.as_ref(), hop.relation.ordinal)?;
                let mut target_key = None;
                let mut seen = false;
                relation.visit_targets(RecordSlot(slot), &mut |id, _| {
                    if seen {
                        target_key = None;
                        return Ok(true);
                    }
                    seen = true;
                    target_key = target_sort_keys.get(id);
                    Ok(false)
                })?;
                let Some(target_key) = target_key else {
                    return Ok(None);
                };
                let term = &sort.bound.terms[*source_index]
                    .as_ref()
                    .ok_or("direct related sort term missing")?[0];
                RankKey {
                    parts: vec![(target_key.clone(), term.descending, term.options)],
                    collection: repo.collections[*source_index].definition.name.clone(),
                    id: record.id.0.to_string(),
                }
            } else {
                segment_rank_key(
                    repo,
                    sort,
                    *source_index,
                    published.as_ref(),
                    record.as_ref(),
                    None,
                )?
            };
            if !ordered {
                total += 1;
                counters.exact_posting_matches += 1;
            }
            let hit = if matches!(published.backing_kind(), RecordBacking::FileRange) {
                let resolved = segment_record_ref(
                    &published.index,
                    &published.active,
                    catalog,
                    RecordSlot(slot),
                )?
                .ok_or("direct relation source retired")?;
                segment_locator_hit(
                    &repo.collections[*source_index],
                    resolved,
                    revision,
                    evidence.clone(),
                )
            } else {
                retained_locator_hit(
                    &repo.collections[*source_index],
                    &published.index,
                    catalog,
                    record.as_ref(),
                    revision,
                    evidence.clone(),
                )?
            };
            if let Some(builder) = cache_builder.as_mut() {
                builder.push(
                    &key,
                    CachedMember {
                        collection_index: *source_index,
                        segment_index: Some(segment_index),
                        slot,
                        matched_fields: hit.matched_fields.clone().into(),
                    },
                );
            }
            offer_merged(
                &mut winners,
                cap,
                MergedPageHit {
                    key,
                    hit,
                    segment: Some((*source_index, segment_index)),
                    record_address: None,
                    retained_bytes: 0,
                },
                retained_budget.zip(context),
                &mut winner_bytes,
            )?;
        }
    }
    let page = winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .map(|winner| {
            let mut hit = winner.hit;
            if query.options.return_mode != ReturnMode::Locator {
                let (collection_index, segment_index) = winner
                    .segment
                    .ok_or("direct relation winner segment missing")?;
                let published = &segments[collection_index][segment_index];
                let payload = get_segment_record_by_id(
                    repo,
                    collection_index,
                    published.as_ref(),
                    &published.active,
                    catalog,
                    revision,
                    &hit.record_id,
                    query.options.return_mode.clone(),
                )?
                .ok_or("direct relation winner retired")?;
                hit.payload = payload.payload;
            }
            Ok::<_, String>(hit)
        })
        .collect::<Result<Vec<_>, _>>()?;
    counters.total_matches = total;
    counters.match_count_complete = true;
    counters.returned_records = page.len() as u64;
    if let Some(filter) = filter {
        let mut relation_terms = Vec::new();
        collect_positive_relation_terms(filter, &mut relation_terms);
        if let Some(relation_term) = relation_terms.first() {
            let term_id = prepared[0]
                .term_id_for_bound_term(relation_term)
                .map_or(crate::query_predicate::TermId::new(u32::MAX), |id| id);
            let collection = &repo.collections[*source_index];
            crate::query_execution::request::record_observation_with_kind(
                repo.instance_id,
                term_id,
                crate::query_execution::domain::domain_id(
                    repo.instance_id,
                    collection.generation.collection.0,
                    collection.generation.generation,
                    None,
                ),
                crate::query_access::contract::AccessId {
                    provider: crate::query_access::contract::ProviderId(4),
                    operation: crate::query_access::contract::OperationId(0),
                },
                collection.generation.collection.0,
                counters.candidates,
                counters.total_matches,
                counters.total_matches,
                Some(if local_filter.is_some() {
                    "relation_source_restricted"
                } else {
                    "relation_target_first"
                }),
            );
        }
    }
    record_source_candidates(repo, context, *source_index, counters.candidates);
    Ok(Some(SearchResult {
        count: query.options.count.then_some(total as usize),
        hits: page,
        counters,
    }))
}

#[allow(clippy::too_many_arguments)]
fn try_semijoin_relation_page(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[PreparedCollectionQuery],
    context: Option<&ExecutionContext>,
) -> Result<Option<SearchResult>, String> {
    if query.options.return_mode != ReturnMode::Locator
        || prepared.len() != 1
        || sort.bound.has_computed
        || base_active.iter().any(|active| active.len() != 0)
    {
        return Ok(None);
    }
    let collection_index = &prepared[0].collection_index;
    let filter = prepared[0].filter.as_ref();
    let Some(fts) = prepared[0].fts.as_ref() else {
        return Ok(None);
    };
    if fts.fields.iter().any(|field| {
        field
            .related
            .as_ref()
            .is_some_and(|hops| hops.iter().any(|hop| !hop.selectors.is_empty()))
    }) {
        return Ok(None);
    }
    if filter.is_some_and(bound_expr_has_relations) {
        return Ok(None);
    }
    let collection = &repo.collections[*collection_index];
    let mut input = crate::index::segment::SegmentRecordSet::default();
    let source_collection = collection.generation.collection;
    let filter_evidence = if query.options.evidence {
        match filter {
            Some(filter) => {
                let Some(fields) = segment_static_evidence_fields(filter) else {
                    return Ok(None);
                };
                Some(fields)
            }
            None => None,
        }
    } else {
        None
    };
    for published in segments[*collection_index].iter() {
        let may = if let Some(filter) = filter {
            let bounds = segment_filter_bounds_with_catalog(
                published.id,
                published.as_ref(),
                &published.active,
                filter,
                collection,
                Some(catalog),
            )?;
            let logical = read_slot_members(published, SegmentSlotFact::Logical)?
                .view()
                .intersect_bitmap(&published.active.bitmap());
            if (bounds.may.as_ref() & &logical) != (bounds.must.as_ref() & &logical) {
                return Ok(None);
            }
            bounds.must.as_ref() & &logical
        } else {
            published.active.bitmap().clone()
        };
        let may = read_slot_members(published, SegmentSlotFact::Logical)?
            .view()
            .intersect_bitmap(&may);
        input.insert_local(source_collection, published.id, may);
    }
    let view = crate::query_execution::relations::view::MergedRelationView::new(
        repo,
        base_active,
        segments,
        catalog,
    )?;
    let Some(witnesses) = crate::query_access::relations::semijoin::qualified_fts_witnesses(
        &view, fts, &input, context,
    )?
    else {
        return Ok(None);
    };
    let mut counters = WorkCounters {
        candidates: witnesses.qualified_evidence.len() as u64,
        ..Default::default()
    };
    counters.add_semijoin(witnesses.work)?;
    let mut selected = Vec::with_capacity(segments[*collection_index].len());
    for published in segments[*collection_index].iter() {
        let mut matches = RoaringBitmap::new();
        let descriptor = published.descriptor();
        for slot in 0..descriptor.slots as u32 {
            let address = crate::index::segment::SegmentRecordAddress {
                collection: descriptor.collection,
                segment: descriptor.id,
                slot: RecordSlot(slot),
            };
            if input.contains(address) && witnesses.qualified_evidence.contains_key(&address) {
                matches.insert(slot);
            }
        }
        let evidence = filter_evidence.clone().unwrap_or_default();
        selected.push((matches, evidence));
    }
    exact_ordered_page(
        repo,
        query,
        sort,
        segments,
        catalog,
        revision,
        *collection_index,
        &selected,
        query
            .options
            .evidence
            .then_some(&witnesses.qualified_evidence),
        counters,
    )
}

struct RelationFileJob {
    segment_index: usize,
    file_id: u32,
    slot: RecordSlot,
    range: ByteRange,
}

struct RelationFileScan {
    counters: WorkCounters,
    primary_opened: bool,
    unavailable: SourceAvailabilityLog,
    relation_unavailable: SourceAvailabilityLog,
    winner: Option<MergedPageHit>,
}

fn bounded_parallel_relation_filter(expr: &BoundExpr) -> bool {
    match expr {
        BoundExpr::Term(term) => {
            term.path.root != PathRoot::Backlinks
                && term
                    .rhs
                    .as_ref()
                    .is_none_or(|rhs| rhs.path.root != PathRoot::Backlinks)
                && term.related.as_ref().is_none_or(|related| {
                    related.hops.len() <= 2
                        && related.hops.iter().all(|hop| {
                            hop.direction == crate::query::binding::relation::HopDirection::Forward
                        })
                })
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            parts.iter().all(bounded_parallel_relation_filter)
        }
        BoundExpr::Not(inner) => bounded_parallel_relation_filter(inner),
    }
}

#[allow(clippy::too_many_arguments)]
fn scan_relation_file_parallel(
    repo: &Repository,
    query: &Query,
    sort: &ResolvedSort<'_>,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    _catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
    collection_index: usize,
    filter: Option<&BoundExpr>,
    filter_proof: Option<&[String]>,
    fts: Option<&BoundFts>,
    job: &RelationFileJob,
    view: crate::query_execution::relations::view::MergedRelationView<'_>,
) -> Result<RelationFileScan, String> {
    let _fold_record = FoldRecordGuard::new_with_sharing(shared_fold_bound(filter));
    let mut scan = RelationFileScan {
        counters: WorkCounters::default(),
        primary_opened: false,
        unavailable: SourceAvailabilityLog::default(),
        relation_unavailable: SourceAvailabilityLog::default(),
        winner: None,
    };
    let address = crate::query_execution::relations::view::MergedRecordSlot {
        collection: repo.collections[collection_index].generation.collection,
        segment: Some(job.segment_index),
        slot: job.slot,
    };
    let resolved = view
        .record(address)?
        .ok_or("active parallel relation source missing")?;
    let path = repo.required_file_root().join(&resolved.file.relative_path);
    let mut reader = match RecordRangeReader::open_for_search(&path, resolved.file.revision, None) {
        Ok(reader) => {
            reader.with_backend(resolved.collection.required_file_reader().reader_handle())
        }
        Err(message) => {
            scan.unavailable.record(SourceDiagnostic {
                file_id: resolved.file.id.0,
                file_path: resolved.file.relative_path.to_path_buf(),
                kind: classify_source_read_failure_for_search(&path, resolved.file.revision, None),
                message,
            });
            return Ok(scan);
        }
    };
    scan.primary_opened = true;
    let (read, _) = match reader.read(&[(job.slot.0, job.range)]) {
        Ok(read) => read,
        Err(message) => {
            scan.unavailable.record(SourceDiagnostic {
                file_id: resolved.file.id.0,
                file_path: resolved.file.relative_path.to_path_buf(),
                kind: classify_source_read_failure_for_search(
                    &path,
                    resolved.file.revision,
                    Some(job.range),
                ),
                message,
            });
            return Ok(scan);
        }
    };
    let bytes = read
        .into_iter()
        .next()
        .ok_or("parallel relation row missing")?
        .1;
    scan.counters.candidates += 1;
    let record = resolved.record;
    let borrowed_value_text = borrowed_value_text(resolved.collection, fts);
    let mut projection = ScanProjection::full(
        fts.is_some()
            && !borrowed_value_text
            && !borrowed_relation_source_text(resolved.collection, fts),
    );
    if fts.is_none()
        && !sort.bound.has_computed
        && resolved
            .collection
            .required_file_reader()
            .scalar_verification()
            == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
        && let Some(fields) = filter.and_then(segment_filter_source_fields)
    {
        projection.top_level = Some(fields);
    }
    projection.resolved_text_only =
        projection.collect_text && fts.is_some_and(|fts| !fts.fields.is_empty());
    let combined_fts = fts.filter(|fts| {
        resolved
            .collection
            .required_file_reader()
            .scalar_verification()
            == Some(crate::source::verification::ScalarVerification::DecodedScalars)
            && source_fts_eligible(resolved.collection, fts, true)
    });
    let (extracted, combined_result) = if record.class == RecordClass::Logical
        && let Some(combined_fts) = combined_fts
    {
        let (extracted, result) = prepare_decoded_record_with_borrowed_fts(
            &resolved.collection.required_file_reader(),
            &resolved.collection.definition.schema,
            &bytes,
            &projection,
            combined_fts,
            query.options.evidence,
            None,
        )?;
        (extracted, Some(result))
    } else if record.class == RecordClass::Logical {
        (
            prepare_query_record_with_execution(
                record.class,
                &resolved.collection.required_file_reader(),
                &resolved.collection.definition.schema,
                &bytes,
                &projection,
                false,
                None,
            )?,
            None,
        )
    } else {
        (
            ExtractedRecord {
                value: Value::Object(Default::default()),
                text: Vec::new(),
            },
            None,
        )
    };
    let ctx = RecordContext {
        value: &extracted.value,
        source_bytes: Some(&bytes),
        record: &record,
        range: (record).record_range(),
        file: Some(&resolved.file),
        collection_name: &resolved.collection.definition.name,
        repository_root: Some(repo.required_file_root()),
    };
    let mut evidence = HashSet::new();
    let mut relation_work = crate::query_execution::relations::work::RelationWork::default();
    let filter_matches = if let Some(fields) = filter_proof {
        if query.options.evidence {
            evidence.extend(fields.iter().cloned());
        }
        true
    } else if let Some(filter) = filter {
        if record.class == RecordClass::RawFallback {
            eval_unavailable_filter(filter, &ctx, &mut evidence)? == Some(true)
        } else {
            crate::query_execution::relations::evaluation::eval_filter_truth_merged(
                &view,
                filter,
                address,
                &ctx,
                &mut evidence,
                &mut relation_work,
                false,
            )? == Some(true)
        }
    } else {
        true
    };
    let fts_matches = if filter_matches {
        if let Some(fts) = fts {
            if record.class == RecordClass::RawFallback && fts.fields.is_empty() {
                let matched = crate::reference::fts::raw_fallback_contains(
                    fts,
                    &String::from_utf8_lossy(&bytes),
                );
                if matched && query.options.evidence {
                    evidence.insert("@record.raw_text".into());
                }
                matched
            } else if borrowed_value_text && record.class == RecordClass::Logical {
                crate::reference::fts::eval_borrowed_value_fts(
                    fts,
                    &ctx,
                    query.options.evidence.then_some(&mut evidence),
                )?
            } else if let Some(result) = &combined_result {
                if query.options.evidence {
                    evidence.extend(result.evidence.iter().cloned());
                }
                result.fts_match
            } else {
                crate::query_execution::relations::evaluation::eval_fts_with_relations_merged(
                    &view,
                    fts,
                    &extracted.text,
                    address,
                    &ctx,
                    &mut evidence,
                    &mut relation_work,
                    false,
                )?
            }
        } else {
            true
        }
    } else {
        false
    };
    scan.counters.add_scan(
        ScanWork {
            files_opened: 1,
            bytes_read: bytes.len() as u64,
            record_parses: u64::from(record.class == RecordClass::Logical),
            raw_text_verifications: u64::from(
                filter_matches
                    && record.class == RecordClass::RawFallback
                    && fts.is_some_and(|fts| fts.fields.is_empty()),
            ),
            scanned_records: 1,
            residual_evaluations: u64::from(filter.is_some()),
            ..ScanWork::default()
        },
        &relation_work,
    )?;
    if filter_matches && fts_matches {
        let key = segment_rank_key(
            repo,
            sort,
            collection_index,
            segments[collection_index][job.segment_index].index.as_ref(),
            &record,
            Some(&ctx),
        )?;
        let mut hit = segment_locator_hit(
            resolved.collection,
            SegmentRecordRef {
                record: record.clone(),
                file: resolved.file,
            },
            revision,
            if query.options.evidence {
                evidence.into_iter().collect()
            } else {
                Vec::new()
            },
        );
        hit.generation = revision.0;
        scan.winner = Some(MergedPageHit {
            key,
            hit,
            segment: Some((collection_index, job.segment_index)),
            record_address: None,
            retained_bytes: 0,
        });
    }
    scan.relation_unavailable = view.take_source_errors();
    Ok(scan)
}

fn merge_parallel_relation_work(
    total: &mut WorkCounters,
    part: &WorkCounters,
) -> Result<(), String> {
    total.candidates += part.candidates;
    total.files_opened += part.files_opened;
    total.bytes_read += part.bytes_read;
    total.record_parses += part.record_parses;
    total.raw_text_verifications += part.raw_text_verifications;
    total.scanned_records += part.scanned_records;
    total.residual_evaluations += part.residual_evaluations;
    total.relation_source_reads += part.relation_source_reads;
    total.relation_target_reads += part.relation_target_reads;
    total.relation_target_file_opens += part.relation_target_file_opens;
    total.relation_edge_visits += part.relation_edge_visits;
    total.relation_backlink_probe_visits += part.relation_backlink_probe_visits;
    total.relation_bytes_read += part.relation_bytes_read;
    total.relation_verified_targets_reused += part.relation_verified_targets_reused;
    crate::query_execution::relations::work::check_query_work_budget(total)
}

#[allow(clippy::too_many_arguments)]
fn read_relation_source_file(
    source: &crate::source::registry::PreparedSource,
    file: &FileEntry,
    path: &std::path::Path,
    ranges: &[(u32, ByteRange)],
    first_address: crate::query_execution::relations::view::MergedRecordSlot,
    context: Option<&crate::query_execution::streaming::ExecutionContext>,
    source_errors: &mut SourceAvailabilityLog,
    opened: &mut HashSet<u32>,
    proof_fields: Option<&[String]>,
    mut process: impl FnMut(
        crate::query_execution::relations::view::MergedRecordSlot,
        Vec<u8>,
        bool,
        Option<&[String]>,
    ) -> Result<(), String>,
) -> Result<(), String> {
    let mut reader = match RecordRangeReader::open_for_search(path, file.revision, context) {
        Ok(reader) => reader.with_backend(source.reader_handle()),
        Err(message) => {
            source_errors.record(SourceDiagnostic {
                file_id: file.id.0,
                file_path: file.relative_path.to_path_buf(),
                kind: classify_source_read_failure_for_search(path, file.revision, None),
                message,
            });
            return Ok(());
        }
    };
    if let Some(context) = context {
        context.charge_reads(1)?;
    }
    opened.insert(file.id.0);
    let mut cursor = 0;
    while cursor < ranges.len() {
        let start = cursor;
        let mut batch_bytes = 0u64;
        while cursor < ranges.len() && cursor - start < 128 {
            let next = ranges[cursor].1.len();
            if cursor > start && batch_bytes.saturating_add(next) > 1024 * 1024 {
                break;
            }
            batch_bytes = batch_bytes.saturating_add(next);
            cursor += 1;
        }
        let read = match reader.read(&ranges[start..cursor]) {
            Ok((read, _)) => read,
            Err(message) => {
                source_errors.record(SourceDiagnostic {
                    file_id: file.id.0,
                    file_path: file.relative_path.to_path_buf(),
                    kind: classify_source_read_failure_for_search(
                        path,
                        file.revision,
                        Some(ranges[start].1),
                    ),
                    message,
                });
                break;
            }
        };
        for (index, (slot, bytes)) in read.into_iter().enumerate() {
            let mut address = first_address;
            address.slot = RecordSlot(slot);
            process(address, bytes, start == 0 && index == 0, proof_fields)?;
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn try_parallel_relation_page(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[PreparedCollectionQuery],
    prune_segments: bool,
    exact_plan: Option<&RelationTargetPrune>,
) -> Result<Option<SearchResult>, String> {
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("page size overflow")?;
    if prepared.len() != 1
        || query.options.return_mode != ReturnMode::Locator
        || sort.bound.has_computed
        || cap > 64
        || !(2..=32).contains(&repo.options.query_workers)
        || base_active.iter().any(|active| active.len() != 0)
    {
        return Ok(None);
    }
    let collection_index = &prepared[0].collection_index;
    let filter = prepared[0].filter.as_ref();
    let fts = prepared[0].fts.as_ref();
    if repo.collections[*collection_index].definition.name.len() > 1024
        || query
            .filter
            .as_ref()
            .is_some_and(|filter| format!("{filter:?}").len() > 1024)
        || fts.as_ref().is_some_and(|fts| {
            fts.fields
                .iter()
                .try_fold(0usize, |bytes, field| {
                    bytes.checked_add(field.evidence_path.len())
                })
                .is_none_or(|bytes| bytes > 1024)
        })
        || filter
            .as_ref()
            .is_some_and(|filter| !bounded_parallel_relation_filter(filter))
        || fts.as_ref().is_some_and(|fts| {
            fts.fields.iter().any(|field| {
                field.related.as_ref().is_some_and(|hops| {
                    hops.len() > 2
                        || hops.iter().any(|hop| {
                            hop.direction != crate::query::binding::relation::HopDirection::Forward
                        })
                })
            })
        })
    {
        return Ok(None);
    }
    if segments
        .iter()
        .flat_map(|group| group.iter())
        .any(|published| {
            !record_ranges_allow_parallel_scan(published.estimates().max_record_range_len)
        })
    {
        return Ok(None);
    }
    let collection = &repo.collections[*collection_index];
    let mut jobs = Vec::new();
    let mut relation_prune: Option<Option<RelationTargetPrune>> = exact_plan.cloned().map(Some);
    for (segment_index, published) in segments[*collection_index].iter().enumerate() {
        let pruned = if prune_segments {
            prune_relation_candidates(
                repo,
                base_active,
                segments,
                catalog,
                published,
                collection,
                filter,
                fts,
                &mut relation_prune,
            )?
        } else {
            PrunedRelationCandidates {
                candidates: None,
                relation_available: false,
            }
        };
        if prune_segments && exact_plan.is_some() && !pruned.relation_available {
            return Ok(None);
        }
        let candidates = pruned.candidates;
        if candidates.as_ref().is_some_and(|may| may.is_empty()) {
            continue;
        }
        // Posting and FTS bounds can leave only a few slots in a large
        // collection. Resolve those slots directly instead of probing every
        // file in the segment to discover the same jobs.
        if let Some(candidates) = candidates.as_ref()
            && candidates.len().saturating_mul(8) < published.file_count() as u64
        {
            let mut selected_files = HashSet::new();
            for slot_number in candidates.iter() {
                let slot = RecordSlot(slot_number);
                if !published.active.contains(slot) {
                    continue;
                }
                let record = super::read_file_range_record(published, slot)?;
                let range = record.range;
                let file = published
                    .file_metadata(record.file)?
                    .ok_or("parallel relation file missing")?;
                if !selected_files.insert(record.file)
                    || range.len() > 64 * 1024
                    || record.id.0.len() > 1024
                    || file.relative_path.to_string_lossy().len() > 4096
                {
                    return Ok(None);
                }
                jobs.push(RelationFileJob {
                    segment_index,
                    file_id: record.file.0,
                    slot,
                    range,
                });
                if jobs.len() > 50_000 {
                    return Ok(None);
                }
            }
            continue;
        }
        let cannot_parallelize = any_segment_file(published.as_ref(), |file| {
            let mut selected = None;
            let mut multiple = false;
            visit_checked_file_slots(published, file.id, &mut |slots| {
                for &slot in slots {
                    if published.active.contains(slot)
                        && candidates.as_ref().is_none_or(|may| may.contains(slot.0))
                        && selected.replace(slot).is_some()
                    {
                        multiple = true;
                        break;
                    }
                }
                Ok(())
            })?;
            if multiple {
                return Ok::<_, String>(true);
            }
            let Some(slot) = selected else {
                return Ok(false);
            };
            let record = super::read_file_range_record(published, slot)?;
            if record.file != file.id {
                return Err("parallel relation file ownership mismatch".into());
            }
            let range = record.range;
            if range.len() > 64 * 1024
                || record.id.0.len() > 1024
                || file.relative_path.to_string_lossy().len() > 4096
            {
                return Ok(true);
            }
            jobs.push(RelationFileJob {
                segment_index,
                file_id: file.id.0,
                slot,
                range,
            });
            Ok(jobs.len() > 50_000)
        })?;
        if cannot_parallelize {
            return Ok(None);
        }
    }
    let view = crate::query_execution::relations::view::MergedRelationView::new(
        repo,
        base_active,
        segments,
        catalog,
    )?;
    let mut counters = WorkCounters::default();
    let mut opened = HashSet::new();
    let mut winners = BinaryHeap::new();
    let mut winner_bytes = 0usize;
    let mut total = 0u64;
    // Each job reads at most one small record. Keep dispatch batches large enough
    // that relation scans spanning thousands of files do not repeatedly enter
    // the worker pool, while bounding the retained scan results.
    for chunk in jobs.chunks(2048) {
        let views = (0..chunk.len())
            .map(|_| view.fork_for_worker())
            .collect::<Vec<_>>();
        let scans = repo.workers.install(|| {
            views
                .into_par_iter()
                .zip(chunk.par_iter())
                .map(|(worker_view, job)| {
                    scan_relation_file_parallel(
                        repo,
                        query,
                        sort,
                        segments,
                        catalog,
                        revision,
                        *collection_index,
                        filter,
                        exact_plan.map(|plan| plan.evidence.as_slice()),
                        fts,
                        job,
                        worker_view,
                    )
                })
                .collect::<Vec<_>>()
        });
        for (job, scan) in chunk.iter().zip(scans) {
            let mut scan = scan?;
            if scan.primary_opened {
                opened.insert(job.file_id);
            }
            merge_parallel_relation_work(&mut counters, &scan.counters)?;
            scan.unavailable.apply(&mut counters);
            scan.relation_unavailable.apply(&mut counters);
            if let Some(winner) = scan.winner.take() {
                total += 1;
                offer_merged(&mut winners, cap, winner, None, &mut winner_bytes)?;
            }
        }
    }
    let page = winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .map(|winner| winner.hit)
        .collect::<Vec<_>>();
    counters.total_matches = total;
    counters.match_count_complete = counters.source_unavailable_events == 0;
    counters.returned_records = page.len() as u64;
    counters.distinct_files = opened.len() as u64;
    if let Some(filter) = filter {
        let mut relation_terms = Vec::new();
        collect_positive_relation_terms(filter, &mut relation_terms);
        let has_local = match filter {
            BoundExpr::And(parts) => parts
                .iter()
                .any(|part| matches!(part, BoundExpr::Term(term) if term.related.is_none())),
            _ => false,
        };
        for relation_term in relation_terms {
            let term_id = prepared[0]
                .term_id_for_bound_term(relation_term)
                .map_or(crate::query_predicate::TermId::new(u32::MAX), |id| id);
            crate::query_execution::request::record_observation_with_kind(
                repo.instance_id,
                term_id,
                crate::query_execution::domain::domain_id(
                    repo.instance_id,
                    collection.generation.collection.0,
                    collection.generation.generation,
                    None,
                ),
                crate::query_access::contract::AccessId {
                    provider: crate::query_access::contract::ProviderId(4),
                    operation: crate::query_access::contract::OperationId(1),
                },
                collection.generation.collection.0,
                counters.candidates,
                counters.total_matches,
                counters.total_matches,
                Some(if has_local {
                    "relation_source_restricted"
                } else {
                    "relation_target_first"
                }),
            );
        }
    }
    Ok(Some(SearchResult {
        count: (query.options.count && counters.match_count_complete).then_some(total as usize),
        hits: page,
        counters,
    }))
}

fn record_source_candidates(
    repo: &Repository,
    context: Option<&ExecutionContext>,
    collection_index: usize,
    candidates: u64,
) {
    let Some(context) = context else {
        return;
    };
    let collection = &repo.collections[collection_index];
    let execution = crate::query_access::contract::AccessExecution {
        context: Some(context),
    };
    crate::query_execution::request::record_observation_for_execution(
        &execution,
        repo.instance_id,
        crate::query_predicate::TermId::new(u32::MAX),
        crate::query_execution::domain::domain_id(
            repo.instance_id,
            collection.generation.collection.0,
            collection.generation.generation,
            None,
        ),
        crate::query_access::contract::AccessId {
            provider: crate::query_access::contract::ProviderId(4),
            operation: crate::query_access::contract::OperationId(99),
        },
        collection.generation.collection.0,
        collection.generation.records.len() as u64,
        candidates,
        0,
        Some("source_candidates"),
    );
}

const MAX_NATIVE_RELATION_LEAF_MEMBERS: usize = 4096;

type NativeIdTruth = HashMap<String, Vec<String>>;

/// Exact truth over local segment slots. Only matched evidence is retained;
/// an unfiltered domain costs one bitmap per segment, not a string-keyed row
/// and address map for every live record.
#[derive(Clone)]
struct NativeTruth {
    bits: Vec<RoaringBitmap>,
    evidence: HashMap<(usize, u32), Vec<String>>,
}

impl NativeTruth {
    fn empty(segments: usize) -> Self {
        Self {
            bits: vec![RoaringBitmap::new(); segments],
            evidence: HashMap::new(),
        }
    }

    fn contains(&self, segment: usize, slot: u32) -> bool {
        self.bits[segment].contains(slot)
    }

    fn insert(&mut self, segment: usize, slot: u32, fields: &[String]) {
        self.bits[segment].insert(slot);
        if !fields.is_empty() {
            self.evidence.insert((segment, slot), fields.to_vec());
        }
    }

    fn len(&self) -> usize {
        self.bits.iter().map(RoaringBitmap::len).sum::<u64>() as usize
    }

    fn and(&mut self, other: Self) {
        for (left, right) in self.bits.iter_mut().zip(&other.bits) {
            *left &= right;
        }
        self.evidence
            .retain(|(segment, slot), _| self.bits[*segment].contains(*slot));
        for (address, fields) in other.evidence {
            if self.bits[address.0].contains(address.1) {
                let evidence = self.evidence.entry(address).or_default();
                evidence.extend(fields);
                evidence.sort();
                evidence.dedup();
            }
        }
    }

    fn or(&mut self, other: Self) {
        for (left, right) in self.bits.iter_mut().zip(&other.bits) {
            *left |= right;
        }
        for (address, fields) in other.evidence {
            let evidence = self.evidence.entry(address).or_default();
            evidence.extend(fields);
            evidence.sort();
            evidence.dedup();
        }
    }

    fn without(mut self, other: Self) -> Self {
        for (left, right) in self.bits.iter_mut().zip(&other.bits) {
            *left -= right;
        }
        self.evidence.clear();
        self
    }
}

fn native_truth_from_ids(
    segments: &[Arc<PublishedSegment>],
    ids: NativeIdTruth,
) -> Result<NativeTruth, String> {
    let mut truth = NativeTruth::empty(segments.len());
    for (id, evidence) in ids {
        let mut found = None;
        for (segment_index, published) in segments.iter().enumerate() {
            if let Some(slot) = published.slot_for_record(&id)?
                && published.active.contains(slot)
            {
                if found.is_some() {
                    return Err("duplicate native relation source ID".into());
                }
                found = Some((segment_index, slot.0));
            }
        }
        if let Some((segment, slot)) = found {
            truth.insert(segment, slot, &evidence);
        }
    }
    Ok(truth)
}

fn required_segment_relation(
    segment: &dyn SegmentReader,
    ordinal: u32,
) -> Result<&dyn RelationReader, String> {
    segment
        .relation(ordinal as usize)?
        .ok_or("segment relation contribution missing".into())
}

fn active_segment_ids(segments: &[Arc<PublishedSegment>]) -> Result<HashSet<String>, String> {
    let mut ids = HashSet::new();
    for segment in segments {
        visit_record_headers(segment.as_ref(), |headers| {
            ids.extend(
                headers
                    .iter()
                    .filter(|record| segment.active.contains(record.slot))
                    .map(|record| record.id.0.to_string()),
            );
            Ok::<_, String>(())
        })?;
    }
    Ok(ids)
}

fn native_multi_hop_truth(
    repo: &Repository,
    query: &Query,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    term: &BoundTerm,
    counters: &mut WorkCounters,
) -> Result<Option<NativeIdTruth>, String> {
    let Some(related) = term.related.as_ref() else {
        return Ok(None);
    };
    if related.hops.len() < 2
        || related.source_operand.is_some()
        || related.hops.iter().any(|hop| !hop.selectors.is_empty())
        || !matches!(term.quantifier, Quantifier::Default | Quantifier::Any)
    {
        return Ok(None);
    }
    if term.quantifier == Quantifier::Default {
        for hop in &related.hops {
            if hop.direction != crate::query::binding::relation::HopDirection::Forward {
                return Ok(None);
            }
            let source = &repo.collections[hop.from.0 as usize];
            let relation = source
                .relation_declaration(hop.relation.ordinal)
                .ok_or("multi-hop relation declaration missing")?;
            if relation.cardinality != crate::schema::relations::RelationCardinality::One {
                return Ok(None);
            }
            for depth in 1..relation.path.len() {
                let ancestor = source
                    .definition
                    .schema
                    .fields()
                    .get(&relation.path[..depth]);
                if ancestor
                    .is_some_and(|field| matches!(field.kind, crate::model::LogicalType::Array(_)))
                {
                    return Ok(None);
                }
            }
        }
    }
    let terminal_collection = related.hops.last().expect("nonempty path").to.0 as usize;
    if segments
        .iter()
        .flat_map(|group| group.iter())
        .map(|published| published.active.len() as usize)
        .sum::<usize>()
        > MAX_NATIVE_RELATION_LEAF_MEMBERS
    {
        return Ok(None);
    }
    let mut matched = HashSet::new();
    for published in segments[terminal_collection].iter() {
        let bounds = segment_filter_bounds_with_catalog(
            published.id,
            published.as_ref(),
            &published.active,
            &BoundExpr::Term((*related.terminal).clone()),
            &repo.collections[terminal_collection],
            Some(catalog),
        )?;
        if bounds.may != bounds.must {
            return Ok(None);
        }
        counters.relation_target_candidates += bounds.may.len();
        counters.relation_target_matches += bounds.must.len();
        for slot in bounds.must.iter() {
            let record = published
                .header(RecordSlot(slot))?
                .ok_or("multi-hop target slot missing")?;
            matched.insert(record.id.0.to_string());
        }
    }
    for hop in related.hops.iter().rev() {
        let mut sources = HashSet::new();
        if hop.direction == crate::query::binding::relation::HopDirection::Forward {
            for published in segments[hop.from.0 as usize].iter() {
                let relation = required_segment_relation(published.as_ref(), hop.relation.ordinal)?;
                for slot in published.active.bitmap().iter() {
                    let record = published
                        .header(RecordSlot(slot))?
                        .ok_or("multi-hop source slot missing")?;
                    relation.visit_targets(RecordSlot(slot), &mut |target, _| {
                        counters.relation_edge_visits += 1;
                        if matched.contains(target) {
                            sources.insert(record.id.0.to_string());
                            return Ok(true);
                        }
                        Ok(false)
                    })?;
                }
            }
        } else {
            let active_sources = active_segment_ids(&segments[hop.from.0 as usize])?;
            for published in segments[hop.to.0 as usize].iter() {
                let relation = required_segment_relation(published.as_ref(), hop.relation.ordinal)?;
                for slot in published.active.bitmap().iter() {
                    let record = published
                        .header(RecordSlot(slot))?
                        .ok_or("multi-hop backlink slot missing")?;
                    if !matched.contains(record.id.0.as_ref()) {
                        continue;
                    }
                    relation.visit_targets(RecordSlot(slot), &mut |source, _| {
                        counters.relation_edge_visits += 1;
                        if active_sources.contains(source) {
                            sources.insert(source.to_owned());
                        }
                        Ok(false)
                    })?;
                }
            }
        }
        matched = sources;
    }
    let evidence = if query.options.evidence
        && term.derived == Derived::None
        && !matches!(term.operator, Operator::Ne | Operator::NotContains)
    {
        let mut fields = vec![
            term.query_path.display.clone(),
            related.evidence_path.clone(),
        ];
        fields.sort();
        fields.dedup();
        fields
    } else {
        Vec::new()
    };
    Ok(Some(
        matched
            .into_iter()
            .map(|id| (id, evidence.clone()))
            .collect(),
    ))
}

fn has_multi_hop(expr: &BoundExpr) -> bool {
    match expr {
        BoundExpr::Term(term) => term
            .related
            .as_ref()
            .is_some_and(|related| related.hops.len() > 1),
        BoundExpr::And(parts) | BoundExpr::Or(parts) => parts.iter().any(has_multi_hop),
        BoundExpr::Not(inner) => has_multi_hop(inner),
    }
}

fn native_boolean_truth(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &PreparedCollectionQuery,
    expression: &BoundExpr,
    universe: &NativeTruth,
    context: Option<&ExecutionContext>,
    counters: &mut WorkCounters,
) -> Result<Option<NativeTruth>, String> {
    match expression {
        BoundExpr::Term(term) if term.related.is_some() => {
            let Some((hop, _)) = eligible_direct_relation_term(repo, term) else {
                let Some(ids) =
                    native_multi_hop_truth(repo, query, segments, catalog, term, counters)?
                else {
                    return Ok(None);
                };
                return Ok(Some(native_truth_from_ids(
                    &segments[prepared.collection_index],
                    ids,
                )?));
            };
            if hop.direction == crate::query::binding::relation::HopDirection::Forward {
                let target_ids = active_segment_ids(&segments[hop.to.0 as usize])?;
                for published in segments[hop.from.0 as usize].iter() {
                    let relation =
                        required_segment_relation(published.as_ref(), hop.relation.ordinal)?;
                    for slot in published.active.bitmap().iter() {
                        let slot = RecordSlot(slot);
                        if relation.source_unavailable(slot)? {
                            return Ok(None);
                        }
                        let mut missing = false;
                        relation.visit_targets(slot, &mut |id, _| {
                            missing = !target_ids.contains(id);
                            Ok(missing)
                        })?;
                        if missing {
                            return Ok(None);
                        }
                    }
                }
            }
            let mut leaf_query = query.clone();
            leaf_query.options.offset = 0;
            leaf_query.options.limit = MAX_NATIVE_RELATION_LEAF_MEMBERS;
            leaf_query.options.return_mode = ReturnMode::Locator;
            leaf_query.options.count = true;
            let mut leaf = prepared.clone();
            leaf.filter = Some(BoundExpr::Term(term.clone()));
            leaf.fts = None;
            let Some(result) = try_direct_relation_page(
                repo,
                &leaf_query,
                base_active,
                segments,
                catalog,
                revision,
                sort,
                &[leaf],
                context,
                None,
                None,
            )?
            else {
                return Ok(None);
            };
            if result.count != Some(result.hits.len()) {
                return Ok(None);
            }
            counters.relation_target_candidates += result.counters.relation_target_candidates;
            counters.relation_target_matches += result.counters.relation_target_matches;
            counters.relation_target_reads += result.counters.relation_target_reads;
            counters.relation_target_file_opens += result.counters.relation_target_file_opens;
            counters.relation_reverse_postings_visited +=
                result.counters.relation_reverse_postings_visited;
            counters.relation_reverse_members_visited +=
                result.counters.relation_reverse_members_visited;
            counters.relation_edge_visits += result.counters.relation_edge_visits;
            counters.relation_bytes_read += result.counters.relation_bytes_read;
            counters.files_opened += result.counters.files_opened;
            counters.bytes_read += result.counters.bytes_read;
            counters.record_parses += result.counters.record_parses;
            Ok(Some(native_truth_from_ids(
                &segments[prepared.collection_index],
                result
                    .hits
                    .into_iter()
                    .map(|hit| (hit.record_id, hit.matched_fields))
                    .collect(),
            )?))
        }
        BoundExpr::Term(term) => {
            let mut truth = NativeTruth::empty(segments[prepared.collection_index].len());
            let collection_index = prepared.collection_index;
            for (segment_index, published) in segments[collection_index].iter().enumerate() {
                let bounds = segment_filter_bounds_with_catalog(
                    published.id,
                    published.as_ref(),
                    &published.active,
                    expression,
                    &repo.collections[collection_index],
                    Some(catalog),
                )?;
                if bounds.may != bounds.must {
                    return Ok(None);
                }
                let evidence = if query.options.evidence
                    && term.derived == Derived::None
                    && !matches!(term.operator, Operator::Ne | Operator::NotContains)
                {
                    vec![term.query_path.display.clone()]
                } else {
                    Vec::new()
                };
                for slot in bounds.must.iter() {
                    if universe.contains(segment_index, slot) {
                        truth.insert(segment_index, slot, &evidence);
                    }
                }
            }
            Ok(Some(truth))
        }
        BoundExpr::And(parts) => {
            let mut remaining = universe.clone();
            for part in parts {
                let Some(truth) = native_boolean_truth(
                    repo,
                    query,
                    base_active,
                    segments,
                    catalog,
                    revision,
                    sort,
                    prepared,
                    part,
                    universe,
                    context,
                    counters,
                )?
                else {
                    return Ok(None);
                };
                remaining.and(truth);
            }
            Ok(Some(remaining))
        }
        BoundExpr::Or(parts) => {
            let mut combined = NativeTruth::empty(universe.bits.len());
            for part in parts {
                let Some(truth) = native_boolean_truth(
                    repo,
                    query,
                    base_active,
                    segments,
                    catalog,
                    revision,
                    sort,
                    prepared,
                    part,
                    universe,
                    context,
                    counters,
                )?
                else {
                    return Ok(None);
                };
                combined.or(truth);
            }
            Ok(Some(combined))
        }
        BoundExpr::Not(inner) => {
            if has_multi_hop(inner) {
                return Ok(None);
            }
            let Some(excluded) = native_boolean_truth(
                repo,
                query,
                base_active,
                segments,
                catalog,
                revision,
                sort,
                prepared,
                inner,
                universe,
                context,
                counters,
            )?
            else {
                return Ok(None);
            };
            Ok(Some(universe.clone().without(excluded)))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn try_exact_native_boolean_page(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[PreparedCollectionQuery],
    context: Option<&ExecutionContext>,
    retained_budget: Option<usize>,
) -> Result<Option<SearchResult>, String> {
    let [entry] = prepared else { return Ok(None) };
    if entry.filter.is_none() && entry.fts.is_none() {
        return Ok(None);
    }
    if sort.bound.has_computed || base_active.iter().any(|active| active.len() != 0) {
        return Ok(None);
    }
    let collection_index = entry.collection_index;
    let active_count = segments[collection_index]
        .iter()
        .map(|published| published.active.len() as usize)
        .sum::<usize>();
    if query.options.evidence && active_count > MAX_NATIVE_RELATION_LEAF_MEMBERS {
        return Ok(None);
    }
    let mut universe = NativeTruth::empty(segments[collection_index].len());
    for (segment_index, published) in segments[collection_index].iter().enumerate() {
        for slot in published.active.bitmap().iter() {
            if let Some(context) = context {
                context.check_string()?;
            }
            let record = published
                .header(RecordSlot(slot))?
                .ok_or("native Boolean source slot missing")?;
            if record.class == RecordClass::Logical {
                universe.insert(segment_index, slot, &[]);
            }
        }
    }
    if let Some(context) = context {
        context.check_slots(universe.len() as u64)?;
    }
    let mut counters = WorkCounters::default();
    let mut truth = if let Some(filter) = entry.filter.as_ref() {
        let Some(truth) = native_boolean_truth(
            repo,
            query,
            base_active,
            segments,
            catalog,
            revision,
            sort,
            entry,
            filter,
            &universe,
            context,
            &mut counters,
        )?
        else {
            return Ok(None);
        };
        truth
    } else {
        universe.clone()
    };
    if let Some(fts) = entry.fts.as_ref() {
        let [field] = fts.fields.as_slice() else {
            return Ok(None);
        };
        let Some([hop]) = field.related.as_deref() else {
            return Ok(None);
        };
        if hop.direction != crate::query::binding::relation::HopDirection::Forward
            || !hop.selectors.is_empty()
            || field.query_path.is_some()
        {
            return Ok(None);
        }
        let mut leaf_query = query.clone();
        leaf_query.options.offset = 0;
        leaf_query.options.limit = MAX_NATIVE_RELATION_LEAF_MEMBERS;
        leaf_query.options.return_mode = ReturnMode::Locator;
        leaf_query.options.count = true;
        let mut leaf = entry.clone();
        leaf.filter = None;
        let Some(fts_result) = try_direct_relation_page(
            repo,
            &leaf_query,
            base_active,
            segments,
            catalog,
            revision,
            sort,
            &[leaf],
            context,
            None,
            None,
        )?
        else {
            return Ok(None);
        };
        if fts_result.count != Some(fts_result.hits.len()) {
            return Ok(None);
        }
        counters.relation_target_candidates += fts_result.counters.relation_target_candidates;
        counters.relation_target_matches += fts_result.counters.relation_target_matches;
        counters.relation_target_reads += fts_result.counters.relation_target_reads;
        counters.relation_target_file_opens += fts_result.counters.relation_target_file_opens;
        counters.relation_bytes_read += fts_result.counters.relation_bytes_read;
        counters.bytes_read += fts_result.counters.bytes_read;
        counters.files_opened += fts_result.counters.files_opened;
        let fts_truth = native_truth_from_ids(
            &segments[collection_index],
            fts_result
                .hits
                .into_iter()
                .map(|hit| (hit.record_id, hit.matched_fields))
                .collect(),
        )?;
        truth.and(fts_truth);
    }
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("native Boolean page size overflow")?;
    let mut winners = BinaryHeap::new();
    let mut winner_bytes = 0usize;
    let mut total = 0u64;
    for (segment_index, bitmap) in truth.bits.iter().enumerate() {
        for number in bitmap.iter() {
            let slot = RecordSlot(number);
            let evidence = truth
                .evidence
                .get(&(segment_index, number))
                .cloned()
                .unwrap_or_default();
            let published = &segments[collection_index][segment_index];
            let record = published
                .header(slot)?
                .ok_or("native Boolean ranked slot missing")?;
            let key = segment_rank_key(
                repo,
                sort,
                collection_index,
                published.as_ref(),
                record.as_ref(),
                None,
            )?;
            let hit = if matches!(published.backing_kind(), RecordBacking::FileRange) {
                let resolved =
                    segment_record_ref(published.as_ref(), &published.active, catalog, slot)?
                        .ok_or("native Boolean file record retired")?;
                segment_locator_hit(
                    &repo.collections[collection_index],
                    resolved,
                    revision,
                    evidence,
                )
            } else {
                retained_locator_hit(
                    &repo.collections[collection_index],
                    &published.index,
                    catalog,
                    record.as_ref(),
                    revision,
                    evidence,
                )?
            };
            total += 1;
            if cap > 0 {
                offer_merged(
                    &mut winners,
                    cap,
                    MergedPageHit {
                        key,
                        hit,
                        segment: Some((collection_index, segment_index)),
                        record_address: None,
                        retained_bytes: 0,
                    },
                    retained_budget.zip(context),
                    &mut winner_bytes,
                )?;
            }
        }
    }
    let page = winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
        .map(|mut winner| {
            if query.options.return_mode != ReturnMode::Locator {
                let (collection_index, segment_index) = winner.segment.expect("native segment");
                let published = &segments[collection_index][segment_index];
                let mut payload = get_segment_record_by_id(
                    repo,
                    collection_index,
                    published.as_ref(),
                    &published.active,
                    catalog,
                    revision,
                    &winner.hit.record_id,
                    query.options.return_mode.clone(),
                )?
                .ok_or("native Boolean winner retired")?;
                payload.matched_fields = winner.hit.matched_fields;
                winner.hit = payload;
            }
            Ok(winner.hit)
        })
        .collect::<Result<Vec<_>, String>>()?;
    counters.total_matches = total;
    counters.exact_posting_matches = total;
    counters.candidates = universe.len() as u64;
    counters.match_count_complete = true;
    counters.returned_records = page.len() as u64;
    Ok(Some(SearchResult {
        count: query.options.count.then_some(total as usize),
        hits: page,
        counters,
    }))
}

#[allow(clippy::too_many_arguments)]
fn try_native_relation_scan(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[PreparedCollectionQuery],
    context: Option<&ExecutionContext>,
    retained_budget: Option<usize>,
    reference: bool,
) -> Result<Option<SearchResult>, String> {
    if base_active.iter().any(|active| active.len() != 0) {
        return Ok(None);
    }
    let view = crate::query_execution::relations::view::MergedRelationView::new(
        repo,
        base_active,
        segments,
        catalog,
    )?;
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("native relation page size overflow")?;
    let mut winners = BinaryHeap::new();
    let mut winner_bytes = 0usize;
    let mut counters = WorkCounters::default();
    let mut total = 0u64;
    let mut sort_cache = MergedSortTargetCache::default();
    sort_cache.configure_execution(context);
    for entry in prepared {
        let collection_index = entry.collection_index;
        let collection = &repo.collections[collection_index];
        let source_projection = if entry.fts.is_none() && query.options.sort.is_empty() {
            entry
                .filter
                .as_ref()
                .and_then(crate::query_predicate::segment_filter_source_fields)
                .filter(|fields| !fields.is_empty())
                .map(|fields| {
                    let mut fields = fields
                        .into_iter()
                        .map(|field| vec![field])
                        .collect::<Vec<_>>();
                    fields.sort();
                    crate::records::reader::RecordProjection {
                        fields,
                        ..Default::default()
                    }
                })
        } else {
            None
        };
        let full_projection = crate::records::reader::RecordProjection {
            text: entry.fts.is_some(),
            ..Default::default()
        };
        let projection = source_projection.as_ref().unwrap_or(&full_projection);
        for (segment_index, published) in segments[collection_index].iter().enumerate() {
            let active_bitmap = published.active.bitmap();
            let mut active_slots = active_bitmap.iter();
            loop {
                let chunk = active_slots
                    .by_ref()
                    .take(64)
                    .map(RecordSlot)
                    .collect::<Vec<_>>();
                if chunk.is_empty() {
                    break;
                }
                let mut provider_values =
                    if matches!(published.backing_kind(), RecordBacking::Provider) {
                        let mut readable = Vec::new();
                        for &slot in &chunk {
                            let record = published
                                .header(slot)?
                                .ok_or("native relation source header missing")?;
                            if record.class != RecordClass::RawFallback {
                                readable.push(slot);
                            }
                        }
                        if let Some(context) = context {
                            context.check_string()?;
                            context.check_slots(
                                counters.candidates.saturating_add(chunk.len() as u64),
                            )?;
                            context.charge_reads(readable.len() as u64)?;
                        }
                        Some(std::collections::VecDeque::from(
                            view.native_provider_batch(
                                crate::query_execution::relations::view::MergedRecordSlot {
                                    collection: published.descriptor().collection,
                                    segment: Some(segment_index),
                                    slot: chunk[0],
                                },
                                &readable,
                                projection,
                            )?,
                        ))
                    } else {
                        None
                    };
                for slot in chunk {
                    if let Some(context) = context {
                        context.check_string()?;
                        context.check_slots(counters.candidates + 1)?;
                    }
                    let address = crate::query_execution::relations::view::MergedRecordSlot {
                        collection: published.descriptor().collection,
                        segment: Some(segment_index),
                        slot,
                    };
                    let record = published
                        .header(slot)?
                        .ok_or("native relation source slot missing")?;
                    let record = record.as_ref();
                    if record.class == RecordClass::RawFallback {
                        continue;
                    }
                    let provider_read = provider_values
                        .as_mut()
                        .and_then(|values| values.pop_front());
                    let (value, text, file, range, source_bytes) = if view.is_native(address) {
                        let Some((value, text)) = (if let Some(read) = provider_read.as_ref() {
                            Ok(Some((
                                std::borrow::Cow::Borrowed(&read.value),
                                std::borrow::Cow::Borrowed(read.text.as_slice()),
                            )))
                        } else {
                            view.native_value_projected(address, projection)
                        })?
                        else {
                            return Err("native relation source body missing".into());
                        };
                        let (file, range) = view.native_origin(address)?;
                        (value, text, file, range, None)
                    } else {
                        let Some(resolved) = view.record(address)? else {
                            return Err("file relation source missing".into());
                        };
                        let path = repo.required_file_root().join(&resolved.file.relative_path);
                        if let Some(context) = context {
                            context.charge_reads(1)?;
                            context.charge_bytes(resolved.record.range.len())?;
                        }
                        let bytes = read_record_range_for_search(
                            resolved.collection.required_file_reader().reader(),
                            &path,
                            resolved.file.revision,
                            resolved.record.range,
                            context,
                        )?;
                        let extracted = prepare_query_record_with_execution(
                            record.class,
                            &collection.required_file_reader(),
                            &collection.definition.schema,
                            &bytes,
                            &ScanProjection::full(entry.fts.is_some()),
                            false,
                            context,
                        )?;
                        counters.files_opened += 1;
                        counters.bytes_read += bytes.len() as u64;
                        counters.record_parses += 1;
                        (
                            std::borrow::Cow::Owned(extracted.value),
                            std::borrow::Cow::Owned(extracted.text),
                            Some(resolved.file),
                            Some(resolved.record.range),
                            Some(bytes),
                        )
                    };
                    let ctx = RecordContext {
                        value: value.as_ref(),
                        source_bytes: source_bytes.as_deref(),
                        record,
                        range,
                        file: file.as_deref(),
                        collection_name: &collection.definition.name,
                        repository_root: repo.repository_root(),
                    };
                    let mut relation_work =
                        crate::query_execution::relations::work::RelationWork::default();
                    relation_work.attach(context);
                    let mut evidence = HashSet::new();
                    let filter_matches = if let Some(filter) = entry.filter.as_ref() {
                        crate::query_execution::relations::evaluation::eval_filter_truth_merged(
                            &view,
                            filter,
                            address,
                            &ctx,
                            &mut evidence,
                            &mut relation_work,
                            reference,
                        )? == Some(true)
                    } else {
                        true
                    };
                    let fts_matches = if filter_matches {
                        entry.fts.as_ref().map_or(Ok(true), |fts| {
                        if text.is_empty()
                            && fts.fields.iter().all(|field| field.related.is_none())
                        {
                            crate::reference::fts::eval_borrowed_value_fts(
                                fts,
                                &ctx,
                                Some(&mut evidence),
                            )
                        } else {
                            crate::query_execution::relations::evaluation::eval_fts_with_relations_merged(
                                &view,
                                fts,
                                &text,
                                address,
                                &ctx,
                                &mut evidence,
                                &mut relation_work,
                                reference,
                            )
                        }
                    })?
                    } else {
                        false
                    };
                    counters.candidates += 1;
                    counters.add_scan(
                        ScanWork {
                            scanned_records: 1,
                            residual_evaluations: u64::from(entry.filter.is_some()),
                            ..ScanWork::default()
                        },
                        &relation_work,
                    )?;
                    if !(filter_matches && fts_matches) {
                        continue;
                    }
                    total += 1;
                    if cap == 0 {
                        continue;
                    }
                    let key = relation_rank_key_merged(
                        repo,
                        sort,
                        &view,
                        segments,
                        address,
                        &ctx,
                        &mut sort_cache,
                        entry.filter.as_ref(),
                        reference,
                    )?
                    .ok_or("native relation sort key unavailable")?;
                    counters.sort_key_evaluations += u64::from(sort.bound.has_computed);
                    let fields = if query.options.evidence {
                        let mut fields = evidence.into_iter().collect::<Vec<_>>();
                        fields.sort();
                        fields
                    } else {
                        Vec::new()
                    };
                    let hit = retained_locator_hit(
                        collection,
                        &published.index,
                        catalog,
                        record,
                        revision,
                        fields,
                    )?;
                    offer_merged(
                        &mut winners,
                        cap,
                        MergedPageHit {
                            key,
                            hit,
                            segment: Some((collection_index, segment_index)),
                            record_address: None,
                            retained_bytes: 0,
                        },
                        retained_budget.zip(context),
                        &mut winner_bytes,
                    )?;
                }
            }
        }
    }
    let mut hits = Vec::new();
    for winner in winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
    {
        let mut hit = winner.hit;
        if query.options.return_mode != ReturnMode::Locator {
            let (collection_index, segment_index) = winner.segment.expect("native source segment");
            let published = &segments[collection_index][segment_index];
            let mut payload = get_segment_record_by_id(
                repo,
                collection_index,
                published.as_ref(),
                &published.active,
                catalog,
                revision,
                &hit.record_id,
                query.options.return_mode.clone(),
            )?
            .ok_or("native relation winner retired")?;
            payload.matched_fields = hit.matched_fields;
            hit = payload;
            counters.selected_payload_reads += 1;
        }
        hits.push(hit);
    }
    counters.total_matches = total;
    counters.sort_target_reads += sort_cache.reads;
    counters.sort_target_reuse += sort_cache.reuse;
    counters.relation_edge_visits += sort_cache.edge_visits;
    counters.relation_source_reads += sort_cache.relation_source_reads;
    counters.relation_bytes_read += sort_cache.relation_source_bytes;
    counters.match_count_complete = true;
    counters.returned_records = hits.len() as u64;
    Ok(Some(SearchResult {
        count: query.options.count.then_some(total as usize),
        hits,
        counters,
    }))
}

pub(super) fn search_segmented_relations(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[PreparedCollectionQuery],
    context: Option<&ExecutionContext>,
    mut cache_builder: Option<&mut LifecycleCacheBuilder>,
    retained_budget: Option<usize>,
    prune_segments: bool,
    reference: bool,
) -> Result<SearchResult, String> {
    if segments
        .iter()
        .flat_map(|group| group.iter())
        .any(|segment| !matches!(segment.backing_kind(), RecordBacking::FileRange))
    {
        if !reference
            && let Some(result) = try_direct_relation_page(
                repo,
                query,
                base_active,
                segments,
                catalog,
                revision,
                sort,
                prepared,
                context,
                cache_builder.as_deref_mut(),
                retained_budget,
            )?
        {
            return Ok(result);
        }
        if !reference
            && let Some(result) = try_exact_native_boolean_page(
                repo,
                query,
                base_active,
                segments,
                catalog,
                revision,
                sort,
                prepared,
                context,
                retained_budget,
            )?
        {
            return Ok(result);
        }
        if let Some(result) = try_native_relation_scan(
            repo,
            query,
            base_active,
            segments,
            catalog,
            revision,
            sort,
            prepared,
            context,
            retained_budget,
            reference,
        )? {
            return Ok(result);
        }
        return Err("native relation query requires a supported direct relation plan".into());
    }
    let mut exact_relation_plan = None;
    if !reference {
        if prepared.len() == 1
            && let Some(filter) = prepared[0].filter.as_ref()
        {
            let mut relation_terms = Vec::new();
            collect_positive_relation_terms(filter, &mut relation_terms);
            let has_local = matches!(
                filter,
                BoundExpr::And(parts)
                    if parts
                        .iter()
                        .any(|part| matches!(part, BoundExpr::Term(term) if term.related.is_none()))
            );
            for relation_term in relation_terms {
                let term_id = prepared[0]
                    .term_id_for_bound_term(relation_term)
                    .map_or(crate::query_predicate::TermId::new(u32::MAX), |id| id);
                let collection = &repo.collections[prepared[0].collection_index];
                crate::query_execution::request::record_observation_with_kind(
                    repo.instance_id,
                    term_id,
                    crate::query_execution::domain::domain_id(
                        repo.instance_id,
                        collection.generation.collection.0,
                        collection.generation.generation,
                        None,
                    ),
                    crate::query_access::contract::AccessId {
                        provider: crate::query_access::contract::ProviderId(4),
                        operation: crate::query_access::contract::OperationId(2),
                    },
                    collection.generation.collection.0,
                    collection.generation.records.len() as u64,
                    0,
                    0,
                    Some(if has_local {
                        "relation_source_restricted"
                    } else {
                        "relation_target_first"
                    }),
                );
            }
        }
        if prepared.len() == 1
            && prepared[0]
                .fts
                .as_ref()
                .is_some_and(|fts| fts.fields.iter().any(|field| field.related.is_some()))
            && let Some(term_id) = prepared[0].text_term
        {
            let collection = &repo.collections[prepared[0].collection_index];
            let has_local = prepared[0]
                .filter
                .as_ref()
                .is_some_and(|filter| !bound_expr_has_relations(filter));
            crate::query_execution::request::record_observation_with_kind(
                repo.instance_id,
                term_id,
                crate::query_execution::domain::domain_id(
                    repo.instance_id,
                    collection.generation.collection.0,
                    collection.generation.generation,
                    None,
                ),
                crate::query_access::contract::AccessId {
                    provider: crate::query_access::contract::ProviderId(4),
                    operation: crate::query_access::contract::OperationId(3),
                },
                collection.generation.collection.0,
                collection.generation.records.len() as u64,
                0,
                0,
                Some(if has_local {
                    "relation_source_restricted"
                } else {
                    "relation_target_first"
                }),
            );
        }
        let traversal_plan = crate::query_plan::operations::relation_page_plan(
            prune_segments,
            cache_builder.is_some(),
            context.is_none() && cache_builder.is_none() && retained_budget.is_none(),
        );
        exact_relation_plan = if prune_segments
            && prepared.len() == 1
            && query.options.return_mode == ReturnMode::Locator
        {
            prepared[0]
                .filter
                .as_ref()
                .map(|filter| exact_relation_source_plan(repo, base_active, segments, filter))
                .transpose()?
                .flatten()
        } else {
            None
        };
        for operation in traversal_plan {
            let result = match operation {
                crate::query_plan::operations::RelationPageOperation::QualifiedSemijoin => {
                    try_semijoin_relation_page(
                        repo,
                        query,
                        base_active,
                        segments,
                        catalog,
                        revision,
                        sort,
                        prepared,
                        context,
                    )?
                }
                crate::query_plan::operations::RelationPageOperation::DirectTargetPrune => {
                    try_direct_relation_page(
                        repo,
                        query,
                        base_active,
                        segments,
                        catalog,
                        revision,
                        sort,
                        prepared,
                        context,
                        cache_builder.as_deref_mut(),
                        retained_budget,
                    )?
                }
                crate::query_plan::operations::RelationPageOperation::ParallelFileScan => {
                    try_parallel_relation_page(
                        repo,
                        query,
                        base_active,
                        segments,
                        catalog,
                        revision,
                        sort,
                        prepared,
                        prune_segments,
                        exact_relation_plan.as_ref(),
                    )?
                }
                crate::query_plan::operations::RelationPageOperation::ReferenceScan => None,
            };
            if let Some(result) = result {
                return Ok(result);
            }
        }
    }
    let view = crate::query_execution::relations::view::MergedRelationView::new(
        repo,
        base_active,
        segments,
        catalog,
    )?;
    let cap = query
        .options
        .offset
        .checked_add(query.options.limit)
        .ok_or("page size overflow")?;
    let mut winners = BinaryHeap::new();
    let mut winner_bytes = 0usize;
    let winner_budget = retained_budget.zip(context);
    let mut counters = WorkCounters::default();
    let mut total = 0u64;
    let mut opened = HashSet::new();
    let mut source_errors = SourceAvailabilityLog::default();
    let mut sort_cache = MergedSortTargetCache::default();
    sort_cache.configure_execution(context);
    for entry in prepared {
        let collection_index = entry.collection_index;
        let filter = &entry.filter;
        let fts = &entry.fts;
        let collection_id = repo.collections[collection_index].generation.collection;
        let collection_plan = &repo.collections[collection_index];
        let direct_plan = (!reference && prune_segments)
            .then(|| {
                DirectSourcePlan::prepare_bound(
                    collection_plan,
                    sort,
                    fts.as_ref(),
                    filter.as_ref(),
                )
            })
            .flatten();
        let borrowed_value_text = borrowed_value_text(collection_plan, fts.as_ref());
        let combined_fts = (!reference).then_some(()).and(fts.as_ref().filter(|fts| {
            collection_plan.required_file_reader().scalar_verification()
                == Some(crate::source::verification::ScalarVerification::DecodedScalars)
                && source_fts_eligible(collection_plan, fts, true)
        }));
        let mut projection = ScanProjection::full(
            fts.is_some()
                && !borrowed_value_text
                && !borrowed_relation_source_text(collection_plan, fts.as_ref()),
        );
        if !reference
            && fts.is_none()
            && !sort.bound.has_computed
            && collection_plan.required_file_reader().scalar_verification()
                == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
            && let Some(fields) = filter.as_ref().and_then(segment_filter_source_fields)
        {
            projection.top_level = Some(fields);
        }
        projection.resolved_text_only =
            projection.collect_text && fts.as_ref().is_some_and(|fts| !fts.fields.is_empty());
        let mut process = |address: crate::query_execution::relations::view::MergedRecordSlot,
                           bytes: Vec<u8>,
                           opened_source: bool,
                           filter_proof: Option<&[String]>|
         -> Result<(), String> {
            let _fold_record =
                FoldRecordGuard::new_with_sharing(shared_fold_bound(filter.as_ref()));
            if let Some(context) = context {
                context.check_string()?;
                context.charge_bytes(bytes.len() as u64)?;
            }
            let Some(resolved) = view.record(address)? else {
                return Ok(());
            };
            #[cfg(test)]
            if reference {
                record_reference_row_visit(&resolved.record.id.0);
            }
            counters.candidates += 1;
            let record = resolved.record;
            let direct_result = if record.class == RecordClass::Logical {
                direct_plan
                    .as_ref()
                    .map(|direct| {
                        crate::source::verification::scan_borrowed_segments(
                            resolved.collection.required_file_reader(),
                            &bytes,
                            &BorrowedScanQuery {
                                fts: direct.fts,
                                filter: direct.filter.map(|(term, _)| term),
                                retain_evidence: query.options.evidence,
                            },
                        )
                    })
                    .transpose()?
            } else {
                None
            };
            let (extracted, combined_result) = if record.class == RecordClass::Logical
                && direct_result.is_none()
                && let Some(combined_fts) = combined_fts
            {
                let (extracted, result) = prepare_decoded_record_with_borrowed_fts(
                    &resolved.collection.required_file_reader(),
                    &resolved.collection.definition.schema,
                    &bytes,
                    &projection,
                    combined_fts,
                    query.options.evidence,
                    context,
                )?;
                (extracted, Some(result))
            } else if record.class == RecordClass::Logical && direct_result.is_none() {
                (
                    prepare_query_record_with_execution(
                        record.class,
                        &resolved.collection.required_file_reader(),
                        &resolved.collection.definition.schema,
                        &bytes,
                        &projection,
                        false,
                        context,
                    )?,
                    None,
                )
            } else {
                (
                    ExtractedRecord {
                        value: Value::Object(Default::default()),
                        text: Vec::new(),
                    },
                    None,
                )
            };
            let ctx = RecordContext {
                value: &extracted.value,
                source_bytes: Some(&bytes),
                record: &record,
                range: (record).record_range(),
                file: Some(&resolved.file),
                collection_name: &resolved.collection.definition.name,
                repository_root: Some(repo.required_file_root()),
            };
            let mut evidence = HashSet::new();
            let mut relation_work =
                crate::query_execution::relations::work::RelationWork::default();
            relation_work.attach(context);
            let filter_matches = if let Some(fields) = filter_proof {
                if query.options.evidence {
                    evidence.extend(fields.iter().cloned());
                }
                true
            } else if let Some(result) = &direct_result {
                result.filter_match
            } else if let Some(filter) = filter.as_ref() {
                if record.class == RecordClass::RawFallback {
                    eval_unavailable_filter(filter, &ctx, &mut evidence)? == Some(true)
                } else {
                    crate::query_execution::relations::evaluation::eval_filter_truth_merged(
                        &view,
                        filter,
                        address,
                        &ctx,
                        &mut evidence,
                        &mut relation_work,
                        reference,
                    )? == Some(true)
                }
            } else {
                true
            };
            let fts_matches = if filter_matches {
                if let Some(result) = &direct_result {
                    if query.options.evidence {
                        evidence.extend(result.evidence.iter().cloned());
                        if result.filter_match
                            && result.fts_match
                            && let Some((term, _)) =
                                direct_plan.as_ref().and_then(|plan| plan.filter)
                            && term.operator == Operator::Contains
                        {
                            evidence.insert(term.query_path.display.clone());
                        }
                    }
                    result.fts_match
                } else if let Some(result) = &combined_result {
                    if query.options.evidence {
                        evidence.extend(result.evidence.iter().cloned());
                    }
                    result.fts_match
                } else if let Some(fts) = fts.as_ref() {
                    if record.class == RecordClass::RawFallback && fts.fields.is_empty() {
                        let matched = crate::reference::fts::raw_fallback_contains(
                            fts,
                            &String::from_utf8_lossy(&bytes),
                        );
                        if matched && query.options.evidence {
                            evidence.insert("@record.raw_text".into());
                        }
                        matched
                    } else if borrowed_value_text && record.class == RecordClass::Logical {
                        crate::reference::fts::eval_borrowed_value_fts(
                            fts,
                            &ctx,
                            query.options.evidence.then_some(&mut evidence),
                        )?
                    } else {
                        crate::query_execution::relations::evaluation::eval_fts_with_relations_merged(
                            &view,
                            fts,
                            &extracted.text,
                            address,
                            &ctx,
                            &mut evidence,
                            &mut relation_work,
                            reference,
                        )?
                    }
                } else {
                    true
                }
            } else {
                false
            };
            counters.add_scan(
                ScanWork {
                    files_opened: u64::from(opened_source),
                    bytes_read: bytes.len() as u64,
                    record_parses: u64::from(
                        direct_result.is_none() && record.class == RecordClass::Logical,
                    ),
                    raw_text_verifications: u64::from(
                        filter_matches
                            && record.class == RecordClass::RawFallback
                            && fts.as_ref().is_some_and(|fts| fts.fields.is_empty()),
                    ),
                    scanned_records: 1,
                    residual_evaluations: u64::from(filter.is_some()),
                    ..ScanWork::default()
                },
                &relation_work,
            )?;
            if !prune_segments && filter.as_ref().is_some_and(bound_expr_has_relations) {
                counters.relation_source_reads += 1;
            }
            if !(filter_matches && fts_matches) {
                return Ok(());
            }
            let source_errors_before_sort = view.source_error_events();
            let Some(key) = relation_rank_key_merged(
                repo,
                sort,
                &view,
                segments,
                address,
                &ctx,
                &mut sort_cache,
                filter.as_ref(),
                reference,
            )?
            else {
                return Ok(());
            };
            if view.source_error_events() > source_errors_before_sort {
                return Ok(());
            }
            total += 1;
            counters.sort_key_evaluations += u64::from(sort.bound.has_computed);
            let hit = segment_locator_hit(
                resolved.collection,
                SegmentRecordRef {
                    record: record.clone(),
                    file: resolved.file,
                },
                revision,
                if query.options.evidence {
                    evidence.into_iter().collect()
                } else {
                    Vec::new()
                },
            );
            if let Some(builder) = (!reference)
                .then_some(cache_builder.as_deref_mut())
                .flatten()
            {
                builder.push(
                    &key,
                    CachedMember {
                        collection_index,
                        segment_index: address.segment,
                        slot: address.slot.0,
                        matched_fields: hit.matched_fields.clone().into(),
                    },
                );
            }
            offer_merged(
                &mut winners,
                cap,
                MergedPageHit {
                    key,
                    hit,
                    segment: address
                        .segment
                        .map(|segment_index| (collection_index, segment_index)),
                    record_address: reference.then_some(address),
                    retained_bytes: 0,
                },
                winner_budget,
                &mut winner_bytes,
            )?;
            Ok(())
        };
        let collection = &repo.collections[collection_index];
        for file_id in &collection.generation.files {
            if let Some(context) = context {
                context.check_string()?;
            }
            let slots = collection
                .generation
                .file_records
                .records_for(*file_id)
                .ok_or("merged base file ownership missing")?;
            let ranges = slots
                .iter()
                .filter(|slot| base_active[collection_index].contains(**slot))
                .map(|slot| {
                    let record = &collection.generation.records[slot.0 as usize];
                    (slot.0, record.range)
                })
                .collect::<Vec<_>>();
            if let Some(context) = context
                && ranges
                    .iter()
                    .any(|(_, range)| range.len() > context.limits.source_record_bytes)
            {
                return Err(context.fail(
                    ExecutionErrorKind::Resource,
                    "merged base record exceeds source read allowance",
                ));
            }
            if ranges.is_empty() {
                continue;
            }
            let first = crate::query_execution::relations::view::MergedRecordSlot {
                collection: collection_id,
                segment: None,
                slot: RecordSlot(ranges[0].0),
            };
            let resolved = view
                .record(first)?
                .ok_or("active merged base file missing")?;
            let path = repo.required_file_root().join(&resolved.file.relative_path);
            read_relation_source_file(
                &resolved.collection.required_file_reader(),
                &resolved.file,
                &path,
                &ranges,
                first,
                context,
                &mut source_errors,
                &mut opened,
                None,
                &mut process,
            )?;
        }
        let mut relation_prune: Option<Option<RelationTargetPrune>> =
            if let Some(plan) = exact_relation_plan.take() {
                Some(Some(plan))
            } else {
                None
            };
        for (segment_index, published) in segments[collection_index].iter().enumerate() {
            let pruned = if prune_segments {
                prune_relation_candidates(
                    repo,
                    base_active,
                    segments,
                    catalog,
                    published,
                    collection,
                    filter.as_ref(),
                    fts.as_ref(),
                    &mut relation_prune,
                )?
            } else {
                PrunedRelationCandidates {
                    candidates: None,
                    relation_available: false,
                }
            };
            let exact_filter_fields = if pruned.relation_available
                && let Some(plan) = relation_prune
                    .as_ref()
                    .and_then(Option::as_ref)
                    .filter(|plan| plan.exact)
            {
                Some(plan.evidence.as_slice())
            } else {
                None
            };
            let candidates = pruned.candidates;
            if candidates.as_ref().is_some_and(|may| may.is_empty()) {
                continue;
            }
            // A posting may leave only a handful of slots in a large segment.
            // Locate their files directly instead of decoding every file row.
            let mut ranges = Vec::new();
            let mut process_file = |file: FileId| -> Result<(), String> {
                const OP: &str = "read merged relation source file slots";
                if let Some(context) = context {
                    context.check_string()?;
                }
                ranges.clear();
                visit_checked_file_slots(published.as_ref(), file, &mut |slots| {
                    for &slot in slots {
                        if published.active.contains(slot)
                            && candidates.as_ref().is_none_or(|may| may.contains(slot.0))
                        {
                            let range = published
                                .origin(slot)?
                                .and_then(RecordOrigin::file)
                                .filter(|locator| locator.file == file)
                                .and_then(|locator| locator.range)
                                .ok_or_else(|| StorageError::invalid(OP, "record range missing"))?;
                            ranges.push((slot.0, range));
                        }
                    }
                    Ok(())
                })?;
                if let Some(context) = context
                    && ranges
                        .iter()
                        .any(|(_, range)| range.len() > context.limits.source_record_bytes)
                {
                    return Err(context.fail(
                        ExecutionErrorKind::Resource,
                        "merged segment record exceeds source read allowance",
                    ));
                }
                if ranges.is_empty() {
                    return Ok(());
                }
                let first = crate::query_execution::relations::view::MergedRecordSlot {
                    collection: collection_id,
                    segment: Some(segment_index),
                    slot: RecordSlot(ranges[0].0),
                };
                let resolved = view
                    .record(first)?
                    .ok_or("active merged segment file missing")?;
                let path = repo.required_file_root().join(&resolved.file.relative_path);
                read_relation_source_file(
                    &resolved.collection.required_file_reader(),
                    &resolved.file,
                    &path,
                    &ranges,
                    first,
                    context,
                    &mut source_errors,
                    &mut opened,
                    exact_filter_fields,
                    &mut process,
                )
            };
            if let Some(may) = candidates.as_ref()
                && may.len().saturating_mul(8) < published.file_count() as u64
            {
                let mut selected = std::collections::BTreeSet::new();
                for number in may.iter() {
                    let slot = RecordSlot(number);
                    if !published.active.contains(slot) {
                        continue;
                    }
                    let locator = published
                        .origin(slot)?
                        .and_then(RecordOrigin::file)
                        .ok_or("merged segment candidate locator missing")?;
                    selected.insert(locator.file);
                }
                for file in selected {
                    if !published.contains_file(file)? {
                        return Err("merged segment candidate file missing".into());
                    }
                    process_file(file)?;
                }
            } else {
                visit_segment_files(published.as_ref(), |file| process_file(file.id))?;
            }
        }
    }
    let mut page = winners
        .into_sorted_vec()
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .collect::<Vec<_>>();
    if query.options.return_mode != ReturnMode::Locator {
        let mut retained = Vec::with_capacity(page.len());
        for mut winner in page {
            if let Some(context) = context {
                context.check_string()?;
                context.charge_reads(1)?;
                context.charge_bytes(winner.hit.source_range_len())?;
            }
            let payload_hit = if let Some(address) = winner.record_address {
                materialize_merged_address(repo, &view, address, query.options.return_mode.clone())
            } else if let Some((collection_index, segment_index)) = winner.segment {
                let published = &segments[collection_index][segment_index];
                get_segment_record_by_id(
                    repo,
                    collection_index,
                    published.as_ref(),
                    &published.active,
                    catalog,
                    revision,
                    &winner.hit.record_id,
                    query.options.return_mode.clone(),
                )
            } else {
                repo.get_record_with_active(
                    &winner.hit.collection,
                    &winner.hit.record_id,
                    query.options.return_mode.clone(),
                    Some(base_active),
                )
            };
            let mut payload_hit = match payload_hit {
                Ok(Some(payload_hit)) => payload_hit,
                Ok(None) => return Err("selected record retired in pinned view".into()),
                Err(message) => {
                    if record_unavailable_hit(
                        repo,
                        catalog,
                        SourceHitLocator::from_hit(&winner.hit),
                        message.clone(),
                        &mut counters,
                    )? {
                        continue;
                    }
                    return Err(message);
                }
            };
            payload_hit.generation = revision.0;
            payload_hit.matched_fields = winner.hit.matched_fields.clone();
            winner.hit = payload_hit;
            retained.push(winner);
        }
        page = retained;
    }
    counters.total_matches = total;
    source_errors.apply(&mut counters);
    view.take_source_errors().apply(&mut counters);
    counters.match_count_complete = counters.source_unavailable_events == 0;
    counters.returned_records = page.len() as u64;
    counters.distinct_files = opened.len() as u64;
    counters.sort_target_reads += sort_cache.reads;
    counters.sort_target_reuse += sort_cache.reuse;
    counters.relation_edge_visits += sort_cache.edge_visits;
    counters.relation_source_reads += sort_cache.relation_source_reads;
    counters.relation_bytes_read += sort_cache.relation_source_bytes;
    counters.files_opened += sort_cache.reads + sort_cache.relation_source_reads;
    counters.record_parses += sort_cache.reads + sort_cache.relation_source_reads;
    counters.bytes_read += sort_cache.bytes_read + sort_cache.relation_source_bytes;
    if !reference && prepared.len() == 1 {
        record_source_candidates(
            repo,
            context,
            prepared[0].collection_index,
            counters.candidates,
        );
    }
    Ok(SearchResult {
        count: (query.options.count && counters.match_count_complete).then_some(total as usize),
        hits: page.into_iter().map(|winner| winner.hit).collect(),
        counters,
    })
}

/// Scan the pinned base and segment domains without posting pruning. This is
/// the independent candidate path used to check merged planner results.
pub(crate) fn search_segmented_reference(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
) -> Result<SearchResult, String> {
    crate::reference::relations::search_segmented(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
    )
}

pub(crate) fn search_segmented_reference_records(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &dyn FileCatalogReader,
    revision: IndexRevision,
    collections: &mut [crate::reference::relations::ReferenceCollectionQuery],
) -> Result<SearchResult, String> {
    if segments.len() != repo.collections.len() || base_active.len() != repo.collections.len() {
        return Err("segment query collection count mismatch".into());
    }
    let sort = if segments
        .iter()
        .flat_map(|group| group.iter())
        .any(|segment| !matches!(segment.backing_kind(), RecordBacking::FileRange))
    {
        ResolvedSort::new(repo, query)?
    } else {
        ResolvedSort::new_reference(repo, query)?
    };
    let prepared = collections
        .iter_mut()
        .map(|entry| PreparedCollectionQuery {
            collection_index: entry.collection_index,
            filter: entry.filter.take(),
            fts: entry.fts.take(),
            filter_shape: None,
            text_term: None,
        })
        .collect::<Vec<_>>();
    search_segmented_relations(
        repo,
        query,
        base_active,
        segments,
        catalog,
        revision,
        &sort,
        &prepared,
        None,
        None,
        None,
        false,
        true,
    )
}

pub(crate) fn explain_segmented(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    revision: IndexRevision,
) -> Result<Vec<QueryPlanTrace>, String> {
    use crate::query_access::contract::{AccessKind, ExplainAccess, ProofStrength};
    use crate::query_execution::restrictions::segment_filter_explain_offers;
    let is_exact_posting = |offer: &crate::query_access::contract::AccessOffer| {
        offer.kind() == AccessKind::ExactMembership && offer.proof() == ProofStrength::Exact
    };
    if segments.len() != repo.collections.len() || base_active.len() != repo.collections.len() {
        return Err("segment explanation collection count mismatch".into());
    }
    let mut traces = explain_with_active(repo, query, Some(base_active))?;
    let prepared = crate::query::prepared::prepare(repo, query)?;
    let resolved_sort = ResolvedSort::new(repo, query)?;
    for trace in &mut traces {
        let collection_index = repo
            .collections
            .iter()
            .position(|collection| collection.definition.name == trace.collection)
            .ok_or("explained collection missing")?;
        let collection = &repo.collections[collection_index];
        let entry = prepared
            .collections
            .iter()
            .find(|entry| entry.collection_index == collection_index)
            .ok_or("explained collection has no prepared query")?;
        let filter = entry.filter.as_ref();
        let fts = entry.fts.as_ref();
        let mut delta = 0u64;
        let mut delta_candidates = 0u64;
        let mut delta_fts_candidates = 0u64;
        for published in segments[collection_index].iter() {
            delta = delta.saturating_add(published.active.len());
            // Ordinary EXPLAIN keeps this estimate symbolic. Applying an
            // access merely to improve the displayed bitmap would violate the
            // diagnostic boundary.
            let mut candidates = filter
                .map(|filter| match filter {
                    BoundExpr::Term(term) => {
                        let exact = segment_filter_explain_offers(
                            published.as_ref(),
                            collection,
                            term,
                            published.active.len(),
                        )?
                        .iter()
                        .filter(|offer| is_exact_posting(offer))
                        .map(|offer| offer.estimated_output())
                        .min();
                        exact.map_or_else(
                            || {
                                estimate_expr(collection, filter, published.active.len())
                                    .map(|(_, estimated)| estimated.min(published.active.len()))
                            },
                            Ok,
                        )
                    }
                    _ => estimate_expr(collection, filter, published.active.len())
                        .map(|(_, estimated)| estimated.min(published.active.len())),
                })
                .transpose()?
                .unwrap_or(published.active.len());
            if let Some(fts) = fts {
                if let Some((text, _)) =
                    crate::query_execution::restrictions::segment_fts_estimate_with_term(
                        published.id,
                        published.as_ref(),
                        collection,
                        fts,
                        crate::query_predicate::TermId::new(0),
                        published.active.len(),
                    )?
                {
                    let text = text.min(published.active.len());
                    delta_fts_candidates = delta_fts_candidates.saturating_add(text);
                    candidates = candidates.min(text);
                } else {
                    delta_fts_candidates =
                        delta_fts_candidates.saturating_add(published.active.len());
                }
            }
            delta_candidates = delta_candidates.saturating_add(candidates);
        }
        trace.generation = revision.0;
        trace.input_records = trace.input_records.saturating_add(delta);
        trace.candidates = trace.candidates.saturating_add(delta_candidates);
        trace.fts_estimated_candidates = fts.as_ref().map(|_| {
            trace
                .fts_estimated_candidates
                .unwrap_or(base_active[collection_index].len())
                .saturating_add(delta_fts_candidates)
        });
        trace.candidates_estimated = true;
        if delta > 0 {
            trace.sort_strategy = "merged base and indexed segment top-k".into();
            trace.sort_strategy_estimated = true;
        }
        if !query.options.sort.is_empty()
            && trace.candidates > 0
            && trace.candidates
                < query
                    .options
                    .offset
                    .saturating_add(query.options.limit)
                    .saturating_mul(4) as u64
        {
            trace.sort_strategy = "bounded_top_k".into();
            trace.sort_strategy_estimated = false;
            trace.planning.reused_order_plans = trace.planning.reused_order_plans.max(1);
            trace.planning.candidate_bitmap_builds = trace.planning.candidate_bitmap_builds.max(1);
        }
        if filter
            .as_ref()
            .is_none_or(|filter| !bound_expr_has_relations(filter))
            && fts
                .as_ref()
                .is_none_or(|fts| fts.fields.iter().all(|field| field.related.is_none()))
            && !(trace.candidates > 0
                && trace.candidates
                    < query
                        .options
                        .offset
                        .saturating_add(query.options.limit)
                        .saturating_mul(4) as u64)
            && segments[collection_index]
                .iter()
                .try_fold(true, |all, segment| {
                    Ok::<_, String>(
                        all && lifecycle_segment_order(
                            segment.as_ref(),
                            &resolved_sort,
                            query,
                            collection_index,
                        )?
                        .is_some(),
                    )
                })?
        {
            trace.fts_strategy = "stored order page traversal".into();
            trace.sort_strategy = if query.options.sort.len() > 1 {
                "stored_composite_order_page"
            } else {
                "stored_order_page"
            }
            .into();
            trace.sort_strategy_estimated = true;
        }
        if delta > 0 {
            trace.filter_exact = false;
            trace.remaining_verification = fts.is_some() || !trace.filter_exact;
        }
        if base_active[collection_index].len() == 0
            && let Some(BoundExpr::Term(term)) = filter
            && let Some(choice) = trace.terms.first_mut()
        {
            let mut operation = None;
            let mut work = 0.0;
            let mut exact_output = 0u64;
            for published in segments[collection_index].iter() {
                let selected = segment_filter_explain_offers(
                    published.as_ref(),
                    collection,
                    term,
                    published.active.len(),
                )?
                .into_iter()
                .find(is_exact_posting);
                let Some(proposal) = selected else {
                    operation = None;
                    break;
                };
                if operation.is_some_and(|previous| previous != proposal.explanation()) {
                    operation = None;
                    break;
                }
                work += proposal.estimated_work(published.active.len());
                exact_output = exact_output.saturating_add(proposal.estimated_output());
                operation = Some(proposal.explanation());
            }
            if let Some(explanation) = operation {
                let old = std::mem::replace(
                    &mut choice.selected,
                    crate::query_execution::restrictions::AccessPathEstimate {
                        kind: crate::query_execution::restrictions::AccessPathKind::Posting,
                        exact: true,
                        coverage: trace.input_records.min(u32::MAX as u64) as u32,
                        estimated_output: delta_candidates,
                        estimated_work: work + delta_candidates as f64 * 0.02,
                        estimated_files: 0.0,
                        estimated_bytes: 0.0,
                        posting: match explanation {
                            Some(ExplainAccess::Posting(id, operation)) => Some((
                                id,
                                operation,
                                trace.input_records.min(u32::MAX as u64) as u32,
                            )),
                            _ => None,
                        },
                    },
                );
                choice.alternatives.push(old);
                choice.reason = if matches!(
                    explanation,
                    Some(ExplainAccess::Posting(
                        _,
                        crate::query_access::contract::PostingOperation::SelfComparePresence
                    ))
                ) {
                    "self-comparison reduced to path presence in indexed segments; estimate only"
                } else {
                    "indexed segment posting offered for this predicate; estimate only"
                }
                .into();
                choice.residual_verification = false;
                trace.candidates = trace
                    .candidates
                    .saturating_sub(delta_candidates)
                    .saturating_add(exact_output);
                if fts.is_none()
                    && trace
                        .terms
                        .iter()
                        .all(|term| term.selected.exact && !term.residual_verification)
                {
                    trace.filter_exact = true;
                    trace.remaining_verification = false;
                }
            } else if let Some(choice) = trace.terms.first_mut()
                && matches!(term.source_kind, LogicalType::String | LogicalType::Unknown)
                && matches!(term.operator, Operator::Eq | Operator::Contains)
                && term.derived == Derived::None
                && term.operation.is_none()
                && term.matcher.is_none()
                && term.gram_request.is_some()
            {
                let mut output = 0u64;
                let mut structural = 0u64;
                for published in segments[collection_index].iter() {
                    let estimate = segment_filter_explain_offers(
                        published.as_ref(),
                        collection,
                        term,
                        published.active.len(),
                    )?
                    .iter()
                    .find(|offer| offer.explanation() == Some(ExplainAccess::Bigram))
                    .map_or(published.active.len(), |offer| offer.estimated_output());
                    output = output.saturating_add(estimate);
                    structural = structural.saturating_add(
                        read_slot_members(published, SegmentSlotFact::EmptyArray)?
                            .view()
                            .intersection_len(
                                crate::query_access::membership::MembershipView::Borrowed(
                                    &published.active.bitmap(),
                                ),
                            ),
                    );
                }
                if output < delta && structural < delta {
                    choice.selected = crate::query_execution::restrictions::AccessPathEstimate {
                        kind: if matches!(term.binding_name.as_str(), "default" | "lower") {
                            crate::query_execution::restrictions::AccessPathKind::RawBigram
                        } else {
                            crate::query_execution::restrictions::AccessPathKind::CustomBigram
                        },
                        exact: false,
                        coverage: trace.input_records.min(u32::MAX as u64) as u32,
                        estimated_output: output,
                        estimated_work: output as f64,
                        estimated_files: 0.0,
                        estimated_bytes: 0.0,
                        posting: None,
                    };
                    choice.gram_candidates = Some(output);
                    choice.structural_candidates =
                        (!term.query_path.has_position).then_some(structural);
                    choice.residual_verification = true;
                    choice.reason =
                        "segment bigram index offers a residual candidate set; estimate only"
                            .into();
                }
            }
        }
        if base_active[collection_index].len() == 0
            && fts.is_none()
            && matches!(filter, Some(BoundExpr::And(_) | BoundExpr::Or(_)))
        {
            let mut terms = Vec::new();
            collect_plain_terms(filter.unwrap(), &mut terms);
            let mut all_exact = !terms.is_empty();
            let mut branch_outputs = Vec::new();
            for term in terms {
                let mut operation = None;
                let mut output = 0u64;
                for published in segments[collection_index].iter() {
                    let proposal = segment_filter_explain_offers(
                        published.as_ref(),
                        collection,
                        term,
                        published.active.len(),
                    )?
                    .into_iter()
                    .find(is_exact_posting);
                    let Some(proposal) = proposal else {
                        all_exact = false;
                        break;
                    };
                    if operation.is_some_and(|previous| previous != proposal.explanation()) {
                        all_exact = false;
                        break;
                    }
                    operation = Some(proposal.explanation());
                    output = output.saturating_add(proposal.estimated_output());
                }
                if !all_exact {
                    break;
                }
                branch_outputs.push(output);
                if let Some(choice) = trace
                    .terms
                    .iter_mut()
                    .find(|choice| choice.span == term.span)
                    && let Some(explanation) = operation
                {
                    choice.selected = crate::query_execution::restrictions::AccessPathEstimate {
                        kind: crate::query_execution::restrictions::AccessPathKind::Posting,
                        exact: true,
                        coverage: trace.input_records.min(u32::MAX as u64) as u32,
                        estimated_output: output,
                        estimated_work: output as f64 * 0.02,
                        estimated_files: 0.0,
                        estimated_bytes: 0.0,
                        posting: match explanation {
                            Some(ExplainAccess::Posting(id, operation)) => Some((
                                id,
                                operation,
                                trace.input_records.min(u32::MAX as u64) as u32,
                            )),
                            _ => None,
                        },
                    };
                    choice.residual_verification = false;
                    choice.reason = "indexed segment posting offered; estimate only".into();
                }
            }
            if all_exact {
                trace.filter_exact = true;
                trace.remaining_verification = false;
                trace.candidates = if matches!(filter, Some(BoundExpr::And(_))) {
                    branch_outputs.into_iter().min().unwrap_or(0)
                } else {
                    branch_outputs.into_iter().sum()
                };
            }
        }
        if base_active[collection_index].len() == 0
            && let Some(filter) = filter
        {
            let mut file_path_terms = Vec::new();
            collect_file_path_terms(filter, &mut file_path_terms);
            for term in file_path_terms {
                if let Some(choice) = trace
                    .terms
                    .iter_mut()
                    .find(|choice| choice.span == term.span)
                {
                    choice.selected = crate::query_execution::restrictions::AccessPathEstimate {
                        kind: crate::query_execution::restrictions::AccessPathKind::FilePathIndex,
                        exact: true,
                        coverage: trace.input_records.min(u32::MAX as u64) as u32,
                        estimated_output: delta_candidates,
                        estimated_work: delta_candidates as f64 * 0.02,
                        estimated_files: 0.0,
                        estimated_bytes: 0.0,
                        posting: None,
                    };
                    choice.residual_verification = false;
                    choice.reason = "pinned segment file-path index".into();
                }
            }
        }
        if base_active[collection_index].len() == 0
            && let Some(filter) = filter
        {
            let mut relation_terms = Vec::new();
            collect_positive_relation_terms(filter, &mut relation_terms);
            for term in relation_terms {
                let input_len =
                    relation_source_input_estimate(repo, segments, collection_index, filter, term)?;
                let Ok((_, selected, alternatives)) =
                    crate::query_access::relations::semijoin::planning_estimate(
                        repo, term, input_len,
                    )
                else {
                    continue;
                };
                if let Some(choice) = trace
                    .terms
                    .iter_mut()
                    .find(|choice| choice.span == term.span)
                {
                    let old = std::mem::replace(&mut choice.selected, selected);
                    choice.alternatives = alternatives;
                    choice.alternatives.push(old);
                    choice.reason = "relation semijoin selected from segment indexes".into();
                    choice.residual_verification = false;
                }
            }
        }
        if base_active[collection_index].len() == 0
            && let Some(fts) = fts
        {
            let input_len =
                segment_filter_input_estimate(repo, segments, collection_index, filter)?;
            if let Some(strategies) =
                crate::query_access::relations::semijoin::planning_fts_strategies(
                    repo, fts, input_len,
                )?
            {
                trace.fts_strategy = if strategies.iter().all(|strategy| {
                    *strategy == crate::query_access::relations::semijoin::Strategy::TargetFirst
                }) {
                    "relation target-first semijoin"
                } else if strategies.iter().all(|strategy| {
                    *strategy
                        == crate::query_access::relations::semijoin::Strategy::SourceRestricted
                }) {
                    "relation source-restricted semijoin"
                } else {
                    "relation mixed semijoin"
                }
                .into();
            }
        }
        if matches!(filter, Some(BoundExpr::Or(_))) {
            let branch_counts = filter
                .as_ref()
                .map(|filter| or_branch_counts(repo, segments, collection_index, filter))
                .transpose()?
                .unwrap_or_default();
            trace.terms.sort_by(|left, right| {
                let rank = |kind| {
                    if kind == crate::query_execution::restrictions::AccessPathKind::Posting {
                        0
                    } else if kind
                        == crate::query_execution::restrictions::AccessPathKind::FilePathIndex
                    {
                        1
                    } else {
                        2
                    }
                };
                rank(left.selected.kind)
                    .cmp(&rank(right.selected.kind))
                    .then_with(|| (!left.selected.exact).cmp(&!right.selected.exact))
                    .then_with(|| {
                        branch_counts
                            .get(&left.span)
                            .copied()
                            .unwrap_or(left.selected.estimated_output)
                            .cmp(
                                &branch_counts
                                    .get(&right.span)
                                    .copied()
                                    .unwrap_or(right.selected.estimated_output),
                            )
                    })
                    .then_with(|| left.field.cmp(&right.field))
            });
            if let Some(BoundExpr::Or(parts)) = filter
                && let Some(priority) = parts.iter().find_map(|part| {
                    let BoundExpr::Term(term) = part else {
                        return None;
                    };
                    collection
                        .definition
                        .schema
                        .fields()
                        .get(&term.path.segments)
                        .is_some_and(|field| field.indexed)
                        .then(|| term.query_path.display.as_str())
                })
                && let Some(position) = trace
                    .terms
                    .iter()
                    .position(|choice| choice.field == priority)
            {
                let priority = trace.terms.remove(position);
                trace.terms.insert(0, priority);
            }
        }
    }
    Ok(traces)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_boolean_truth_keeps_large_domain_in_segment_bitmaps() {
        let mut universe = NativeTruth::empty(64);
        for segment in 0..64 {
            for slot in 0..1024 {
                universe.insert(segment, slot, &[]);
            }
        }
        assert_eq!(universe.len(), 65_536);
        assert!(universe.evidence.is_empty());
        let mut left = NativeTruth::empty(64);
        left.insert(3, 10, &["left".into()]);
        left.insert(3, 11, &[]);
        let mut right = NativeTruth::empty(64);
        right.insert(3, 10, &["right".into()]);
        right.insert(4, 12, &[]);
        left.and(right);
        assert_eq!(left.len(), 1);
        assert_eq!(left.evidence[&(3, 10)], ["left", "right"]);
        let inverse = universe.without(left);
        assert_eq!(inverse.len(), 65_535);
        assert!(!inverse.contains(3, 10));
    }

    #[test]
    fn relation_candidate_composition_preserves_ownership_and_membership() {
        let (unrestricted, relation_available) = combine_relation_candidates(None, None);
        assert!(unrestricted.is_none());
        assert!(!relation_available);

        let filter = Arc::new(RoaringBitmap::from_iter([1, 4]));
        let (filter_only, relation_available) =
            combine_relation_candidates(Some(Arc::clone(&filter)), None);
        assert!(Arc::ptr_eq(filter_only.as_ref().unwrap(), &filter));
        assert!(!relation_available);

        let relation = Arc::new(RoaringBitmap::from_iter([3, 4]));
        let (relation_only, relation_available) =
            combine_relation_candidates(None, Some(Arc::clone(&relation)));
        assert!(Arc::ptr_eq(relation_only.as_ref().unwrap(), &relation));
        assert!(relation_available);

        let (intersection, relation_available) =
            combine_relation_candidates(Some(Arc::clone(&filter)), Some(relation));
        assert_eq!(
            intersection.as_deref(),
            Some(&RoaringBitmap::from_iter([4]))
        );
        assert!(relation_available);

        let (empty_result, relation_available) = combine_relation_candidates(
            Some(Arc::new(RoaringBitmap::from_iter([2]))),
            Some(Arc::new(RoaringBitmap::new())),
        );
        assert!(
            empty_result
                .as_ref()
                .is_some_and(|result| result.is_empty())
        );
        assert!(relation_available);
    }
}
