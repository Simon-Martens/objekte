//! Schema relation paths and indexed backlink filter requests.

use objekte_core::{JsonQuery, PathStep, Repository};
use serde_json::Value;
use std::collections::BTreeMap;

fn filter_path(path: &[PathStep]) -> String {
    let mut text = String::new();
    for step in path {
        match step {
            PathStep::Field(name) => {
                text.push('[');
                text.push_str(&serde_json::to_string(name).expect("field names serialize"));
                text.push(']');
            }
            PathStep::Each => text.push_str("[*]"),
        }
    }
    text
}

/// One indexed filter per source collection that can link to this item.
pub(super) fn backlink_requests(repo: &Repository, collection: &str, id: &str) -> Vec<JsonQuery> {
    let mut by_source = BTreeMap::<String, Vec<String>>::new();
    for edge in repo
        .model()
        .graph
        .edges
        .iter()
        .filter(|edge| edge.target == collection)
    {
        by_source
            .entry(edge.source.clone())
            .or_default()
            .push(format!("{} ?= $target", filter_path(&edge.path)));
    }
    by_source
        .into_iter()
        .map(|(source, predicates)| JsonQuery {
            filter: predicates.join(" || "),
            parameters: BTreeMap::from([("target".into(), Value::String(id.into()))]),
            collections: Some(vec![source]),
            limit: i64::MAX as usize,
            count: false,
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
#[path = "relations.test.rs"]
mod tests;
