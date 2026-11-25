//! JSON decoding, projection, diagnostics, and logical-value construction.

use std::collections::{BTreeMap, HashSet};

use serde::Deserialize;
use serde::de::{DeserializeSeed, IgnoredAny, MapAccess, SeqAccess, Visitor};

use objekte_search_core::model::{ArrayValue, Number, Value};

use objekte_search_core::source::text::text_segments;
use objekte_search_core::source::{
    DiagnosticCategory, ExtractedRecord, ReaderDiagnostic, ReaderFailure, ScanProjection,
};

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

pub fn json_reader_failure(issue: ReaderDiagnostic) -> ReaderFailure {
    if issue.category == DiagnosticCategory::ResourceLimit {
        ReaderFailure::Fatal(issue.message)
    } else {
        ReaderFailure::Malformed(issue)
    }
}

/// Construction-only visitor. The normal query reader still supports its
/// projection path; decoded-scalar JSON keeps its lexical extraction path.
struct LogicalBuildValue(Value);

impl<'de> Deserialize<'de> for LogicalBuildValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct LogicalVisitor;

        impl<'de> Visitor<'de> for LogicalVisitor {
            type Value = LogicalBuildValue;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a JSON value")
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(LogicalBuildValue(Value::Null))
            }

            fn visit_bool<E: serde::de::Error>(self, value: bool) -> Result<Self::Value, E> {
                Ok(LogicalBuildValue(Value::Bool(value)))
            }

            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Self::Value, E> {
                Ok(LogicalBuildValue(Value::Number(Number::integer(
                    value as i128,
                ))))
            }

            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Self::Value, E> {
                Ok(LogicalBuildValue(Value::Number(Number::integer(
                    value as i128,
                ))))
            }

            fn visit_f64<E: serde::de::Error>(self, value: f64) -> Result<Self::Value, E> {
                let number = serde_json::Number::from_f64(value)
                    .ok_or_else(|| E::custom("non-finite number"))?;
                Ok(LogicalBuildValue(Value::Number(
                    Number::from_json(&number).map_err(E::custom)?,
                )))
            }

            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Self::Value, E> {
                Ok(LogicalBuildValue(Value::String(value.to_owned())))
            }

            fn visit_string<E: serde::de::Error>(self, value: String) -> Result<Self::Value, E> {
                Ok(LogicalBuildValue(Value::String(value)))
            }

            fn visit_seq<A: SeqAccess<'de>>(
                self,
                mut sequence: A,
            ) -> Result<Self::Value, A::Error> {
                let mut values = Vec::new();
                while let Some(LogicalBuildValue(value)) = sequence.next_element()? {
                    values.push(value);
                }
                Ok(LogicalBuildValue(Value::Array(
                    ArrayValue::from_unrestricted(values),
                )))
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let Some(first_key) = map.next_key::<String>()? else {
                    return Ok(LogicalBuildValue(Value::Object(BTreeMap::new())));
                };
                // serde_json/arbitrary_precision represents a number outside
                // i64/u64 as a single-entry map with this private key. Match
                // serde_json::Value's handling, including an object whose
                // first key is the same reserved spelling.
                if first_key == "$serde_json::private::Number" {
                    let token = map.next_value::<String>()?;
                    let number = serde_json::from_str::<serde_json::Number>(&token)
                        .map_err(serde::de::Error::custom)?;
                    return Ok(LogicalBuildValue(Value::Number(
                        Number::from_json(&number).map_err(serde::de::Error::custom)?,
                    )));
                }
                let mut values = BTreeMap::new();
                values.insert(first_key, map.next_value::<LogicalBuildValue>()?.0);
                while let Some((key, LogicalBuildValue(value))) = map.next_entry()? {
                    values.insert(key, value);
                }
                Ok(LogicalBuildValue(Value::Object(values)))
            }
        }

        deserializer.deserialize_any(LogicalVisitor)
    }
}

pub(crate) fn parse_json_for_build_diagnosed(
    bytes: &[u8],
) -> Result<ExtractedRecord, ReaderDiagnostic> {
    match serde_json::from_slice::<LogicalBuildValue>(bytes) {
        Ok(LogicalBuildValue(value)) => Ok(ExtractedRecord {
            value,
            text: Vec::new(),
        }),
        Err(_) => parse_json_for_query_diagnosed(bytes, &ScanProjection::full(false)),
    }
}

pub fn parse_json_for_query_diagnosed(
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
        text = text_segments(&value);
    }
    Ok(ExtractedRecord { value, text })
}

pub fn parse_json_for_query(
    bytes: &[u8],
    projection: &ScanProjection,
) -> Result<ExtractedRecord, String> {
    parse_json_for_query_diagnosed(bytes, projection).map_err(|issue| issue.message)
}

pub fn parse_json_for_query_trusted(
    bytes: &[u8],
    projection: &ScanProjection,
) -> Result<ExtractedRecord, String> {
    let value = super::scalars::extract_query_value(bytes, projection, true)?;
    let mut text = Vec::new();
    if projection.collect_text {
        text = text_segments(&value);
    }
    Ok(ExtractedRecord { value, text })
}

pub(crate) fn parse_json_decoded_for_query_diagnosed(
    bytes: &[u8],
    projection: &ScanProjection,
) -> Result<ExtractedRecord, ReaderDiagnostic> {
    if !projection.collect_text {
        return parse_json_for_query_diagnosed(bytes, projection);
    }
    let (value, text) =
        super::scalars::extract_query_record(bytes, projection, false).map_err(|error| {
            if error.contains("resource limit") {
                return ReaderDiagnostic::new(
                    DiagnosticCategory::ResourceLimit,
                    "json.value",
                    error,
                );
            }
            match serde_json::from_slice::<serde_json::Value>(bytes) {
                Err(syntax) => json_issue(bytes, syntax),
                Ok(_) => ReaderDiagnostic::new(
                    DiagnosticCategory::LogicalValidation,
                    "json.value",
                    error,
                ),
            }
        })?;
    Ok(ExtractedRecord { value, text })
}

pub(crate) fn parse_json_decoded_for_query_trusted(
    bytes: &[u8],
    projection: &ScanProjection,
) -> Result<ExtractedRecord, String> {
    let (value, text) = super::scalars::extract_query_record(bytes, projection, true)?;
    Ok(ExtractedRecord { value, text })
}

pub(crate) fn parse_json_decoded_for_build_diagnosed(
    bytes: &[u8],
) -> Result<ExtractedRecord, ReaderDiagnostic> {
    let (value, text) = super::scalars::build_record(bytes).map_err(|error| {
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

pub fn parse_json(bytes: &[u8]) -> Result<ExtractedRecord, String> {
    parse_json_for_query(bytes, &ScanProjection::full(true))
}
