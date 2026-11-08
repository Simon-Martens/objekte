//! Bind a query against open collection definitions without reading records.

use std::collections::HashMap;

use crate::query::Query;
use crate::relation_query::{bind_filter_with_relations, bind_fts_with_relations};
use crate::repository::Repository;
use crate::sort_binding::BoundSort;


impl Repository {
    /// Check scope, predicate bindings, full-text fields, and sorting without
    /// running a search or opening source files. The same binders run during
    /// execution, so related paths and collection-specific types are checked.
    pub fn validate_query(&self, query: &Query) -> Result<(), String> {
        let resolved = query.resolve_current_clock()?;
        let query = resolved.as_ref();
        let mut parameter_types = HashMap::new();
        let mut selected = 0usize;
        for (index, collection) in self.collections().iter().enumerate() {
            if !query.collections.is_empty()
                && !query.collections.contains(&collection.definition.name)
            {
                continue;
            }
            selected += 1;
            if let Some(filter) = &query.filter {
                bind_filter_with_relations(self, index, filter, &mut parameter_types)?;
            }
            if let Some(fts) = &query.fts {
                bind_fts_with_relations(self, index, fts)?;
            }
        }
        if selected == 0 {
            return Err("unknown or unloaded collection scope".into());
        }
        BoundSort::bind(self, query)?;
        Ok(())
    }
}


#[cfg(test)]
#[path = "query_validation.test.rs"]
mod tests;
