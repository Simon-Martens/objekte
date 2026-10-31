//! Results and options for an explicit scan of stored JSON field shapes.

use crate::{JsonPath, TextFormat};
use serde::Serialize;


/// Work requested from an explicit inference scan.
#[derive(Debug, Clone, Copy)]
pub struct InferOptions {
    pub auto_index: bool,
    pub write_schema: bool,
}


impl Default for InferOptions {
    fn default() -> Self {
        Self {
            auto_index: true,
            write_schema: false,
        }
    }
}


/// Observed non-null JSON shape, with a separate enum classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InferredKind {
    Boolean,
    Integer,
    Number,
    Enum,
    String,
    Array,
    Object,
    Null,
    Mixed,
}


/// A reference pattern confirmed against the repository inventory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InferredLinkKind {
    RecordId,
    JsonPointer,
}


/// High-confidence relation evidence for one string field.
#[derive(Debug, Clone, Serialize)]
pub struct InferredLink {
    pub kind: InferredLinkKind,
    pub target_collection: String,
    pub resolved_values: usize,
    pub total_values: usize,
}


/// Shape, semantic hints, and index status for one nested JSON key path.
#[derive(Debug, Clone, Serialize)]
pub struct FieldInference {
    pub path: JsonPath,
    pub display_path: String,
    pub kind: InferredKind,
    pub present_records: usize,
    pub missing_records: usize,
    pub null_values: usize,
    pub multiple_values: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enum_values: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub link: Option<InferredLink>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_format: Option<TextFormat>,
    pub indexed: bool,
}


/// One collection's explicit scan and the fields it observed.
#[derive(Debug, Clone, Serialize)]
pub struct CollectionInference {
    pub collection: String,
    pub files: usize,
    pub parsed: usize,
    pub malformed: usize,
    pub fields: Vec<FieldInference>,
    pub indexed_paths: usize,
    pub schema_written: bool,
    #[serde(skip)]
    pub(crate) source_snapshot: String,
    #[serde(skip)]
    pub(crate) schema_snapshot: Option<String>,
}


/// Results for one repository or collection inference request.
#[derive(Debug, Clone, Serialize)]
pub struct InferenceReport {
    pub collections: Vec<CollectionInference>,
    pub elapsed_ms: f64,
    #[serde(skip)]
    pub(crate) reference_snapshot: String,
}
