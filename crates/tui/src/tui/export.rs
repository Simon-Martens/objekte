//! Stream selected records or a complete search to a new JSON file.

use std::fs::{self, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;

use objekte_search_core::query::Query;
use objekte_search_core::repository::{Payload, Repository, SearchHit};
use serde_json::{Value, json};

use super::settings;

fn record(hit: &SearchHit) -> Result<Value, String> {
    match &hit.payload {
        Some(Payload::Bytes(bytes)) => {
            if hit.record_class == objekte_search_core::inventory::RecordClass::RawFallback {
                Ok(Value::String(String::from_utf8_lossy(bytes).into_owned()))
            } else {
                serde_json::from_slice(bytes).map_err(|error| error.to_string())
            }
        }
        Some(Payload::Logical(value)) => {
            serde_json::to_value(value).map_err(|error| error.to_string())
        }
        Some(Payload::LogicalUnavailable { .. }) => Ok(Value::Null),
        None => Err("query did not return record data".into()),
    }
}

pub fn hit_value(hit: &SearchHit) -> Result<Value, String> {
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
        "record": record(hit)?,
    }))
}

pub fn one(hit: &SearchHit, path: &Path) -> Result<(), String> {
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

pub(super) fn all(
    repo: &Repository,
    mut query: Query,
    source: &settings::Query,
    path: &Path,
    limit: Option<usize>,
) -> Result<usize, String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(file);
    let outcome = (|| -> Result<usize, String> {
        let source = serde_json::to_string(&json!({
            "search": source.search,
            "filter": source.filter,
            "collection": source.collection,
            "sort": query.options.sort,
        }))
        .map_err(|error| error.to_string())?;
        write!(
            writer,
            "{{\n  \"kind\": \"search\",\n  \"query\": {source},\n  \"hits\": [\n"
        )
        .map_err(|error| error.to_string())?;
        let mut offset = query.options.offset;
        let start = offset;
        let mut expected = None;
        loop {
            query.options.offset = offset;
            query.options.limit =
                limit.map_or(100, |limit| 100.min(limit.saturating_sub(offset - start)));
            if query.options.limit == 0 {
                break;
            }
            let page = repo.search(&query)?;
            let total = page.count.ok_or("export requires an exact count")?;
            if expected.is_some_and(|count| count != total) {
                return Err("result count changed during export".into());
            }
            expected = Some(total);
            if page.hits.is_empty() && offset < total {
                return Err("query returned an incomplete export page".into());
            }
            for hit in &page.hits {
                if offset > start {
                    writer
                        .write_all(b",\n")
                        .map_err(|error| error.to_string())?;
                }
                serde_json::to_writer(&mut writer, &hit_value(hit)?)
                    .map_err(|error| error.to_string())?;
                offset += 1;
            }
            if offset >= total || limit.is_some_and(|limit| offset - start >= limit) {
                break;
            }
        }
        let exported = offset - start;
        write!(writer, "\n  ],\n  \"total\": {exported}\n}}\n")
            .map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())?;
        Ok(exported)
    })();
    drop(writer);
    if outcome.is_err() {
        let _ = fs::remove_file(path);
    }
    outcome
}

#[cfg(test)]
#[path = "export.test.rs"]
mod tests;
