//! Parse and validate authoritative JSON source files on demand.
use crate::Collection;
use serde::{Deserialize, Serialize};
use serde_json::Value;


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    Pending,
    Valid,
    Invalid,
    IncorrectJson,
}


// INFO: Diagnostic location, code, message, and optional schema path. Computed on demand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// JSON Pointer into the original record. Its value remains available in Record.data.
    pub location: String,
    pub schema_path: Option<String>,
    pub code: String,
    pub message: String,
}


impl ValidationIssue {
    pub(crate) fn new(location: &str, code: &str, message: &str) -> Self {
        Self {
            location: location.into(),
            schema_path: None,
            code: code.into(),
            message: message.into(),
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
    pub fn parse(bytes: &[u8]) -> Self {
        let parsed = std::str::from_utf8(bytes)
            .map_err(|e| e.to_string())
            .and_then(|s| crate::json::parse(s).map_err(|e| e.to_string()));
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

    pub fn validate(&mut self, c: &Collection) {
        if self.state.status == RecordStatus::IncorrectJson {
            return;
        }
        self.state.issues = c.schema.issues(&self.data);
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
