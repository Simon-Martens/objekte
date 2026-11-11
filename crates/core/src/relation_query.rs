//! Repository-aware binding and exact evaluation of finite relation paths.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::Arc;

use crate::inventory::{ActiveRecords, RecordClass};
use crate::lifecycle::PublishedSegment;
use crate::lifecycle_catalog::FileCatalog;
use crate::measurement::{SourceAvailabilityLog, SourceDiagnostic};
use crate::model::{CollectionId, FieldPath, LogicalType, PathRoot, RecordSlot, Value};
use crate::posting::ScalarKey;
use crate::query::{FilterExpr, FtsField, FtsQuery, Literal, Operand, Quantifier, Term};
use crate::query_path::{PathStep, QueryPath};
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, RecordContext, bind_field_probe, bind_filter_with_parameters,
    bind_fts, eval_borrowed_value_fts_field, eval_bound_fts, eval_bound_fts_field,
    eval_bound_fts_membership, eval_cross_record_field_term, eval_term,
};
use crate::relation::{RelationCardinality, RelationId, RelationTarget};
use crate::relation_target_cache::TargetValueCache;
use crate::repository::CollectionState;
use crate::repository::Repository;
use crate::source::{ScanProjection, TextDerivation, TextSegment, read_record_range_for_search};
use crate::source_access::classify_source_read_failure_for_search;

const MAX_RELATION_HOPS: usize = 8;
const MAX_PATH_STEPS: usize = 64;
const MAX_QUERY_EDGE_VISITS: u64 = 32_000_000;
const MAX_QUERY_BACKLINK_PROBES: u64 = 32_000_000;
const MAX_QUERY_RECORD_READS: u64 = 5_000_000;
const MAX_QUERY_RELATION_BYTES: u64 = 1024 * 1024 * 1024;

fn relation_target_uses_value_text(collection: &CollectionState) -> bool {
    !collection.source.is_extension() && matches!(collection.source.id(), "json" | "jsonl")
}

#[cfg(test)]
static BORROWED_RELATION_TARGETS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn borrowed_relation_targets() -> usize {
    BORROWED_RELATION_TARGETS.load(std::sync::atomic::Ordering::Relaxed)
}


/// One record address in a pinned base-plus-delta publication. A local slot
/// alone cannot identify a record after multiple segments have been added.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(crate) struct MergedRecordSlot {
    pub collection: CollectionId,
    pub segment: Option<usize>,
    pub slot: RecordSlot,
}


pub(crate) struct MergedRecord<'a> {
    pub record: &'a crate::inventory::RecordEntry,
    pub file: &'a crate::inventory::FileEntry,
    pub collection: &'a CollectionState,
}


/// Query-local relation lookup over a single immutable publication. Forward
/// edges and reverse postings are borrowed from the relevant segment on demand.
pub(crate) struct MergedRelationView<'a> {
    repo: &'a Repository,
    base_active: &'a [ActiveRecords],
    segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &'a FileCatalog,
    source_errors: RefCell<SourceAvailabilityLog>,
}


impl<'a> MergedRelationView<'a> {
    /// A worker keeps its own source-availability log while borrowing the
    /// same immutable pinned publication and catalog.
    pub(crate) fn fork_for_worker(&self) -> Self {
        Self {
            repo: self.repo,
            base_active: self.base_active,
            segments: self.segments,
            catalog: self.catalog,
            source_errors: RefCell::new(SourceAvailabilityLog::default()),
        }
    }

    pub(crate) fn source_error_events(&self) -> u64 {
        self.source_errors.borrow().events()
    }

    pub(crate) fn take_source_errors(&self) -> SourceAvailabilityLog {
        std::mem::take(&mut *self.source_errors.borrow_mut())
    }

    pub(crate) fn record_source_failure(
        &self,
        file: &crate::inventory::FileEntry,
        range: crate::inventory::ByteRange,
        message: String,
    ) {
        let path = self.repo.files.root.join(&file.relative_path);
        self.source_errors.borrow_mut().record(SourceDiagnostic {
            file_id: file.id.0,
            file_path: file.relative_path.clone(),
            kind: classify_source_read_failure_for_search(&path, file.revision, Some(range)),
            message,
        });
    }

    pub(crate) fn repository_root(&self) -> &std::path::Path {
        &self.repo.files.root
    }

    pub(crate) fn segment_ordinal(&self, address: MergedRecordSlot) -> Option<u64> {
        address
            .segment
            .map(|index| self.segments[address.collection.0 as usize][index].ordinal)
    }

    pub(crate) fn relation_cardinality(
        &self,
        relation: RelationId,
    ) -> Result<RelationCardinality, String> {
        Ok(self.repo.collections[relation.source.0 as usize]
            .relations
            .get(relation.ordinal as usize)
            .ok_or("merged relation declaration missing")?
            .declaration
            .cardinality)
    }

    pub(crate) fn new(
        repo: &'a Repository,
        base_active: &'a [ActiveRecords],
        segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
        catalog: &'a FileCatalog,
    ) -> Result<Self, String> {
        if base_active.len() != repo.collections.len() || segments.len() != repo.collections.len() {
            return Err("merged relation collection count mismatch".into());
        }
        for (collection_index, shards) in segments.iter().enumerate() {
            for published in shards.iter() {
                if published.index.collection.0 as usize != collection_index
                    || published.active.slot_count() as usize != published.index.records.len()
                {
                    return Err("merged relation segment scope mismatch".into());
                }
                for (relation_index, relation) in published.index.relations.iter().enumerate() {
                    if relation.record_count() as usize != published.index.records.len()
                        || repo.collections[collection_index]
                            .relations
                            .get(relation_index)
                            .is_none_or(|base| base.declaration != *relation.declaration())
                    {
                        return Err("merged relation segment declaration mismatch".into());
                    }
                }
            }
        }
        Ok(Self {
            repo,
            base_active,
            segments,
            catalog,
            source_errors: RefCell::new(SourceAvailabilityLog::default()),
        })
    }

