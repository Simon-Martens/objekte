//! Source framing is format-specific; indexing consumes one extracted record at a time.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Arc;

use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, Visitor};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::inventory::{ByteRange, SourceRevision};
use crate::measurement::SourceUnavailableKind;
use crate::model::Value;
use crate::streaming::{ExecutionContext, ExecutionErrorKind};

fn source_changed(execution: Option<&ExecutionContext>, message: String) -> String {
    if let Some(execution) = execution {
        execution.fail(ExecutionErrorKind::SourceChanged, message)
    } else {
        message
    }
}

/// Classify an error from a source reader. Call this only for open/read errors,
/// after resource and cancellation checks have completed. The metadata probe
/// happens solely on the error path and never accepts new bytes as old ranges.
pub(crate) fn classify_source_read_failure(
    path: &Path,
    expected: SourceRevision,
    range: Option<ByteRange>,
) -> SourceUnavailableKind {
    probe_source_unavailable(path, expected, range).unwrap_or(SourceUnavailableKind::Unreadable)
}

/// A metadata/open probe for distinguishing source unavailability from parse,
/// schema, and programming failures after a composite operation returned an
/// untyped error. It runs only on that operation's error path.
pub(crate) fn probe_source_unavailable(
    path: &Path,
    expected: SourceRevision,
    range: Option<ByteRange>,
) -> Option<SourceUnavailableKind> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(SourceUnavailableKind::Missing);
        }
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    };
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    };
    let actual = match SourceRevision::from_metadata(&metadata) {
        Ok(actual) => actual,
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    };
    if actual != expected {
        return Some(SourceUnavailableKind::RevisionMismatch);
    }
    if range.is_some_and(|range| range.start > range.end || range.end > expected.size) {
        return Some(SourceUnavailableKind::UnusableRange);
    }
    None
}

/// Classify a lifecycle read failure using the same file-incarnation proof as
/// the checked reader. A same-size, same-mtime replacement is a mismatch.
pub(crate) fn classify_source_read_failure_checked(
    path: &Path,
    expected: SourceRevision,
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
    range: Option<ByteRange>,
) -> SourceUnavailableKind {
    probe_source_unavailable_checked(path, expected, created_ns, fingerprint, range)
        .unwrap_or(SourceUnavailableKind::Unreadable)
}

pub(crate) fn probe_source_unavailable_checked(
    path: &Path,
    expected: SourceRevision,
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
    range: Option<ByteRange>,
) -> Option<SourceUnavailableKind> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Some(SourceUnavailableKind::Missing);
        }
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    };
    let proof = SourceIdentityProof {
        created_ns,
        fingerprint,
    };
    match opened_identity_matches(&mut file, expected, proof) {
        Ok(true) => {}
        Ok(false) => return Some(SourceUnavailableKind::RevisionMismatch),
        Err(_) => return Some(SourceUnavailableKind::Unreadable),
    }
    if range.is_some_and(|range| range.start > range.end || range.end > expected.size) {
        return Some(SourceUnavailableKind::UnusableRange);
    }
    None
}

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
}


/// Extracted searchable string with its logical field path.
#[derive(Clone, Debug)]
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


pub fn finalize_extracted_text(
    reader: &dyn SourceReader,
    record: &mut ExtractedRecord,
    converted_fields: bool,
) {
    if converted_fields && reader.text_derivation() == TextDerivation::LogicalStringsAfterConversion
    {
        record.text = text_segments(&record.value);
    }
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


pub trait SourceReader: Send + Sync {
    fn name(&self) -> &'static str;
    fn text_derivation(&self) -> TextDerivation {
        TextDerivation::ReaderSegments
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        LogicalTextCoverage::ReaderSelected
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String>;
    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String>;

    /// Opt in only when `frame` treats each nonblank newline-delimited span as
    /// one record, excluding the LF byte and retaining any preceding CR.
    /// The default keeps third-party readers on whole-file framing.
    fn supports_bounded_json_lines_build(&self) -> bool {
        false
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
}


/// Stable source identity, discovery suffix, and reader implementation.
#[derive(Clone)]
pub struct SourceRegistration {
    pub id: String,
    pub version: u32,
    pub extension: String,
    pub reader: Arc<dyn SourceReader>,
}


/// Prepared source handle resolved once per collection.
#[derive(Clone)]
pub struct PreparedSource(Arc<SourceRegistration>, bool);


impl PreparedSource {
    pub fn id(&self) -> &str {
        &self.0.id
    }

    pub fn version(&self) -> u32 {
        self.0.version
    }

    pub fn extension(&self) -> &str {
        &self.0.extension
    }

    pub fn reader(&self) -> &dyn SourceReader {
        self.0.reader.as_ref()
    }

    pub(crate) fn is_extension(&self) -> bool {
        !self.1
    }
}


/// Repository-local source registrations. The built-ins use this same table.
#[derive(Clone, Default)]
pub struct SourceRegistry {
    entries: HashMap<String, Arc<SourceRegistration>>,
    builtin_ids: HashSet<String>,
}


impl SourceRegistry {
    pub fn builtin() -> Self {
        let mut sources = Self::default();
        sources
            .register("json", 1, "json", Arc::new(JsonReader))
            .expect("valid JSON source registration");
        sources
            .register(
                "json_decoded_scalars",
                1,
                "json",
                Arc::new(JsonDecodedScalarsReader),
            )
            .expect("valid decoded-scalar JSON source registration");
        sources
            .register("jsonl", 1, "jsonl", Arc::new(JsonLinesReader))
            .expect("valid JSON Lines source registration");
        sources
            .builtin_ids
            .extend(["json".into(), "json_decoded_scalars".into(), "jsonl".into()]);
        sources
    }

    pub fn register(
        &mut self,
        id: impl Into<String>,
        version: u32,
        extension: impl Into<String>,
        reader: Arc<dyn SourceReader>,
    ) -> Result<(), String> {
        let id = id.into();
        let extension = extension.into();
        if id.is_empty() || version == 0 || extension.is_empty() || extension.contains(['/', '\\'])
        {
            return Err("invalid source registration".into());
        }
        if self.entries.contains_key(&id) {
            return Err(format!("duplicate source registration: {id}"));
        }
        self.entries.insert(
            id.clone(),
            Arc::new(SourceRegistration {
                id,
                version,
                extension,
                reader,
            }),
        );
        Ok(())
    }

    pub fn resolve(&self, format: &SourceFormat) -> Result<PreparedSource, String> {
        self.entries
            .get(format.id())
            .cloned()
            .map(|source| PreparedSource(source, self.builtin_ids.contains(format.id())))
            .ok_or_else(|| format!("unknown source registration: {}", format.id()))
    }
}


/// Reader for a single standalone JSON record per file.
pub struct JsonReader;


/// Built-in one-document JSON reader with decoded scalar-value FTS text.
pub struct JsonDecodedScalarsReader;


/// Reader for independently parseable JSON Lines records.
pub struct JsonLinesReader;


/// Deserializer seed that retains selected top-level JSON members.
struct SelectedObject<'a>(&'a HashSet<String>);


/// Map visitor that skips JSON members outside a query projection.
struct SelectedVisitor<'a>(&'a HashSet<String>);


impl<'de> Visitor<'de> for SelectedVisitor<'_> {
    type Value = serde_json::Map<String, serde_json::Value>;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a JSON object")
    }

    fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Self::Value, M::Error> {
        let mut selected = serde_json::Map::new();
        while let Some(key) = map.next_key::<String>()? {
            if self.0.contains(&key) {
                selected.insert(key, map.next_value()?);
            } else {
                map.next_value::<IgnoredAny>()?;
            }
        }
        Ok(selected)
    }
}


