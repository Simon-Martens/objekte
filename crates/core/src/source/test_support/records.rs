//! Typed text fixtures for exercising the source contract in core tests.
//! Records use ("field"=value) for maps and [value,...] for arrays.

use std::collections::{BTreeMap, HashSet};
use std::io::{BufReader, Read};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::inventory::ByteRange;
use crate::model::{ArrayValue, Number, Value};
use crate::query::json_string::unescape_json_string;
use crate::source::verification::{
    BorrowedScanResult, ScalarQuery, ScalarText, ScalarVerification,
};
use crate::source::{
    BuildFramer, DiagnosticCategory, ExtractedRecord, LogicalTextCoverage, ReaderDiagnostic,
    ReaderFailure, ScanProjection, SourceReader, TextDerivation, TextSegment,
};

use super::framing::FixtureLines;

pub(crate) const DOCUMENT_READER: FixtureReader = FixtureReader::new(false, false);
pub(crate) const SCALAR_READER: FixtureReader = FixtureReader::new(false, true);
pub(crate) const LINE_READER: FixtureReader = FixtureReader::new(true, false);

static SCANS: AtomicUsize = AtomicUsize::new(0);
static COMBINED: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn borrowed_scan_calls() -> usize {
    SCANS.load(Ordering::Relaxed)
}

pub(crate) fn combined_scan_calls() -> usize {
    COMBINED.load(Ordering::Relaxed)
}

pub(crate) struct FixtureReader {
    lines: bool,
    scalars: bool,
}

impl FixtureReader {
    const fn new(lines: bool, scalars: bool) -> Self {
        Self { lines, scalars }
    }

    fn projected(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
        trusted: bool,
    ) -> Result<ExtractedRecord, ReaderFailure> {
        let omitted = if trusted && !projection.collect_text {
            projection.top_level.as_ref()
        } else {
            None
        };
        let (mut value, events) = read_fixture_projected(bytes, omitted)?;
        if let Some(paths) = &projection.top_level
            && let Value::Object(fields) = &mut value
        {
            fields.retain(|name, _| paths.contains(name));
        }
        let text = if projection.collect_text {
            events
                .into_iter()
                .filter(|event| {
                    !event.empty_array
                        && (self.scalars || event.string)
                        && (event.visible || (self.scalars && !projection.resolved_text_only))
                })
                .map(|event| event.segment)
                .collect()
        } else {
            Vec::new()
        };
        Ok(ExtractedRecord { value, text })
    }

    fn evaluate(
        &self,
        events: Vec<FixtureEvent>,
        query: &ScalarQuery<'_>,
    ) -> Result<BorrowedScanResult, String> {
        let _scratch = crate::source::verification::scalar_match_session();
        let mut evidence = Vec::new();
        let mut fts_match = query.text.is_none();
        let mut any_filter = false;
        let mut all_filter = true;
        let mut selected = false;
        for event in events {
            let TextSegment { path, text } = event.segment;
            if let Some(filter) = query.filter
                && event.visible
                && (path == filter.path() || event.empty_array && filter.path().starts_with(&path))
            {
                selected = true;
                let matched = if event.empty_array {
                    filter.is_multi() && !filter.matches_any()
                } else {
                    event.string && filter.matches(&text)
                };
                any_filter |= matched;
                all_filter &= matched;
            }
            if let Some(fts) = query.text
                && !event.empty_array
                && (self.scalars || event.string)
                && (event.visible || (self.scalars && fts.is_unscoped()))
            {
                fts_match |= fts.matches(&path, &text, &mut evidence, query.retain_evidence)?;
            }
        }
        let filter_match = match query.filter {
            None => true,
            Some(filter) if filter.matches_any() => any_filter,
            Some(_) => selected && all_filter,
        };
        Ok(BorrowedScanResult {
            fts_match,
            filter_match,
            evidence: evidence.into_iter().collect::<HashSet<_>>(),
        })
    }
}

