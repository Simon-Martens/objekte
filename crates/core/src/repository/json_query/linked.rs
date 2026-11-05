//! Relation paths: read linked records, bind filters, and semi-join candidates.

use super::*;
use crate::json_filter::RelationContext;
use std::{collections::HashMap, io::Read, sync::Arc};

/// Above this many linked IDs, an unindexed relation field is scanned
/// instead of unioning one bigram candidate set per ID.
const BIGRAM_SEMIJOIN_LIMIT: usize = 512;


impl Repository {
    /// Bind a filter to one collection, following its schema relations.
    pub(in crate::repository) fn bind_filter(
        &mut self,
        filter: &JsonFilter,
        c: &Collection,
    ) -> Result<JsonFilter> {
        let bound = match self.relation_context(c, filter.paths())? {
            Some(context) => filter.bind_relations(&c.name, context),
            None => filter.clone(),
        };
        if bound.has_registered_text_terms(&c.fields) {
            let registry = self
                .extensions
                .as_ref()
                .ok_or_else(|| Error::Request("text definitions are unavailable".into()))?;
            bound.for_fields_with_text(c, registry)
        } else {
            Ok(bound.for_fields(&c.fields))
        }
    }

    /// Compile a collection's sort plan, reading terms that follow relations.
    pub(in crate::repository) fn sort_plan(
        &mut self,
        c: &Collection,
        sorts: &[crate::Sort],
    ) -> Result<crate::sorting::SortPlan> {
        let mut plan = crate::sorting::SortPlan::compile(c, sorts)?;
        if let Some(context) = self.relation_context(c, &plan.paths())? {
            plan.bind_relations(&c.name, context);
        }
        Ok(plan)
    }

    /// Resolve `paths` through the schema relations of `c` and read every
    /// linked collection they reach once. `None` means no path crosses one.
    pub(in crate::repository) fn relation_context(
        &mut self,
        c: &Collection,
        paths: &[JsonPath],
    ) -> Result<Option<Arc<RelationContext>>> {
        let collections = &self.model.collections;
        // Only a schema declares relations; other paths are ordinary JSON.
        let fields_of = |name: &str| {
            collections
                .get(name)
                .filter(|collection| collection.schema.is_some())
                .map(|collection| &collection.fields)
        };
        let Some((mut context, keys)) = RelationContext::plan(&c.name, paths, &fields_of) else {
            return Ok(None);
        };
        for (target, keys) in keys {
            let started = Instant::now();
            let records =
                if let Some(mut selected) = self.selected_route(std::slice::from_ref(&target))? {
                    let (linked, snapshot) = selected.pop().expect("linked collection exists");
                    let mut records = HashMap::new();
                    let mut bytes = Vec::new();
                    for source in snapshot.sources.iter() {
                        let record = self.selected_record(&linked, source, &mut bytes, false)?;
                        if !record.state.status.is_malformed() {
                            records.insert(record.id, record.data);
                        }
                    }
                    records
                } else {
                    let linked = self.collection(&target)?;
                    let (records, reads) = read_linked(&linked, keys.as_ref())?;
                    self.metrics.source_reads += reads as u64;
                    self.metrics.parses += reads as u64;
                    records
                };
            self.metrics.parse_nanos += nanos(started);
            context.insert_table(target, records);
        }
        Ok(Some(Arc::new(context)))
    }
}


/// Read every parseable record of a linked collection, keeping `keys`.
fn read_linked(
    c: &Collection,
    keys: Option<&BTreeSet<String>>,
) -> Result<(HashMap<String, Value>, usize)> {
    let source = Source::open_scoped(&c.path, c.recursive)?;
    let ids = source.inventory()?.into_keys().collect::<Vec<_>>();
    let keys = keys.map(|keys| keys.iter().cloned().collect::<Vec<_>>());
    let records = ids
        .par_iter()
        .map_init(Vec::new, |bytes, id| -> Result<Option<(String, Value)>> {
            let path = c.path.join(format!("{id}.json"));
            let mut file = match source.file(id) {
                Ok(file) => file,
                // A file removed since the inventory is simply not linked.
                Err(Error::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            bytes.clear();
            file.read_to_end(bytes).map_err(|e| io(&path, e))?;
            let document = match &keys {
                Some(keys) => Document::parse_pruned(bytes, keys),
                None => Document::parse(bytes),
            };
            Ok((!document.state.status.is_malformed()).then(|| (id.clone(), document.data)))
        })
        .collect::<Result<Vec<_>>>()?;
    source.finish()?;
    let reads = records.len();
    Ok((records.into_iter().flatten().collect(), reads))
}


impl crate::file_picker::FilePicker {
    /// Slots whose values at `path` may include one of `ids`. Typed postings
    /// are exact for indexed fields; otherwise each ID is a bigram literal.
    /// Changed and appended slots stay candidates.
    pub(in crate::repository) fn relation_candidate_bits(
        &self,
        path: &JsonPath,
        ids: &BTreeSet<String>,
    ) -> Option<Vec<u64>> {
        if ids.is_empty() {
            return Some(vec![0; self.slot_count().div_ceil(64)]);
        }
        if let Some(bits) = self.typed_id_bits(path, ids) {
            return Some(bits);
        }
        if ids.len() > BIGRAM_SEMIJOIN_LIMIT {
            return None;
        }
        self.literal_union_bits(&ids.iter().cloned().collect::<Vec<_>>())
    }
}
