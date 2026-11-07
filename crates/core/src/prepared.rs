//! Bounded repository-local reuse of validated, typed query bindings.

use std::collections::{HashMap, VecDeque};
use std::mem::size_of;

use crate::query::Query;
use crate::reference::{BoundExpr, BoundFts};
use crate::repository::Repository;


/// Bound query terms for the selected collection generations.
#[derive(Clone)]
pub(crate) struct PreparedQuery {
    pub collections: Vec<(usize, Option<BoundExpr>, Option<BoundFts>)>,
}


impl PreparedQuery {
    fn retained_heap_bytes(&self) -> usize {
        self.collections
            .capacity()
            .saturating_mul(size_of::<(usize, Option<BoundExpr>, Option<BoundFts>)>())
            .saturating_add(
                self.collections
                    .iter()
                    .fold(0usize, |total, (_, filter, fts)| {
                        total.saturating_add(
                            filter
                                .as_ref()
                                .map_or(0, crate::reference::bound_expr_retained_heap_bytes)
                                .saturating_add(
                                    fts.as_ref()
                                        .map_or(0, crate::reference::bound_fts_retained_heap_bytes),
                                ),
                        )
                    }),
            )
    }
}


/// One prepared-query cache value and its memory cost.
struct Entry {
    query: PreparedQuery,
    bytes: usize,
}


/// Bounded cache of resolved query preparations.
pub(crate) struct PreparedCache {
    budget: usize,
    bytes: usize,
    entries: HashMap<Vec<u8>, Entry>,
    order: VecDeque<Vec<u8>>,
}


impl PreparedCache {
    pub fn new(budget: usize) -> Self {
        Self {
            budget,
            bytes: 0,
            entries: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&mut self, key: &[u8]) -> Option<PreparedQuery> {
        let query = self.entries.get(key)?.query.clone();
        if let Some(index) = self.order.iter().position(|old| old == key) {
            self.order.remove(index);
        }
        self.order.push_back(key.to_vec());
        Some(query)
    }

    fn entry_bytes(key: &[u8], query: &PreparedQuery) -> usize {
        // The map key and LRU key each own a byte vector. Charge the stored
        // entry, its node/queue overhead, and owned buffers in the bound tree.
        // Repository-owned binding operations are shared and excluded.
        size_of::<Entry>()
            .saturating_add(size_of::<Vec<u8>>() + 4 * size_of::<usize>())
            .saturating_add(key.len().saturating_mul(2))
            .saturating_add(query.retained_heap_bytes())
    }

    fn insert(&mut self, key: Vec<u8>, query: PreparedQuery, bytes: usize) {
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
        self.entries.insert(key, Entry { query, bytes });
    }

    #[cfg(test)]
    pub fn used_bytes(&self) -> usize {
        self.bytes
    }
}


pub(crate) fn prepare(repo: &Repository, query: &Query) -> Result<(PreparedQuery, bool), String> {
    // The cache is owned by one immutable published repository generation.
    // Thus its key is scoped to exactly its collection generations, alphabet,
    // schema and binding versions. Return mode/page/sort do not alter binding.
    let key = bincode::serialize(&(&query.collections, &query.filter, &query.fts))
        .map_err(|error| format!("prepared query key: {error}"))?;
    if let Some(found) = repo.prepared.lock().map_err(|e| e.to_string())?.get(&key) {
        return Ok((found, true));
    }
    let mut collections = Vec::new();
    let mut parameter_types = HashMap::new();
    for (index, collection) in repo.collections.iter().enumerate() {
        if !query.collections.is_empty() && !query.collections.contains(&collection.definition.name)
        {
            continue;
        }
        let filter = query
            .filter
            .as_ref()
            .map(|f| {
                crate::relation_query::bind_filter_with_relations(
                    repo,
                    index,
                    f,
                    &mut parameter_types,
                )
            })
            .transpose()?;
        let fts = query
            .fts
            .as_ref()
            .map(|fts| crate::relation_query::bind_fts_with_relations(repo, index, fts))
            .transpose()?;
        collections.push((index, filter, fts));
    }
    if collections.is_empty() {
        return Err("unknown or unloaded collection scope".into());
    }
    let prepared = PreparedQuery { collections };
    let bytes = PreparedCache::entry_bytes(&key, &prepared);
    let cached = prepared.clone();
    repo.prepared
        .lock()
        .map_err(|e| e.to_string())?
        .insert(key, cached, bytes);
    Ok((prepared, false))
}

#[cfg(test)]
#[path = "prepared.test.rs"]
mod tests;