impl SourceReader for FixtureReader {
    fn name(&self) -> &'static str {
        if self.lines {
            "fixture-lines"
        } else if self.scalars {
            "fixture-scalars"
        } else {
            "fixture-record"
        }
    }

    fn supports_relative_path_identity(&self) -> bool {
        !self.lines
    }

    fn supports_verified_layout(&self) -> bool {
        true
    }

    fn prefers_file_batches(&self) -> bool {
        !self.lines
    }

    fn uses_extension_query_limits(&self) -> bool {
        false
    }

    fn supports_trusted_query_extraction(&self) -> bool {
        true
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        LogicalTextCoverage::AllStringFields
    }

    fn text_derivation(&self) -> TextDerivation {
        if self.scalars {
            TextDerivation::ReaderSegments
        } else {
            TextDerivation::LogicalStringsAfterConversion
        }
    }

    fn scalar_verification(&self) -> Option<ScalarVerification> {
        Some(if self.scalars {
            ScalarVerification::DecodedScalars
        } else {
            ScalarVerification::ResolvedStrings
        })
    }

    fn scan_scalars(
        &self,
        bytes: &[u8],
        query: &ScalarQuery<'_>,
    ) -> Result<BorrowedScanResult, String> {
        SCANS.fetch_add(1, Ordering::Relaxed);
        let (_, events) = read_fixture(bytes).map_err(failure_message)?;
        self.evaluate(events, query)
    }

    fn project_and_scan_scalars(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
        query: ScalarText<'_>,
        retain_evidence: bool,
    ) -> Result<(Value, BorrowedScanResult), String> {
        COMBINED.fetch_add(1, Ordering::Relaxed);
        let (mut value, events) = read_fixture(bytes).map_err(failure_message)?;
        if let Some(paths) = &projection.top_level
            && let Value::Object(fields) = &mut value
        {
            fields.retain(|name, _| paths.contains(name));
        }
        let result = self.evaluate(
            events,
            &ScalarQuery {
                text: Some(query),
                filter: None,
                retain_evidence,
            },
        )?;
        Ok((value, result))
    }

    fn uses_pull_framing(&self) -> bool {
        self.lines
    }

    fn maximum_records(&self, file_bytes: u64) -> u64 {
        if self.lines {
            file_bytes.div_ceil(2)
        } else {
            1
        }
    }

    fn build_framer<'a>(
        &self,
        input: &'a mut dyn Read,
        limit: Option<usize>,
    ) -> Option<Box<dyn BuildFramer + 'a>> {
        if !self.lines {
            return None;
        }
        let mut framer = FixtureLines::with_buffered_reader(BufReader::new(input));
        if let Some(limit) = limit {
            framer.limit_record_bytes(limit);
        }
        Some(Box::new(framer))
    }

    fn frame(&self, bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
        if !self.lines {
            return Ok(vec![ByteRange {
                start: 0,
                end: bytes.len() as u64,
            }]);
        }
        let mut offset = 0u64;
        Ok(bytes
            .split(|byte| *byte == b'\n')
            .filter_map(|line| {
                let start = offset;
                let end = start + line.len() as u64;
                offset = end + 1;
                (!line.iter().all(u8::is_ascii_whitespace)).then_some(ByteRange { start, end })
            })
            .collect())
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.parse_outcome(bytes).map_err(failure_message)
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        self.projected(bytes, &ScanProjection::full(true), false)
    }

    fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        self.projected(bytes, &ScanProjection::full(self.scalars), false)
    }

    fn parse_for_query(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        self.projected(bytes, projection, false)
            .map_err(failure_message)
    }

    fn parse_for_query_trusted(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        self.projected(bytes, projection, true)
            .map_err(failure_message)
    }
}

struct FixtureEvent {
    segment: TextSegment,
    visible: bool,
    string: bool,
    empty_array: bool,
}

struct FixtureParser<'a> {
    input: &'a str,
    projected_fields: Option<&'a HashSet<String>>,
    pos: usize,
    path: Vec<String>,
    events: Vec<FixtureEvent>,
}

fn failure_message(failure: ReaderFailure) -> String {
    match failure {
        ReaderFailure::Fatal(message) => message,
        ReaderFailure::Malformed(diagnostic) => diagnostic.message,
    }
}

fn issue(
    bytes: &[u8],
    position: usize,
    category: DiagnosticCategory,
    message: impl Into<String>,
) -> ReaderFailure {
    let code = if category == DiagnosticCategory::Encoding {
        "fixture.encoding"
    } else {
        "fixture.parse"
    };
    let mut issue = ReaderDiagnostic::new(category, code, message);
    issue.relative_byte = Some(position as u64);
    let prefix = &bytes[..position.min(bytes.len())];
    issue.line = Some(1 + prefix.iter().filter(|&&byte| byte == b'\n').count() as u64);
    issue.column = Some(
        prefix
            .iter()
            .rev()
            .take_while(|&&byte| byte != b'\n')
            .count() as u64
            + 1,
    );
    ReaderFailure::Malformed(issue)
}