    pub(crate) fn record(
        &self,
        address: MergedRecordSlot,
    ) -> Result<Option<MergedRecord<'a>>, String> {
        let index = address.collection.0 as usize;
        let collection = self
            .repo
            .collections
            .get(index)
            .ok_or("merged collection missing")?;
        let (record, original_file) = if let Some(segment_index) = address.segment {
            let published = self.segments[index]
                .get(segment_index)
                .ok_or("merged segment missing")?;
            if !published.active.contains(address.slot) {
                return Ok(None);
            }
            let segment = &published.index;
            let record = segment
                .record(address.slot)
                .ok_or("merged segment slot missing")?;
            let file = &segment
                .file(record.file)
                .ok_or("merged segment file missing")?
                .entry;
            (record, file)
        } else {
            if !self.base_active[index].contains(address.slot) {
                return Ok(None);
            }
            let record = collection
                .generation
                .records
                .get(address.slot.0 as usize)
                .ok_or("merged base slot missing")?;
            let file = self
                .repo
                .files
                .files
                .get(record.file.0 as usize)
                .ok_or("merged base file missing")?;
            (record, file)
        };
        let file = self
            .catalog
            .entry(record.file)
            .ok_or("active merged record file is retired")?;
        if file.revision != original_file.revision
            || !file.collections.contains(&address.collection)
        {
            return Err("active merged record file changed without retirement".into());
        }
        Ok(Some(MergedRecord {
            record,
            file,
            collection,
        }))
    }

    pub(crate) fn resolve_id(
        &self,
        collection: CollectionId,
        id: &str,
    ) -> Result<Option<MergedRecordSlot>, String> {
        let index = collection.0 as usize;
        let state = self
            .repo
            .collections
            .get(index)
            .ok_or("merged target collection missing")?;
        let mut found = None;
        if let Some(slot) = state.id_lookup.get(id).copied()
            && self.base_active[index].contains(slot)
        {
            found = Some(MergedRecordSlot {
                collection,
                segment: None,
                slot,
            });
        }
        for (segment_index, published) in self.segments[index].iter().enumerate() {
            if let Some(slot) = published.index.id_lookup.get(id).copied()
                && published.active.contains(slot)
            {
                if found.is_some() {
                    return Err("duplicate active merged record ID".into());
                }
                found = Some(MergedRecordSlot {
                    collection,
                    segment: Some(segment_index),
                    slot,
                });
            }
        }
        Ok(found)
    }

    pub(crate) fn forward_edges(
        &self,
        address: MergedRecordSlot,
        relation_index: usize,
    ) -> Result<(Vec<(&str, &[u32])>, bool, bool, bool, bool), String> {
        let index = address.collection.0 as usize;
        if let Some(segment_index) = address.segment {
            let published = &self.segments[index][segment_index];
            let contribution = published
                .index
                .relations
                .get(relation_index)
                .ok_or("merged segment relation missing")?;
            Ok((
                contribution.references_for_source(address.slot).collect(),
                contribution.present().contains(address.slot.0),
                contribution.nulls().contains(address.slot.0),
                contribution.empty_arrays().contains(address.slot.0),
                contribution.unavailable().contains(address.slot.0),
            ))
        } else {
            let relation = self.repo.collections[index]
                .relations
                .get(relation_index)
                .ok_or("merged base relation missing")?;
            Ok((
                relation
                    .edges(address.slot)
                    .ok_or("merged base relation slot missing")?
                    .iter()
                    .map(|edge| (edge.target_id(), edge.positions.as_slice()))
                    .collect(),
                relation.present().contains(address.slot.0),
                relation.nulls().contains(address.slot.0),
                relation.empty_arrays().contains(address.slot.0),
                relation.unavailable().contains(address.slot.0),
            ))
        }
    }

    pub(crate) fn backlink_sources(
        &self,
        relation: RelationId,
        target_id: &str,
    ) -> Result<Vec<MergedRecordSlot>, String> {
        let index = relation.source.0 as usize;
        let collection = self
            .repo
            .collections
            .get(index)
            .ok_or("backlink source missing")?;
        let base = collection
            .relations
            .get(relation.ordinal as usize)
            .ok_or("backlink relation missing")?;
        let mut sources = Vec::new();
        if let Some(slots) = self.repo.relation_reverse_sources(base, target_id) {
            for slot in slots.iter() {
                let slot = RecordSlot(slot);
                if self.base_active[index].contains(slot) {
                    sources.push(MergedRecordSlot {
                        collection: relation.source,
                        segment: None,
                        slot,
                    });
                }
            }
        }
        for (segment_index, published) in self.segments[index].iter().enumerate() {
            let relation_contribution = published
                .index
                .relations
                .get(relation.ordinal as usize)
                .ok_or("backlink segment relation missing")?;
            if let Some(posting) = published
                .index
                .postings
                .fields
                .get(&relation_contribution.declaration().path)
                .and_then(|field| field.values.get(&ScalarKey::Id(target_id.to_owned())))
            {
                for slot in posting.iter() {
                    let slot = RecordSlot(slot);
                    if published.active.contains(slot) {
                        sources.push(MergedRecordSlot {
                            collection: relation.source,
                            segment: Some(segment_index),
                            slot,
                        });
                    }
                }
            }
        }
        sources.sort_unstable_by(|a, b| {
            let id = |address: &MergedRecordSlot| {
                if let Some(segment_index) = address.segment {
                    &self.segments[index][segment_index].index.records[address.slot.0 as usize]
                        .id
                        .0
                } else {
                    &collection.generation.records[address.slot.0 as usize].id.0
                }
            };
            id(a).cmp(id(b))
        });
        sources.dedup();
        Ok(sources)
    }
}


/// Direction of a declared, collection-scoped relation hop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HopDirection {
    Forward,
    Backlink,
}


/// One resolved hop and its explicit array selectors in source order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundRelationHop {
    pub relation: RelationId,
    pub direction: HopDirection,
    pub selectors: Vec<Option<u32>>,
    pub from: CollectionId,
    pub to: CollectionId,
}


/// A finite path that begins at the matching source and ends at a local term.
#[derive(Clone, Debug)]
pub struct BoundRelationPath {
    pub hops: Vec<BoundRelationHop>,
    pub terminal: Box<BoundTerm>,
    pub source_operand: Option<Box<BoundTerm>>,
    pub target_is_lhs: bool,
    pub evidence_path: String,
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EvaluationMode {
    Indexed,
    Reference,
}


/// Physical work caused by relation traversal, separate from the source scan.
#[derive(Clone, Default)]
pub(crate) struct RelationWork {
    pub source_reads: u64,
    pub target_reads: u64,
    pub target_cache_hits: u64,
    pub edge_visits: u64,
    pub backlink_probe_visits: u64,
    pub bytes_read: u64,
    execution: Option<crate::streaming::ExecutionContext>,
    active: Option<Arc<Vec<ActiveRecords>>>,
}


fn charge(counter: &mut u64, amount: u64, limit: u64, name: &str) -> Result<(), String> {
    *counter = counter
        .checked_add(amount)
        .ok_or_else(|| format!("relation {name} overflow"))?;
    if *counter > limit {
        return Err(format!("relation query exceeds {limit} {name}"));
    }
    Ok(())
}


impl RelationWork {
    pub(crate) fn attach(&mut self, execution: Option<&crate::streaming::ExecutionContext>) {
        self.execution = execution.cloned();
        self.active = execution.and_then(|context| context.active_records_arc());
    }

    pub(crate) fn attach_active(&mut self, active: Option<Arc<Vec<ActiveRecords>>>) {
        self.active = active;
    }

    fn is_active(&self, collection: CollectionId, slot: RecordSlot) -> bool {
        self.active
            .as_ref()
            .is_none_or(|domains| domains[collection.0 as usize].contains(slot))
    }

    fn before_read(&self, bytes: u64, target: bool) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.check_string()?;
            let limit = if target {
                execution.limits.target_record_bytes
            } else {
                execution.limits.source_record_bytes
            };
            if bytes > limit {
                return Err(execution.fail(
                    crate::streaming::ExecutionErrorKind::Resource,
                    "relation record exceeds source read allowance",
                ));
            }
        }
        Ok(())
    }

    fn edge(&mut self) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_edges(1)?;
        }
        charge(
            &mut self.edge_visits,
            1,
            MAX_QUERY_EDGE_VISITS,
            "edge visits",
        )
    }

    fn backlink_probe(&mut self) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_edges(1)?;
        }
        charge(
            &mut self.backlink_probe_visits,
            1,
            MAX_QUERY_BACKLINK_PROBES,
            "backlink probes",
        )
    }

    fn target_read(&mut self, bytes: u64) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_reads(1)?;
            execution.charge_bytes(bytes)?;
        }
        charge(
            &mut self.target_reads,
            1,
            MAX_QUERY_RECORD_READS,
            "target reads",
        )?;
        charge(
            &mut self.bytes_read,
            bytes,
            MAX_QUERY_RELATION_BYTES,
            "source bytes",
        )
    }

    fn source_read(&mut self, bytes: u64) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_reads(1)?;
            execution.charge_bytes(bytes)?;
        }
        charge(
            &mut self.source_reads,
            1,
            MAX_QUERY_RECORD_READS,
            "source reads",
        )?;
        charge(
            &mut self.bytes_read,
            bytes,
            MAX_QUERY_RELATION_BYTES,
            "source bytes",
        )
    }
}


