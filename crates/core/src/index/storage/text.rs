//! Text-index configuration and representation identity.

use std::collections::HashMap;

use crate::schema::Schema;

use serde::{Deserialize, Serialize};

pub const BIGRAM_VERSION: u32 = 3;
pub const FOLDED_REPRESENTATION: &str = "unicode-casefold-0.2.0/unicode-9.0.0/full/non-turkic";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BigramAlphabet {
    Ascii,
    Unicode,
}

/// Derived once when a collection is published or restored. It contains only
/// small binding names; postings and membership remain in their owners.
#[derive(Clone, Debug, Default)]
pub(crate) struct TextBindingCatalog {
    pub(crate) canonical: HashMap<Vec<String>, String>,
}

impl TextBindingCatalog {
    pub fn new(schema: &Schema) -> Self {
        let mut canonical = HashMap::new();
        for (path, name) in schema.custom.keys() {
            canonical
                .entry(path.clone())
                .and_modify(|current: &mut String| {
                    if name < current {
                        current.clone_from(name);
                    }
                })
                .or_insert_with(|| name.clone());
        }
        Self { canonical }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct TextAccessPolicy<'a> {
    pub schema: &'a Schema,
    pub bindings: &'a TextBindingCatalog,
    pub logical_text_complete: bool,
}
#[derive(Clone, Copy)]
pub(crate) struct SegmentTextAccess<'a> {
    pub policy: TextAccessPolicy<'a>,
    pub domain: crate::query_access::membership::DomainId,
    pub instance: u64,
    pub collection: crate::model::CollectionId,
}