fn read_fixture(bytes: &[u8]) -> Result<(Value, Vec<FixtureEvent>), ReaderFailure> {
    read_fixture_projected(bytes, None)
}

fn read_fixture_projected(
    bytes: &[u8],
    projected_fields: Option<&HashSet<String>>,
) -> Result<(Value, Vec<FixtureEvent>), ReaderFailure> {
    let input = std::str::from_utf8(bytes).map_err(|error| {
        issue(
            bytes,
            error.valid_up_to(),
            DiagnosticCategory::Encoding,
            error.to_string(),
        )
    })?;
    let mut parser = FixtureParser {
        input,
        projected_fields,
        pos: 0,
        path: Vec::new(),
        events: Vec::new(),
    };
    let value = parser.value(0).map_err(|message| {
        if message.contains("resource limit") {
            ReaderFailure::Fatal(message)
        } else {
            issue(bytes, parser.pos, DiagnosticCategory::Syntax, message)
        }
    })?;
    if parser.peek().is_some() {
        return Err(issue(
            bytes,
            parser.pos,
            DiagnosticCategory::Syntax,
            "trailing fixture text",
        ));
    }
    Ok((value, parser.events))
}

impl FixtureParser<'_> {
    fn peek(&mut self) -> Option<u8> {
        while self
            .input
            .as_bytes()
            .get(self.pos)
            .is_some_and(u8::is_ascii_whitespace)
        {
            self.pos += 1;
        }
        self.input.as_bytes().get(self.pos).copied()
    }

    fn eat(&mut self, token: u8) -> bool {
        if self.peek() == Some(token) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn expect(&mut self, token: u8) -> Result<(), String> {
        self.eat(token)
            .then_some(())
            .ok_or_else(|| format!("expected {}", char::from(token)))
    }

    fn string(&mut self) -> Result<String, String> {
        self.expect(b'"')?;
        let start = self.pos - 1;
        while let Some(&byte) = self.input.as_bytes().get(self.pos) {
            self.pos += 1;
            if byte == b'"' {
                return unescape_json_string(&self.input[start..self.pos]);
            }
            if byte == b'\\' && self.pos < self.input.len() {
                self.pos += 1;
            }
        }
        Err("unfinished fixture string".into())
    }

    fn scalar(&mut self, value: Value, text: String, string: bool) -> Value {
        self.events.push(FixtureEvent {
            visible: true,
            string,
            empty_array: false,
            segment: TextSegment {
                path: self.path.clone(),
                text,
            },
        });
        value
    }

    fn value(&mut self, depth: usize) -> Result<Value, String> {
        if depth >= 128 {
            return Err("fixture nesting exceeds resource limit".into());
        }
        match self.peek() {
            Some(b'(') => {
                self.pos += 1;
                let mut fields = BTreeMap::new();
                let mut event_ranges = BTreeMap::new();
                if !self.eat(b')') {
                    loop {
                        let name = self.string()?;
                        self.expect(b'=')?;
                        self.path.push(name.clone());
                        let start = self.events.len();
                        // The fixture uses ! as an invalid value that its trusted
                        // reader may skip when the field was not requested.
                        let skip = depth == 0
                            && self
                                .projected_fields
                                .is_some_and(|fields| !fields.contains(&name))
                            && self.eat(b'!');
                        let value = if skip {
                            Value::Null
                        } else {
                            self.value(depth + 1)?
                        };
                        let end = self.events.len();
                        self.path.pop();
                        if let Some(old) = event_ranges.insert(name.clone(), start..end) {
                            for event in &mut self.events[old] {
                                event.visible = false;
                            }
                        }
                        fields.insert(name, value);
                        if self.eat(b')') {
                            break;
                        }
                        self.expect(b',')?;
                    }
                }
                Ok(Value::Object(fields))
            }
            Some(b'[') => {
                self.pos += 1;
                let mut items = Vec::new();
                if self.eat(b']') {
                    self.events.push(FixtureEvent {
                        segment: TextSegment {
                            path: self.path.clone(),
                            text: String::new(),
                        },
                        visible: true,
                        string: false,
                        empty_array: true,
                    });
                } else {
                    loop {
                        items.push(self.value(depth + 1)?);
                        if self.eat(b']') {
                            break;
                        }
                        self.expect(b',')?;
                    }
                }
                Ok(Value::Array(ArrayValue::from_unrestricted(items)))
            }
            Some(b'"') => {
                let text = self.string()?;
                Ok(self.scalar(Value::String(text.clone()), text, true))
            }
            _ => {
                let start = self.pos;
                while self.input.as_bytes().get(self.pos).is_some_and(|&byte| {
                    !byte.is_ascii_whitespace() && !matches!(byte, b',' | b']' | b')')
                }) {
                    self.pos += 1;
                }
                if self.pos == start {
                    return Err("missing fixture value".into());
                }
                let token = &self.input[start..self.pos];
                let value = match token {
                    "true" => Value::Bool(true),
                    "false" => Value::Bool(false),
                    "null" => Value::Null,
                    _ => Value::Number(Number::parse_decimal_literal(token)?),
                };
                Ok(self.scalar(value, token.to_owned(), false))
            }
        }
    }
}

