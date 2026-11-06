//! Complete verified membership and ordering, independent of source payloads.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use crate::query::{FilterExpr, Operand, Query};


/// Verified record slot and evidence retained for reuse.
#[derive(Clone, Debug)]
pub(crate) struct CachedMember {
    pub collection_index: usize,
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

    pub fn set_budget(&mut self, budget: usize) {
        self.budget = budget;
        self.bytes = 0;
        self.entries.clear();
        self.order.clear();
    }

    pub fn get(&mut self, key: &[u8]) -> Option<Arc<CachedResult>> {
        let result = Arc::clone(&self.entries.get(key)?.result);
        if let Some(index) = self.order.iter().position(|old| old == key) {
            self.order.remove(index);
        }
        self.order.push_back(key.to_vec());
        Some(result)
    }

    pub fn insert(&mut self, key: Vec<u8>, members: Vec<CachedMember>) {
        let bytes = key.len()
            + 128
            + members
                .iter()
                .map(|member| {
                    std::mem::size_of::<CachedMember>()
                        + member
                            .matched_fields
                            .iter()
                            .map(|field| field.len() + std::mem::size_of::<String>())
                            .sum::<usize>()
                })
                .sum::<usize>();
        if bytes > self.budget {
            return;
        }
        if let Some(old) = self.entries.remove(&key) {
            self.bytes -= old.bytes;
        }
        if let Some(index) = self.order.iter().position(|old| old == &key) {
            self.order.remove(index);
        }
        while self.bytes.saturating_add(bytes) > self.budget {
            if let Some(old) = self.order.pop_front() {
                if let Some(entry) = self.entries.remove(&old) {
                    self.bytes -= entry.bytes;
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
    bincode::serialize(&(
        &query.collections,
        &filter,
        &query.fts,
        &query.options.sort,
        query.options.evidence,
    ))
    .map_err(|error| format!("verified query key: {error}"))
}

#[cfg(test)]
#[path = "verified.test.rs"]
mod tests;
