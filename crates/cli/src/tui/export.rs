//! JSON export of one hit or the complete query result stream.

use super::state::{App, Focus, Hit, Request};
use objekte_core::Error;
use serde_json::json;
use std::{
    fs::{self, OpenOptions},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

fn io_error(path: &Path, source: io::Error) -> Error {
    Error::Io {
        path: path.to_path_buf(),
        source,
    }
}

fn append_hit(
    writer: &mut BufWriter<fs::File>,
    path: &Path,
    first: &mut bool,
    hit: &serde_json::Value,
) -> objekte_core::Result<()> {
    if !*first {
        writer
            .write_all(b",\n")
            .map_err(|error| io_error(path, error))?;
    }
    *first = false;
    serde_json::to_writer(writer, hit).map_err(|error| io_error(path, io::Error::other(error)))
}

fn write_all(
    app: &mut App<'_>,
    path: &Path,
    request: &Request,
) -> Result<usize, Box<dyn std::error::Error>> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = BufWriter::new(file);
    let outcome = (|| -> Result<usize, Box<dyn std::error::Error>> {
        let whole = request.whole_result();
        let (kind, query) = match &whole {
            Request::Filter(query) => ("filter", serde_json::to_value(query)?),
            Request::Text(query) => ("full_text", serde_json::to_value(query)?),
        };
        write!(
            writer,
            "{{\n  \"kind\": {},\n  \"query\": {},\n  \"hits\": [\n",
            serde_json::to_string(kind)?,
            serde_json::to_string(&query)?
        )?;
        let mut first = true;
        let (total, truncated) = match &whole {
            Request::Filter(query) => {
                let summary = app.repo
                    .query_json_stream(query, |_, record| {
                        append_hit(&mut writer, path, &mut first, &json!({"record":record}))
                    })?;
                (summary.total, summary.truncated)
            }
            Request::Text(query) => {
                let summary = app.repo
                    .search_text_stream(query, |_, hit| {
                        append_hit(&mut writer, path, &mut first, &json!(hit))
                    })?;
                (summary.total, summary.truncated)
            }
        };
        let exported = total.unwrap_or(0);
        write!(writer, "\n  ],\n  \"total\": {exported},\n  \"truncated\": {truncated}\n}}\n")?;
        writer.flush()?;
        Ok(exported)
    })();
    drop(writer);
    if outcome.is_err() {
        let _ = fs::remove_file(path);
    }
    outcome
}

fn write_one(
    path: &Path,
    request: Option<&Request>,
    hit: &Hit,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = BufWriter::new(file);
    let result = (|| -> Result<(), Box<dyn std::error::Error>> {
        let query = match request {
            Some(Request::Filter(query)) => serde_json::to_value(query)?,
            Some(Request::Text(query)) => serde_json::to_value(query)?,
            None => serde_json::Value::Null,
        };
        let value = match (request, hit) {
            (None, _) => json!({"kind":"record","query":query,"hit":{"record":hit.record()}}),
            (Some(_), Hit::Filter(record)) => {
                json!({"kind":"filter","query":query,"hit":{"record":record}})
            }
            (Some(_), Hit::Text(hit)) => json!({"kind":"full_text","query":query,"hit":hit}),
        };
        serde_json::to_writer_pretty(&mut writer, &value)?;
        writer.write_all(b"\n")?;
        writer.flush()?;
        Ok(())
    })();
    drop(writer);
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}


impl App<'_> {
    pub(super) fn finish_export(&mut self) {
        let path = PathBuf::from(self.export_input.text.trim());
        if path.as_os_str().is_empty() {
            self.message = "Choose a JSON file path.".into();
            return;
        }
        let Some(request) = self.last_request.clone() else {
            self.message = "Run a query before exporting.".into();
            self.focus = self.export_return_focus;
            return;
        };
        let result = if self.focus == Focus::ExportAll {
            write_all(self, &path, &request)
                .map(|count| format!("Exported {count} hits to {}", path.display()))
        } else {
            let linked =
                self.export_return_focus == Focus::Detail && self.links.override_hit.is_some();
            let hit = if self.export_return_focus == Focus::Detail {
                self.current_detail_hit()
            } else {
                self.hits.get(self.selected)
            };
            hit.ok_or_else(|| "No selected hit".into())
                .and_then(|hit| write_one(&path, (!linked).then_some(&request), hit))
                .map(|()| format!("Exported hit to {}", path.display()))
        };
        match result {
            Ok(message) => {
                self.message = message;
                self.focus = self.export_return_focus;
            }
            Err(error) => self.message = format!("Export failed: {error}"),
        }
    }
}


#[cfg(test)]
#[path = "export.test.rs"]
mod tests;