/// Combine bounded scanner chunks into one query-wide relation work budget.
pub(crate) fn check_query_work_budget(
    counters: &crate::workload::WorkCounters,
) -> Result<(), String> {
    for (actual, limit, name) in [
        (
            counters.relation_edge_visits,
            MAX_QUERY_EDGE_VISITS,
            "edge visits",
        ),
        (
            counters.relation_backlink_probe_visits,
            MAX_QUERY_BACKLINK_PROBES,
            "backlink probes",
        ),
        (
            counters.relation_target_reads,
            MAX_QUERY_RECORD_READS,
            "target reads",
        ),
        (
            counters.relation_source_reads,
            MAX_QUERY_RECORD_READS,
            "source reads",
        ),
        (
            counters.relation_bytes_read,
            MAX_QUERY_RELATION_BYTES,
            "source bytes",
        ),
    ] {
        if actual > limit {
            return Err(format!("relation query exceeds {limit} {name}"));
        }
    }
    Ok(())
}


/// Immutable traversal choices shared by every hop of one predicate.
#[derive(Clone, Copy)]
struct TraverseConfig<'a> {
    repo: &'a Repository,
    hops: &'a [BoundRelationHop],
    quantifier: Quantifier,
    collect_text: bool,
    mode: EvaluationMode,
    target_values: Option<&'a TargetValueCache>,
}


/// Bind a Boolean expression with collection-specific relation declarations.
pub(crate) fn bind_filter_with_relations(
    repo: &Repository,
    collection_index: usize,
    expr: &FilterExpr,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<BoundExpr, String> {
    match expr {
        FilterExpr::Term(term) => {
            if let Some(related) = bind_related_term(repo, collection_index, term, parameter_types)?
            {
                Ok(BoundExpr::Term(related))
            } else {
                bind_filter_with_parameters(
                    expr,
                    &repo.collections[collection_index].definition.schema,
                    parameter_types,
                )
            }
        }
        FilterExpr::And(parts) => Ok(BoundExpr::And(
            parts
                .iter()
                .map(|part| {
                    bind_filter_with_relations(repo, collection_index, part, parameter_types)
                })
                .collect::<Result<_, _>>()?,
        )),
        FilterExpr::Or(parts) => Ok(BoundExpr::Or(
            parts
                .iter()
                .map(|part| {
                    bind_filter_with_relations(repo, collection_index, part, parameter_types)
                })
                .collect::<Result<_, _>>()?,
        )),
        FilterExpr::Not(inner) => Ok(BoundExpr::Not(Box::new(bind_filter_with_relations(
            repo,
            collection_index,
            inner,
            parameter_types,
        )?))),
    }
}

/// Bind selected relation fields against the target collection's schema.
/// Unscoped FTS remains local to the source collection.
pub(crate) fn bind_fts_with_relations(
    repo: &Repository,
    collection_index: usize,
    request: &FtsQuery,
) -> Result<BoundFts, String> {
    let mut local = request.clone();
    local.fields.clear();
    let mut related = Vec::new();
    let mut related_base = None;
    let mut local_positions = Vec::new();
    for selected in &request.fields {
        let syntax = selected
            .query_path
            .clone()
            .unwrap_or_else(|| selected.path.display());
        let path = QueryPath::parse(&syntax)?;
        if path.field != selected.path {
            return Err("selected FTS query path disagrees with its field".into());
        }
        if let Some((hops, terminal)) = resolve_path(repo, collection_index, &path)? {
            let target = hops.last().ok_or("relation FTS has no hop")?.to.0 as usize;
            if terminal.has_explicit_array_step
                && repo.collections[target].source.reader().text_derivation()
                    != TextDerivation::LogicalStringsAfterConversion
            {
                return Err(
                    "selected FTS array positions require logical-string text derivation".into(),
                );
            }
            let target_request = FtsQuery {
                fields: vec![FtsField {
                    path: terminal.field.clone(),
                    binding: selected.binding.clone(),
                    query_path: None,
                }],
                ..request.clone()
            };
            let mut bound = bind_fts(&target_request, &repo.collections[target].definition.schema)?;
            let mut field = bound
                .fields
                .pop()
                .ok_or("relation FTS field was not bound")?;
            if related_base.is_none() {
                related_base = Some(bound);
            }
            if terminal.has_explicit_array_step {
                field.query_path = Some(terminal);
            }
            field.related = Some(hops);
            field.evidence_path = syntax;
            related.push(field);
        } else {
            let mut direct = selected.clone();
            direct.query_path = None;
            local.fields.push(direct);
            if path.has_explicit_array_step {
                if repo.collections[collection_index]
                    .source
                    .reader()
                    .text_derivation()
                    != TextDerivation::LogicalStringsAfterConversion
                {
                    return Err(
                        "selected FTS array positions require logical-string text derivation"
                            .into(),
                    );
                }
                local_positions.push((local.fields.len() - 1, path, syntax));
            }
        }
    }
    if local.fields.is_empty() && !related.is_empty() {
        let mut bound = related_base.ok_or("relation FTS base is missing")?;
        bound.fields = related;
        return Ok(bound);
    }
    let mut bound = bind_fts(
        &local,
        &repo.collections[collection_index].definition.schema,
    )?;
    for (index, path, syntax) in local_positions {
        let field = bound
            .fields
            .get_mut(index)
            .ok_or("selected FTS field is missing")?;
        field.query_path = Some(path);
        field.evidence_path = syntax;
    }
    if !related.is_empty() {
        if local.fields.is_empty() {
            bound.fields.clear();
        }
        bound.fields.extend(related);
    }
    Ok(bound)
}

/// Keep unavailable relation truth through Boolean negation. The final query
/// boundary accepts only a definite true result.
pub(crate) fn eval_filter_truth(
    repo: &Repository,
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            let result = if term.related.is_some() {
                eval_related_term_mode(repo, term, ctx, mode, work, None)?
            } else {
                Some(eval_term(term, ctx)?)
            };
            if result == Some(true)
                && term.derived == crate::reference::Derived::None
                && !matches!(
                    term.operator,
                    crate::query::Operator::Ne | crate::query::Operator::NotContains
                )
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
                if let Some(related) = &term.related {
                    evidence.insert(related.evidence_path.clone());
                }
            }
            Ok(result)
        }
        BoundExpr::And(parts) => {
            let mut answer = Some(true);
            let mut fields = std::collections::HashSet::new();
            for part in parts {
                if answer == Some(false) {
                    break;
                }
                let next = eval_filter_truth(repo, part, ctx, &mut fields, mode, work)?;
                answer = match (answer, next) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (None, _) | (_, None) => None,
                    _ => Some(true),
                };
            }
            if answer == Some(true) {
                evidence.extend(fields);
            }
            Ok(answer)
        }
        BoundExpr::Or(parts) => {
            let mut answer = Some(false);
            for part in parts {
                let mut fields = std::collections::HashSet::new();
                let next = eval_filter_truth(repo, part, ctx, &mut fields, mode, work)?;
                answer = match (answer, next) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (None, _) | (_, None) => None,
                    _ => Some(false),
                };
                if next == Some(true) {
                    evidence.extend(fields);
                }
            }
            Ok(answer)
        }
        BoundExpr::Not(inner) => {
            Ok(
                eval_filter_truth(repo, inner, ctx, &mut Default::default(), mode, work)?
                    .map(|value| !value),
            )
        }
    }
}

