//! TUI-compatible selected and complete JSON export from a retained repository.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;

use objekte_search_core::lifecycle::IndexView;
use objekte_search_core::repository::SearchHit;
use objekte_search_core::run::DirectLookupResult;
use objekte_search_core::streaming::{
    CancellationToken, ResultScope, StreamControl, StreamEvent, StreamMode, StreamOptions,
    StreamOutcome,
};
use objekte_search_core::workload::ReturnMode;
use objekte_tui::export::{hit_value, one};
use serde_json::json;

use super::batch::{create_output_file, validate_output_path};
use super::{BackendError, BackendErrorKind, OpenedSession, QuerySpec, build_query};

/// Write one selected hit in the existing terminal export format.
pub fn export_selected_json(
    session: &OpenedSession,
    hit: &SearchHit,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<(), BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    validate_output_path(session, path)?;
    one(hit, path).map_err(|message| BackendError::new(BackendErrorKind::Output, message))
}

/// Re-resolve a record before a Source action, using its canonical identity
/// and the current pinned publication rather than a prior grid row or preview.
pub fn export_selected_json_on_view(
    session: &OpenedSession,
    view: &IndexView,
    collection: &str,
    id: &str,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<(), BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let hit = match view
        .get_record_available(collection, id, ReturnMode::Bytes)
        .map_err(|message| BackendError::new(BackendErrorKind::Query, message))?
    {
        DirectLookupResult::Found(hit) => hit,
        DirectLookupResult::NotFound => {
            return Err(BackendError::new(
                BackendErrorKind::Query,
                "record is no longer available",
            ));
        }
        DirectLookupResult::SourceUnavailable(diagnostic) => {
            return Err(BackendError::new(
                BackendErrorKind::Query,
                diagnostic.message,
            ));
        }
    };
    export_selected_json(session, &hit, path, cancellation)
}

/// Stream the requested result range once into the terminal search export format.
pub fn export_all_json(
    session: &OpenedSession,
    spec: &QuerySpec,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<usize, BackendError> {
    let view = session
        .repository
        .index()
        .view()
        .map_err(|error| BackendError::new(BackendErrorKind::Query, error.to_string()))?;
    export_all_json_on_view(session, &view, spec, path, cancellation)
}

/// Export the complete effective query from one immutable index publication.
pub fn export_all_json_on_view(
    session: &OpenedSession,
    view: &IndexView,
    spec: &QuerySpec,
    path: &Path,
    cancellation: &CancellationToken,
) -> Result<usize, BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    if let Some(scope) = &spec.scope
        && !session.collections.contains(scope)
    {
        return Err(BackendError::new(
            BackendErrorKind::Options,
            format!("collection {scope} is not open"),
        ));
    }
    let mut query =
        build_query(spec).map_err(|message| BackendError::new(BackendErrorKind::Query, message))?;
    query.options.count = false;
    let source = serde_json::to_string(&json!({
        "search": spec.full_text,
        "filter": spec.filter,
        "collection": spec.scope,
        "sort": query.options.sort,
    }))
    .map_err(|error| BackendError::new(BackendErrorKind::Output, error.to_string()))?;
    let file = create_output_file(session, path)?;
    let result = (|| -> Result<usize, BackendError> {
        let mut writer = BufWriter::new(file);
        write!(
            writer,
            "{{\n  \"kind\": \"search\",\n  \"query\": {source},\n  \"hits\": [\n"
        )
        .map_err(output_error)?;
        let mut exported = 0usize;
        if spec.limit != Some(0) {
            let scope = match spec.limit {
                Some(limit) => ResultScope::Page {
                    offset: spec.offset,
                    limit,
                },
                None => ResultScope::All {
                    offset: spec.offset,
                },
            };
            let mut sink_error = None;
            let result = view.search_stream_scoped(
                &query,
                scope,
                StreamOptions {
                    mode: StreamMode::OrderedRows,
                    ..StreamOptions::default()
                },
                cancellation,
                &mut |event| {
                    if let StreamEvent::OrderedRows { rows, .. } = event {
                        for hit in rows {
                            let outcome = (|| -> Result<(), String> {
                                if exported > 0 {
                                    writer
                                        .write_all(b",\n")
                                        .map_err(|error| error.to_string())?;
                                }
                                let value = hit_value(hit)?;
                                serde_json::to_writer(&mut writer, &value)
                                    .map_err(|error| error.to_string())?;
                                exported += 1;
                                Ok(())
                            })();
                            if let Err(message) = outcome {
                                sink_error = Some(message.clone());
                                return Err(message);
                            }
                        }
                    }
                    Ok(StreamControl::Continue)
                },
            );
            if let Some(message) = sink_error {
                return Err(BackendError::new(BackendErrorKind::Output, message));
            }
            match result {
                Ok(StreamOutcome::Complete(summary)) => {
                    if summary.counters.source_unavailable_events > 0
                        || (matches!(scope, ResultScope::All { .. })
                            && !summary.counters.match_count_complete)
                    {
                        return Err(BackendError::new(
                            BackendErrorKind::Query,
                            "some source records could not be checked; export is incomplete",
                        ));
                    }
                }
                Ok(StreamOutcome::Cancelled(_)) => return Err(BackendError::cancelled()),
                Err(error) => {
                    return Err(BackendError::new(
                        BackendErrorKind::Query,
                        error.to_string(),
                    ));
                }
            }
        }
        write!(writer, "\n  ],\n  \"total\": {exported}\n}}\n").map_err(output_error)?;
        writer.flush().map_err(output_error)?;
        Ok(exported)
    })();
    if result.is_err() {
        let _ = fs::remove_file(path);
    }
    result
}

fn output_error(error: std::io::Error) -> BackendError {
    BackendError::new(BackendErrorKind::Output, error.to_string())
}

#[cfg(test)]
#[path = "export.test.rs"]
mod tests;
