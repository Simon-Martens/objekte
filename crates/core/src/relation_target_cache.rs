//! Query-local projected targets for source-dependent relation comparisons.

use std::collections::{BTreeMap, HashMap, HashSet};

use roaring::RoaringBitmap;

use crate::inventory::RecordClass;
use crate::model::{CollectionId, FileId, PathRoot, RecordSlot, Value};
use crate::reference::{BoundExpr, BoundTerm};
use crate::relation::RelationTarget;
use crate::relation_query::HopDirection;
use crate::repository::Repository;
use crate::source::ScanProjection;
use crate::source_access::SourceAccessSession;
use crate::streaming::{ExecutionContext, ExecutionErrorKind};

const MAX_TARGET_SLOTS: u64 = 5_000_000;
const MAX_EDGE_VISITS: u64 = 32_000_000;
const MAX_RETAINED_VALUE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 64 * 1024 * 1024;
const BATCH_BYTES: u64 = 1024 * 1024;
// Conservative stream charge for a target's bitmap membership, grouped range,
// and map entry while prefetching. It is an estimate, not process RSS.
const STREAM_TARGET_SLOT_BYTES: u64 = 128;


/// Physical preparation work, charged to the query before source scanning.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct TargetCacheWork {
    pub target_candidates: u64,
    pub target_reads: u64,
    pub target_file_opens: u64,
    pub bytes_read: u64,
    pub edge_visits: u64,
}


/// Immutable projected logical values shared by parallel source scanners.
pub(crate) struct TargetValueCache {
    values: HashMap<(CollectionId, RecordSlot), Value>,
    projected_fields: HashMap<CollectionId, HashSet<String>>,
    pub work: TargetCacheWork,
    pub retained_bytes: u64,
}


impl TargetValueCache {
    pub(crate) fn estimated_bytes(&self) -> usize {
        self.values.iter().fold(0usize, |total, (_, value)| {
            total.saturating_add(
                std::mem::size_of::<(CollectionId, RecordSlot, Value)>()
                    .saturating_add(retained_value_bytes(value) as usize)
                    .saturating_add(32),
            )
        })
    }

    pub fn get(&self, collection: CollectionId, slot: RecordSlot) -> Option<&Value> {
        self.values.get(&(collection, slot))
    }

    pub(crate) fn get_projected_field(
        &self,
        collection: CollectionId,
        slot: RecordSlot,
        field: &str,
    ) -> Option<&Value> {
        self.projected_fields
            .get(&collection)
            .filter(|fields| fields.contains(field))
            .and_then(|_| self.get(collection, slot))
    }
}


fn source_dependent_terms<'a>(expr: &'a BoundExpr, output: &mut Vec<&'a BoundTerm>) {
    match expr {
        BoundExpr::Term(term) => {
            if term
                .related
                .as_ref()
                .is_some_and(|path| path.source_operand.is_some())
            {
                output.push(term);
            }
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            for part in parts {
                source_dependent_terms(part, output);
            }
        }
        BoundExpr::Not(inner) => source_dependent_terms(inner, output),
    }
}

pub(crate) fn has_source_dependent(expr: &BoundExpr) -> bool {
    let mut terms = Vec::new();
    source_dependent_terms(expr, &mut terms);
    !terms.is_empty()
}

fn positions_match(selectors: &[Option<u32>], positions: &[u32]) -> bool {
    selectors.len() <= positions.len()
        && selectors
            .iter()
            .zip(positions)
            .all(|(selector, position)| selector.is_none_or(|index| index == *position))
}

fn retained_value_bytes(value: &Value) -> u64 {
    let mut total = 0u64;
    let mut pending = vec![value];
    while let Some(next) = pending.pop() {
        total = total.saturating_add(std::mem::size_of::<Value>() as u64);
        match next {
            Value::String(text) | Value::Id(text) => {
                total = total.saturating_add(text.capacity() as u64);
            }
            Value::Object(fields) => {
                for (name, field) in fields {
                    total = total.saturating_add(name.capacity() as u64 + 64);
                    pending.push(field);
                }
            }
            Value::Array(array) => {
                total = total.saturating_add(
                    (array.values.capacity() * std::mem::size_of::<Value>()) as u64,
                );
                pending.extend(&array.values);
            }
            _ => {}
        }
    }
    total
}