pub(crate) fn fixture_value_from_text(text: &str) -> Value {
    read_fixture(text.as_bytes())
        .map_err(failure_message)
        .unwrap()
        .0
}

pub(crate) trait FixtureValue {
    fn into_fixture_value(self) -> Value;
}

impl FixtureValue for &str {
    fn into_fixture_value(self) -> Value {
        Value::String(self.into())
    }
}

impl FixtureValue for String {
    fn into_fixture_value(self) -> Value {
        Value::String(self)
    }
}

impl FixtureValue for bool {
    fn into_fixture_value(self) -> Value {
        Value::Bool(self)
    }
}

impl<T: FixtureValue + Clone> FixtureValue for &T {
    fn into_fixture_value(self) -> Value {
        self.clone().into_fixture_value()
    }
}

impl FixtureValue for Value {
    fn into_fixture_value(self) -> Value {
        self
    }
}

impl<T: FixtureValue> FixtureValue for Vec<T> {
    fn into_fixture_value(self) -> Value {
        Value::Array(ArrayValue::from_unrestricted(
            self.into_iter()
                .map(FixtureValue::into_fixture_value)
                .collect(),
        ))
    }
}

impl<T: FixtureValue, const N: usize> FixtureValue for [T; N] {
    fn into_fixture_value(self) -> Value {
        Value::Array(ArrayValue::from_unrestricted(
            self.into_iter()
                .map(FixtureValue::into_fixture_value)
                .collect(),
        ))
    }
}

macro_rules! fixture_integers {
    ($($integer:ty),+) => {
        $(
            impl FixtureValue for $integer {
                fn into_fixture_value(self) -> Value {
                    Value::Number(Number::Integer(self as i128))
                }
            }
        )+
    };
}

fixture_integers!(u8, u16, u32, u64, usize, i8, i16, i32, i64, i128, isize);

impl FixtureValue for f64 {
    fn into_fixture_value(self) -> Value {
        Value::Number(Number::float(self).unwrap())
    }
}

pub(crate) trait FixtureText {
    fn fixture_text(&self) -> String;
}

impl FixtureText for Value {
    fn fixture_text(&self) -> String {
        let mut output = String::new();
        write_fixture_value(&mut output, self);
        output
    }
}

fn write_fixture_value(output: &mut String, value: &Value) {
    use crate::query::json_string::write_json_string_literal;
    use std::fmt::Write;

    match value {
        Value::Null => output.push_str("null"),
        Value::Bool(value) => write!(output, "{value}").unwrap(),
        Value::Number(Number::Integer(value)) => write!(output, "{value}").unwrap(),
        Value::Number(Number::Float(value)) => write!(output, "{value}").unwrap(),
        Value::Number(Number::ExactDecimal(value)) => write!(output, "{value}").unwrap(),
        Value::String(value) => write_json_string_literal(output, value),
        Value::Array(array) => {
            output.push('[');
            for (index, value) in array.values.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_fixture_value(output, value);
            }
            output.push(']');
        }
        Value::Object(fields) => {
            output.push('(');
            for (index, (key, value)) in fields.iter().enumerate() {
                if index != 0 {
                    output.push(',');
                }
                write_json_string_literal(output, key);
                output.push('=');
                write_fixture_value(output, value);
            }
            output.push(')');
        }
        _ => panic!("fixture source has no encoding for {value:?}"),
    }
}
