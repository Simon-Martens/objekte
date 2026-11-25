//! Format-independent source contracts, framing, extraction, and access.

use std::collections::HashSet;
use std::io::Read;

use serde::{Deserialize, Serialize};

use crate::inventory::ByteRange;
use crate::model::Value;

pub mod access;
pub mod extraction;
pub(crate) mod framing;
#[cfg(test)]
#[path = "../../../loader-json/src/document/mod.rs"]
pub(crate) mod json;
#[cfg(test)]
#[path = "../../../loader-jsonl/src/reader.rs"]
pub(crate) mod json_lines;
#[cfg(test)]
use json_lines::JsonLinesReader;
#[cfg(test)]
#[path = "../../../loader-jsonl/src/framing.rs"]
mod jsonl_framing;

pub(crate) mod preparation;
pub mod registry;
#[cfg(test)]
mod test_support;
pub mod text;

pub use framing::BoundedBuildChunk as BuildChunk;
pub mod verification;

#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceFormat {
    Json,
    JsonDecodedScalars,
    JsonLines,
    Registered(String),
}

impl SourceFormat {
    pub fn registered(id: impl Into<String>) -> Self {
        Self::Registered(id.into())
    }

    pub fn id(&self) -> &str {
        match self {
            Self::Json => "json",
            Self::JsonDecodedScalars => "json_decoded_scalars",
            Self::JsonLines => "jsonl",
            Self::Registered(id) => id,
        }
    }

    pub(crate) fn supports_relative_path_identity(&self) -> bool {
        matches!(self, Self::Json | Self::JsonDecodedScalars)
    }
}

/// Extracted searchable string with its logical field path.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TextSegment {
    pub path: Vec<String>,
    pub text: String,
}