impl<'de> DeserializeSeed<'de> for SelectedObject<'_> {
    type Value = serde_json::Map<String, serde_json::Value>;

    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        deserializer: D,
    ) -> Result<Self::Value, D::Error> {
        deserializer.deserialize_map(SelectedVisitor(self.0))
    }
}


fn json_issue(bytes: &[u8], error: serde_json::Error) -> ReaderDiagnostic {
    let invalid_utf8 = std::str::from_utf8(bytes).err();
    let category = if error.to_string().contains("recursion limit exceeded") {
        DiagnosticCategory::ResourceLimit
    } else if invalid_utf8.is_some() {
        DiagnosticCategory::Encoding
    } else {
        DiagnosticCategory::Syntax
    };
    let mut issue = ReaderDiagnostic::new(category, "json.parse", error.to_string());
    issue.line = (error.line() > 0).then_some(error.line() as u64);
    issue.column = (error.column() > 0).then_some(error.column() as u64);
    if let Some(invalid) = invalid_utf8 {
        issue.relative_byte = Some(invalid.valid_up_to() as u64);
    } else if let (Some(line), Some(column)) = (issue.line, issue.column) {
        let mut start = 0usize;
        for _ in 1..line {
            let Some(next) = bytes[start..].iter().position(|byte| *byte == b'\n') else {
                return issue;
            };
            start += next + 1;
        }
        issue.relative_byte = Some(
            (start as u64)
                .saturating_add(column.saturating_sub(1))
                .min(bytes.len() as u64),
        );
    }
    issue
}

fn json_reader_failure(issue: ReaderDiagnostic) -> ReaderFailure {
    if issue.category == DiagnosticCategory::ResourceLimit {
        ReaderFailure::Fatal(issue.message)
    } else {
        ReaderFailure::Malformed(issue)
    }
}

fn parse_json_for_query_diagnosed(
    bytes: &[u8],
    projection: &ScanProjection,
) -> Result<ExtractedRecord, ReaderDiagnostic> {
    let object_root = bytes
        .iter()
        .copied()
        .find(|byte| !byte.is_ascii_whitespace())
        == Some(b'{');
    let json = if let Some(fields) = projection.top_level.as_ref().filter(|_| object_root) {
        let mut deserializer = serde_json::Deserializer::from_slice(bytes);
        let selected = SelectedObject(fields)
            .deserialize(&mut deserializer)
            .map_err(|e| json_issue(bytes, e))?;
        deserializer.end().map_err(|e| json_issue(bytes, e))?;
        serde_json::Value::Object(selected)
    } else {
        serde_json::from_slice(bytes).map_err(|e| json_issue(bytes, e))?
    };
    let value = Value::from_json(json).map_err(|error| {
        let category = if error.contains("resource limit") {
            DiagnosticCategory::ResourceLimit
        } else {
            DiagnosticCategory::LogicalValidation
        };
        ReaderDiagnostic::new(category, "json.value", error)
    })?;
    let mut text = Vec::new();
    if projection.collect_text {
        collect_strings(&value, &mut Vec::new(), &mut text);
    }
    Ok(ExtractedRecord { value, text })
}

