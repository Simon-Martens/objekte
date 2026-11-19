//! Segment relation planning and page execution.

use super::*;

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
    view: &crate::query_execution::relations::evaluation::MergedRelationView<'_>,
    address: crate::query_execution::relations::evaluation::MergedRecordSlot,
    return_mode: ReturnMode,
) -> Result<Option<SearchHit>, String> {
    let Some(resolved) = view.record(address)? else {
        return Ok(None);
    };
    let record = resolved.record;
    let file = resolved.file;
    let payload = match return_mode {
        ReturnMode::Locator => None,
        ReturnMode::Logical if record.class == RecordClass::RawFallback => {
            Some(Payload::LogicalUnavailable {
                diagnostic_ref: record.diagnostic_ref,
            })
        }
        ReturnMode::Bytes | ReturnMode::Logical => {
            let path = repo.files.root.join(&file.relative_path);
            let bytes = crate::source::access::read_record_range_for_search(
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
        file_id: file.id.0,
        file_path: file.relative_path.to_path_buf(),
        file_size: file.revision.size,
        file_created_ns: file.created_ns,
        byte_start: record.range.start,
        byte_end: record.range.end,
        modified_ns: record.revision.modified_ns,
        matched_fields: Vec::new(),
        file_status: file.status,
        record_class: record.class,
        diagnostic_ref: record.diagnostic_ref,
        payload,
    }))
}

