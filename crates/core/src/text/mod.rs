//! Full-text requests and traversal of every searchable JSON leaf value.

mod malformed;
mod render;
mod values;

use crate::Record;
use serde::{Deserialize, Serialize};

pub(crate) use malformed::malformed_search_text;
pub(crate) use render::render_text;
pub(crate) use values::{TextRules, visit_json_text, visit_sonic_text};


/// Which representation and matching syntax a search uses.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    #[default]
    /// Literal substring in rendered text, including partial words.
    Word,
    /// Literal substring in decoded source values with markup intact.
    WordRaw,
    /// Regex in rendered text.
    RegexClean,
    /// Regex in decoded source values with markup intact.
    RegexRaw,
}


/// Full-text search, optionally scoped to collections and JSON leaf paths.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TextQuery {
    pub pattern: String,
    pub mode: TextMode,
    pub case_sensitive: bool,
    pub collections: Option<Vec<String>>,
    /// Leaf paths such as `annotation` or `contributors[].note`.
    pub fields: Option<Vec<String>>,
    pub limit: usize,
    pub offset: usize,
    pub count: bool,
    /// Sort terms applied within each collection. Empty means ID order.
    pub sort: Vec<crate::Sort>,
}


impl Default for TextQuery {
    fn default() -> Self {
        Self {
            pattern: String::new(),
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