fn parse_json_for_query(
    bytes: &[u8],
    projection: &ScanProjection,
) -> Result<ExtractedRecord, String> {
    parse_json_for_query_diagnosed(bytes, projection).map_err(|issue| issue.message)
}

fn parse_json_decoded_for_query_diagnosed(
    bytes: &[u8],
    projection: &ScanProjection,
) -> Result<ExtractedRecord, ReaderDiagnostic> {
    let mut extracted = parse_json_for_query_diagnosed(
        bytes,
        &ScanProjection {
            top_level: projection.top_level.clone(),
            collect_text: false,
            resolved_text_only: false,
        },
    )?;
    if projection.collect_text {
        extracted.text =
            crate::json_scalar_text::scalar_segments(bytes, projection.resolved_text_only, true)
                .map_err(|error| {
                    ReaderDiagnostic::new(DiagnosticCategory::Syntax, "json.scalar_text", error)
                })?;
    }
    Ok(extracted)
}

fn parse_json_decoded_for_build_diagnosed(
    bytes: &[u8],
) -> Result<ExtractedRecord, ReaderDiagnostic> {
    let (value, text) = crate::json_scalar_text::build_record(bytes).map_err(|error| {
        if error.contains("resource limit") {
            return ReaderDiagnostic::new(DiagnosticCategory::ResourceLimit, "json.value", error);
        }
        match serde_json::from_slice::<serde_json::Value>(bytes) {
            Err(syntax) => json_issue(bytes, syntax),
            Ok(_) => ReaderDiagnostic::new(DiagnosticCategory::Syntax, "json.scalar_text", error),
        }
    })?;
    Ok(ExtractedRecord { value, text })
}

fn parse_json(bytes: &[u8]) -> Result<ExtractedRecord, String> {
    parse_json_for_query(bytes, &ScanProjection::full(true))
}

fn collect_strings(value: &Value, path: &mut Vec<String>, output: &mut Vec<TextSegment>) {
    match value {
        Value::String(s) | Value::Id(s) => output.push(TextSegment {
            path: path.clone(),
            text: s.clone(),
        }),
        Value::Object(object) => {
            for (name, value) in object {
                path.push(name.clone());
                collect_strings(value, path, output);
                path.pop();
            }
        }
        Value::Array(array) => {
            for value in &array.values {
                collect_strings(value, path, output);
            }
        }
        _ => {}
    }
}

pub fn text_segments(value: &Value) -> Vec<TextSegment> {
    let mut output = Vec::new();
    collect_strings(value, &mut Vec::new(), &mut output);
    output
}

#[cfg(test)]
pub(crate) fn visit_logical_strings(value: &Value, emit: &mut dyn FnMut(&str)) {
    let _ = visit_logical_strings_and_empty(value, emit);
}

/// Visit every logical string segment while observing empty arrays in the
/// same traversal. The Boolean is a conservative record-wide structural fact.
pub(crate) fn visit_logical_strings_and_empty(value: &Value, emit: &mut dyn FnMut(&str)) -> bool {
    match value {
        Value::String(text) | Value::Id(text) => {
            emit(text);
            false
        }
        Value::Object(fields) => {
            let mut empty = false;
            for value in fields.values() {
                empty |= visit_logical_strings_and_empty(value, emit);
            }
            empty
        }
        Value::Array(array) => {
            let mut empty = array.values.is_empty();
            for value in &array.values {
                empty |= visit_logical_strings_and_empty(value, emit);
            }
            empty
        }
        _ => false,
    }
}


impl SourceReader for JsonReader {
    fn name(&self) -> &'static str {
        "json"
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        LogicalTextCoverage::AllStringFields
    }

    fn text_derivation(&self) -> TextDerivation {
        TextDerivation::LogicalStringsAfterConversion
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        Ok(vec![ByteRange::checked(
            0,
            file.len() as u64,
            file.len() as u64,
        )?])
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        parse_json(bytes)
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_for_query_diagnosed(bytes, &ScanProjection::full(true))
            .map_err(json_reader_failure)
    }

    fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_for_query_diagnosed(bytes, &ScanProjection::full(false))
            .map_err(json_reader_failure)
    }

    fn parse_for_query(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        parse_json_for_query(bytes, projection)
    }
}


impl SourceReader for JsonDecodedScalarsReader {
    fn name(&self) -> &'static str {
        "json_decoded_scalars"
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        LogicalTextCoverage::AllStringFields
    }

    fn text_derivation(&self) -> TextDerivation {
        TextDerivation::ReaderSegments
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        JsonReader.frame(file)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        parse_json_decoded_for_query_diagnosed(bytes, &ScanProjection::full(true))
            .map_err(|issue| issue.message)
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_decoded_for_query_diagnosed(bytes, &ScanProjection::full(true))
            .map_err(json_reader_failure)
    }

    fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_decoded_for_build_diagnosed(bytes).map_err(json_reader_failure)
    }

    fn parse_for_query(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        parse_json_decoded_for_query_diagnosed(bytes, projection).map_err(|issue| issue.message)
    }
}