/// Prefetch a conservative superset of targets for forward source-dependent
/// terms. The ordinary traversal still performs positional and three-valued
/// evaluation; the cache changes only how a reached target's value is loaded.
pub(crate) fn prepare(
    repo: &Repository,
    source_collection: usize,
    expr: &BoundExpr,
    candidates: &RoaringBitmap,
    execution: Option<&ExecutionContext>,
) -> Result<Option<TargetValueCache>, String> {
    prepare_with_limits(
        repo,
        source_collection,
        expr,
        candidates,
        None,
        None,
        execution,
    )
}

pub(crate) fn prepare_stream(
    repo: &Repository,
    source_collection: usize,
    expr: &BoundExpr,
    candidates: &RoaringBitmap,
    cancellation: &crate::streaming::CancellationToken,
    max_retained_bytes: usize,
    execution: Option<&ExecutionContext>,
) -> Result<Option<TargetValueCache>, String> {
    prepare_with_limits(
        repo,
        source_collection,
        expr,
        candidates,
        Some(cancellation),
        Some(max_retained_bytes as u64),
        execution,
    )
}

fn mark_resource(message: String, execution: Option<&ExecutionContext>) -> String {
    if let Some(execution) = execution {
        execution.fail(ExecutionErrorKind::Resource, message)
    } else {
        message
    }
}

fn limit_error(message: String, streaming: bool, execution: Option<&ExecutionContext>) -> String {
    mark_resource(
        if streaming {
            format!("stream resource: {message}")
        } else {
            message
        },
        execution,
    )
}

fn stream_target_slot_charge(
    slots: u64,
    limit: Option<u64>,
    execution: Option<&ExecutionContext>,
) -> Result<u64, String> {
    let bytes = slots.checked_mul(STREAM_TARGET_SLOT_BYTES).ok_or_else(|| {
        mark_resource(
            "stream resource: target slot accounting overflow".into(),
            execution,
        )
    })?;
    if limit.is_some_and(|limit| bytes > limit) {
        return Err(mark_resource(
            "stream resource: related target bookkeeping exceeds buffered-byte limit".into(),
            execution,
        ));
    }
    Ok(bytes)
}

