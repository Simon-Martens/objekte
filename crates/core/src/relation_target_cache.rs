//! Query-local projected targets for source-dependent relation comparisons.

use std::collections::{BTreeMap, HashMap, HashSet};

use roaring::RoaringBitmap;

use crate::inventory::RecordClass;
use crate::model::{CollectionId, FileId, PathRoot, RecordSlot, Value};
use crate::record_preparation::prepare_query_record;
use crate::reference::{BoundExpr, BoundTerm};
use crate::relation::RelationTarget;
use crate::relation_query::HopDirection;
use crate::repository::Repository;
use crate::source::ScanProjection;
use crate::source_access::SourceAccessSession;

const MAX_TARGET_SLOTS: u64 = 5_000_000;
const MAX_EDGE_VISITS: u64 = 32_000_000;
const MAX_RETAINED_VALUE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_RECORD_BYTES: u64 = 64 * 1024 * 1024;
const BATCH_BYTES: u64 = 1024 * 1024;


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
    pub work: TargetCacheWork,
    pub retained_bytes: u64,
}


impl TargetValueCache {
    pub fn get(&self, collection: CollectionId, slot: RecordSlot) -> Option<&Value> {
        self.values.get(&(collection, slot))
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
) -> Result<Option<TargetValueCache>, String> {
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
                if relation.unavailable().contains(slot) {
                    continue;
                }
                for edge in relation
                    .edges(RecordSlot(slot))
                    .ok_or("relation source slot missing")?
                {
                    if !positions_match(&hop.selectors, &edge.positions) {
                        continue;
                    }
                    work.edge_visits = work
                        .edge_visits
                        .checked_add(1)
                        .ok_or("target prefetch edge count overflow")?;
                    if work.edge_visits > MAX_EDGE_VISITS {
                        return Err(format!(
                            "target prefetch exceeds {MAX_EDGE_VISITS} edge visits"
                        ));
                    }
                    if let RelationTarget::Resolved(target) = edge.target {
                        next.insert(target.0);
                    }
                }
            }
            if next.len() > MAX_TARGET_SLOTS {
                return Err(format!("target prefetch exceeds {MAX_TARGET_SLOTS} slots"));
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
    }
    if targets.is_empty() {
        return Ok(None);
    }
    let mut values = HashMap::new();
    let mut retained = 0u64;
    let mut access = SourceAccessSession::new(&repo.files);
    for (collection_id, (slots, fields)) in targets {
        work.target_candidates = work.target_candidates.saturating_add(slots.len());
        if work.target_candidates > MAX_TARGET_SLOTS {
            return Err(format!("target prefetch exceeds {MAX_TARGET_SLOTS} slots"));
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
        let projection = ScanProjection {
            top_level: Some(fields),
            collect_text: false,
        };
        for (file_id, ranges) in grouped {
            let mut start = 0;
            while start < ranges.len() {
                let first_bytes = ranges[start].1.len();
                if first_bytes > MAX_RECORD_BYTES {
                    return Err(format!(
                        "target prefetch record exceeds {MAX_RECORD_BYTES} bytes"
                    ));
                }
                let mut end = start + 1;
                let mut batch_bytes = first_bytes;
                while end < ranges.len()
                    && end - start < 128
                    && batch_bytes.saturating_add(ranges[end].1.len()) <= BATCH_BYTES
                {
                    batch_bytes += ranges[end].1.len();
                    end += 1;
                }
                let chunk = &ranges[start..end];
                start = end;
                let read = access.read(file_id, chunk)?;
                work.target_file_opens += read.opened;
                work.bytes_read += read.bytes_read;
                for (slot, bytes) in read.records {
                    let projected = prepare_query_record(
                        &collection.source,
                        &collection.definition.schema,
                        &bytes,
                        &projection,
                        false,
                    )?
                    .value;
                    retained = retained.saturating_add(retained_value_bytes(&projected));
                    if retained > MAX_RETAINED_VALUE_BYTES {
                        return Err(format!(
                            "target prefetch exceeds {MAX_RETAINED_VALUE_BYTES} retained bytes"
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
        work,
        retained_bytes: retained,
    }))
}

#[cfg(test)]
#[path = "relation_target_cache.test.rs"]
mod tests;