impl SourceReader for JsonLinesReader {
    fn name(&self) -> &'static str {
        "jsonl"
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        LogicalTextCoverage::AllStringFields
    }

    fn text_derivation(&self) -> TextDerivation {
        TextDerivation::LogicalStringsAfterConversion
    }

    fn supports_bounded_json_lines_build(&self) -> bool {
        true
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        let mut result = Vec::new();
        let mut start = 0usize;
        for (i, byte) in file.iter().enumerate() {
            if *byte == b'\n' {
                if !file[start..i].iter().all(u8::is_ascii_whitespace) {
                    result.push(ByteRange::checked(
                        start as u64,
                        i as u64,
                        file.len() as u64,
                    )?);
                }
                start = i + 1;
            }
        }
        if start < file.len() && !file[start..].iter().all(u8::is_ascii_whitespace) {
            result.push(ByteRange::checked(
                start as u64,
                file.len() as u64,
                file.len() as u64,
            )?);
        }
        Ok(result)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        parse_json(bytes)
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_for_query_diagnosed(bytes, &ScanProjection::full(true))
            .map_err(json_reader_failure)
    }

    fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_for_query_diagnosed(bytes, &ScanProjection::full(false))
            .map_err(json_reader_failure)
    }

    fn parse_for_query(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        parse_json_for_query(bytes, projection)
    }
}


/// Framing and parse counts from one source extraction.
#[derive(Default, Clone, Copy, Debug)]
pub struct ExtractionCounters {
    pub framed_records: u64,
    pub record_parses: u64,
    pub failed_parses: u64,
}


/// Call `consume` once for every independently parseable record. No index
/// family rereads or reparses the source; builders share the extracted value.
pub fn extract_file<F>(
    path: &Path,
    format: SourceFormat,
    expected: SourceRevision,
    consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(ByteRange, ExtractedRecord) -> Result<(), String>,
{
    let source = SourceRegistry::builtin().resolve(&format)?;
    extract_file_with_reader(path, source.reader(), expected, consume)
}

pub fn extract_file_with_reader<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    mut consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(ByteRange, ExtractedRecord) -> Result<(), String>,
{
    extract_file_outcomes(path, source, expected, |outcome| match outcome {
        SourceOutcome::Logical { range, record } => consume(range, record),
        SourceOutcome::Malformed {
            range, diagnostic, ..
        } => Err(format!(
            "{}:{}..{}: {}",
            path.display(),
            range.start,
            range.end,
            diagnostic.message
        )),
        SourceOutcome::Unreadable { diagnostic } => {
            Err(format!("{}: {}", path.display(), diagnostic.message))
        }
    })
}

fn validate_reader_ranges(ranges: &[ByteRange], file_size: usize) -> Result<(), String> {
    let size = u64::try_from(file_size).map_err(|_| "source size overflow")?;
    let mut previous_end = 0u64;
    for range in ranges {
        ByteRange::checked(range.start, range.end, size)
            .map_err(|error| format!("reader contract: {error}"))?;
        usize::try_from(range.start).map_err(|_| "reader contract: range start overflow")?;
        usize::try_from(range.end).map_err(|_| "reader contract: range end overflow")?;
        if range.start < previous_end {
            return Err("reader contract: record ranges overlap or are out of order".into());
        }
        previous_end = range.end;
    }
    Ok(())
}

/// Frame a source once for independent construction jobs. Errors that the
/// ordinary extraction path records as file outcomes use that path unchanged.
pub(crate) fn frame_file_for_build(
    path: &Path,
    source: &dyn SourceReader,
) -> Result<Option<(Arc<Vec<u8>>, Vec<ByteRange>)>, String> {
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return Ok(None),
    };
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return Ok(None);
    }
    let ranges = match source.frame_outcome(&bytes) {
        Ok(ranges) => ranges,
        Err(ReaderFailure::Malformed(_)) => return Ok(None),
        Err(ReaderFailure::Fatal(error)) => {
            return Err(format!("{}: reader fatal: {error}", path.display()));
        }
    };
    validate_reader_ranges(&ranges, bytes.len())?;
    Ok(Some((Arc::new(bytes), ranges)))
}


/// One bounded piece of a newline-framed source. Ranges address `bytes`;
/// `file_offset` translates them into the repository's absolute file ranges.
pub(crate) struct BoundedBuildChunk {
    pub bytes: Vec<u8>,
    pub ranges: Vec<ByteRange>,
    pub file_offset: u64,
    pub preceding_lines: u64,
}


struct PendingBuildLine {
    bytes: Vec<u8>,
    file_offset: u64,
    preceding_lines: u64,
}


/// Build-only line framing. It holds one chunk and, when a boundary is
/// reached, at most one pending line. A single oversized line is admitted as
/// its own chunk rather than retaining the complete source file.
pub(crate) struct BoundedJsonLines<R: BufRead = BufReader<std::fs::File>> {
    reader: R,
    file_offset: u64,
    preceding_lines: u64,
    scratch: Vec<u8>,
    pending: Option<PendingBuildLine>,
    eof: bool,
}


impl BoundedJsonLines<BufReader<std::fs::File>> {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        Ok(Self::from_reader(BufReader::new(file)))
    }
}


impl<R: BufRead> BoundedJsonLines<R> {
    pub(crate) fn from_reader(reader: R) -> Self {
        Self {
            reader,
            file_offset: 0,
            preceding_lines: 0,
            scratch: Vec::new(),
            pending: None,
            eof: false,
        }
    }

