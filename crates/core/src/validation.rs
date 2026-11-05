//! Parse and validate authoritative JSON source files on demand.
use crate::Collection;
use serde::{Deserialize, Serialize, de::DeserializeSeed};
use serde_json::Value;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    NoSchema,
    Pending,
    Valid,
    Invalid,
    IncorrectJson,
    IncorrectFormat,
}


impl RecordStatus {
    /// The serialized name used by `@status` filters and sorts.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::NoSchema => "no_schema",
            Self::Pending => "pending",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::IncorrectJson => "incorrect_json",
            Self::IncorrectFormat => "incorrect_format",
        }
    }

    /// A source whose bytes could not produce a logical record.
    pub fn is_malformed(self) -> bool {
        matches!(self, Self::IncorrectJson | Self::IncorrectFormat)
    }
}


/// Zero-based, half-open byte offsets in the original source buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSpan {
    pub start: usize,
    pub end: usize,
}


/// Source syntax location; distinct from a logical JSON Pointer or schema path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceIssue {
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub span: Option<SourceSpan>,
}


// INFO: Diagnostic location, code, message, and optional schema path. Computed on demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// JSON Pointer into a decoded logical record. Empty for source syntax
    /// issues; `source`, when present, refers to original bytes instead.
    pub location: String,
    pub schema_path: Option<String>,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<SourceIssue>,
}


impl ValidationIssue {
    pub(crate) fn new(location: &str, code: &str, message: &str) -> Self {
        Self {
            location: location.into(),
            schema_path: None,
            code: code.into(),
            message: message.into(),
            source: None,
        }
    }
}


// INFO: Overall validation status and collected issues for one record. Computed on demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordState {
    pub status: RecordStatus,
    pub issues: Vec<ValidationIssue>,
}


// INFO: Parsed JSON value paired with its validation state.
pub(crate) struct Document {
    pub data: Value,
    pub state: RecordState,
}


impl std::ops::Deref for Document {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.data
    }
}


impl Document {
    /// Retain a failed format parse without inventing a logical null value.
    pub(crate) fn malformed(
        format: &str,
        diagnostics: Vec<crate::extensions::FormatDiagnostic>,
    ) -> Self {
        let status = if format == "objekte:json" {
            RecordStatus::IncorrectJson
        } else {
            RecordStatus::IncorrectFormat
        };
        Self {
            data: Value::Null,
            state: RecordState {
                status,
                issues: diagnostics
                    .into_iter()
                    .map(|diagnostic| ValidationIssue {
                        location: String::new(),
                        schema_path: None,
                        code: diagnostic.code,
                        message: diagnostic.message,
                        source: Some(SourceIssue {
                            format: format.into(),
                            span: diagnostic.span,
                        }),
                    })
                    .collect(),
            },
        }
    }

    pub fn parse(bytes: &[u8]) -> Self {
        let parsed = serde_json::from_slice(bytes).map_err(|e| e.to_string());
        match parsed {
            Ok(data) => Self {
                data,
                state: RecordState {
                    status: RecordStatus::Pending,
                    issues: Vec::new(),
                },
            },
            Err(e) => Self {
                data: Value::Null,
                state: RecordState {
                    status: RecordStatus::IncorrectJson,
                    issues: vec![ValidationIssue::new("", "json_syntax", &e)],
                },
            },
        }
    }

    /// Parse only the listed top-level members for predicate evaluation.
    /// Other members are syntax-checked and skipped. Inputs where skipping
    /// could accept what a full parse rejects use the full parse instead.
    pub(crate) fn parse_pruned(bytes: &[u8], keys: &[String]) -> Self {
        if std::str::from_utf8(bytes).is_err() || !prune_safe(bytes) {
            return Self::parse(bytes);
        }
        let mut de = serde_json::Deserializer::from_slice(bytes);
        match prune::Pruned(keys)
            .deserialize(&mut de)
            .and_then(|data| de.end().map(|()| data))
        {
            Ok(data) => Self {
                data,
                state: RecordState {
                    status: RecordStatus::Pending,
                    issues: Vec::new(),
                },
            },
            // Reproduce the full parser's exact diagnostic.
            Err(_) => Self::parse(bytes),
        }
    }

    pub fn validate(&mut self, c: &Collection) {
        if self.state.status.is_malformed() {
            return;
        }
        let Some(schema) = &c.schema else {
            self.state.status = RecordStatus::NoSchema;
            return;
        };
        self.state.issues = schema.issues(&self.data);
        self.state.status = if self.state.issues.is_empty() {
            RecordStatus::Valid
        } else {
            RecordStatus::Invalid
        };
    }
}


// INFO: Counters and elapsed time for an explicit validation request.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ValidationMetrics {
    pub validated: u64,
    pub applied: u64,
    pub validation_nanos: u64,
}


/// Skipped members bypass two full-parse checks: the nesting limit of 128
/// and lone-surrogate escapes. Fewer than 128 brackets cannot reach the limit,
/// and any surrogate escape falls back conservatively.
fn prune_safe(bytes: &[u8]) -> bool {
    // Non-object roots, including numbers, always use the full parse.
    if bytes.iter().find(|byte| !byte.is_ascii_whitespace()) != Some(&b'{') {
        return false;
    }
    // Two vectorizable passes; escapes are rare in UTF-8 source files.
    let brackets: usize = bytes
        .iter()
        .map(|&byte| usize::from(byte == b'[' || byte == b'{'))
        .sum();
    if brackets >= 128 {
        return false;
    }
    !bytes.contains(&b'\\')
        || !bytes.windows(3).any(|window| {
            window[0] == b'\\' && window[1] == b'u' && matches!(window[2], b'd' | b'D')
        })
}


mod prune {
    use serde::de::{DeserializeSeed, Deserializer, IgnoredAny, MapAccess, Visitor};
    use serde_json::{Map, Value};
    use std::fmt;

    pub(super) struct Pruned<'k>(pub &'k [String]);

    struct Key<'k>(&'k [String]);

    impl<'de> DeserializeSeed<'de> for Key<'_> {
        type Value = Option<String>;
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
            d.deserialize_str(self)
        }
    }

    impl<'de> Visitor<'de> for Key<'_> {
        type Value = Option<String>;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("an object key")
        }
        fn visit_str<E>(self, key: &str) -> Result<Self::Value, E> {
            Ok(self
                .0
                .iter()
                .any(|kept| kept == key)
                .then(|| key.to_owned()))
        }
    }

    impl<'de> DeserializeSeed<'de> for Pruned<'_> {
        type Value = Value;
        fn deserialize<D: Deserializer<'de>>(self, d: D) -> Result<Value, D::Error> {
            d.deserialize_any(self)
        }
    }

    impl<'de> Visitor<'de> for Pruned<'_> {
        type Value = Value;
        fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
            f.write_str("a JSON value")
        }
        fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Value, M::Error> {
            let mut out = Map::new();
            while let Some(key) = map.next_key_seed(Key(self.0))? {
                match key {
                    // Later duplicates replace earlier ones, as in a full parse.
                    Some(key) => {
                        out.insert(key, map.next_value()?);
                    }
                    None => {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
            }
            Ok(Value::Object(out))
        }
    }
}


#[cfg(test)]
#[path = "validation.test.rs"]
mod tests;
