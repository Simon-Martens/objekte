//! Complete verified membership and ordering, independent of source payloads.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::query::{FilterExpr, Operand, Query};


/// Verified record slot and evidence retained for reuse.
#[derive(Clone, Debug)]
pub(crate) struct CachedMember {
    pub collection_index: usize,
    /// Pinned lifecycle segment position, or the static/base generation.
    /// This is only interpreted together with the index instance and revision.
    pub segment_index: Option<usize>,
    pub slot: u32,
    pub matched_fields: Vec<String>,
}


/// Complete membership set held in the result cache.
pub(crate) struct CachedResult {
    pub members: Vec<CachedMember>,
}


/// Result cache value together with its byte cost.
struct Entry {
    result: Arc<CachedResult>,
    bytes: usize,
}


/// Memory-bounded cache of complete verified results.
pub(crate) struct VerifiedCache {
    budget: usize,
    bytes: usize,
    entries: HashMap<Vec<u8>, Entry>,
    order: VecDeque<Vec<u8>>,
}


impl VerifiedCache {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    pub fn enabled(&self) -> bool {
        self.budget != 0
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    /// The caller must drop displaced results after releasing its cache lock.
    pub fn set_budget(&mut self, budget: usize) -> Vec<Arc<CachedResult>> {
        self.budget = budget;
        self.bytes = 0;
        let displaced = self.entries.drain().map(|(_, entry)| entry.result).collect();
        self.order.clear();
        displaced
    }

    pub fn get(&mut self, key: &[u8]) -> Option<Arc<CachedResult>> {
        let result = Arc::clone(&self.entries.get(key)?.result);
        if let Some(index) = self.order.iter().position(|old| old == key) {
            self.order.remove(index);
        }
        self.order.push_back(key.to_vec());
        Some(result)
    }

    /// The caller must drop displaced results after releasing its cache lock.
    pub fn insert(&mut self, key: Vec<u8>, members: Vec<CachedMember>) -> Vec<Arc<CachedResult>> {
        let bytes = key.len()
            .saturating_add(128)
            .saturating_add(members
                .iter()
                .map(|member| {
                    std::mem::size_of::<CachedMember>()
                        .saturating_add(member
                            .matched_fields
                            .iter()
                            .map(|field| field.len().saturating_add(std::mem::size_of::<String>()))
                            .fold(0usize, usize::saturating_add))
                })
                .fold(0usize, usize::saturating_add));
        let mut displaced = Vec::new();
        if bytes > self.budget {
            displaced.push(Arc::new(CachedResult { members }));
            return displaced;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
            displaced.push(old.result);
        }
        if let Some(index) = self.order.iter().position(|old| old == &key) {
            self.order.remove(index);
        }
        while self.bytes.saturating_add(bytes) > self.budget {
            if let Some(old) = self.order.pop_front() {
                if let Some(entry) = self.entries.remove(&old) {
                    self.bytes -= entry.bytes;
                    displaced.push(entry.result);
                }
            } else {
                break;
            }
        }
        self.bytes += bytes;
        self.order.push_back(key.clone());
        self.entries.insert(
            key,
            Entry {
                result: Arc::new(CachedResult { members }),
                bytes,
            },
        );
        displaced
    }

    #[cfg(test)]
    pub fn used_bytes(&self) -> usize {
        self.bytes
    }
}


pub(crate) fn key(query: &Query) -> Result<Vec<u8>, String> {
    // Repository ownership supplies the generation, alphabet, schema and
    // binding-version scope; options below determine membership/order/evidence.
    // Source byte spans only locate diagnostics. Equivalent numeric operands,
    // including differently spelled byte sizes, must share result identity.
    fn erase_spans(expr: &mut FilterExpr) {
        match expr {
            FilterExpr::Term(term) => {
                term.span = (0, 0);
                match &mut term.operand {
                    Operand::Field(field) => field.span = (0, 0),
                    Operand::Macro { span, .. } => *span = (0, 0),
                    _ => {}
                }
            }
            FilterExpr::And(parts) | FilterExpr::Or(parts) => {
                for part in parts {
                    erase_spans(part);
                }
            }
            FilterExpr::Not(inner) => erase_spans(inner),
        }
    }
    let mut filter = query.filter.clone();
    if let Some(filter) = &mut filter {
        erase_spans(filter);
    }
    let mut key = vec![0];
    key.extend(bincode::serialize(&(
        &query.collections,
        &filter,
        &query.fts,
        &query.options.sort,
        query.options.evidence,
    ))
    .map_err(|error| format!("verified query key: {error}"))?);
    Ok(key)
}

/// Lifecycle entries are scoped to one index instance and immutable
/// publication. The prefix keeps them disjoint from static repository keys.
pub(crate) fn lifecycle_key(
    query: &Query,
    instance_id: u64,
    revision: u64,
) -> Result<Vec<u8>, String> {
    let mut key = key(query)?;
    key[0] = 1;
    key.splice(1..1, instance_id.to_le_bytes().into_iter().chain(revision.to_le_bytes()));
    Ok(key)
}

#[cfg(test)]
#[path = "verified.test.rs"]
mod tests;