    pub fn next_chunk(
        &mut self,
        max_bytes: usize,
        max_ranges: usize,
    ) -> Result<Option<BoundedBuildChunk>, String> {
        if max_bytes == 0 || max_ranges == 0 {
            return Err("bounded framing requires positive limits".into());
        }
        let mut chunk: Option<BoundedBuildChunk> = None;
        loop {
            let line = if let Some(line) = self.pending.take() {
                line
            } else if self.eof {
                break;
            } else {
                self.scratch.clear();
                let size = self
                    .reader
                    .read_until(b'\n', &mut self.scratch)
                    .map_err(|error| error.to_string())?;
                if size == 0 {
                    self.eof = true;
                    break;
                }
                let start = self.file_offset;
                let preceding = self.preceding_lines;
                self.file_offset = self
                    .file_offset
                    .checked_add(size as u64)
                    .ok_or("source offset overflow")?;
                if self.scratch.last() == Some(&b'\n') {
                    self.preceding_lines = self
                        .preceding_lines
                        .checked_add(1)
                        .ok_or("source line count overflow")?;
                }
                PendingBuildLine {
                    bytes: std::mem::take(&mut self.scratch),
                    file_offset: start,
                    preceding_lines: preceding,
                }
            };
            let content_len = line.bytes.len() - usize::from(line.bytes.last() == Some(&b'\n'));
            let record = !line.bytes[..content_len]
                .iter()
                .all(u8::is_ascii_whitespace);
            let full = chunk.as_ref().is_some_and(|chunk| {
                chunk.bytes.len().saturating_add(line.bytes.len()) > max_bytes
                    || (record && chunk.ranges.len() == max_ranges)
            });
            if full {
                self.pending = Some(line);
                break;
            }
            let current = chunk.get_or_insert_with(|| BoundedBuildChunk {
                bytes: Vec::new(),
                ranges: Vec::new(),
                file_offset: line.file_offset,
                preceding_lines: line.preceding_lines,
            });
            let local_start = current.bytes.len();
            let local_end = local_start
                .checked_add(content_len)
                .ok_or("chunk offset overflow")?;
            if current.bytes.is_empty() {
                current.bytes = line.bytes;
            } else {
                current.bytes.extend_from_slice(&line.bytes);
                self.scratch = line.bytes;
            }
            if record {
                current.ranges.push(ByteRange::checked(
                    local_start as u64,
                    local_end as u64,
                    current.bytes.len() as u64,
                )?);
            }
            if current.bytes.len() >= max_bytes || current.ranges.len() == max_ranges {
                break;
            }
        }
        Ok(chunk)
    }
}


pub(crate) fn extract_framed_build_outcomes<F>(
    path: &Path,
    source: &dyn SourceReader,
    bytes: &[u8],
    ranges: &[ByteRange],
    consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    extract_framed_outcomes_impl(path, source, bytes, ranges, true, consume)
}

fn extract_framed_outcomes_impl<F>(
    path: &Path,
    source: &dyn SourceReader,
    bytes: &[u8],
    ranges: &[ByteRange],
    for_build: bool,
    mut consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    let mut counters = ExtractionCounters::default();
    let mut coordinate_cursor = FileCoordinateCursor::default();
    for &range in ranges {
        let part = &bytes[range.start as usize..range.end as usize];
        counters.framed_records += 1;
        match if for_build {
            source.parse_for_build_outcome(part)
        } else {
            source.parse_outcome(part)
        } {
            Ok(record) => {
                counters.record_parses += 1;
                consume(SourceOutcome::Logical { range, record })?;
            }
            Err(ReaderFailure::Malformed(mut diagnostic)) => {
                counters.failed_parses += 1;
                file_coordinates(bytes, range, &mut diagnostic, &mut coordinate_cursor);
                consume(SourceOutcome::Malformed {
                    range,
                    diagnostic,
                    bytes: part.to_vec(),
                })?;
            }
            Err(ReaderFailure::Fatal(error)) => {
                return Err(format!(
                    "{}:{}..{}: reader fatal: {error}",
                    path.display(),
                    range.start,
                    range.end
                ));
            }
        }
    }
    Ok(counters)
}

#[cfg(test)]
thread_local! {
    static FILE_COORDINATE_SCANNED_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}


/// Running file position for ordered reader ranges, bounded independently of file size.
#[derive(Default)]
struct FileCoordinateCursor {
    scanned_to: usize,
    earlier_lines: u64,
    line_start: usize,
}


fn file_coordinates(
    file: &[u8],
    range: ByteRange,
    diagnostic: &mut ReaderDiagnostic,
    cursor: &mut FileCoordinateCursor,
) {
    let start = range.start as usize;
    // Reader ranges are validated as ordered and non-overlapping before this
    // loop. Scan the intervening bytes once, including gaps between records.
    let prefix = &file[cursor.scanned_to..start];
    #[cfg(test)]
    FILE_COORDINATE_SCANNED_BYTES.with(|scanned| scanned.set(scanned.get() + prefix.len()));
    for (index, byte) in prefix.iter().enumerate() {
        if *byte == b'\n' {
            cursor.earlier_lines += 1;
            cursor.line_start = cursor.scanned_to + index + 1;
        }
    }
    cursor.scanned_to = start;
    if let Some(line) = diagnostic.line {
        diagnostic.line = line.checked_add(cursor.earlier_lines);
        if line == 1 {
            diagnostic.column = diagnostic
                .column
                .and_then(|column| column.checked_add((start - cursor.line_start) as u64));
        }
    }
}

pub fn extract_file_outcomes<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    extract_file_outcomes_impl(path, source, expected, false, None, consume)
}

pub(crate) fn extract_file_build_outcomes<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    extract_file_outcomes_impl(path, source, expected, true, None, consume)
}

