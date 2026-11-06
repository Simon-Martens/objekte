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
            serde_json::from_slice(bytes).map_err(|error| error.to_string())
        }
        Some(Payload::Logical(value)) => {
            serde_json::to_value(value).map_err(|error| error.to_string())
        }
        None => Err("query did not return record data".into()),
    }
}

fn hit_value(hit: &SearchHit) -> Result<Value, String> {
    Ok(json!({
        "collection": hit.collection,
        "id": hit.record_id,
        "file": hit.file_path.to_string_lossy(),
        "byte_start": hit.byte_start,
        "byte_end": hit.byte_end,
        "matched_fields": hit.matched_fields,
        "record": record(hit)?,
    }))
}

pub(super) fn one(hit: &SearchHit, path: &Path) -> Result<(), String> {
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
        let mut offset = 0;
        let mut expected = None;
        loop {
            query.options.offset = offset;
            let page = repo.search(&query)?;
            if expected.is_some_and(|count| count != page.count) {
                return Err("result count changed during export".into());
            }
            expected = Some(page.count);
            if page.hits.is_empty() && offset < page.count {
                return Err("query returned an incomplete export page".into());
            }
            for hit in &page.hits {
                if offset > 0 {
                    writer
                        .write_all(b",\n")
                        .map_err(|error| error.to_string())?;
                }
                serde_json::to_writer(&mut writer, &hit_value(hit)?)
                    .map_err(|error| error.to_string())?;
                offset += 1;
            }
            if offset >= page.count {
                break;
            }
        }
        write!(writer, "\n  ],\n  \"total\": {offset}\n}}\n").map_err(|error| error.to_string())?;
        writer.flush().map_err(|error| error.to_string())?;
        Ok(offset)
    })();
    drop(writer);
    if outcome.is_err() {
        let _ = fs::remove_file(path);
    }
    outcome
}