fn prepare_with_limits(
    repo: &Repository,
    source_collection: usize,
    expr: &BoundExpr,
    candidates: &RoaringBitmap,
    cancellation: Option<&crate::streaming::CancellationToken>,
    max_retained_bytes: Option<u64>,
    execution: Option<&ExecutionContext>,
) -> Result<Option<TargetValueCache>, String> {
    if let Some(execution) = execution {
        execution.check_slots(candidates.len())?;
    }
    let mut terms = Vec::new();
    source_dependent_terms(expr, &mut terms);
    let mut targets = BTreeMap::<CollectionId, (RoaringBitmap, HashSet<String>)>::new();
    let mut work = TargetCacheWork::default();
    let source = &repo.collections[source_collection];
    for term in terms {
        let related = term
            .related
            .as_ref()
            .ok_or("source-dependent term lost relation")?;
        if related.terminal.path.root != PathRoot::Data
            || related
                .hops
                .iter()
                .any(|hop| hop.direction != HopDirection::Forward)
        {
            continue;
        }
        let mut current = candidates & &source.logical_records;
        for hop in &related.hops {
            let collection = repo
                .collections
                .get(hop.relation.source.0 as usize)
                .ok_or("relation source collection missing")?;
            let relation = collection
                .relations
                .get(hop.relation.ordinal as usize)
                .ok_or("relation declaration missing")?;
            let mut next = RoaringBitmap::new();
            for slot in current.iter() {
                if let Some(execution) = execution {
                    execution.check_string()?;
                }
                if cancellation.is_some_and(crate::streaming::CancellationToken::is_cancelled) {
                    return Err("stream cancelled".into());
                }
                if relation.unavailable().contains(slot) {
                    continue;
                }
                for edge in relation
                    .edges(RecordSlot(slot))
                    .ok_or("relation source slot missing")?
                {
                    if let Some(execution) = execution {
                        execution.charge_edges(1)?;
                    }
                    if !positions_match(&hop.selectors, &edge.positions) {
                        continue;
                    }
                    work.edge_visits = work.edge_visits.checked_add(1).ok_or_else(|| {
                        limit_error(
                            "target prefetch edge count overflow".into(),
                            max_retained_bytes.is_some(),
                            execution,
                        )
                    })?;
                    if work.edge_visits > MAX_EDGE_VISITS {
                        return Err(limit_error(
                            format!("target prefetch exceeds {MAX_EDGE_VISITS} edge visits"),
                            max_retained_bytes.is_some(),
                            execution,
                        ));
                    }
                    if let RelationTarget::Resolved(target) = edge.target
                        && next.insert(target.0)
                        && max_retained_bytes.is_some()
                    {
                        stream_target_slot_charge(next.len(), max_retained_bytes, execution)?;
                    }
                }
            }
            if next.len() > MAX_TARGET_SLOTS {
                return Err(limit_error(
                    format!("target prefetch exceeds {MAX_TARGET_SLOTS} slots"),
                    max_retained_bytes.is_some(),
                    execution,
                ));
            }
            if let Some(execution) = execution {
                execution.check_slots(next.len())?;
            }
            current = next;
        }
        let terminal_collection = related.hops.last().ok_or("relation path has no hop")?.to;
        let entry = targets.entry(terminal_collection).or_default();
        entry.0 |= current;
        let first = related
            .terminal
            .path
            .segments
            .first()
            .ok_or("source-dependent target field is empty")?;
        entry.1.insert(first.clone());
        if max_retained_bytes.is_some() {
            let slots = targets.values().try_fold(0u64, |total, (slots, _)| {
                total.checked_add(slots.len()).ok_or_else(|| {
                    mark_resource(
                        "stream resource: target slot count overflow".into(),
                        execution,
                    )
                })
            })?;
            stream_target_slot_charge(slots, max_retained_bytes, execution)?;
        }
    }
    if targets.is_empty() {
        return Ok(None);
    }
    let mut values = HashMap::new();
    let mut projected_fields = HashMap::new();
    let mut retained = if max_retained_bytes.is_some() {
        let slots = targets.values().try_fold(0u64, |total, (slots, _)| {
            total.checked_add(slots.len()).ok_or_else(|| {
                mark_resource(
                    "stream resource: target slot count overflow".into(),
                    execution,
                )
            })
        })?;
        stream_target_slot_charge(slots, max_retained_bytes, execution)?
    } else {
        0
    };
    let mut access = SourceAccessSession::new(&repo.files);
    if let Some(execution) = execution {
        access.configure_execution(execution);
    }
    for (collection_id, (slots, fields)) in targets {
        if cancellation.is_some_and(crate::streaming::CancellationToken::is_cancelled) {
            return Err("stream cancelled".into());
        }
        work.target_candidates = work.target_candidates.saturating_add(slots.len());
        if let Some(execution) = execution {
            execution.check_slots(work.target_candidates)?;
        }
        if work.target_candidates > MAX_TARGET_SLOTS {
            return Err(limit_error(
                format!("target prefetch exceeds {MAX_TARGET_SLOTS} slots"),
                max_retained_bytes.is_some(),
                execution,
            ));
        }
        let collection = &repo.collections[collection_id.0 as usize];
        let mut grouped = BTreeMap::<FileId, Vec<(u32, crate::inventory::ByteRange)>>::new();
        for slot in slots.iter() {
            let record = collection
                .generation
                .records
                .get(slot as usize)
                .ok_or("target prefetch slot missing")?;
            if record.class == RecordClass::RawFallback {
                continue;
            }
            grouped
                .entry(record.file)
                .or_default()
                .push((slot, record.range));
        }
        projected_fields.insert(collection_id, fields.clone());
        let projection = ScanProjection {
            top_level: Some(fields),
            collect_text: false,
            resolved_text_only: false,
        };
        for (file_id, ranges) in grouped {
            if cancellation.is_some_and(crate::streaming::CancellationToken::is_cancelled) {
                return Err("stream cancelled".into());
            }
            let mut start = 0;
            while start < ranges.len() {
                if cancellation.is_some_and(crate::streaming::CancellationToken::is_cancelled) {
                    return Err("stream cancelled".into());
                }
                let remaining = max_retained_bytes.map(|limit| limit.saturating_sub(retained));
                let first_bytes = ranges[start].1.len();
                if let Some(execution) = execution {
                    execution.check_string()?;
                    if first_bytes > execution.limits.target_record_bytes {
                        return Err(execution.fail(
                            ExecutionErrorKind::Resource,
                            "target prefetch record exceeds source read allowance",
                        ));
                    }
                }
                if first_bytes > MAX_RECORD_BYTES {
                    return Err(limit_error(
                        format!("target prefetch record exceeds {MAX_RECORD_BYTES} bytes"),
                        max_retained_bytes.is_some(),
                        execution,
                    ));
                }
                if remaining.is_some_and(|limit| first_bytes > limit) {
                    return Err(mark_resource(
                        "stream resource: target record exceeds read allowance".into(),
                        execution,
                    ));
                }
                let mut end = start + 1;
                let mut batch_bytes = first_bytes;
                while end < ranges.len()
                    && end - start < 128
                    && batch_bytes.saturating_add(ranges[end].1.len())
                        <= BATCH_BYTES.min(remaining.unwrap_or(BATCH_BYTES))
                {
                    batch_bytes += ranges[end].1.len();
                    end += 1;
                }
                let chunk = &ranges[start..end];
                start = end;
                if let Some(execution) = execution {
                    execution.charge_reads(chunk.len() as u64)?;
                    execution.charge_bytes(batch_bytes)?;
                }
                let read = if let Some(limit) = remaining {
                    access.read_bounded(
                        file_id,
                        chunk,
                        usize::try_from(limit).unwrap_or(usize::MAX),
                    )?
                } else {
                    access.read(file_id, chunk)?
                };
                work.target_file_opens += read.opened;
                work.bytes_read += read.bytes_read;
                for (slot, bytes) in read.records {
                    if cancellation.is_some_and(crate::streaming::CancellationToken::is_cancelled) {
                        return Err("stream cancelled".into());
                    }
                    let projected = crate::record_preparation::prepare_query_record_with_execution(
                        &collection.source,
                        &collection.definition.schema,
                        &bytes,
                        &projection,
                        false,
                        execution,
                    )?
                    .value;
                    if let Some(execution) = execution {
                        execution.check_string()?;
                    }
                    retained = retained.saturating_add(retained_value_bytes(&projected));
                    if max_retained_bytes.is_some_and(|limit| retained > limit) {
                        return Err(mark_resource(
                            "stream resource: related target cache exceeds buffered-byte limit"
                                .into(),
                            execution,
                        ));
                    }
                    if retained > MAX_RETAINED_VALUE_BYTES {
                        return Err(limit_error(
                            format!(
                                "target prefetch exceeds {MAX_RETAINED_VALUE_BYTES} retained bytes"
                            ),
                            max_retained_bytes.is_some(),
                            execution,
                        ));
                    }
                    work.target_reads += 1;
                    values.insert((collection_id, RecordSlot(slot)), projected);
                }
            }
        }
    }
    Ok(Some(TargetValueCache {
        values,
        projected_fields,
        work,
        retained_bytes: retained,
    }))
}

#[cfg(test)]
#[path = "relation_target_cache.test.rs"]
mod tests;