/// Capture the exact whole-file digest while the fresh builder's normal
/// extraction read is still resident. An unreadable file has no fingerprint.
pub(crate) fn extract_file_build_outcomes_hashed<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    consume: F,
) -> Result<(ExtractionCounters, Option<[u8; 32]>), String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    let mut fingerprint = None;
    let counters = extract_file_outcomes_impl(
        path,
        source,
        expected,
        true,
        Some(&mut fingerprint),
        consume,
    )?;
    Ok((counters, fingerprint))
}

fn extract_file_outcomes_impl<F>(
    path: &Path,
    source: &dyn SourceReader,
    expected: SourceRevision,
    for_build: bool,
    fingerprint: Option<&mut Option<[u8; 32]>>,
    mut consume: F,
) -> Result<ExtractionCounters, String>
where
    F: FnMut(SourceOutcome) -> Result<(), String>,
{
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) => {
            consume(SourceOutcome::Unreadable {
                diagnostic: ReaderDiagnostic::new(
                    DiagnosticCategory::Io,
                    "source.open",
                    error.to_string(),
                ),
            })?;
            return Ok(ExtractionCounters::default());
        }
    };
    if !for_build {
        check_revision(path, &file, expected)?;
    }
    let mut bytes = Vec::new();
    if let Err(error) = file.read_to_end(&mut bytes) {
        consume(SourceOutcome::Unreadable {
            diagnostic: ReaderDiagnostic::new(
                DiagnosticCategory::Io,
                "source.read",
                error.to_string(),
            ),
        })?;
        return Ok(ExtractionCounters::default());
    }
    if let Some(fingerprint) = fingerprint {
        if bytes.len() as u64 == expected.size {
            *fingerprint = Some(Sha256::digest(&bytes).into());
        }
    }
    if !for_build {
        check_revision(path, &file, expected)?;
        if bytes.len() as u64 != expected.size {
            return Err("stale source revision during extraction".into());
        }
    }
    let ranges = match source.frame_outcome(&bytes) {
        Ok(ranges) => ranges,
        Err(ReaderFailure::Malformed(mut diagnostic)) => {
            let size = u64::try_from(bytes.len()).map_err(|_| "source size overflow")?;
            let range = ByteRange::checked(0, size, size)?;
            file_coordinates(
                &bytes,
                range,
                &mut diagnostic,
                &mut FileCoordinateCursor::default(),
            );
            consume(SourceOutcome::Malformed {
                range,
                diagnostic,
                bytes,
            })?;
            if !for_build {
                check_revision(path, &file, expected)?;
            }
            return Ok(ExtractionCounters {
                framed_records: 1,
                record_parses: 0,
                failed_parses: 0,
            });
        }
        Err(ReaderFailure::Fatal(error)) => {
            return Err(format!("{}: reader fatal: {error}", path.display()));
        }
    };
    validate_reader_ranges(&ranges, bytes.len())?;
    let counters =
        extract_framed_outcomes_impl(path, source, &bytes, &ranges, for_build, &mut consume)?;
    if !for_build {
        check_revision(path, &file, expected)?;
    }
    Ok(counters)
}

fn check_revision(
    path: &Path,
    file: &std::fs::File,
    expected: SourceRevision,
) -> Result<(), String> {
    let opened = SourceRevision::from_metadata(&file.metadata().map_err(|e| e.to_string())?)?;
    let named = SourceRevision::from_metadata(
        &std::fs::metadata(path).map_err(|e| format!("stale source revision: {e}"))?,
    )?;
    if opened != expected || named != expected {
        return Err("stale source revision".into());
    }
    Ok(())
}

fn opened_revision_matches(file: &std::fs::File, expected: SourceRevision) -> Result<bool, String> {
    let opened = SourceRevision::from_metadata(&file.metadata().map_err(|e| e.to_string())?)?;
    Ok(opened == expected)
}

#[derive(Clone, Copy)]
struct SourceIdentityProof {
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
}

fn opened_identity_matches(
    file: &mut std::fs::File,
    expected: SourceRevision,
    proof: SourceIdentityProof,
) -> Result<bool, String> {
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if SourceRevision::from_metadata(&metadata)? != expected {
        return Ok(false);
    }
    if let Some(created_ns) = proof.created_ns {
        let created = metadata
            .created()
            .ok()
            .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_nanos() as i128);
        return Ok(created == Some(created_ns));
    }
    let Some(fingerprint) = proof.fingerprint else {
        return Ok(false);
    };
    // Creation time is unavailable on this filesystem. Hash the same opened
    // handle, restoring its cursor so a subsequent range read stays correct.
    let position = file.stream_position().map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    let digest = (|| -> Result<[u8; 32], String> {
        loop {
            let count = file.read(&mut buffer).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            hasher.update(&buffer[..count]);
        }
        Ok(hasher.finalize().into())
    })();
    file.seek(SeekFrom::Start(position))
        .map_err(|error| error.to_string())?;
    Ok(digest? == fingerprint && opened_revision_matches(file, expected)?)
}

fn check_opened_revision(file: &std::fs::File, expected: SourceRevision) -> Result<(), String> {
    if !opened_revision_matches(file, expected)? {
        return Err("stale source revision".into());
    }
    Ok(())
}

/// Read only a stored range after checking the recorded source revision.
pub fn read_record_range(
    path: &Path,
    revision: SourceRevision,
    range: ByteRange,
) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    check_opened_revision(&file, revision)?;
    ByteRange::checked(range.start, range.end, revision.size)?;
    read_record_range_from_opened(&mut file, range)
}