/// Three-valued membership stops at decisive branches without evidence sets.
pub(crate) fn eval_filter_truth_membership(
    repo: &Repository,
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            if term.related.is_some() {
                eval_related_term_mode(repo, term, ctx, mode, work, None)
            } else {
                eval_term(term, ctx).map(Some)
            }
        }
        BoundExpr::And(parts) => {
            let mut unknown = false;
            for part in parts {
                match eval_filter_truth_membership(repo, part, ctx, mode, work)? {
                    Some(false) => return Ok(Some(false)),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            Ok(if unknown { None } else { Some(true) })
        }
        BoundExpr::Or(parts) => {
            let mut unknown = false;
            for part in parts {
                match eval_filter_truth_membership(repo, part, ctx, mode, work)? {
                    Some(true) => return Ok(Some(true)),
                    None => unknown = true,
                    Some(false) => {}
                }
            }
            Ok(if unknown { None } else { Some(false) })
        }
        BoundExpr::Not(inner) => {
            Ok(eval_filter_truth_membership(repo, inner, ctx, mode, work)?.map(|value| !value))
        }
    }
}

/// Evaluate selected local and relation FTS fields with source identity intact.
pub(crate) fn eval_fts_with_relations(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    eval_fts_with_relations_inner(repo, fts, segments, ctx, Some(evidence), mode, work)
}

/// Membership-only route avoids constructing evidence and stops at one match.
pub(crate) fn eval_fts_with_relations_membership(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    eval_fts_with_relations_inner(repo, fts, segments, ctx, None, mode, work)
}

fn eval_fts_with_relations_inner(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    mut evidence: Option<&mut std::collections::HashSet<String>>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    if fts.fields.iter().all(|field| field.related.is_none()) {
        return if let Some(evidence) = evidence {
            eval_bound_fts(fts, segments, ctx, evidence)
        } else {
            eval_bound_fts_membership(fts, segments, ctx)
        };
    }
    let mut matched = false;
    for field in &fts.fields {
        if let Some(hops) = &field.related {
            let target_collection =
                &repo.collections[hops.last().ok_or("empty FTS relation path")?.to.0 as usize];
            let borrowed_text = relation_target_uses_value_text(target_collection);
            let source_index = repo
                .collections
                .iter()
                .position(|collection| collection.definition.name == ctx.collection_name)
                .ok_or("relation FTS source collection is unknown")?;
            let mut truth = Some(false);
            let config = TraverseConfig {
                repo,
                hops,
                quantifier: Quantifier::Any,
                collect_text: !borrowed_text,
                mode,
                target_values: None,
            };
            visit_hop(
                &config,
                0,
                source_index,
                ctx.record.slot,
                &mut truth,
                work,
                &[],
                &mut |target, target_segments, _| {
                    if borrowed_text {
                        #[cfg(test)]
                        BORROWED_RELATION_TARGETS
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug_assert!(target_segments.is_empty());
                        eval_borrowed_value_fts_field(field, fts.case_sensitivity, target, None)
                    } else {
                        eval_bound_fts_field(
                            field,
                            fts.case_sensitivity,
                            target_segments,
                            target,
                            None,
                        )
                    }
                },
            )?;
            if truth == Some(true) {
                matched = true;
                if let Some(evidence) = evidence.as_deref_mut() {
                    evidence.insert(field.evidence_path.clone());
                } else {
                    return Ok(true);
                }
            }
        } else if eval_bound_fts_field(
            field,
            fts.case_sensitivity,
            segments,
            ctx,
            evidence.as_deref_mut(),
        )? {
            matched = true;
            if evidence.is_none() {
                return Ok(true);
            }
        }
    }
    Ok(matched)
}

