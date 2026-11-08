//! CLI-compatible batch execution over a retained repository.

use std::fs::{self, File, OpenOptions};
use std::path::{Path, PathBuf};

use objekte_cli::execution::{ExistingRun, Exit, run_on_repository};
use objekte_cli::query_input::QuerySource;
use objekte_search_core::streaming::CancellationToken;

use super::{BackendError, BackendErrorKind, OpenedSession};


/// Batch text already in memory, or a physical query file.
#[derive(Clone, Debug)]
pub enum BatchSource {
    DirectStructured(String),
    DirectFullText(String),
    Text(String),
    File(PathBuf),
    IndexOnly,
}


/// CLI page/count controls applied to every batch query.
#[derive(Clone, Debug)]
pub struct BatchSpec {
    pub source: BatchSource,
    pub offset: usize,
    pub limit: Option<usize>,
    pub count: bool,
}


/// Protocol exit status and the file containing the ordered NDJSON events.
#[derive(Clone, Debug)]
pub struct BatchReport {
    pub exit: Exit,
    pub path: PathBuf,
    pub bytes_written: u64,
}


/// Run every line through the CLI parser and event protocol without reopening data.
pub fn run_batch(
    session: &OpenedSession,
    spec: &BatchSpec,
    output_path: &Path,
    cancellation: &CancellationToken,
) -> Result<BatchReport, BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let mut args = session.args.clone();
    args.tui = false;
    args.query = None;
    args.fts = None;
    args.query_file = None;
    args.offset = spec.offset;
    args.limit = spec.limit;
    args.count = spec.count;
    let source = match &spec.source {
        BatchSource::DirectStructured(text) => {
            args.query = Some(text.clone());
            QuerySource::open(&args)
        }
        BatchSource::DirectFullText(text) => {
            args.fts = Some(text.clone());
            QuerySource::open(&args)
        }
        BatchSource::Text(text) => {
            args.query_file = Some("<pasted batch>".into());
            Ok(QuerySource::from_text(text.clone()))
        }
        BatchSource::File(path) => {
            args.query_file = Some(
                path.to_str()
                    .ok_or_else(|| {
                        BackendError::new(BackendErrorKind::Options, "query file path is not UTF-8")
                    })?
                    .to_owned(),
            );
            QuerySource::open(&args)
        }
        BatchSource::IndexOnly => {
            if args.load_index.is_none() && args.write_index.is_none() {
                return Err(BackendError::new(
                    BackendErrorKind::Options,
                    "index-only output requires an explicit load or write index operation",
                ));
            }
            QuerySource::open(&args)
        }
    };
    let file = create_output_file(session, output_path)?;
    let exit = run_on_repository(existing_run(session, &args), source, file, cancellation);
    finish_output(exit, output_path)
}

pub(super) fn existing_run<'a>(
    session: &'a OpenedSession,
    args: &'a objekte_cli::arguments::Arguments,
) -> ExistingRun<'a> {
    ExistingRun {
        args,
        paths: &session.paths,
        repository: &session.repository,
        collections: &session.collections,
        schema_fields: session.diagnostics.schema_fields,
        schema_files: &session.diagnostics.schema_files,
        ignored_repository_metadata: session.diagnostics.ignored_repository_metadata,
        startup_ns: session.diagnostics.open_ns,
        snapshot_save: session.diagnostics.snapshot_save.clone(),
    }
}

pub(super) fn finish_output(exit: Exit, output_path: &Path) -> Result<BatchReport, BackendError> {
    if matches!(exit, Exit::OutputError | Exit::PipeClosed) {
        let _ = fs::remove_file(output_path);
        return Err(BackendError::new(
            BackendErrorKind::Output,
            format!("batch output failed ({exit:?})"),
        ));
    }
    let bytes_written = fs::metadata(output_path)
        .map_err(|error| BackendError::new(BackendErrorKind::Output, error.to_string()))?
        .len();
    Ok(BatchReport {
        exit,
        path: output_path.to_path_buf(),
        bytes_written,
    })
}

/// Restrict output to an existing parent outside every scanned collection root.
pub(super) fn validate_output_path(
    session: &OpenedSession,
    output_path: &Path,
) -> Result<(), BackendError> {
    let parent = output_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or_else(|| {
            BackendError::new(
                BackendErrorKind::Options,
                "output path needs an existing parent directory",
            )
        })?;
    let parent = parent
        .canonicalize()
        .map_err(|error| BackendError::new(BackendErrorKind::Options, error.to_string()))?;
    let file_name = output_path.file_name().ok_or_else(|| {
        BackendError::new(BackendErrorKind::Options, "output path needs a file name")
    })?;
    let resolved = parent.join(file_name);
    if session.repository.collections().iter().any(|collection| {
        let source = session.paths.root.join(&collection.definition.source_path);
        let source = source.canonicalize().unwrap_or(source);
        resolved.starts_with(&source)
    }) {
        return Err(BackendError::new(
            BackendErrorKind::Options,
            "output must be outside scanned collection directories",
        ));
    }
    Ok(())
}

pub(super) fn create_output_file(
    session: &OpenedSession,
    output_path: &Path,
) -> Result<File, BackendError> {
    validate_output_path(session, output_path)?;
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output_path)
        .map_err(|error| BackendError::new(BackendErrorKind::Output, error.to_string()))
}

#[cfg(test)]
#[path = "batch.test.rs"]
mod tests;