/// Lifecycle read tied to a pinned file incarnation. When birth time is not
/// available, the raw digest proves the opened handle instead.
pub(crate) fn read_record_range_checked(
    path: &Path,
    revision: SourceRevision,
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
    range: ByteRange,
) -> Result<Vec<u8>, String> {
    read_record_range_checked_impl(path, revision, created_ns, fingerprint, range, None)
}

pub(crate) fn read_record_range_checked_with_execution(
    path: &Path,
    revision: SourceRevision,
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
    range: ByteRange,
    execution: &ExecutionContext,
) -> Result<Vec<u8>, String> {
    read_record_range_checked_impl(
        path,
        revision,
        created_ns,
        fingerprint,
        range,
        Some(execution),
    )
}

fn read_record_range_checked_impl(
    path: &Path,
    revision: SourceRevision,
    created_ns: Option<i128>,
    fingerprint: Option<[u8; 32]>,
    range: ByteRange,
    execution: Option<&ExecutionContext>,
) -> Result<Vec<u8>, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let proof = SourceIdentityProof {
        created_ns,
        fingerprint,
    };
    if !opened_identity_matches(&mut file, revision, proof)? {
        return Err(source_changed(execution, "stale source identity".into()));
    }
    ByteRange::checked(range.start, range.end, revision.size)?;
    let bytes = read_record_range_from_opened(&mut file, range)?;
    if !opened_identity_matches(&mut file, revision, proof)? {
        return Err(source_changed(execution, "stale source identity".into()));
    }
    Ok(bytes)
}

/// Query read with an explicit typed source-change channel.
pub(crate) fn read_record_range_with_execution(
    path: &Path,
    revision: SourceRevision,
    range: ByteRange,
    execution: &ExecutionContext,
) -> Result<Vec<u8>, String> {
    execution.check_string()?;
    let mut file =
        std::fs::File::open(path).map_err(|error| format!("{}: {error}", path.display()))?;
    if !opened_revision_matches(&file, revision)? {
        return Err(source_changed(
            Some(execution),
            "stale source revision".into(),
        ));
    }
    ByteRange::checked(range.start, range.end, revision.size)?;
    read_record_range_from_opened(&mut file, range)
}

/// Build-only fallback read: discovery supplied the source revision, while
/// construction assumes the source remains stable and performs no new probe.
pub(crate) fn read_record_range_for_build(
    path: &Path,
    range: ByteRange,
) -> Result<Vec<u8>, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    ByteRange::checked(range.start, range.end, u64::MAX)?;
    read_record_range_from_opened(&mut file, range)
}

fn read_record_range_from_opened(
    file: &mut std::fs::File,
    range: ByteRange,
) -> Result<Vec<u8>, String> {
    let len = usize::try_from(range.len()).map_err(|_| "record range too large")?;
    let mut bytes = vec![0; len];
    file.seek(SeekFrom::Start(range.start))
        .map_err(|e| e.to_string())?;
    file.read_exact(&mut bytes).map_err(|e| e.to_string())?;
    Ok(bytes)
}

/// Read a bounded batch from one file handle. Nearby ranges share a window;
/// distant ranges remain sparse reads. The returned byte count is physical
/// source bytes, including small gaps inside coalesced windows.
pub fn read_record_ranges(
    path: &Path,
    revision: SourceRevision,
    ranges: &[(u32, ByteRange)],
) -> Result<(Vec<(u32, Vec<u8>)>, u64), String> {
    RecordRangeReader::open(path, revision)?.read(ranges)
}


/// One opened source handle for a bounded sequence of range batches.
pub struct RecordRangeReader {
    file: std::fs::File,
    revision: SourceRevision,
    first_read: bool,
    position: Option<u64>,
    execution: Option<ExecutionContext>,
    identity: Option<SourceIdentityProof>,
    #[cfg(test)]
    pub(crate) seeks: u64,
    #[cfg(test)]
    pub(crate) max_window_read: usize,
}


impl RecordRangeReader {
    pub fn open(path: &Path, revision: SourceRevision) -> Result<Self, String> {
        Self::open_with_execution(path, revision, None)
    }

    pub(crate) fn open_with_execution(
        path: &Path,
        revision: SourceRevision,
        execution: Option<&ExecutionContext>,
    ) -> Result<Self, String> {
        Self::open_with_identity(path, revision, None, execution)
    }

    pub(crate) fn open_checked(
        path: &Path,
        revision: SourceRevision,
        created_ns: Option<i128>,
        fingerprint: Option<[u8; 32]>,
    ) -> Result<Self, String> {
        Self::open_with_identity(
            path,
            revision,
            Some(SourceIdentityProof {
                created_ns,
                fingerprint,
            }),
            None,
        )
    }

    pub(crate) fn open_checked_with_execution(
        path: &Path,
        revision: SourceRevision,
        created_ns: Option<i128>,
        fingerprint: Option<[u8; 32]>,
        execution: Option<&ExecutionContext>,
    ) -> Result<Self, String> {
        Self::open_with_identity(
            path,
            revision,
            Some(SourceIdentityProof {
                created_ns,
                fingerprint,
            }),
            execution,
        )
    }