fn bind_related_term(
    repo: &Repository,
    collection_index: usize,
    term: &Term,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<Option<BoundTerm>, String> {
    let left = resolve_path(repo, collection_index, &term.path)?;
    let right = if let Operand::Field(field) = &term.operand {
        resolve_path(repo, collection_index, &field.path)?
    } else {
        None
    };
    if let (Some((left_hops, left_terminal)), Some((right_hops, right_terminal))) = (&left, &right)
    {
        if left_hops != right_hops {
            return Err("independent relation field operands need a shared occurrence path".into());
        }
        let target_index = left_hops.last().ok_or("relation path has no hop")?.to.0 as usize;
        let mut local = term.clone();
        local.path = left_terminal.clone();
        if let Operand::Field(field) = &mut local.operand {
            field.path = right_terminal.clone();
        }
        local.quantifier = Quantifier::Default;
        let BoundExpr::Term(mut terminal) = bind_filter_with_parameters(
            &FilterExpr::Term(local),
            &repo.collections[target_index].definition.schema,
            parameter_types,
        )?
        else {
            return Err("related field comparison did not bind".into());
        };
        if term.quantifier == Quantifier::Any && terminal.multi {
            terminal.quantifier = Quantifier::Any;
        }
        let mut outer = terminal.clone();
        outer.rhs = None;
        outer.path = term.path.field.clone();
        outer.query_path = term.path.clone();
        outer.quantifier = term.quantifier;
        outer.multi = true;
        outer.related = Some(Box::new(BoundRelationPath {
            hops: left_hops.clone(),
            terminal: Box::new(terminal),
            source_operand: None,
            target_is_lhs: true,
            evidence_path: if let Operand::Field(field) = &term.operand {
                field.path.display.clone()
            } else {
                term.path.display.clone()
            },
        }));
        return Ok(Some(outer));
    }
    let target_is_lhs = left.is_some();
    let Some((hops, terminal_path)) = left.or(right) else {
        return Ok(None);
    };
    let target_index = hops.last().ok_or("relation path has no hop")?.to.0 as usize;
    let mut terminal_term = term.clone();
    terminal_term.path = terminal_path;
    terminal_term.quantifier = Quantifier::Default;
    let target_schema = &repo.collections[target_index].definition.schema;
    let (mut terminal, source_operand) = if let Operand::Field(field) = &term.operand {
        if !target_is_lhs {
            terminal_term.modifiers = field.modifiers.clone();
        }
        terminal_term.operand = Operand::Literal(Literal::Null);
        let target = bind_field_probe(&terminal_term, target_schema, parameter_types)?;
        let mut source_term = term.clone();
        if target_is_lhs {
            source_term.path = field.path.clone();
            source_term.modifiers = field.modifiers.clone();
        }
        source_term.operand = Operand::Literal(Literal::Null);
        source_term.quantifier = Quantifier::Default;
        let source = bind_field_probe(
            &source_term,
            &repo.collections[collection_index].definition.schema,
            parameter_types,
        )?;
        if target.kind != LogicalType::Unknown
            && source.kind != LogicalType::Unknown
            && target.kind != source.kind
        {
            return Err(format!(
                "incompatible field operand types: {:?} and {:?}",
                target.kind, source.kind
            ));
        }
        if source.multi {
            let source_scopes = explicit_scopes(&source.query_path);
            let first_hop = hops.first().ok_or("relation path has no hop")?;
            if first_hop.direction != HopDirection::Forward {
                return Err("source-dependent multivalued field needs a forward shared [*] occurrence scope".into());
            }
            let relation =
                &repo.collections[collection_index].relations[first_hop.relation.ordinal as usize];
            let relation_scopes = relation_source_scopes(
                if target_is_lhs {
                    &term.path
                } else {
                    &field.path
                },
                &relation.declaration.path,
            );
            if source_scopes.is_empty()
                || !relation_scopes.starts_with(&source_scopes)
                || source.implicit_array
                || matches!(source.source_kind, LogicalType::Array(_))
            {
                return Err(
                    "source-dependent multivalued field needs a shared [*] relation occurrence scope or explicit indexes"
                        .into(),
                );
            }
        }
        for operation in [target.operation.as_ref(), source.operation.as_ref()]
            .into_iter()
            .flatten()
        {
            if !operation.supports_dynamic_comparison() {
                return Err("custom binding does not support dynamic field comparison".into());
            }
        }
        (target, Some(Box::new(source)))
    } else {
        let BoundExpr::Term(target) = bind_filter_with_parameters(
            &FilterExpr::Term(terminal_term),
            target_schema,
            parameter_types,
        )?
        else {
            return Err("relation terminal did not bind to a term".into());
        };
        (target, None)
    };
    if term.quantifier == Quantifier::Any && terminal.multi {
        terminal.quantifier = Quantifier::Any;
    }
    let mut outer = terminal.clone();
    outer.rhs = None;
    outer.path = term.path.field.clone();
    outer.query_path = term.path.clone();
    outer.quantifier = term.quantifier;
    outer.multi |= hops.iter().any(|hop| {
        hop.direction == HopDirection::Backlink
            || repo.collections[hop.relation.source.0 as usize].relations
                [hop.relation.ordinal as usize]
                .declaration
                .cardinality
                == RelationCardinality::Many
    });
    outer.related = Some(Box::new(BoundRelationPath {
        hops,
        terminal: Box::new(terminal),
        source_operand,
        target_is_lhs,
        evidence_path: if target_is_lhs {
            term.path.display.clone()
        } else if let Operand::Field(field) = &term.operand {
            field.path.display.clone()
        } else {
            term.path.display.clone()
        },
    }));
    Ok(Some(outer))
}

fn explicit_scopes(path: &QueryPath) -> Vec<Vec<PathStep>> {
    let mut prefix = Vec::new();
    let mut scopes = Vec::new();
    for step in &path.steps {
        prefix.push(step.clone());
        if matches!(step, PathStep::EachElement) {
            scopes.push(prefix.clone());
        }
    }
    scopes
}

// Only source-side array scopes before the first relation field can pair with
// source values. Later selectors belong to target traversal, not source data.
fn relation_source_scopes(path: &QueryPath, relation_fields: &[String]) -> Vec<Vec<PathStep>> {
    let mut prefix = Vec::new();
    let mut scopes = Vec::new();
    let mut field_count = 0;
    for step in &path.steps {
        prefix.push(step.clone());
        if matches!(step, PathStep::EachElement) {
            scopes.push(prefix.clone());
        }
        if matches!(step, PathStep::Field(_)) {
            field_count += 1;
            if field_count == relation_fields.len() {
                break;
            }
        }
    }
    scopes
}

/// Resolve the first explicit relation at each collection boundary.
pub(crate) fn resolve_path(
    repo: &Repository,
    collection_index: usize,
    path: &QueryPath,
) -> Result<Option<(Vec<BoundRelationHop>, QueryPath)>, String> {
    if path.steps.len() > MAX_PATH_STEPS {
        return Err("relation path exceeds the 64-step limit".into());
    }
    let mut current = collection_index;
    let mut remaining = path.steps.as_slice();
    let mut hops = Vec::new();
    if path.field.root == PathRoot::Backlinks {
        let Some(PathStep::Field(alias)) = remaining.first() else {
            return Err("backlink path requires an alias".into());
        };
        let relation = repo
            .backlink_relation(&repo.collections[current].definition.name, alias)
            .ok_or_else(|| format!("unknown backlink alias {alias}"))?;
        let mut consumed = 1;
        let mut selectors = Vec::new();
        while let Some(step) = remaining.get(consumed) {
            match step {
                PathStep::ArrayIndex(index) => selectors.push(Some(*index)),
                PathStep::EachElement => selectors.push(None),
                PathStep::Field(_) => break,
            }
            consumed += 1;
        }
        if !remaining[consumed..]
            .iter()
            .any(|step| matches!(step, PathStep::Field(_)))
        {
            return Err("backlink path requires a terminal field".into());
        }
        let to = relation.id.source;
        hops.push(BoundRelationHop {
            relation: relation.id,
            direction: HopDirection::Backlink,
            selectors,
            from: repo.collections[current].generation.collection,
            to,
        });
        current = to.0 as usize;
        remaining = &remaining[consumed..];
    } else if path.field.root != PathRoot::Data {
        return Ok(None);
    }
    loop {
        let schema = &repo.collections[current].definition.schema;
        let mut prefix = Vec::new();
        let mut relation_at = None;
        for (index, step) in remaining.iter().enumerate() {
            let PathStep::Field(name) = step else {
                continue;
            };
            prefix.push(name.clone());
            if let Some(declaration) = schema.relations().get(&prefix) {
                let mut consumed = index + 1;
                let mut selectors = remaining[..consumed]
                    .iter()
                    .filter_map(selector)
                    .collect::<Vec<_>>();
                while let Some(step) = remaining.get(consumed) {
                    if matches!(step, PathStep::Field(_)) {
                        break;
                    }
                    selectors.extend(selector(step));
                    consumed += 1;
                }
                if remaining[consumed..]
                    .iter()
                    .any(|step| matches!(step, PathStep::Field(_)))
                {
                    let relation = repo.collections[current]
                        .relations
                        .iter()
                        .find(|item| item.declaration.path == declaration.path)
                        .ok_or("declared relation index is missing")?;
                    relation_at = Some((relation, consumed, selectors));
                    break;
                }
            }
        }
        let Some((relation, consumed, selectors)) = relation_at else {
            break;
        };
        if hops.len() >= MAX_RELATION_HOPS {
            return Err("relation path exceeds the 8-hop limit".into());
        }
        let target = relation.target_collection;
        hops.push(BoundRelationHop {
            relation: relation.id,
            direction: HopDirection::Forward,
            selectors,
            from: repo.collections[current].generation.collection,
            to: target,
        });
        current = target.0 as usize;
        remaining = &remaining[consumed..];
    }
    if hops.is_empty() {
        return Ok(None);
    }
    if remaining.is_empty() || !matches!(remaining.first(), Some(PathStep::Field(_))) {
        return Err("relation path requires a terminal field".into());
    }
    let segments = remaining
        .iter()
        .filter_map(|step| match step {
            PathStep::Field(name) => Some(name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let field = FieldPath {
        root: PathRoot::Data,
        segments,
    };
    let terminal = QueryPath {
        display: field.display(),
        field,
        steps: remaining.to_vec(),
        has_explicit_array_step: remaining
            .iter()
            .any(|step| !matches!(step, PathStep::Field(_))),
        has_position: remaining
            .iter()
            .any(|step| matches!(step, PathStep::ArrayIndex(_))),
    };
    Ok(Some((hops, terminal)))
}

fn selector(step: &PathStep) -> Option<Option<u32>> {
    match step {
        PathStep::Field(_) => None,
        PathStep::ArrayIndex(index) => Some(Some(*index)),
        PathStep::EachElement => Some(None),
    }
}

/// Exact three-valued result for a relation term. Unknown means a dangling or
/// logically unavailable target, so Boolean negation must keep it unknown.
pub(crate) fn eval_related_term(
    repo: &Repository,
    term: &BoundTerm,
    source: &RecordContext<'_>,
    work: &mut RelationWork,
    target_values: Option<&TargetValueCache>,
) -> Result<Option<bool>, String> {
    eval_related_term_mode(
        repo,
        term,
        source,
        EvaluationMode::Indexed,
        work,
        target_values,
    )
}

fn eval_related_term_mode(
    repo: &Repository,
    term: &BoundTerm,
    source: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
    target_values: Option<&TargetValueCache>,
) -> Result<Option<bool>, String> {
    let related = term.related.as_ref().ok_or("relation term is not bound")?;
    let start = repo
        .collections
        .iter()
        .position(|collection| collection.definition.name == source.collection_name)
        .ok_or("relation source collection is unknown")?;
    let mut truth = if term.quantifier == Quantifier::Any {
        Some(false)
    } else {
        Some(true)
    };
    let config = TraverseConfig {
        repo,
        hops: &related.hops,
        quantifier: term.quantifier,
        collect_text: false,
        mode,
        target_values: target_values.filter(|_| {
            related.source_operand.is_some() && related.terminal.path.root == PathRoot::Data
        }),
    };
    visit_hop(
        &config,
        0,
        start,
        source.record.slot,
        &mut truth,
        work,
        &[],
        &mut |target, _, source_positions| {
            if let Some(source_operand) = &related.source_operand {
                eval_cross_record_field_term(
                    &related.terminal,
                    source_operand,
                    target,
                    source,
                    related.target_is_lhs,
                    source_positions,
                    &related.hops[0].selectors,
                )
            } else {
                eval_term(&related.terminal, target)
            }
        },
    )?;
    Ok(truth)
}

fn merge_truth(current: &mut Option<bool>, next: Option<bool>, quantifier: Quantifier) {
    *current = if quantifier == Quantifier::Any {
        match (*current, next) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (None, _) | (_, None) => None,
            _ => Some(false),
        }
    } else {
        match (*current, next) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (None, _) | (_, None) => None,
            _ => Some(true),
        }
    };
}

fn decisive(truth: Option<bool>, quantifier: Quantifier) -> bool {
    truth == Some(quantifier == Quantifier::Any)
}

fn visit_merged_hop(
    view: &MergedRelationView<'_>,
    hops: &[BoundRelationHop],
    depth: usize,
    address: MergedRecordSlot,
    quantifier: Quantifier,
    collect_text: bool,
    truth: &mut Option<bool>,
    work: &mut RelationWork,
    source_positions: &[u32],
    terminal: &mut impl FnMut(&RecordContext<'_>, &[TextSegment], &[u32]) -> Result<bool, String>,
) -> Result<(), String> {
    if decisive(*truth, quantifier) {
        return Ok(());
    }
    let Some(current) = view.record(address)? else {
        merge_truth(truth, None, quantifier);
        return Ok(());
    };
    if depth == hops.len() {
        if current.record.class == RecordClass::RawFallback {
            merge_truth(truth, None, quantifier);
            return Ok(());
        }
        if current.record.range.len() > MAX_QUERY_RELATION_BYTES {
            return Err(format!(
                "relation target record exceeds {MAX_QUERY_RELATION_BYTES} source bytes"
            ));
        }
        work.before_read(current.record.range.len(), true)?;
        let path = view.repo.files.root.join(&current.file.relative_path);
        if let Some(execution) = work.execution.as_ref() {
            execution.check_string()?;
        }
        let bytes = match read_record_range_for_search(
            &path,
            current.file.revision,
            current.record.range,
            work.execution.as_ref(),
        ) {
            Ok(bytes) => bytes,
            Err(message) => {
                view.record_source_failure(current.file, current.record.range, message);
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
        };
        work.target_read(bytes.len() as u64)?;
        let extracted = crate::record_preparation::prepare_query_record_with_execution(
            current.record.class,
            &current.collection.source,
            &current.collection.definition.schema,
            &bytes,
            &ScanProjection::full(collect_text),
            false,
            work.execution.as_ref(),
        )?;
        let ctx = RecordContext {
            value: &extracted.value,
            source_bytes: Some(&bytes),
            record: current.record,
            file: current.file,
            collection_name: &current.collection.definition.name,
            repository_root: &view.repo.files.root,
        };
        merge_truth(
            truth,
            Some(terminal(&ctx, &extracted.text, source_positions)?),
            quantifier,
        );
        return Ok(());
    }
    let hop = &hops[depth];
    if hop.from != address.collection {
        return Err("merged relation hop collection mismatch".into());
    }
    let relation_index = hop.relation.ordinal as usize;
    match hop.direction {
        HopDirection::Forward => {
            let (references, present, null, _, unavailable) =
                view.forward_edges(address, relation_index)?;
            if unavailable {
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
            let mut selected = 0usize;
            for (target_id, positions) in references {
                if !positions_match(&hop.selectors, positions) {
                    continue;
                }
                work.edge()?;
                selected += 1;
                if let Some(target) = view.resolve_id(hop.to, target_id)? {
                    visit_merged_hop(
                        view,
                        hops,
                        depth + 1,
                        target,
                        quantifier,
                        collect_text,
                        truth,
                        work,
                        if depth == 0 {
                            positions
                        } else {
                            source_positions
                        },
                        terminal,
                    )?;
                } else {
                    merge_truth(truth, None, quantifier);
                }
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0 && (!present || null || hop.selectors.iter().any(Option::is_some)) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
        HopDirection::Backlink => {
            let target_id = current.record.id.0.as_str();
            let sources = view.backlink_sources(hop.relation, target_id)?;
            let mut selected = 0usize;
            for (position, source) in sources.into_iter().enumerate() {
                work.backlink_probe()?;
                work.edge()?;
                if hop.selectors.first().is_some_and(|selector| {
                    selector.is_some_and(|index| index as usize != position)
                }) {
                    continue;
                }
                selected += 1;
                visit_merged_hop(
                    view,
                    hops,
                    depth + 1,
                    source,
                    quantifier,
                    collect_text,
                    truth,
                    work,
                    source_positions,
                    terminal,
                )?;
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
    }
    Ok(())
}

fn eval_related_term_merged(
    view: &MergedRelationView<'_>,
    term: &BoundTerm,
    source_slot: MergedRecordSlot,
    source: &RecordContext<'_>,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    let related = term
        .related
        .as_ref()
        .ok_or("merged relation term is not bound")?;
    let mut truth = Some(term.quantifier != Quantifier::Any);
    visit_merged_hop(
        view,
        &related.hops,
        0,
        source_slot,
        term.quantifier,
        false,
        &mut truth,
        work,
        &[],
        &mut |target, _, positions| {
            if let Some(source_operand) = &related.source_operand {
                eval_cross_record_field_term(
                    &related.terminal,
                    source_operand,
                    target,
                    source,
                    related.target_is_lhs,
                    positions,
                    &related.hops[0].selectors,
                )
            } else {
                eval_term(&related.terminal, target)
            }
        },
    )?;
    Ok(truth)
}

pub(crate) fn eval_filter_truth_merged(
    view: &MergedRelationView<'_>,
    expr: &BoundExpr,
    source_slot: MergedRecordSlot,
    source: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            let result = if term.related.is_some() {
                eval_related_term_merged(view, term, source_slot, source, work)?
            } else {
                Some(eval_term(term, source)?)
            };
            if result == Some(true)
                && term.derived == crate::reference::Derived::None
                && !matches!(
                    term.operator,
                    crate::query::Operator::Ne | crate::query::Operator::NotContains
                )
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
                if let Some(related) = &term.related {
                    evidence.insert(related.evidence_path.clone());
                }
            }
            Ok(result)
        }
        BoundExpr::And(parts) => {
            let mut truth = Some(true);
            let mut fields = std::collections::HashSet::new();
            for part in parts {
                if truth == Some(false) {
                    break;
                }
                let next =
                    eval_filter_truth_merged(view, part, source_slot, source, &mut fields, work)?;
                truth = match (truth, next) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (None, _) | (_, None) => None,
                    _ => Some(true),
                };
            }
            if truth == Some(true) {
                evidence.extend(fields);
            }
            Ok(truth)
        }
        BoundExpr::Or(parts) => {
            let mut truth = Some(false);
            for part in parts {
                let mut fields = std::collections::HashSet::new();
                let next =
                    eval_filter_truth_merged(view, part, source_slot, source, &mut fields, work)?;
                truth = match (truth, next) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (None, _) | (_, None) => None,
                    _ => Some(false),
                };
                if next == Some(true) {
                    evidence.extend(fields);
                }
            }
            Ok(truth)
        }
        BoundExpr::Not(inner) => eval_filter_truth_merged(
            view,
            inner,
            source_slot,
            source,
            &mut Default::default(),
            work,
        )
        .map(|truth| truth.map(|value| !value)),
    }
}

pub(crate) fn eval_fts_with_relations_merged(
    view: &MergedRelationView<'_>,
    fts: &BoundFts,
    segments: &[TextSegment],
    source_slot: MergedRecordSlot,
    source: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    work: &mut RelationWork,
) -> Result<bool, String> {
    if fts.fields.iter().all(|field| field.related.is_none()) {
        return eval_bound_fts(fts, segments, source, evidence);
    }
    let mut matched = false;
    for field in &fts.fields {
        if let Some(hops) = &field.related {
            let target_collection =
                &view.repo.collections[hops.last().ok_or("empty FTS relation path")?.to.0 as usize];
            let borrowed_text = relation_target_uses_value_text(target_collection);
            let mut truth = Some(false);
            visit_merged_hop(
                view,
                hops,
                0,
                source_slot,
                Quantifier::Any,
                !borrowed_text,
                &mut truth,
                work,
                &[],
                &mut |target, target_segments, _| {
                    if borrowed_text {
                        #[cfg(test)]
                        BORROWED_RELATION_TARGETS
                            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        debug_assert!(target_segments.is_empty());
                        eval_borrowed_value_fts_field(field, fts.case_sensitivity, target, None)
                    } else {
                        eval_bound_fts_field(
                            field,
                            fts.case_sensitivity,
                            target_segments,
                            target,
                            None,
                        )
                    }
                },
            )?;
            if truth == Some(true) {
                matched = true;
                evidence.insert(field.evidence_path.clone());
            }
        } else if eval_bound_fts_field(
            field,
            fts.case_sensitivity,
            segments,
            source,
            Some(evidence),
        )? {
            matched = true;
        }
    }
    Ok(matched)
}

fn visit_hop(
    config: &TraverseConfig<'_>,
    depth: usize,
    collection_index: usize,
    source_slot: RecordSlot,
    truth: &mut Option<bool>,
    work: &mut RelationWork,
    source_positions: &[u32],
    terminal: &mut impl FnMut(&RecordContext<'_>, &[TextSegment], &[u32]) -> Result<bool, String>,
) -> Result<(), String> {
    let TraverseConfig {
        repo,
        hops,
        quantifier,
        collect_text,
        mode,
        target_values,
    } = *config;
    if decisive(*truth, quantifier) {
        return Ok(());
    }
    if !work.is_active(
        repo.collections[collection_index].generation.collection,
        source_slot,
    ) {
        merge_truth(truth, None, quantifier);
        return Ok(());
    }
    if depth == hops.len() {
        let collection = &repo.collections[collection_index];
        let record = &collection.generation.records[source_slot.0 as usize];
        if record.class == RecordClass::RawFallback {
            merge_truth(truth, None, quantifier);
            return Ok(());
        }
        if let Some(value) =
            target_values.and_then(|cache| cache.get(collection.generation.collection, source_slot))
        {
            work.target_cache_hits += 1;
            let file = &repo.files.files[record.file.0 as usize];
            let ctx = RecordContext {
                value,
                source_bytes: None,
                record,
                file,
                collection_name: &collection.definition.name,
                repository_root: &repo.files.root,
            };
            merge_truth(
                truth,
                Some(terminal(&ctx, &[], source_positions)?),
                quantifier,
            );
            return Ok(());
        }
        let file = &repo.files.files[record.file.0 as usize];
        let path = repo
            .files
            .native_path(record.file)
            .ok_or("missing target source path")?;
        if record.range.len() > MAX_QUERY_RELATION_BYTES {
            return Err(format!(
                "relation target record exceeds {MAX_QUERY_RELATION_BYTES} source bytes"
            ));
        }
        work.before_read(record.range.len(), true)?;
        let bytes = read_record_range_for_search(
            &path,
            record.revision,
            record.range,
            work.execution.as_ref(),
        )?;
        work.target_read(bytes.len() as u64)?;
        let extracted = crate::record_preparation::prepare_query_record_with_execution(
            record.class,
            &collection.source,
            &collection.definition.schema,
            &bytes,
            &ScanProjection::full(collect_text),
            false,
            work.execution.as_ref(),
        )?;
        let ctx = RecordContext {
            value: &extracted.value,
            source_bytes: Some(&bytes),
            record,
            file,
            collection_name: &collection.definition.name,
            repository_root: &repo.files.root,
        };
        merge_truth(
            truth,
            Some(terminal(&ctx, &extracted.text, source_positions)?),
            quantifier,
        );
        return Ok(());
    }
    let hop = &hops[depth];
    if hop.from.0 as usize != collection_index {
        return Err("relation hop collection mismatch".into());
    }
    let source_collection = &repo.collections[hop.relation.source.0 as usize];
    let relation = source_collection
        .relations
        .get(hop.relation.ordinal as usize)
        .ok_or("relation hop index is missing")?;
    if mode == EvaluationMode::Reference {
        match hop.direction {
            HopDirection::Forward => {
                let Some(value) = read_reference_value(repo, collection_index, source_slot, work)?
                else {
                    merge_truth(truth, None, quantifier);
                    return Ok(());
                };
                let references = logical_references(&value, &relation.declaration.path)?;
                let present = references.present;
                let null = references.null;
                let target_collection = &repo.collections[hop.to.0 as usize];
                let targets = &target_collection.generation.records;
                let mut selected = 0usize;
                for (id, positions) in references.values {
                    if !positions_match(&hop.selectors, &positions) {
                        continue;
                    }
                    work.edge()?;
                    selected += 1;
                    let target_slot = if work.active.is_some() {
                        target_collection
                            .id_lookup
                            .get(id.as_str())
                            .copied()
                            .filter(|slot| work.is_active(hop.to, *slot))
                    } else {
                        targets
                            .iter()
                            .find(|record| record.id.0 == id)
                            .map(|record| record.slot)
                    };
                    if let Some(target_slot) = target_slot {
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            target_slot,
                            truth,
                            work,
                            if depth == 0 {
                                &positions
                            } else {
                                source_positions
                            },
                            terminal,
                        )?;
                    } else {
                        merge_truth(truth, None, quantifier);
                    }
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0 && (!present || null || hop.selectors.iter().any(Option::is_some))
                {
                    merge_truth(truth, Some(false), quantifier);
                }
            }
            HopDirection::Backlink => {
                let target =
                    &repo.collections[collection_index].generation.records[source_slot.0 as usize];
                let mut seen = 0usize;
                let mut selected = 0usize;
                for candidate in &source_collection.identity_order {
                    if !work.is_active(hop.to, *candidate) {
                        continue;
                    }
                    work.backlink_probe()?;
                    let Some(value) =
                        read_reference_value(repo, hop.to.0 as usize, *candidate, work)?
                    else {
                        continue;
                    };
                    let references = logical_references(&value, &relation.declaration.path)?;
                    if !references.values.iter().any(|(id, _)| id == &target.id.0) {
                        continue;
                    }
                    work.edge()?;
                    if hop
                        .selectors
                        .first()
                        .is_none_or(|selector| selector.is_none_or(|index| index as usize == seen))
                    {
                        selected += 1;
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            *candidate,
                            truth,
                            work,
                            source_positions,
                            terminal,
                        )?;
                    }
                    seen += 1;
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                    merge_truth(truth, Some(false), quantifier);
                }
            }
        }
        return Ok(());
    }
    match hop.direction {
        HopDirection::Forward => {
            if relation.unavailable().contains(source_slot.0) {
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
            let edges = relation
                .edges(source_slot)
                .ok_or("relation source slot is missing")?;
            let mut selected = 0;
            for edge in edges {
                if !positions_match(&hop.selectors, &edge.positions) {
                    continue;
                }
                work.edge()?;
                selected += 1;
                let target = if work.active.is_some() {
                    repo.collections[hop.to.0 as usize]
                        .id_lookup
                        .get(edge.target_id())
                        .copied()
                        .filter(|slot| work.is_active(hop.to, *slot))
                } else if let RelationTarget::Resolved(target) = edge.target {
                    Some(target)
                } else {
                    None
                };
                match target {
                    Some(target) => visit_hop(
                        config,
                        depth + 1,
                        hop.to.0 as usize,
                        target,
                        truth,
                        work,
                        if depth == 0 {
                            &edge.positions
                        } else {
                            source_positions
                        },
                        terminal,
                    )?,
                    None => merge_truth(truth, None, quantifier),
                }
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0
                && (!relation.present().contains(source_slot.0)
                    || relation.nulls().contains(source_slot.0)
                    || hop.selectors.iter().any(Option::is_some))
            {
                merge_truth(truth, Some(false), quantifier);
            }
        }
        HopDirection::Backlink => {
            let target =
                &repo.collections[collection_index].generation.records[source_slot.0 as usize];
            let sources = repo.relation_reverse_sources(relation, &target.id.0);
            let mut seen = 0usize;
            let mut selected = 0usize;
            if let Some(sources) = sources {
                if hop.selectors.iter().all(Option::is_none) {
                    // Without a numeric selector, Boolean truth is independent
                    // of identity-order rank. Visit only actual reverse hits.
                    for source in sources.iter() {
                        if !work.is_active(hop.to, RecordSlot(source)) {
                            continue;
                        }
                        work.backlink_probe()?;
                        work.edge()?;
                        selected += 1;
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            RecordSlot(source),
                            truth,
                            work,
                            source_positions,
                            terminal,
                        )?;
                        if decisive(*truth, quantifier) {
                            break;
                        }
                    }
                } else {
                    // A positional backlink means rank among sources in
                    // identity order, so retain that ordering here.
                    for source in &source_collection.identity_order {
                        if !work.is_active(hop.to, *source) {
                            continue;
                        }
                        work.backlink_probe()?;
                        if !sources.contains(source.0) {
                            continue;
                        }
                        work.edge()?;
                        if hop.selectors.first().is_none_or(|selector| {
                            selector.is_none_or(|index| index as usize == seen)
                        }) {
                            selected += 1;
                            visit_hop(
                                config,
                                depth + 1,
                                hop.to.0 as usize,
                                *source,
                                truth,
                                work,
                                source_positions,
                                terminal,
                            )?;
                        }
                        seen += 1;
                        if decisive(*truth, quantifier) {
                            break;
                        }
                    }
                }
            }
            if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
    }
    Ok(())
}

fn read_reference_value(
    repo: &Repository,
    collection_index: usize,
    slot: RecordSlot,
    work: &mut RelationWork,
) -> Result<Option<crate::model::Value>, String> {
    let collection = &repo.collections[collection_index];
    if !work.is_active(collection.generation.collection, slot) {
        return Ok(None);
    }
    let record = collection
        .generation
        .records
        .get(slot.0 as usize)
        .ok_or("reference relation slot is missing")?;
    if record.class == RecordClass::RawFallback {
        return Ok(None);
    }
    let path = repo
        .files
        .native_path(record.file)
        .ok_or("reference relation path is missing")?;
    if record.range.len() > MAX_QUERY_RELATION_BYTES {
        return Err(format!(
            "relation source record exceeds {MAX_QUERY_RELATION_BYTES} source bytes"
        ));
    }
    work.before_read(record.range.len(), false)?;
    let bytes = read_record_range_for_search(
        &path,
        record.revision,
        record.range,
        work.execution.as_ref(),
    )?;
    work.source_read(bytes.len() as u64)?;
    let extracted = crate::record_preparation::prepare_query_record_with_execution(
        record.class,
        &collection.source,
        &collection.definition.schema,
        &bytes,
        &ScanProjection::full(false),
        false,
        work.execution.as_ref(),
    )?;
    Ok(Some(extracted.value))
}


/// Independent logical ID occurrences and presence/null state.
struct LogicalReferences {
    present: bool,
    null: bool,
    values: Vec<(String, Vec<u32>)>,
}


/// An intentionally separate, iterative oracle for declared ID occurrences.
/// It walks logical values directly and never consults build-time edge state.
fn logical_references(value: &Value, path: &[String]) -> Result<LogicalReferences, String> {
    let mut found = false;
    let mut null = false;
    let mut references = Vec::new();
    let mut pending = vec![(value, 0usize, Vec::<u32>::new())];
    while let Some((value, offset, positions)) = pending.pop() {
        match value {
            Value::Array(array) => {
                if array.values.is_empty() {
                    found = true;
                }
                for (index, item) in array.values.iter().enumerate().rev() {
                    let mut child_positions = positions.clone();
                    child_positions
                        .push(u32::try_from(index).map_err(|_| "relation array index overflow")?);
                    pending.push((item, offset, child_positions));
                }
            }
            Value::Object(fields) if offset < path.len() => {
                if let Some(next) = fields.get(&path[offset]) {
                    pending.push((next, offset + 1, positions));
                }
            }
            Value::Id(id) if offset == path.len() => {
                found = true;
                references.push((id.clone(), positions));
            }
            Value::Null if offset == path.len() => {
                found = true;
                null = true;
            }
            _ if offset == path.len() => return Err("relation field has non-ID value".into()),
            _ => {}
        }
    }
    Ok(LogicalReferences {
        present: found,
        null,
        values: references,
    })
}

fn positions_match(selectors: &[Option<u32>], positions: &[u32]) -> bool {
    selectors.len() <= positions.len()
        && selectors
            .iter()
            .zip(positions)
            .all(|(selector, position)| selector.is_none_or(|index| index == *position))
}

#[cfg(test)]
#[path = "relation_query.test.rs"]
mod tests;
