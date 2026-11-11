//! Record display and export values owned by the desktop application.

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use objekte_search_core::inventory::RecordClass;
use objekte_search_core::model::{Number, Value};
use objekte_search_core::repository::{Payload, SearchHit};
use serde_json::{Value as Json, json};

/// Decode a result for the field inspector. Non-JSON source bytes stay in raw view.
pub(crate) fn json_value(hit: &SearchHit) -> Option<Json> {
    match &hit.payload {
        Some(Payload::Bytes(bytes)) => serde_json::from_slice(bytes).ok(),
        Some(Payload::Logical(value)) => Some(logical_json(value)),
        _ => None,
    }
}

fn logical_json(value: &Value) -> Json {
    match value {
        Value::Null => Json::Null,
        Value::String(text) | Value::Id(text) => Json::String(text.clone()),
        Value::Number(Number::Integer(number)) => serde_json::from_str(&number.to_string())
            .unwrap_or_else(|_| Json::String(number.to_string())),
        Value::Number(Number::Float(number)) => {
            serde_json::from_str(&number.to_string()).unwrap_or(Json::Null)
        }
        Value::Number(Number::ExactDecimal(number)) => serde_json::from_str(&number.to_string())
            .unwrap_or_else(|_| Json::String(number.to_string())),
        Value::Bool(value) => Json::Bool(*value),
        Value::Date(value) => Json::String(value.to_string()),
        Value::Datetime(value) => Json::String(value.to_string()),
        Value::Unixtime(value) => Json::Number((*value).into()),
        Value::Object(fields) => Json::Object(
            fields
                .iter()
                .map(|(name, value)| (name.clone(), logical_json(value)))
                .collect(),
        ),
        Value::Array(array) => Json::Array(array.values.iter().map(logical_json).collect()),
    }
}

pub(crate) fn raw_text(hit: &SearchHit) -> String {
    let text = match &hit.payload {
        Some(Payload::Bytes(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
        Some(Payload::Logical(_)) => json_value(hit)
            .and_then(|value| serde_json::to_string_pretty(&value).ok())
            .unwrap_or_default(),
        Some(Payload::LogicalUnavailable { .. }) => "Logical value unavailable".into(),
        None => "Record payload not loaded".into(),
    };
    text.chars()
        .map(|ch| {
            if ch.is_control() && ch != '\n' && ch != '\t' {
                '�'
            } else {
                ch
            }
        })
        .collect()
}

fn export_record(hit: &SearchHit) -> Result<Json, String> {
    match &hit.payload {
        Some(Payload::Bytes(bytes)) => {
            if hit.record_class == RecordClass::RawFallback {
                Ok(Json::String(String::from_utf8_lossy(bytes).into_owned()))
            } else {
                serde_json::from_slice(bytes).map_err(|error| error.to_string())
            }
        }
        Some(Payload::Logical(value)) => {
            serde_json::to_value(value).map_err(|error| error.to_string())
        }
        Some(Payload::LogicalUnavailable { .. }) => Ok(Json::Null),
        None => Err("query did not return record data".into()),
    }
}

pub(crate) fn hit_value(hit: &SearchHit) -> Result<Json, String> {
    Ok(json!({
        "collection": hit.collection,
        "id": hit.record_id,
        "file": hit.file_path.to_string_lossy(),
        "byte_start": hit.byte_start,
        "byte_end": hit.byte_end,
        "matched_fields": hit.matched_fields,
        "file_status": hit.file_status.code(),
        "file_status_label": hit.file_status.label(),
        "record_class": hit.record_class.code(),
        "diagnostic_ref": hit.diagnostic_ref.0,
        "logical_unavailable": matches!(hit.payload, Some(Payload::LogicalUnavailable { .. })),
        "record": export_record(hit)?,
    }))
}

pub(crate) fn one(hit: &SearchHit, path: &Path) -> Result<(), String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(file);
    let outcome = (|| -> Result<(), String> {
        serde_json::to_writer_pretty(&mut writer, &hit_value(hit)?)
            .map_err(|error| error.to_string())?;
        writer.write_all(b"\n").map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())
    })();
    drop(writer);
    if outcome.is_err() {
        let _ = fs::remove_file(path);
    }
    outcome
}
