//! Full-text requests and traversal of every searchable JSON leaf value.

mod malformed;
mod render;
mod values;

use crate::Record;
use serde::{Deserialize, Serialize};

pub(crate) use malformed::malformed_search_text;
pub(crate) use render::render_text;
pub(crate) use values::{TextRules, visit_json_text, visit_sonic_text};


/// Which representation of a value a search matches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextMode {
    #[default]
    Word,
    RegexClean,
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
        }
    }
}


/// A matching record and the JSON leaf paths that matched it.
#[derive(Debug, Clone, Serialize)]
pub struct TextHit {
    pub record: Record,
    pub fields: Vec<String>,
}


/// Page of full-text hits with optional total and query metrics.
#[derive(Debug, Clone, Serialize)]
pub struct TextQueryResult {
    pub hits: Vec<TextHit>,
    pub total: Option<usize>,
    pub metrics: crate::Metrics,
    pub query_plan: Vec<String>,
}


/// Final count and metrics after streamed full-text hits are verified.
#[derive(Debug, Clone, Serialize)]
pub struct TextQueryStreamSummary {
    pub returned: usize,
    pub total: Option<usize>,
    pub metrics: crate::Metrics,
    pub query_plan: Vec<String>,
}
