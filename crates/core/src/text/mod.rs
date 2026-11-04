//! Full-text requests and traversal of every searchable JSON leaf value.

mod malformed;
mod matcher;
mod render;
mod values;

use crate::Record;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub(crate) use malformed::malformed_search_text;
pub(crate) use matcher::TextPattern;
pub(crate) use render::{needs_normalizing, render_text, render_text_into};
#[cfg(test)]
pub(crate) use values::visit_sonic_text;
pub(crate) use values::{TextRules, visit_json_text, visit_sonic_index_text};


/// Which representation and matching syntax a search uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    #[default]
    /// Literal substring in normalized text: markup removed, punctuation
    /// ignored, whitespace collapsed. Case follows `case_sensitive`.
    Word,
    /// Literal substring in decoded source values, exactly as stored.
    WordRaw,
    /// Regex over decoded source values exactly as stored, including markup,
    /// punctuation, and whitespace. The pattern sets its own case flags,
    /// such as `(?i)`; `case_sensitive` does not apply.
    #[serde(alias = "regex_raw")]
    Regex,
}


/// Full-text search, optionally scoped to collections and JSON leaf paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextQuery {
    /// Passed unchanged to the selected word or regex matcher.
    pub pattern: String,
    /// Optional JSON field predicate applied before full-text matching; may end in `SORT BY`.
    pub filter: Option<String>,
    /// Typed parameters used by `filter`.
    pub parameters: BTreeMap<String, Value>,
    pub mode: TextMode,
    pub case_sensitive: bool,
    pub collections: Option<Vec<String>>,
    /// Leaf paths such as `annotation` or `contributors[].note`.
    pub fields: Option<Vec<String>>,
    pub limit: usize,
    pub offset: usize,
    pub count: bool,
    /// Sort terms applied to final matches within each collection. Empty means ID order.
    pub sort: Vec<crate::Sort>,
}


impl Default for TextQuery {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            filter: None,
            parameters: BTreeMap::new(),
            mode: TextMode::Word,
            case_sensitive: false,
            collections: None,
            fields: None,
            limit: 50,
            offset: 0,
            count: false,
            sort: Vec::new(),
        }
    }
}


/// A matching record, query field paths, and concrete matched leaf paths.
#[derive(Debug, Clone, Serialize)]
pub struct TextHit {
    pub record: Record,
    /// Canonical paths such as `contents[].incipit_statement`.
    pub fields: Vec<String>,
    /// Actual matched leaves such as `contents[9].incipit_statement`.
    pub matched_paths: Vec<String>,
}


/// Page of full-text hits with optional total and query metrics.
#[derive(Debug, Clone, Serialize)]
pub struct TextQueryResult {
    pub hits: Vec<TextHit>,
    pub total: Option<usize>,
    pub truncated: bool,
    /// Wall-clock duration of this search in milliseconds.
    pub elapsed_ms: f64,
    pub metrics: crate::Metrics,
    pub query_plan: Vec<String>,
}


/// Final count and metrics after streamed full-text hits are verified.
#[derive(Debug, Clone, Serialize)]
pub struct TextQueryStreamSummary {
    pub returned: usize,
    pub total: Option<usize>,
    pub truncated: bool,
    /// Wall-clock duration through the final streamed hit in milliseconds.
    pub elapsed_ms: f64,
    pub metrics: crate::Metrics,
    pub query_plan: Vec<String>,
}