    fn open_with_identity(
        path: &Path,
        revision: SourceRevision,
        identity: Option<SourceIdentityProof>,
        execution: Option<&ExecutionContext>,
    ) -> Result<Self, String> {
        if let Some(execution) = execution {
            execution.check_string()?;
        }
        let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let matched = if let Some(proof) = identity {
            opened_identity_matches(&mut file, revision, proof)?
        } else {
            opened_revision_matches(&file, revision)?
        };
        if !matched {
            let message = if identity.is_some() {
                "stale source identity"
            } else {
                "stale source revision"
            };
            return Err(source_changed(execution, message.into()));
        }
        Ok(Self {
            file,
            revision,
            first_read: true,
            position: Some(0),
            execution: execution.cloned(),
            identity,
            #[cfg(test)]
            seeks: 0,
            #[cfg(test)]
            max_window_read: 0,
        })
    }

    fn current_matches(&mut self) -> Result<bool, String> {
        if let Some(proof) = self.identity {
            opened_identity_matches(&mut self.file, self.revision, proof)
        } else {
            opened_revision_matches(&self.file, self.revision)
        }
    }

    fn check_current(&mut self) -> Result<(), String> {
        if !self.current_matches()? {
            return Err(source_changed(
                self.execution.as_ref(),
                "stale source identity".into(),
            ));
        }
        Ok(())
    }

    fn read_at(&mut self, start: u64, bytes: &mut [u8]) -> Result<(), String> {
        if self.position != Some(start) {
            // A failed seek or a partial read can leave a reused handle anywhere.
            self.position = None;
            #[cfg(test)]
            {
                self.seeks += 1;
            }
            self.file
                .seek(SeekFrom::Start(start))
                .map_err(|e| e.to_string())?;
            self.position = Some(start);
        }
        if let Err(error) = self.file.read_exact(bytes) {
            self.position = None;
            return Err(error.to_string());
        }
        self.position = Some(start + bytes.len() as u64);
        Ok(())
    }

    pub fn read(
        &mut self,
        ranges: &[(u32, ByteRange)],
    ) -> Result<(Vec<(u32, Vec<u8>)>, u64), String> {
        if self.first_read {
            self.first_read = false;
        } else {
            self.check_current()?;
        }
        if let [(slot, range)] = ranges {
            ByteRange::checked(range.start, range.end, self.revision.size)?;
            let len = usize::try_from(range.len()).map_err(|_| "record range too large")?;
            #[cfg(test)]
            {
                self.max_window_read = self.max_window_read.max(len);
            }
            let mut bytes = vec![0; len];
            self.read_at(range.start, &mut bytes)?;
            if self.identity.is_some() {
                self.check_current()?;
            }
            return Ok((vec![(*slot, bytes)], len as u64));
        }
        let mut ordered = ranges.to_vec();
        ordered.sort_by_key(|(_, range)| range.start);
        for (_, range) in &ordered {
            ByteRange::checked(range.start, range.end, self.revision.size)?;
        }
        let mut output = Vec::with_capacity(ordered.len());
        let mut bytes_read = 0u64;
        let mut cursor = 0;
        while cursor < ordered.len() {
            let start = ordered[cursor].1.start;
            let mut end = ordered[cursor].1.end;
            let mut next = cursor + 1;
            while next < ordered.len() {
                let candidate = ordered[next].1;
                if candidate.start > end.saturating_add(4096)
                    || candidate.end.saturating_sub(start) > 1024 * 1024
                {
                    break;
                }
                end = end.max(candidate.end);
                next += 1;
            }
            let len = usize::try_from(end - start).map_err(|_| "read window too large")?;
            #[cfg(test)]
            {
                self.max_window_read = self.max_window_read.max(len);
            }
            let mut buffer = vec![0; len];
            self.read_at(start, &mut buffer)?;
            bytes_read += len as u64;
            for (slot, range) in &ordered[cursor..next] {
                let a = (range.start - start) as usize;
                let b = (range.end - start) as usize;
                output.push((*slot, buffer[a..b].to_vec()));
            }
            cursor = next;
        }
        if self.identity.is_some() {
            self.check_current()?;
        }
        Ok((output, bytes_read))
    }

    /// Stream-specific direct range reads. The checked sum bounds all returned
    /// byte vectors; no gap window or second copy of any record is allocated.
    pub(crate) fn read_bounded(
        &mut self,
        ranges: &[(u32, ByteRange)],
        max_total_bytes: usize,
    ) -> Result<(Vec<(u32, Vec<u8>)>, u64), String> {
        if self.first_read {
            self.first_read = false;
        } else {
            self.check_current()?;
        }
        let mut ordered = ranges.to_vec();
        ordered.sort_by_key(|(_, range)| range.start);
        let mut total = 0usize;
        for (_, range) in &ordered {
            ByteRange::checked(range.start, range.end, self.revision.size)?;
            let len = usize::try_from(range.len())
                .map_err(|_| "stream resource: record range too large")?;
            total = total
                .checked_add(len)
                .ok_or("stream resource: source range budget overflow")?;
            if total > max_total_bytes {
                return Err("stream resource: source ranges exceed read allowance".into());
            }
        }
        let mut output = Vec::with_capacity(ordered.len());
        for (slot, range) in ordered {
            let len = range.len() as usize;
            #[cfg(test)]
            {
                self.max_window_read = self.max_window_read.max(len);
            }
            let mut bytes = vec![0; len];
            self.read_at(range.start, &mut bytes)?;
            output.push((slot, bytes));
        }
        if self.identity.is_some() {
            self.check_current()?;
        }
        Ok((output, total as u64))
    }
}


#[cfg(test)]
#[path = "source.test.rs"]
mod tests;
