//! Full-text requests and traversal of every searchable JSON leaf value.

mod malformed;
mod matcher;
mod render;
mod values;

use crate::Record;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ops::Deref;

pub(crate) use malformed::malformed_search_text;
pub(crate) use matcher::TextPattern;
#[cfg(test)]
pub(crate) use render::take_normalization_calls;
pub(crate) use render::{needs_normalizing, render_text, render_text_into};
#[cfg(test)]
pub(crate) use values::visit_sonic_text;
pub(crate) use values::{TextRules, visit_json_text, visit_sonic_index_text};

/// Accept the historical `[]` spelling as well as JSON-filter `[*]` array
/// traversal, without rewriting quoted object keys that contain brackets.
pub(crate) fn parse_text_path(path: &str) -> crate::Result<crate::JsonPath> {
    let mut expanded = String::with_capacity(path.len());
    let mut chars = path.chars().peekable();
    let mut quote = None;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if let Some(open) = quote {
            expanded.push(ch);
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == open {
                quote = None;
            }
            continue;
        }
        if matches!(ch, '\'' | '"') {
            quote = Some(ch);
            expanded.push(ch);
        } else if ch == '[' && chars.peek() == Some(&']') {
            chars.next();
            expanded.push_str("[*]");
        } else {
            expanded.push(ch);
        }
    }
    crate::JsonFilter::parse_path(&expanded)
}


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


/// One logical field and an optional explicit text definition. A bare field
/// serializes as a string; an explicit definition serializes as an object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextFieldSelector {
    path: String,
    search: Option<String>,
}


impl TextFieldSelector {
    pub fn new(path: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            search: None,
        }
    }

    pub fn with_search(path: impl Into<String>, search: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            search: Some(search.into()),
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn search(&self) -> Option<&str> {
        self.search.as_deref()
    }

    /// Parse a CLI-style field with an optional alias or canonical selector.
    /// A colon inside a quoted JSON key remains part of the path.
    pub fn parse(input: &str) -> crate::Result<Self> {
        let mut quote = None;
        let mut escaped = false;
        let mut bracket = 0_usize;
        let mut selector = None;
        for (index, ch) in input.char_indices() {
            if let Some(open) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == open {
                    quote = None;
                }
                continue;
            }
            match ch {
                '\'' | '"' if bracket > 0 => quote = Some(ch),
                '[' => bracket += 1,
                ']' => bracket = bracket.saturating_sub(1),
                ':' if bracket == 0 => {
                    selector = Some(index);
                    break;
                }
                _ => {}
            }
        }
        let (path, search) = if let Some(index) = selector {
            let (path, suffix) = input.split_at(index);
            let suffix = &suffix[1..];
            let identity = if suffix.starts_with("text(") && suffix.ends_with(')') {
                serde_json::from_str::<String>(&suffix[5..suffix.len() - 1]).map_err(|_| {
                    crate::Error::Request("text selector needs a quoted canonical identity".into())
                })?
            } else {
                suffix.to_owned()
            };
            if identity.is_empty() {
                return Err(crate::Error::Request("text selector is empty".into()));
            }
            (path, (identity != "fts").then_some(identity))
        } else {
            (input, None)
        };
        parse_text_path(path)?;
        Ok(Self {
            path: path.into(),
            search,
        })
    }
}


impl From<String> for TextFieldSelector {
    fn from(path: String) -> Self {
        Self::new(path)
    }
}


impl From<&str> for TextFieldSelector {
    fn from(path: &str) -> Self {
        Self::new(path)
    }
}


impl Deref for TextFieldSelector {
    type Target = str;
    fn deref(&self) -> &str {
        &self.path
    }
}


impl Serialize for TextFieldSelector {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        match &self.search {
            Some(search) => {
                serde_json::json!({"path": self.path, "search": search}).serialize(serializer)
            }
            None => self.path.serialize(serializer),
        }
    }
}


impl<'de> Deserialize<'de> for TextFieldSelector {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Form {
            Path(String),
            Selected { path: String, search: String },
        }


        Ok(match Form::deserialize(deserializer)? {
            Form::Path(path) => Self::new(path),
            Form::Selected { path, search } => Self::with_search(path, search),
        })
    }
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
    pub fields: Option<Vec<TextFieldSelector>>,
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
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TextDefinitionMatch {
    pub path: String,
    pub definition: String,
}


/// A matching record, query field paths, and concrete matched leaf paths.
#[derive(Debug, Clone, Serialize)]
pub struct TextHit {
    pub record: Record,
    /// Canonical paths such as `contents[].incipit_statement`.
    pub fields: Vec<String>,
    /// Actual matched leaves such as `contents[9].incipit_statement`.
    pub matched_paths: Vec<String>,
    /// Definition and concrete path for each registered interpretation that matched.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub matched_definitions: Vec<TextDefinitionMatch>,
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
