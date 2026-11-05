//! Bounded repository-local reuse of validated, typed query bindings.

use std::collections::{HashMap, VecDeque};

use crate::query::Query;
use crate::reference::{BoundExpr, bind_filter};
use crate::repository::{Repository, validate_sort};

#[derive(Clone)]
pub(crate) struct PreparedQuery {
    pub collections: Vec<(usize, Option<BoundExpr>)>,
}

struct Entry {
    query: PreparedQuery,
    bytes: usize,
}

pub(crate) struct PreparedCache {
    budget: usize,
    bytes: usize,
    entries: HashMap<String, Entry>,
    order: VecDeque<String>,
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
    fn get(&mut self, key: &str) -> Option<PreparedQuery> {
        let query = self.entries.get(key)?.query.clone();
        if let Some(index) = self.order.iter().position(|old| old == key) {
            self.order.remove(index);
        }
        self.order.push_back(key.to_owned());
        Some(query)
    }
    fn insert(&mut self, key: String, query: PreparedQuery) {
        // Debug formatting is an upper-bound-oriented charge for the small
        // bound tree; the cache never stores source records or bitmap sets.
        let bytes = key.len() + format!("{:?}", query.collections).len() * 2 + 128;
        if bytes > self.budget {
            return;
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
    let key = format!("{:?}|{:?}|{:?}", query.collections, query.filter, query.fts);
    if let Some(found) = repo.prepared.lock().map_err(|e| e.to_string())?.get(&key) {
        validate_sort(repo, query)?;
        return Ok((found, true));
    }
    validate_sort(repo, query)?;
    let mut collections = Vec::new();
    for (index, collection) in repo.collections.iter().enumerate() {
        if !query.collections.is_empty() && !query.collections.contains(&collection.definition.name)
        {
            continue;
        }
        let filter = query
            .filter
            .as_ref()
            .map(|f| bind_filter(f, &collection.definition.schema))
            .transpose()?;
        if let Some(fts) = &query.fts {
            for field in &fts.fields {
                collection.definition.schema.field(&field.path)?;
                if let Some(name) = &field.binding {
                    if name != "lower"
                        && !collection
                            .definition
                            .schema
                            .custom
                            .contains_key(&(field.path.segments.clone(), name.clone()))
                    {
                        return Err(format!("unknown FTS binding {name}"));
                    }
                }
            }
        }
        collections.push((index, filter));
    }
    if collections.is_empty() {
        return Err("unknown or unloaded collection scope".into());
    }
    let prepared = PreparedQuery { collections };
    repo.prepared
        .lock()
        .map_err(|e| e.to_string())?
        .insert(key, prepared.clone());
    Ok((prepared, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounded_lru_evicts_without_exceeding_budget() {
        let mut cache = PreparedCache::new(280);
        let query = PreparedQuery {
            collections: Vec::new(),
        };
        cache.insert("a".into(), query.clone());
        assert!(cache.get("a").is_some());
        cache.insert("b".into(), query.clone());
        assert!(cache.used_bytes() <= 280);
        assert!(cache.get("a").is_some());
        cache.insert("c".into(), query);
        assert!(cache.get("b").is_none());
        assert!(cache.get("a").is_some());
        assert!(cache.get("c").is_some());
    }
}