/// Logical value and text produced by a source reader.
#[derive(Clone, Debug)]
pub struct ExtractedRecord {
    pub value: Value,
    pub text: Vec<TextSegment>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCategory {
    Framing,
    Encoding,
    Syntax,
    LogicalValidation,
    ResourceLimit,
    Io,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReaderDiagnostic {
    pub category: DiagnosticCategory,
    pub code: String,
    pub message: String,
    pub relative_byte: Option<u64>,
    pub line: Option<u64>,
    pub column: Option<u64>,
    pub field_path: Option<Vec<String>>,
}

impl ReaderDiagnostic {
    pub fn new(
        category: DiagnosticCategory,
        code: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            category,
            code: code.into(),
            message: message.into(),
            relative_byte: None,
            line: None,
            column: None,
            field_path: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum ReaderFailure {
    Malformed(ReaderDiagnostic),
    Fatal(String),
}

#[derive(Clone)]
pub enum SourceOutcome {
    Logical {
        range: ByteRange,
        record: ExtractedRecord,
    },
    Malformed {
        range: ByteRange,
        diagnostic: ReaderDiagnostic,
        bytes: Vec<u8>,
    },
    Unreadable {
        diagnostic: ReaderDiagnostic,
    },
}

/// Fields required to verify a query against one source record. `None` keeps
/// the complete record; otherwise unrelated top-level members are skipped by
/// the source reader while JSON syntax is still checked.
#[derive(Clone, Debug)]
pub struct ScanProjection {
    pub top_level: Option<HashSet<String>>,
    pub collect_text: bool,
    pub resolved_text_only: bool,
}

/// Whether each logical string field appears as an intact, unchanged text
/// segment. A custom reader must opt in explicitly; its default segments are
/// reader-owned and may omit or split fields present in the logical record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogicalTextCoverage {
    AllStringFields,
    ReaderSelected,
}

/// Owner of searchable segments after logical type conversion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextDerivation {
    ReaderSegments,
    LogicalStringsAfterConversion,
}

impl ScanProjection {
    pub fn full(collect_text: bool) -> Self {
        Self {
            top_level: None,
            collect_text,
            resolved_text_only: false,
        }
    }

    pub fn full_resolved_text() -> Self {
        Self {
            top_level: None,
            collect_text: true,
            resolved_text_only: true,
        }
    }
}

/// Forward framing over a revision-bound input supplied by core. The framer
/// owns format state and buffers, while core bounds decoder jobs and indexes
/// the emitted record ranges in order.
pub trait BuildFramer {
    fn next_chunk(
        &mut self,
        max_bytes: usize,
        max_records: usize,
    ) -> Result<Option<BuildChunk>, String>;
    fn retained_input_capacity(&self) -> usize {
        0
    }
}

pub(crate) fn fingerprint_input(input: &mut dyn Read) -> std::io::Result<[u8; 32]> {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    std::io::copy(input, &mut digest)?;
    Ok(digest.finalize().into())
}

pub trait SourceReader: Send + Sync {
    fn name(&self) -> &'static str;

    /// Read a whole document from the catalog's opened input. The caller can
    /// reuse the destination allocation. Streaming readers can instead frame
    /// bounded chunks directly over that same input.
    fn read_document(&self, input: &mut dyn Read, output: &mut Vec<u8>) -> std::io::Result<()> {
        input.read_to_end(output).map(|_| ())
    }

    /// Read an exact record range or a coalesced window. Core validates its
    /// location against the indexed revision and positions the input first.
    /// The returned bytes must be identical to the original source bytes.
    fn read_range(&self, input: &mut dyn Read, output: &mut [u8]) -> std::io::Result<()> {
        input.read_exact(output)
    }

    /// Verify source contents from the beginning of the catalog's opened
    /// input. All readers use the same raw SHA-256 identity.
    fn fingerprint(&self, input: &mut dyn Read) -> std::io::Result<[u8; 32]> {
        fingerprint_input(input)
    }

    /// True when one file represents at most one record and the reader
    /// supports using the file's relative path as that record's identity.
    fn supports_relative_path_identity(&self) -> bool {
        false
    }
    /// True when record layout depends only on the input bytes, so verified
    /// replacement and path-independent moves can reuse the indexed layout.
    fn supports_verified_layout(&self) -> bool {
        false
    }
    /// Whether ordinary files can be packed into build batches. A batched
    /// reader must return a conservative bound from `maximum_records`.
    fn prefers_file_batches(&self) -> bool {
        false
    }

    /// Whether core must apply the additional generic-reader logical value
    /// allowances. Backends with bounded query projection can opt out.
    fn uses_extension_query_limits(&self) -> bool {
        true
    }

    fn scalar_verification(&self) -> Option<verification::ScalarVerification> {
        None
    }

    fn scan_scalars(
        &self,
        _bytes: &[u8],
        _query: &verification::ScalarQuery<'_>,
    ) -> Result<verification::BorrowedScanResult, String> {
        Err("reader does not support borrowed scalar scanning".into())
    }

    fn project_and_scan_scalars(
        &self,
        _bytes: &[u8],
        _projection: &ScanProjection,
        _query: verification::ScalarText<'_>,
        _evidence: bool,
    ) -> Result<(Value, verification::BorrowedScanResult), String> {
        Err("reader does not support a combined projection and scalar scan".into())
    }

    fn text_derivation(&self) -> TextDerivation {
        TextDerivation::ReaderSegments
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        LogicalTextCoverage::ReaderSelected
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String>;
    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String>;

    /// Optional format-neutral bounded build framing. Consume the supplied
    /// input once and emit consecutive chunks covering its original bytes.
    /// Ranges address each chunk; `file_offset` maps them to absolute offsets.
    /// Core validates offsets and owns each chunk after the callback. Its
    /// records must be independently decodable by `parse_for_build_outcome`,
    /// which may run concurrently for different chunks. Framing itself can
    /// retain state across emissions. An error after any emission aborts the
    /// file candidate. Return `None` and leave streaming support disabled to
    /// use the ordinary `frame` adapter.
    fn stream_build_chunks(
        &self,
        _input: &mut dyn Read,
        _max_chunk_bytes: usize,
        _max_ranges: usize,
        _emit: &mut dyn FnMut(BuildChunk) -> Result<(), String>,
    ) -> Option<Result<(), String>> {
        None
    }

    /// Whether fresh construction should use `stream_build_chunks` instead of
    /// buffering the file for `frame`. Implement together with that method.
    fn supports_streaming_build(&self) -> bool {
        false
    }

    /// Opt in to independently decodable chunks pulled from an opened source.
    fn uses_pull_framing(&self) -> bool {
        false
    }

    fn build_framer<'a>(
        &self,
        _input: &'a mut dyn Read,
        _max_record_bytes: Option<usize>,
    ) -> Option<Box<dyn BuildFramer + 'a>> {
        None
    }

    /// Conservative record bound for ordinary file batches. Readers with
    /// unbounded record expansion use their own scheduled file jobs.
    fn maximum_records(&self, _file_bytes: u64) -> u64 {
        1
    }

    /// Optional conservative projection estimate, checked before query parsing.
    /// Readers without an estimate are checked after parsing their logical value.
    fn estimate_query_value_bytes(
        &self,
        _bytes: &[u8],
        _projection: &ScanProjection,
    ) -> Option<u64> {
        None
    }

    fn frame_outcome(&self, file: &[u8]) -> Result<Vec<ByteRange>, ReaderFailure> {
        self.frame(file).map_err(ReaderFailure::Fatal)
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        self.parse(bytes).map_err(ReaderFailure::Fatal)
    }

    /// Build-only parsing may omit owned text segments when the declared text
    /// representation is the logical strings after schema conversion.
    fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        self.parse_outcome(bytes)
    }

    fn intrinsic_id(
        &self,
        record: &ExtractedRecord,
        path: &[String],
    ) -> Result<Option<String>, String> {
        let mut value = &record.value;
        for segment in path {
            let Value::Object(fields) = value else {
                return Ok(None);
            };
            let Some(next) = fields.get(segment) else {
                return Ok(None);
            };
            value = next;
        }
        match value {
            Value::String(id) | Value::Id(id) => Ok(Some(id.clone())),
            Value::Null => Ok(None),
            _ => Err(format!(
                "intrinsic record ID at {} must be a string",
                path.join(".")
            )),
        }
    }

    fn parse_for_query(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        let _ = projection;
        self.parse(bytes)
    }

    /// Opt in only when successful build parsing validates the complete source
    /// record and `parse_for_query_trusted` can extract it without revalidation.
    /// Changing that validation contract requires an explicit index rebuild.
    fn supports_trusted_query_extraction(&self) -> bool {
        false
    }

    /// An explicit index operation already validated this record. The caller
    /// invokes this hook only for readers that opt in above.
    fn parse_for_query_trusted(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        self.parse_for_query(bytes, projection)
    }
}