#[derive(Clone)]
struct RelationTargetPrune {
    source_path: Vec<String>,
    target_ids: HashSet<String>,
    exact: bool,
    evidence: Vec<String>,
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
    let relation = source.relations.get(hop.relation.ordinal as usize)?;
    if term.quantifier == Quantifier::Default {
        if relation.declaration.cardinality != crate::schema::relations::RelationCardinality::One {
            return None;
        }
        for depth in 1..relation.declaration.path.len() {
            let ancestor = source
                .definition
                .schema
                .fields()
                .get(&relation.declaration.path[..depth])?;
            if matches!(ancestor.kind, crate::model::LogicalType::Array(_)) {
                return None;
            }
        }
    }
    Some((hop, &related.terminal))
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
        &IndexedCollectionSegment,
        &ActiveRecords,
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
        let Some(mut may) = target_candidates(&published.index, &published.active, target)? else {
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
                .index
                .record(RecordSlot(slot))
                .ok_or("relation target candidate missing")?;
            target_ids.insert(record.id.0.to_string());
        }
    }
    let relation = repo
        .collections
        .get(hop.from.0 as usize)
        .and_then(|collection| collection.relations.get(hop.relation.ordinal as usize))
        .ok_or("relation source declaration missing")?;
    Ok(Some(RelationTargetPrune {
        source_path: relation.declaration.path.clone(),
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
            &published.index,
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
                .index
                .record(RecordSlot(slot))
                .ok_or("exact relation target slot missing")?;
            target_ids.insert(record.id.0.to_string());
            if target_ids.len() > 100_000 {
                return Ok(None);
            }
        }
    }
    let relation = repo.collections[hop.from.0 as usize]
        .relations
        .get(hop.relation.ordinal as usize)
        .ok_or("exact relation declaration missing")?;
    let mut evidence = vec![term.query_path.display.clone()];
    evidence.push(term.related.as_ref().unwrap().evidence_path.clone());
    evidence.sort();
    evidence.dedup();
    Ok(Some(RelationTargetPrune {
        source_path: relation.declaration.path.clone(),
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
            |segment, active, collection| {
                segment_filter_bounds(
                    segment,
                    active,
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
            |segment, _active, collection| segment_fts_candidates(segment, collection, &local),
        );
    }
    Ok(None)
}

fn relation_source_candidates(
    published: &PublishedSegment,
    plan: &RelationTargetPrune,
) -> Result<Option<RoaringBitmap>, String> {
    let Some(posting) = published.index.postings.fields.get(&plan.source_path) else {
        return Ok(None);
    };
    let mut may = if plan.exact {
        RoaringBitmap::new()
    } else {
        &published.active.bitmap() - &published.index.logical_records
    };
    for id in &plan.target_ids {
        if let Some(slots) =
            posting.value_posting(&crate::index::posting::ScalarKey::Id(id.clone()))
        {
            may |= slots;
        }
    }
    may &= published.active.bitmap();
    Ok(Some(may))
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
    catalog: &FileCatalog,
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
    let path = repo.files.root.join(&resolved.file.relative_path);
    let bytes = match read_record_range_for_search(
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
    let borrow_value_text = !target_collection.source.is_extension()
        && target_collection.source.scalar_verification()
            == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
        && matches!(predicate, DirectRelationPredicate::Fts(_));
    let extracted = prepare_query_record_with_execution(
        resolved.record.class,
        &target_collection.source,
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
        record: resolved.record,
        file: resolved.file,
        collection_name: &target_collection.definition.name,
        repository_root: &repo.files.root,
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
    catalog: &FileCatalog,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[PreparedCollectionQuery],
    context: Option<&ExecutionContext>,
    mut cache_builder: Option<&mut LifecycleCacheBuilder>,
    retained_budget: Option<usize>,
) -> Result<Option<SearchResult>, String> {
    if query.options.return_mode != ReturnMode::Locator || prepared.len() != 1 {
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
                    && eligible_one_hop_relation_term(repo, term).is_some()
                    && matches!(second, BoundExpr::Term(_))
                {
                    (term, Some(second))
                } else if let BoundExpr::Term(term) = second
                    && eligible_one_hop_relation_term(repo, term).is_some()
                    && matches!(first, BoundExpr::Term(_))
                {
                    (term, Some(first))
                } else {
                    return Ok(None);
                }
            }
            _ => return Ok(None),
        };
        let Some((hop, terminal)) = eligible_one_hop_relation_term(repo, term) else {
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
                &published.index,
                &published.active,
                local_filter,
                &repo.collections[*source_index],
                Some(catalog),
            )?;
            // Data predicates cannot match a raw fallback. Prove only the
            // logical source domain used by the relation reverse posting.
            let logical = &published.active.bitmap() & &published.index.logical_records;
            let may = bounds.may.as_ref() & &logical;
            let must = bounds.must.as_ref() & &logical;
            if may != must {
                return Ok(None);
            }
            local_proofs.push(must);
        }
    }
    let source_relation = repo.collections[*source_index]
        .relations
        .get(hop.relation.ordinal as usize)
        .ok_or("direct relation declaration missing")?;
    let direct_sort_column = if sort.bound.has_computed {
        let Some([term]) = sort.bound.terms[*source_index].as_deref() else {
            return Ok(None);
        };
        let BoundSortSource::Computed(computed) = &term.source else {
            return Ok(None);
        };
        let Some(path) = computed.direct_related_target_column(hop) else {
            return Ok(None);
        };
        if source_relation.declaration.cardinality
            != crate::schema::relations::RelationCardinality::One
        {
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
            let Ok(column) = published.index.sorting.column(path) else {
                return Ok(None);
            };
            if !crate::query::sort_binding::stored_column_matches_config(column, config) {
                return Ok(None);
            }
        }
        Some(path.to_owned())
    } else {
        None
    };
    let source_path = &source_relation.declaration.path;
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
        let segment = &published.index;
        let (may, must) = match &predicate {
            DirectRelationPredicate::Term(term) => {
                let bounds = segment_filter_bounds_with_catalog(
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
                segment_fts_candidates(segment, target_collection, local)?
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
            && may.iter().all(|slot| {
                segment.record(RecordSlot(slot)).is_some_and(|record| {
                    record.range.len() <= 64 * 1024
                        && record.id.0.len() <= 1024
                        && catalog
                            .entry(record.file)
                            .is_some_and(|file| file.relative_path.to_string_lossy().len() <= 4096)
                })
            });
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
            let resolved =
                segment_record_ref(segment, &published.active, catalog, RecordSlot(slot))?
                    .ok_or("direct relation target retired")?;
            if resolved.record.class == RecordClass::RawFallback {
                continue;
            }
            let matched = if must.as_ref().is_some_and(|must| must.contains(slot)) {
                // This is an index-only Locator/count proof. Actual payload or
                // residual reads use the checked pinned source identity.
                true
            } else {
                let path = repo.files.root.join(&resolved.file.relative_path);
                if let Some(context) = context {
                    context.charge_reads(1)?;
                    context.charge_bytes(resolved.record.range.len())?;
                }
                let bytes = match read_record_range_for_search(
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
                let borrow_value_text = !target_collection.source.is_extension()
                    && target_collection.source.scalar_verification()
                        == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
                    && matches!(predicate, DirectRelationPredicate::Fts(_));
                let extracted = prepare_query_record_with_execution(
                    resolved.record.class,
                    &target_collection.source,
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
                    record: resolved.record,
                    file: resolved.file,
                    collection_name: &target_collection.definition.name,
                    repository_root: &repo.files.root,
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
                if !target_ids.contains(resolved.record.id.0.as_ref()) {
                    let sort_key = if let Some(path) = direct_sort_column.as_deref() {
                        Some(segment.sorting.key(path, RecordSlot(slot))?.clone())
                    } else {
                        None
                    };
                    let sort_bytes = sort_key.as_ref().map_or(0, |key| {
                        resolved
                            .record
                            .id
                            .0
                            .len()
                            .saturating_add(crate::query::sort_binding::key_bytes(key))
                            .saturating_add(64)
                    });
                    if !reserve_route(
                        resolved
                            .record
                            .id
                            .0
                            .len()
                            .saturating_add(64)
                            .saturating_add(sort_bytes),
                    ) {
                        return Ok(None);
                    }
                    if let Some(key) = sort_key {
                        target_sort_keys.insert(resolved.record.id.0.clone(), key);
                    }
                    target_ids.insert(resolved.record.id.0.to_string());
                }
            }
        }
    }
    let mut source_candidates = Vec::new();
    counters.relation_edge_visits = counters
        .relation_edge_visits
        .saturating_add(target_ids.len() as u64);
    for (segment_index, published) in segments[*source_index].iter().enumerate() {
        let Some(posting) = published.index.postings.fields.get(source_path) else {
            return Ok(None);
        };
        let (mut may, lists, members) = posting.union_id_postings(&target_ids);
        counters.relation_reverse_postings_visited += lists;
        counters.relation_reverse_members_visited += members;
        may &= published.active.bitmap();
        may &= &published.index.logical_records;
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
    if context.is_none()
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
            lifecycle_segment_order(&published.index, sort, query, *source_index)
        } else {
            None
        };
        if stored_order.is_some() {
            // A global top `cap` cannot include a row ranked below its own
            // shard's top `cap`. Count the full exact bitmap without building
            // rank keys and hit locators for every matching source.
            total += may.len();
            counters.exact_posting_matches += may.len();
        }
        let slots: Box<dyn Iterator<Item = u32> + '_> = if let Some(order) = stored_order {
            Box::new(
                order
                    .iter()
                    .filter(|slot| may.contains(slot.0))
                    .take(cap)
                    .map(|slot| slot.0),
            )
        } else {
            Box::new(may.iter())
        };
        for slot in slots {
            if let Some(context) = context {
                context.check_string()?;
            }
            let resolved = segment_record_ref(
                &published.index,
                &published.active,
                catalog,
                RecordSlot(slot),
            )?
            .ok_or("direct relation source retired")?;
            let key = if direct_sort_column.is_some() {
                let relation = published
                    .index
                    .relations
                    .get(hop.relation.ordinal as usize)
                    .ok_or("direct relation source contribution missing")?;
                let mut references = relation.references_for_source(RecordSlot(slot));
                let Some((target_id, _)) = references.next() else {
                    return Ok(None);
                };
                if references.next().is_some() {
                    return Ok(None);
                }
                let Some(target_key) = target_sort_keys.get(target_id) else {
                    return Ok(None);
                };
                let term = &sort.bound.terms[*source_index]
                    .as_ref()
                    .ok_or("direct related sort term missing")?[0];
                RankKey {
                    parts: vec![(target_key.clone(), term.descending, term.options)],
                    collection: repo.collections[*source_index].definition.name.clone(),
                    id: resolved.record.id.0.to_string(),
                }
            } else {
                segment_rank_key(
                    repo,
                    sort,
                    *source_index,
                    &published.index,
                    resolved.record,
                    None,
                )?
            };
            if stored_order.is_none() {
                total += 1;
                counters.exact_posting_matches += 1;
            }
            let hit = segment_locator_hit(
                &repo.collections[*source_index],
                resolved,
                revision,
                evidence.clone(),
            );
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
        .map(|winner| winner.hit)
        .collect::<Vec<_>>();
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
    catalog: &FileCatalog,
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
    let mut filter_evidence = Vec::new();
    for published in segments[*collection_index].iter() {
        let may = if let Some(filter) = filter {
            let bounds = segment_filter_bounds_with_catalog(
                &published.index,
                &published.active,
                filter,
                collection,
                Some(catalog),
            )?;
            let logical = &published.active.bitmap() & &published.index.logical_records;
            if (bounds.may.as_ref() & &logical) != (bounds.must.as_ref() & &logical) {
                return Ok(None);
            }
            bounds.must.as_ref() & &logical
        } else {
            published.active.bitmap().clone()
        };
        let may = &may & &published.index.logical_records;
        input.insert_local(source_collection, published.id, may);
        if query.options.evidence
            && filter.is_some()
            && filter_evidence.is_empty()
            && let Some(fields) = filter.and_then(segment_static_evidence_fields)
        {
            filter_evidence = fields;
        }
    }
    let view = crate::query_execution::relations::evaluation::MergedRelationView::new(
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
        for (slot, record) in published.index.records.iter().enumerate() {
            let address = crate::index::segment::SegmentRecordAddress {
                collection: published.index.collection,
                segment: published.id,
                slot: record.slot,
            };
            if input.contains(address) && witnesses.qualified_evidence.contains_key(&address) {
                matches.insert(slot as u32);
            }
        }
        let mut evidence = filter_evidence.clone();
        if query.options.evidence {
            for slot in matches.iter() {
                let record = published
                    .index
                    .record(RecordSlot(slot))
                    .ok_or("semijoin source evidence record missing")?;
                let address = crate::index::segment::SegmentRecordAddress {
                    collection: published.index.collection,
                    segment: published.id,
                    slot: record.slot,
                };
                if let Some(fields) = witnesses.qualified_evidence.get(&address) {
                    evidence.extend(fields.iter().cloned());
                }
            }
            evidence.sort();
            evidence.dedup();
        }
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
    _catalog: &FileCatalog,
    revision: IndexRevision,
    collection_index: usize,
    filter: Option<&BoundExpr>,
    filter_proof: Option<&[String]>,
    fts: Option<&BoundFts>,
    job: &RelationFileJob,
    view: crate::query_execution::relations::evaluation::MergedRelationView<'_>,
) -> Result<RelationFileScan, String> {
    let _fold_record = FoldRecordGuard::new_with_sharing(shared_fold_bound(filter));
    let mut scan = RelationFileScan {
        counters: WorkCounters::default(),
        primary_opened: false,
        unavailable: SourceAvailabilityLog::default(),
        relation_unavailable: SourceAvailabilityLog::default(),
        winner: None,
    };
    let address = crate::query_execution::relations::evaluation::MergedRecordSlot {
        collection: repo.collections[collection_index].generation.collection,
        segment: Some(job.segment_index),
        slot: job.slot,
    };
    let resolved = view
        .record(address)?
        .ok_or("active parallel relation source missing")?;
    let path = repo.files.root.join(&resolved.file.relative_path);
    let mut reader = match RecordRangeReader::open_for_search(&path, resolved.file.revision, None) {
        Ok(reader) => reader,
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
        && resolved.collection.source.scalar_verification()
            == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
        && let Some(fields) = filter.and_then(segment_filter_source_fields)
    {
        projection.top_level = Some(fields);
    }
    projection.resolved_text_only =
        projection.collect_text && fts.is_some_and(|fts| !fts.fields.is_empty());
    let combined_fts = fts.filter(|fts| {
        resolved.collection.source.scalar_verification()
            == Some(crate::source::verification::ScalarVerification::DecodedScalars)
            && source_fts_eligible(resolved.collection, fts, true)
    });
    let (extracted, combined_result) = if record.class == RecordClass::Logical
        && let Some(combined_fts) = combined_fts
    {
        let (extracted, result) = prepare_decoded_record_with_borrowed_fts(
            &resolved.collection.source,
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
                &resolved.collection.source,
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
        record,
        file: resolved.file,
        collection_name: &resolved.collection.definition.name,
        repository_root: &repo.files.root,
    };
    let mut evidence = HashSet::new();
    let mut relation_work = crate::query_execution::relations::evaluation::RelationWork::default();
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
            &segments[collection_index][job.segment_index].index,
            record,
            Some(&ctx),
        )?;
        let mut hit = segment_locator_hit(
            resolved.collection,
            SegmentRecordRef {
                record,
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
    crate::query_execution::relations::evaluation::check_query_work_budget(total)
}

#[allow(clippy::too_many_arguments)]
fn try_parallel_relation_page(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
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
        .any(|published| !record_ranges_allow_parallel_scan(published.index.max_record_range_len()))
    {
        return Ok(None);
    }
    let collection = &repo.collections[*collection_index];
    let mut jobs = Vec::new();
    let mut relation_prune: Option<Option<RelationTargetPrune>> = exact_plan.cloned().map(Some);
    for (segment_index, published) in segments[*collection_index].iter().enumerate() {
        let mut candidates = if prune_segments {
            let filter_may = filter
                .as_ref()
                .map(|filter| {
                    segment_filter_bounds_with_catalog(
                        &published.index,
                        &published.active,
                        filter,
                        collection,
                        Some(catalog),
                    )
                    .map(|bounds| bounds.may)
                })
                .transpose()?;
            let fts_may = fts
                .as_ref()
                .map(|fts| segment_fts_candidates(&published.index, collection, fts))
                .transpose()?
                .flatten();
            match (filter_may, fts_may) {
                (Some(filter), Some(fts)) => Some(filter.as_ref() & fts),
                (Some(filter), None) => Some(filter.as_ref().clone()),
                (None, Some(fts)) => Some(fts),
                (None, None) => None,
            }
        } else {
            None
        };
        if prune_segments {
            let local_count = candidates
                .as_ref()
                .map_or_else(|| published.active.len(), RoaringBitmap::len);
            if local_count > 256 && relation_prune.is_none() {
                relation_prune = Some(relation_target_prune(
                    repo,
                    base_active,
                    segments,
                    filter,
                    fts,
                )?);
            }
            if let Some(Some(plan)) = relation_prune.as_ref()
                && let Some(relation_may) = relation_source_candidates(published, plan)?
            {
                candidates =
                    Some(candidates.map_or(relation_may.clone(), |may| may & relation_may));
            }
            if let Some(plan) = exact_plan
                && relation_source_candidates(published, plan)?.is_none()
            {
                return Ok(None);
            }
        }
        if candidates.as_ref().is_some_and(RoaringBitmap::is_empty) {
            continue;
        }
        // Posting and FTS bounds can leave only a few slots in a large
        // collection. Resolve those slots directly instead of probing every
        // file in the segment to discover the same jobs.
        if let Some(candidates) = candidates.as_ref()
            && candidates.len().saturating_mul(8) < published.index.file_count() as u64
        {
            let mut selected_files = HashSet::new();
            for slot_number in candidates.iter() {
                let slot = RecordSlot(slot_number);
                if !published.active.contains(slot) {
                    continue;
                }
                let record = published
                    .index
                    .record(slot)
                    .ok_or("parallel relation slot missing")?;
                let file = published
                    .index
                    .file(record.file)
                    .ok_or("parallel relation file missing")?;
                if !selected_files.insert(record.file)
                    || record.range.len() > 64 * 1024
                    || record.id.0.len() > 1024
                    || file.entry.relative_path.to_string_lossy().len() > 4096
                {
                    return Ok(None);
                }
                jobs.push(RelationFileJob {
                    segment_index,
                    file_id: record.file.0,
                    slot,
                    range: record.range,
                });
                if jobs.len() > 50_000 {
                    return Ok(None);
                }
            }
            continue;
        }
        for file_index in 0..published.index.file_count() {
            let file = published
                .index
                .files
                .get(file_index)
                .ok_or("parallel relation file missing")?;
            let mut selected = file.records.iter().filter(|slot| {
                published.active.contains(**slot)
                    && candidates.as_ref().is_none_or(|may| may.contains(slot.0))
            });
            let Some(slot) = selected.next().copied() else {
                continue;
            };
            if selected.next().is_some() {
                return Ok(None);
            }
            let record = published
                .index
                .record(slot)
                .ok_or("parallel relation slot missing")?;
            if record.range.len() > 64 * 1024
                || record.id.0.len() > 1024
                || file.entry.relative_path.to_string_lossy().len() > 4096
            {
                return Ok(None);
            }
            jobs.push(RelationFileJob {
                segment_index,
                file_id: file.entry.id.0,
                slot,
                range: record.range,
            });
            if jobs.len() > 50_000 {
                return Ok(None);
            }
        }
    }
    let view = crate::query_execution::relations::evaluation::MergedRelationView::new(
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

pub(super) fn search_segmented_relations(
    repo: &Repository,
    query: &Query,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    sort: &ResolvedSort<'_>,
    prepared: &[PreparedCollectionQuery],
    context: Option<&ExecutionContext>,
    mut cache_builder: Option<&mut LifecycleCacheBuilder>,
    retained_budget: Option<usize>,
    prune_segments: bool,
    reference: bool,
) -> Result<SearchResult, String> {
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
    let view = crate::query_execution::relations::evaluation::MergedRelationView::new(
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
            collection_plan.source.scalar_verification()
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
            && collection_plan.source.scalar_verification()
                == Some(crate::source::verification::ScalarVerification::ResolvedStrings)
            && let Some(fields) = filter.as_ref().and_then(segment_filter_source_fields)
        {
            projection.top_level = Some(fields);
        }
        projection.resolved_text_only =
            projection.collect_text && fts.as_ref().is_some_and(|fts| !fts.fields.is_empty());
        let mut process =
            |address: crate::query_execution::relations::evaluation::MergedRecordSlot,
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
                            direct
                                .scalar_verification
                                .ok_or_else(|| "direct scalar verification missing".to_owned())
                                .and_then(|verification| {
                                    verification.scan_borrowed_segments(
                                        &bytes,
                                        &BorrowedScanQuery {
                                            fts: direct.fts,
                                            filter: direct.filter.map(|(term, _)| term),
                                            retain_evidence: query.options.evidence,
                                        },
                                    )
                                })
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
                        &resolved.collection.source,
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
                            &resolved.collection.source,
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
                    record,
                    file: resolved.file,
                    collection_name: &resolved.collection.definition.name,
                    repository_root: &repo.files.root,
                };
                let mut evidence = HashSet::new();
                let mut relation_work =
                    crate::query_execution::relations::evaluation::RelationWork::default();
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
                        record,
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
            let first = crate::query_execution::relations::evaluation::MergedRecordSlot {
                collection: collection_id,
                segment: None,
                slot: RecordSlot(ranges[0].0),
            };
            let resolved = view
                .record(first)?
                .ok_or("active merged base file missing")?;
            let path = repo.files.root.join(&resolved.file.relative_path);
            let mut reader =
                match RecordRangeReader::open_for_search(&path, resolved.file.revision, context) {
                    Ok(reader) => reader,
                    Err(message) => {
                        source_errors.record(SourceDiagnostic {
                            file_id: resolved.file.id.0,
                            file_path: resolved.file.relative_path.to_path_buf(),
                            kind: classify_source_read_failure_for_search(
                                &path,
                                resolved.file.revision,
                                None,
                            ),
                            message,
                        });
                        continue;
                    }
                };
            if let Some(context) = context {
                context.charge_reads(1)?;
            }
            opened.insert(file_id.0);
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
                            file_id: resolved.file.id.0,
                            file_path: resolved.file.relative_path.to_path_buf(),
                            kind: classify_source_read_failure_for_search(
                                &path,
                                resolved.file.revision,
                                Some(ranges[start].1),
                            ),
                            message,
                        });
                        break;
                    }
                };
                for (index, (slot, bytes)) in read.into_iter().enumerate() {
                    process(
                        crate::query_execution::relations::evaluation::MergedRecordSlot {
                            collection: collection_id,
                            segment: None,
                            slot: RecordSlot(slot),
                        },
                        bytes,
                        start == 0 && index == 0,
                        None,
                    )?;
                }
            }
        }
        let mut relation_prune: Option<Option<RelationTargetPrune>> =
            if let Some(plan) = exact_relation_plan.take() {
                Some(Some(plan))
            } else {
                None
            };
        for (segment_index, published) in segments[collection_index].iter().enumerate() {
            let mut candidates = if prune_segments {
                let filter_may = filter
                    .as_ref()
                    .map(|filter| {
                        segment_filter_bounds_with_catalog(
                            &published.index,
                            &published.active,
                            filter,
                            collection,
                            Some(catalog),
                        )
                        .map(|bounds| bounds.may)
                    })
                    .transpose()?;
                let fts_may = fts
                    .as_ref()
                    .map(|fts| segment_fts_candidates(&published.index, collection, fts))
                    .transpose()?
                    .flatten();
                match (filter_may, fts_may) {
                    (Some(filter), Some(fts)) => Some(filter.as_ref() & fts),
                    (Some(filter), None) => Some(filter.as_ref().clone()),
                    (None, Some(fts)) => Some(fts),
                    (None, None) => None,
                }
            } else {
                None
            };
            if prune_segments {
                let local_count = candidates
                    .as_ref()
                    .map_or_else(|| published.active.len(), RoaringBitmap::len);
                if local_count > 256 && relation_prune.is_none() {
                    relation_prune = Some(relation_target_prune(
                        repo,
                        base_active,
                        segments,
                        filter.as_ref(),
                        fts.as_ref(),
                    )?);
                }
                if let Some(Some(plan)) = relation_prune.as_ref()
                    && let Some(relation_may) = relation_source_candidates(published, plan)?
                {
                    candidates =
                        Some(candidates.map_or(relation_may.clone(), |may| may & relation_may));
                }
            }
            let exact_filter_fields = if let Some(plan) = relation_prune
                .as_ref()
                .and_then(Option::as_ref)
                .filter(|plan| plan.exact)
            {
                relation_source_candidates(published, plan)?.map(|_| plan.evidence.as_slice())
            } else {
                None
            };
            if candidates.as_ref().is_some_and(RoaringBitmap::is_empty) {
                continue;
            }
            // A posting may leave only a handful of slots in a large segment.
            // Locate their files directly instead of decoding every file row.
            let file_indices: Box<dyn Iterator<Item = usize>> = if let Some(may) =
                candidates.as_ref()
                && may.len().saturating_mul(8) < published.index.file_count() as u64
            {
                let mut selected = std::collections::BTreeSet::new();
                for number in may.iter() {
                    let slot = RecordSlot(number);
                    if !published.active.contains(slot) {
                        continue;
                    }
                    let record = published
                        .index
                        .record(slot)
                        .ok_or("merged segment candidate missing")?;
                    let index = published
                        .index
                        .file_index
                        .get(&record.file)
                        .copied()
                        .ok_or("merged segment candidate file missing")?;
                    selected.insert(index);
                }
                Box::new(selected.into_iter())
            } else {
                Box::new(0..published.index.file_count())
            };
            for file_index in file_indices {
                let file = published
                    .index
                    .files
                    .get(file_index)
                    .ok_or("merged segment file missing")?;
                if let Some(context) = context {
                    context.check_string()?;
                }
                let mut ranges = Vec::new();
                for slot in &file.records {
                    if published.active.contains(*slot)
                        && candidates.as_ref().is_none_or(|may| may.contains(slot.0))
                    {
                        let record = published
                            .index
                            .record(*slot)
                            .ok_or("merged segment record missing")?;
                        ranges.push((slot.0, record.range));
                    }
                }
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
                    continue;
                }
                let first = crate::query_execution::relations::evaluation::MergedRecordSlot {
                    collection: collection_id,
                    segment: Some(segment_index),
                    slot: RecordSlot(ranges[0].0),
                };
                let resolved = view
                    .record(first)?
                    .ok_or("active merged segment file missing")?;
                let path = repo.files.root.join(&resolved.file.relative_path);
                let mut reader = match RecordRangeReader::open_for_search(
                    &path,
                    resolved.file.revision,
                    context,
                ) {
                    Ok(reader) => reader,
                    Err(message) => {
                        source_errors.record(SourceDiagnostic {
                            file_id: resolved.file.id.0,
                            file_path: resolved.file.relative_path.to_path_buf(),
                            kind: classify_source_read_failure_for_search(
                                &path,
                                resolved.file.revision,
                                None,
                            ),
                            message,
                        });
                        continue;
                    }
                };
                if let Some(context) = context {
                    context.charge_reads(1)?;
                }
                opened.insert(file.entry.id.0);
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
                                file_id: resolved.file.id.0,
                                file_path: resolved.file.relative_path.to_path_buf(),
                                kind: classify_source_read_failure_for_search(
                                    &path,
                                    resolved.file.revision,
                                    Some(ranges[start].1),
                                ),
                                message,
                            });
                            break;
                        }
                    };
                    for (index, (slot, bytes)) in read.into_iter().enumerate() {
                        process(
                            crate::query_execution::relations::evaluation::MergedRecordSlot {
                                collection: collection_id,
                                segment: Some(segment_index),
                                slot: RecordSlot(slot),
                            },
                            bytes,
                            start == 0 && index == 0,
                            exact_filter_fields,
                        )?;
                    }
                }
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
                context.charge_bytes(winner.hit.byte_end.saturating_sub(winner.hit.byte_start))?;
            }
            let payload_hit = if let Some(address) = winner.record_address {
                materialize_merged_address(repo, &view, address, query.options.return_mode.clone())
            } else if let Some((collection_index, segment_index)) = winner.segment {
                let published = &segments[collection_index][segment_index];
                get_segment_record_by_id(
                    repo,
                    collection_index,
                    &published.index,
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
                        SourceHitLocator::from(&winner.hit),
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
    catalog: &FileCatalog,
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
    catalog: &FileCatalog,
    revision: IndexRevision,
    collections: &mut [crate::reference::relations::ReferenceCollectionQuery],
) -> Result<SearchResult, String> {
    if segments.len() != repo.collections.len() || base_active.len() != repo.collections.len() {
        return Err("segment query collection count mismatch".into());
    }
    let sort = ResolvedSort::new_reference(repo, query)?;
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
                        let catalog = crate::query_access::catalog::IndexCatalog::segment(
                            &published.index,
                            collection,
                        );
                        let exact = catalog
                            .field_postings(
                                &term.path.segments,
                                &term.binding_name,
                                term.operation.is_some(),
                            )
                            .into_iter()
                            .flatten()
                            .filter_map(|access| {
                                access.assess(term, catalog.logical, &mut 0).transpose()
                            })
                            .collect::<Result<Vec<_>, String>>()?
                            .into_iter()
                            .filter(|proposal| proposal.exact)
                            .map(|proposal| proposal.output.min(published.active.len()))
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
                    crate::query_execution::restrictions::native_segment_estimate_fts_candidates(
                        &published.index,
                        collection,
                        fts,
                    )
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
            && segments[collection_index].iter().all(|segment| {
                lifecycle_segment_order(&segment.index, &resolved_sort, query, collection_index)
                    .is_some()
            })
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
                let catalog = crate::query_access::catalog::IndexCatalog::segment(
                    &published.index,
                    collection,
                );
                let selected = catalog
                    .field_postings(
                        &term.path.segments,
                        &term.binding_name,
                        term.operation.is_some(),
                    )
                    .into_iter()
                    .flatten()
                    .filter_map(|access| access.assess(term, catalog.logical, &mut 0).transpose())
                    .collect::<Result<Vec<_>, String>>()?
                    .into_iter()
                    .find(|proposal| proposal.exact);
                let Some(proposal) = selected else {
                    operation = None;
                    break;
                };
                if operation.is_some_and(|previous| previous != (proposal.id, proposal.operation)) {
                    operation = None;
                    break;
                }
                work += proposal.lookup_work;
                exact_output =
                    exact_output.saturating_add(proposal.output.min(published.active.len()));
                operation = Some((proposal.id, proposal.operation));
            }
            if let Some((id, operation)) = operation {
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
                        posting: Some((
                            id,
                            operation,
                            trace.input_records.min(u32::MAX as u64) as u32,
                        )),
                    },
                );
                choice.alternatives.push(old);
                choice.reason = if operation
                    == crate::query_access::catalog::PostingOperation::SelfComparePresence
                {
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
                    if let Some(estimated) =
                        crate::query_execution::restrictions::segment_text_estimate(
                            &published.index,
                            term,
                            collection,
                        )
                    {
                        output = output.saturating_add(estimated.min(published.active.len()));
                    } else {
                        output = output.saturating_add(published.active.len());
                    }
                    structural = structural.saturating_add(
                        (&published.index.empty_array_records & &published.active.bitmap()).len(),
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
                    let catalog = crate::query_access::catalog::IndexCatalog::segment(
                        &published.index,
                        collection,
                    );
                    let proposal = catalog
                        .field_postings(
                            &term.path.segments,
                            &term.binding_name,
                            term.operation.is_some(),
                        )
                        .into_iter()
                        .flatten()
                        .filter_map(|access| {
                            access.assess(term, catalog.logical, &mut 0).transpose()
                        })
                        .collect::<Result<Vec<_>, String>>()?
                        .into_iter()
                        .find(|proposal| proposal.exact);
                    let Some(proposal) = proposal else {
                        all_exact = false;
                        break;
                    };
                    if operation
                        .is_some_and(|previous| previous != (proposal.id, proposal.operation))
                    {
                        all_exact = false;
                        break;
                    }
                    operation = Some((proposal.id, proposal.operation));
                    output = output.saturating_add(proposal.output.min(published.active.len()));
                }
                if !all_exact {
                    break;
                }
                branch_outputs.push(output);
                if let Some(choice) = trace
                    .terms
                    .iter_mut()
                    .find(|choice| choice.span == term.span)
                    && let Some((id, operation)) = operation
                {
                    choice.selected = crate::query_execution::restrictions::AccessPathEstimate {
                        kind: crate::query_execution::restrictions::AccessPathKind::Posting,
                        exact: true,
                        coverage: trace.input_records.min(u32::MAX as u64) as u32,
                        estimated_output: output,
                        estimated_work: output as f64 * 0.02,
                        estimated_files: 0.0,
                        estimated_bytes: 0.0,
                        posting: Some((
                            id,
                            operation,
                            trace.input_records.min(u32::MAX as u64) as u32,
                        )),
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
