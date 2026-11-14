//! One repository opening and sequential, ordered query execution.

#[cfg(feature = "tui")]
use std::io::IsTerminal;
use std::io::{self, BufWriter, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use objekte_search_core::index_storage::{IndexFlushReport, IndexReadiness, IndexStorageTarget};
use objekte_search_core::inference::InferenceIndexStatus;
use objekte_search_core::inventory::RecordClass;
use objekte_search_core::lifecycle::{Index, IndexRevision, IndexView};
use objekte_search_core::lifecycle_caller::IndexedRepository;
use objekte_search_core::path_repr;
use objekte_search_core::query::Query;
use objekte_search_core::streaming::{
    CancellationToken, ResultScope, StreamControl, StreamEvent, StreamMode, StreamOptions,
    StreamOutcome,
};
use serde_json::{Value, json};

use crate::arguments::{Alphabet, Arguments, IndexTargetChoice, ResolvedPaths};
use crate::loader::{ConfiguredDefinitions, configure_definitions, repository_options};
use crate::output::{
    EventWriter, counters_with_ms, detailed_elapsed_ns, detailed_start, milliseconds_value as ms,
};
use crate::query_input::{QueryInput, QueryKind, QuerySource};

#[cfg(feature = "tui")]
pub fn run_tui(args: &Arguments, token: &CancellationToken) -> Exit {
    if let Err(error) = args.validate_input() {
        eprintln!("obj: {error}");
        return Exit::Arguments;
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        eprintln!("obj: --tui requires an interactive input and output terminal");
        return Exit::Arguments;
    }
    let configured = (|| {
        let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
        let paths = args.resolve_paths(&cwd)?;
        let load_target = args.resolved_load_target(&paths)?;
        let flush_target = args.resolved_flush_target(&paths)?;
        let configured = configure_definitions(args, &paths)?;
        let options = repository_options(args);
        let mut source = QuerySource::open(args)?;
        let (initial_search, initial_filter) = match source.next_input()? {
            Some(input) => {
                let (_, decoded) = input.parse(args)?;
                match input.kind {
                    QueryKind::Fts => (decoded.unwrap_or_default(), String::new()),
                    QueryKind::Structured => (String::new(), input.original),
                }
            }
            None => (String::new(), String::new()),
        };
        Ok::<_, String>((
            paths.root,
            configured,
            options,
            load_target,
            flush_target,
            initial_search,
            initial_filter,
        ))
    })();
    let (root, configured, options, load_target, mut startup_flush, mut search, mut filter) =
        match configured {
            Ok(configured) => configured,
            Err(error) => {
                eprintln!("obj: {error}");
                return Exit::Arguments;
            }
        };
    let mut pending_repository = None;
    let mut first_open = true;
    loop {
        let ui = crate::tui::TuiOptions {
            single_collection: !args.multiple_collections(),
            initial_search: search,
            initial_filter: filter,
            offset: args.offset,
            limit: args.limit,
            interruption: Some(token.clone()),
            load_context: Some(crate::tui::TuiLoadContext {
                root: root.clone(),
                definitions: configured.definitions.clone(),
                options,
            }),
            startup_flush: startup_flush.take(),
            loading_label: Some(
                if first_open && load_target.is_some() {
                    "Opening saved index"
                } else {
                    "Building search index"
                }
                .to_owned(),
            ),
        };
        let opening_root = root.clone();
        let definitions = configured.definitions.clone();
        let opening_target = if first_open {
            load_target.clone()
        } else {
            None
        };
        let ready = pending_repository.take();
        first_open = false;
        let result = crate::tui::run_with_opening_loader(
            move || {
                if let Some(repository) = ready {
                    return Ok(crate::tui::TuiOpening::Ready(repository));
                }
                if let Some(target) = opening_target {
                    match Index::try_load(&opening_root, definitions.clone(), options, &target)
                        .map_err(|error| crate::storage_view::format_index_error(&error))?
                    {
                        Some(index) => {
                            return IndexedRepository::new(index)
                                .map(crate::tui::TuiOpening::Ready)
                                .map_err(|error| error.to_string());
                        }
                        None => {}
                    }
                }
                Index::start_incremental_build(&opening_root, definitions, options)
                    .map(crate::tui::TuiOpening::Initial)
                    .map_err(|error| error.to_string())
            },
            ui,
        );
        match result {
            Ok(Some(request)) => {
                pending_repository = Some(request.repository);
                search = request.search;
                filter = request.filter;
            }
            Ok(None) => return Exit::Success,
            Err(error)
                if error
                    .downcast_ref::<io::Error>()
                    .is_some_and(|error| error.kind() == io::ErrorKind::Interrupted) =>
            {
                return Exit::Cancelled;
            }
            Err(error) => {
                eprintln!("obj: {error}");
                return Exit::Operation;
            }
        }
    }
}

/// Exit classifications specified by the `obj` command protocol.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Exit {
    Success = 0,
    Operation = 1,
    Arguments = 2,
    Cancelled = 130,
    PipeClosed = 254,
    OutputError = 255,
}

/// Metadata from the opening operation, reused by the desktop batch runner.
pub struct ExistingRun<'a> {
    pub args: &'a Arguments,
    pub paths: &'a ResolvedPaths,
    pub repository: &'a IndexedRepository,
    pub collections: &'a [String],
    pub schema_fields: usize,
    pub schema_files: &'a [PathBuf],
    pub ignored_repository_metadata: bool,
    pub startup_ns: u128,
}

/// A desktop query already parsed through the shared core query builder.
pub struct PreparedQuery {
    pub input: String,
    pub query: Result<Query, String>,
    pub decoded_fts: Option<String>,
}

/// Execute a CLI batch against a repository already opened by another frontend.
pub fn run_on_repository<W: Write>(
    run: ExistingRun<'_>,
    source: Result<QuerySource, String>,
    writer: W,
    token: &CancellationToken,
) -> Exit {
    let started = Instant::now();
    let mut output = EventWriter::new(BufWriter::with_capacity(64 * 1024, writer));
    if let Err(error) = output.emit(run_start_event(
        run.args,
        run.paths,
        run.collections,
        run.schema_fields,
        run.schema_files,
        run.ignored_repository_metadata,
    )) {
        return output_failure(error);
    }
    let source = match source {
        Ok(source) => source,
        Err(message) => {
            eprintln!("obj: {message}");
            if let Err(error) = output
                .emit(detailed_run_event(
                    json!({"type":"run_error", "stage":"query_input", "error":message}),
                    &started,
                ))
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            return Exit::Operation;
        }
    };
    run_queries(
        RunTail {
            repo: run.repository,
            args: run.args,
            started: &started,
            startup_ns: run.startup_ns,
            initial_incremental: false,
            flush_report: None,
        },
        source,
        &mut output,
        token,
    )
}

/// Emit one prepared desktop query through the same CLI event formatter and stream.
pub fn run_prepared_on_repository<W: Write>(
    run: ExistingRun<'_>,
    prepared: PreparedQuery,
    writer: W,
    token: &CancellationToken,
) -> Exit {
    let started = Instant::now();
    let mut output = EventWriter::new(BufWriter::with_capacity(64 * 1024, writer));
    let mut start = run_start_event(
        run.args,
        run.paths,
        run.collections,
        run.schema_fields,
        run.schema_files,
        run.ignored_repository_metadata,
    );
    start["input_mode"] = json!("combined");
    if let Err(error) = output.emit(start) {
        return output_failure(error);
    }
    let run = RunTail {
        repo: run.repository,
        args: run.args,
        started: &started,
        startup_ns: run.startup_ns,
        initial_incremental: false,
        flush_report: None,
    };
    if token.is_cancelled() {
        return cancel_run(&mut output, 0, 0, &started);
    }
    let input = QueryInput {
        original: prepared.input.clone(),
        line: None,
        kind: QueryKind::Structured,
    };
    match execute_query(
        run.repo,
        run.args,
        &input,
        Some(&prepared),
        1,
        &mut output,
        token,
    ) {
        Exit::Success => complete_run(run, 1, 0, &mut output),
        Exit::Operation => complete_run(run, 0, 1, &mut output),
        Exit::Cancelled => cancel_run(&mut output, 0, 0, &started),
        other => other,
    }
}

/// Run one command using the supplied output sink and cancellation flag.
pub fn run<W: Write>(args: &Arguments, writer: W, token: &CancellationToken) -> Exit {
    let started = Instant::now();
    let mut output = EventWriter::new(BufWriter::with_capacity(64 * 1024, writer));
    if let Err(message) = args.validate_input() {
        eprintln!("obj: {message}\nTry 'obj --help' for usage.");
        return Exit::Arguments;
    }
    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(error) => {
            eprintln!("obj: current directory: {error}");
            return Exit::Arguments;
        }
    };
    let paths = match args.resolve_paths(&cwd) {
        Ok(paths) => paths,
        Err(message) => {
            eprintln!("obj: {message}");
            return Exit::Arguments;
        }
    };
    let load_target = match args.resolved_load_target(&paths) {
        Ok(target) => target,
        Err(message) => {
            eprintln!("obj: {message}");
            return Exit::Arguments;
        }
    };
    let flush_target = match args.resolved_flush_target(&paths) {
        Ok(target) => target,
        Err(message) => {
            eprintln!("obj: {message}");
            return Exit::Arguments;
        }
    };
    let configured = match configure_definitions(args, &paths) {
        Ok(configuration) => configuration,
        Err(message) => {
            eprintln!("obj: {message}");
            return Exit::Arguments;
        }
    };
    let collection_names = configured
        .definitions
        .iter()
        .map(|definition| definition.name.clone())
        .collect::<Vec<_>>();
    let options = repository_options(args);
    // A fresh construction starts before opening query input. The input is
    // replayed after completion to preserve the ordered machine protocol.
    let mut source = None;
    let opened = if let Some(target) = load_target.as_ref() {
        match open_saved_repository(&paths, &configured, options, target, token) {
            Ok(Some(opened)) => Ok(opened),
            Ok(None) => {
                eprintln!("obj: no saved index at the selected location; building in memory");
                open_incremental_repository(args, &paths, &configured, options, &mut source, token)
            }
            Err(error) => Err(error),
        }
    } else {
        open_incremental_repository(args, &paths, &configured, options, &mut source, token)
    };
    if token.is_cancelled() {
        return cancel_run(&mut output, 0, 0, &started);
    }
    let (repo, startup_ns) = match opened {
        Ok(result) => result,
        Err(message) => {
            eprintln!("obj: {message}");
            if let Err(error) = output
                .emit(detailed_run_event(
                    json!({"type":"run_error", "stage":"startup", "error":message}),
                    &started,
                ))
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            return Exit::Operation;
        }
    };
    let flush_report = if let Some(target) = flush_target.as_ref() {
        match repo.index().flush_to(target) {
            Ok(report) => Some(report),
            Err(error) => {
                let message = crate::storage_view::format_index_error(&error);
                eprintln!("obj: save index: {message}");
                if let Err(output_error) = output
                    .emit(detailed_run_event(
                        json!({"type":"run_error", "stage":"index_flush", "error":message}),
                        &started,
                    ))
                    .and_then(|_| output.flush())
                {
                    return output_failure(output_error);
                }
                return Exit::Operation;
            }
        }
    } else {
        None
    };
    if let Err(error) = output.emit(run_start_event(
        args,
        &paths,
        &collection_names,
        configured.schema_fields,
        &configured.schema_files,
        configured.ignored_repository_metadata,
    )) {
        return output_failure(error);
    }
    let source = match source.unwrap_or_else(|| QuerySource::open(args)) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("obj: {message}");
            if let Err(error) = output
                .emit(detailed_run_event(
                    json!({"type":"run_error", "stage":"query_input", "error":message}),
                    &started,
                ))
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            return Exit::Operation;
        }
    };
    run_queries(
        RunTail {
            repo: &repo,
            args,
            started: &started,
            startup_ns,
            initial_incremental: !repo.index().restored_from_storage(),
            flush_report: flush_report.as_ref(),
        },
        source,
        &mut output,
        token,
    )
}

fn open_saved_repository(
    paths: &ResolvedPaths,
    configured: &ConfiguredDefinitions,
    options: objekte_search_core::repository::RepositoryOptions,
    target: &IndexStorageTarget,
    token: &CancellationToken,
) -> Result<Option<(IndexedRepository, u128)>, String> {
    let started = Instant::now();
    let Some(index) = Index::try_load(
        paths.root.clone(),
        configured.definitions.clone(),
        options,
        target,
    )
    .map_err(|error| crate::storage_view::format_index_error(&error))?
    else {
        return Ok(None);
    };
    if token.is_cancelled() {
        return Err("index opening cancelled".into());
    }
    if index
        .wait_for_reconciliation()
        .map_err(|error| error.to_string())?
        .is_some_and(|report| report.cancelled)
    {
        return Err("index reconciliation cancelled".into());
    }
    if token.is_cancelled() {
        return Err("index opening cancelled".into());
    }
    let repo = IndexedRepository::new(index).map_err(|error| error.to_string())?;
    Ok(Some((repo, started.elapsed().as_nanos())))
}

/// Work on a fresh index as batches become searchable, while retaining the
/// batch protocol's exact final query and snapshot behavior.
fn open_incremental_repository(
    args: &Arguments,
    paths: &ResolvedPaths,
    configured: &ConfiguredDefinitions,
    options: objekte_search_core::repository::RepositoryOptions,
    source: &mut Option<Result<QuerySource, String>>,
    token: &CancellationToken,
) -> Result<(IndexedRepository, u128), String> {
    let started = Instant::now();
    let task = Index::start_incremental_build(&paths.root, configured.definitions.clone(), options)
        .map_err(|error| error.to_string())?;
    *source = Some(QuerySource::open(args));
    let preview_allowed = args.query_file.as_deref().is_none_or(|path| {
        path != "-" && std::fs::metadata(path).is_ok_and(|metadata| metadata.file_type().is_file())
    });
    let preview_input = if preview_allowed {
        source
            .as_mut()
            .and_then(|source| source.as_mut().ok())
            .and_then(|source| source.peek_first_input().ok().flatten())
    } else {
        None
    };
    let preview_query = preview_input.and_then(|input| {
        input.parse(args).ok().map(|(mut query, _)| {
            query.options.offset = 0;
            query.options.limit = 100;
            query.options.count = false;
            query.options.return_mode = objekte_search_core::workload::ReturnMode::Locator;
            query.options.evidence = false;
            query
        })
    });
    let mut prior_view: Option<IndexView> = None;
    let mut preview_supported = true;
    let mut cursor = 0;
    loop {
        if token.is_cancelled() {
            task.cancel();
            return Err("index build cancelled".into());
        }
        let snapshot = task.snapshot().map_err(|error| error.to_string())?;
        if snapshot.cursor != cursor {
            cursor = snapshot.cursor;
            if let (Some(query), Some(view)) = (&preview_query, snapshot.view) {
                if snapshot.status.searchable_batches > 0
                    && !snapshot.status.construction_complete
                    && preview_supported
                {
                    let supported = if let Some(previous) = prior_view.as_ref() {
                        matches!(
                            view.search_added_since_with_cancellation(previous, query, token),
                            Ok(Some(_))
                        )
                    } else {
                        view.search_available_with_cancellation(query, token)
                            .is_ok()
                    };
                    if supported {
                        prior_view = Some(view);
                    } else {
                        preview_supported = false;
                    }
                }
            }
        }
        if snapshot.status.construction_complete {
            break;
        }
        if matches!(
            snapshot.status.phase,
            objekte_search_core::lifecycle::InitialBuildPhase::Failed
                | objekte_search_core::lifecycle::InitialBuildPhase::Cancelled
        ) {
            return Err(snapshot
                .status
                .error
                .unwrap_or_else(|| "index build stopped".into()));
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    let index = task.wait_complete().map_err(|error| error.to_string())?;
    let repo = IndexedRepository::new(index).map_err(|error| error.to_string())?;
    let startup_ns = started.elapsed().as_nanos();
    Ok((repo, startup_ns))
}

fn run_start_event(
    args: &Arguments,
    paths: &ResolvedPaths,
    collection_names: &[String],
    schema_fields: usize,
    schema_files: &[PathBuf],
    ignored_repository_metadata: bool,
) -> Value {
    let input_mode = if args.query_file.is_some() {
        "query_file"
    } else if args.fts.is_some() {
        "fts"
    } else if args.query.is_some() {
        "positional"
    } else {
        "index_only"
    };
    json!({
        "type": "run_start",
        "protocol_version": 2,
        "executable_version": env!("CARGO_PKG_VERSION"),
        "core_model_version": objekte_search_core::MODEL_VERSION,
        "root": path_repr::query_path(&paths.root),
        "collection": (!args.multiple_collections()).then_some("files"),
        "collections": collection_names,
        "recursive": if args.repo.is_some() { false } else { !args.no_recurse },
        "hidden": args.repo.is_none(),
        "symlinks": "skip",
        "extension": (args.repo.is_none()).then_some(".json"),
        "alphabet": match args.index_alphabet { Alphabet::Ascii => "ascii", Alphabet::Unicode => "unicode" },
        "text_policy": if args.repo.is_some() { "configured" } else { "decoded_scalar_values" },
        "schema_fields": schema_fields,
        "schema_file": paths.schema_file.as_deref().map(path_repr::query_path),
        "schema_files": schema_files.iter().map(|path| path_repr::query_path(path)).collect::<Vec<_>>(),
        "schema_repository_metadata_ignored": ignored_repository_metadata,
        "inference": if args.auto_index { "auto_index" } else { "disabled" },
        "input_mode": input_mode,
        "load_index": args.load_index.map(index_target_label),
        "load_index_directory": paths.load_index_directory.as_deref().map(path_repr::query_path),
        "flush_index": args.flush_index.map(index_target_label),
        "flush_index_directory": paths.flush_index_directory.as_deref().map(path_repr::query_path),
    })
}

#[derive(Clone, Copy)]
struct RunTail<'a> {
    repo: &'a IndexedRepository,
    args: &'a Arguments,
    started: &'a Instant,
    startup_ns: u128,
    initial_incremental: bool,
    flush_report: Option<&'a IndexFlushReport>,
}

fn run_queries<W: Write>(
    run: RunTail<'_>,
    mut source: QuerySource,
    output: &mut EventWriter<W>,
    token: &CancellationToken,
) -> Exit {
    let repo = run.repo;
    let args = run.args;
    let started = run.started;
    let mut successes = 0usize;
    let mut failures = 0usize;
    let mut query_id = 0u64;
    loop {
        if token.is_cancelled() {
            return cancel_run(output, successes, failures, started);
        }
        let input = match source.next_input() {
            Ok(Some(input)) => input,
            Ok(None) => break,
            Err(message) => {
                eprintln!("obj: {message}");
                if let Err(error) = output
                    .emit(detailed_run_event(
                        json!({"type":"run_error", "stage":"query_input",
                    "error":message, "query_successes":successes, "query_errors":failures}),
                        started,
                    ))
                    .and_then(|_| output.flush())
                {
                    return output_failure(error);
                }
                return Exit::Operation;
            }
        };
        query_id += 1;
        let query_exit = execute_query(repo, args, &input, None, query_id, output, token);
        match query_exit {
            Exit::Success => successes += 1,
            Exit::Operation => failures += 1,
            Exit::Cancelled => {
                return cancel_run(output, successes, failures, started);
            }
            Exit::Arguments => return Exit::Arguments,
            Exit::PipeClosed => return Exit::PipeClosed,
            Exit::OutputError => return Exit::OutputError,
        }
    }
    if token.is_cancelled() {
        return cancel_run(output, successes, failures, started);
    }
    if source.is_file()
        && source.yielded() == 0
        && args.load_index.is_none()
        && args.flush_index.is_none()
    {
        let message = "query file contains no queries";
        eprintln!("obj: {message}");
        if let Err(error) = output
            .emit(detailed_run_event(
                json!({"type":"run_error", "stage":"query_input", "error":message}),
                started,
            ))
            .and_then(|_| output.flush())
        {
            return output_failure(error);
        }
        return Exit::Operation;
    }
    complete_run(run, successes, failures, output)
}

fn complete_run<W: Write>(
    run: RunTail<'_>,
    successes: usize,
    failures: usize,
    output: &mut EventWriter<W>,
) -> Exit {
    let repo = run.repo;
    let args = run.args;
    let started = run.started;
    let startup_ns = run.startup_ns;
    let terminal = if failures == 0 {
        "run_complete"
    } else {
        "run_error"
    };
    #[cfg(feature = "detailed-profiling")]
    let stats = repo.startup();
    let index_timings_ms = repo.index().timings_ms().unwrap_or_default();
    let live = match repo.index().status() {
        Ok(status) => status,
        Err(error) => {
            let message = error.to_string();
            if let Err(error) = output
                .emit(detailed_run_event(
                    json!({"type":"run_error", "stage":"index_status", "error":message}),
                    &started,
                ))
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            return Exit::Operation;
        }
    };
    let storage = match repo.index().storage_status() {
        Ok(status) => status,
        Err(error) => {
            let message = error.to_string();
            if let Err(error) = output
                .emit(detailed_run_event(
                    json!({"type":"run_error", "stage":"index_storage_status", "error":message}),
                    &started,
                ))
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            return Exit::Operation;
        }
    };
    let base_fallback: usize = repo
        .collections()
        .iter()
        .flat_map(|item| &item.generation.records)
        .filter(|record| record.class == RecordClass::RawFallback)
        .count();
    let automatic_index_paths = if args.diagnostic {
        repo.inference_index_diagnostics("files")
    } else {
        None
    };
    let mut event = json!({
        "type": terminal,
        "query_successes": successes,
        "query_errors": failures,
        "files": live.indexed_files,
        "records": live.live_records,
        "index_revision": live.revision.0,
        "fallback_records": (run.initial_incremental || live.revision == IndexRevision::INITIAL)
            .then_some(live.fallback_records),
        "base_fallback_records": base_fallback,
        "build_parses": repo.build_parses(),
        "build_failed_parses": repo.build_failed_parses(),
        "index_timings_ms": index_timings_ms,
        "protocol_version": 2,
        "index_action": if repo.index().restored_from_storage() { "loaded_saved_index" } else { "built_in_memory" },
        "index_flush_report": run.flush_report.map(|report| json!({
            "saved_revision": report.saved_revision.0,
            "store_generation": report.store_generation,
            "destination": path_repr::query_path(&report.managed_directory),
            "unchanged": report.unchanged,
            "newer_changes_unsaved": report.newer_changes_unsaved,
            "segment_bytes_written": report.segment_bytes_written,
            "segments_reused": report.segments_reused,
        })),
        "index_persistence": {
            "readiness": match storage.readiness {
                IndexReadiness::Building => "building",
                _ => "searchable",
            },
            "source_reconciliation_complete": storage.source_reconciliation_complete,
            "current_revision": live.revision.0,
            "last_saved_revision": live.saved_revision.map(|revision| revision.0),
            "last_store_generation": storage.durable_generation,
            "loaded_origin": repo.index().loaded_origin().ok().flatten().as_deref().map(path_repr::query_path),
            "last_save_directory": storage.managed_directory.as_deref().map(path_repr::query_path),
            "last_error": storage.last_error,
            "last_error_kind": storage.last_error_kind.map(|kind| kind.as_str()),
            "segments_full_validated": storage.segments_full_validated,
            "segments_stamp_trusted": storage.segments_stamp_trusted,
            "validation_stamp_error": storage.validation_stamp_error,
            "validation_stamp_error_kind": storage.validation_stamp_error_kind.map(|kind| kind.as_str()),
        },
        "inference": if args.auto_index { "auto_index" } else { "disabled" },
        "automatic_index_summary": repo.inference_report("files").map(|report| json!({
            "complete": report.complete,
            "observed_paths": report.observed_paths,
            "omitted_paths": report.omitted_paths,
            "policy_version": report.policy.version,
            "max_batch_bytes": report.policy.max_batch_bytes,
            "max_total_bytes": report.policy.max_total_bytes,
            "max_repository_bytes": report.policy.max_repository_bytes,
            "max_short_string_index_bytes_per_path": report.policy.max_short_string_index_bytes_per_path,
            "admitted_paths": report.admitted_paths,
            "short_string_fields": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::Built && field.short_string_distinct_values > 0).count(),
            "short_string_complete_fields": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::Built && field.short_string_distinct_values > 0 && field.short_string_unresolved_records == 0).count(),
            "short_string_partial_fields": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::Built && field.short_string_distinct_values > 0 && field.short_string_unresolved_records > 0).count(),
            "budget_rejections": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::BudgetExceeded).count(),
            "incompatible_fields": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::Incompatible).count(),
            "retained_bytes_estimate": report.retained_bytes_estimate,
        })),
    });
    #[cfg(feature = "detailed-profiling")]
    {
        event["startup_ms"] = ms(startup_ns);
        event["discovery_ms"] = ms(stats.build_discovery_ns as u128);
        event["extraction_index_ms"] = ms(stats.build_extraction_index_ns as u128);
        event["finalization_ms"] = ms(stats.build_finalization_ns as u128);
        event["run_elapsed_ms"] = ms(started.elapsed().as_nanos());
    }

    #[cfg(not(feature = "detailed-profiling"))]
    let _ = (startup_ns, started);
    if let Some(paths) = automatic_index_paths {
        event["automatic_index_paths"] = json!(paths);
    }
    if args.multiple_collections() && args.diagnostic {
        let collections = repo
            .collections()
            .iter()
            .filter_map(|collection| {
                let name = &collection.definition.name;
                let report = repo.inference_report(name)?;
                Some((
                    name.clone(),
                    json!({
                        "complete": report.complete,
                        "observed_paths": report.observed_paths,
                        "admitted_paths": report.admitted_paths,
                        "paths": repo.inference_index_diagnostics(name),
                    }),
                ))
            })
            .collect::<std::collections::BTreeMap<_, _>>();
        event["automatic_index_collections"] = json!(collections);
    }

    if let Err(error) = output.emit(event).and_then(|_| output.flush()) {
        return output_failure(error);
    }
    if failures == 0 {
        Exit::Success
    } else {
        Exit::Operation
    }
}

fn cancel_run<W: Write>(
    output: &mut EventWriter<W>,
    successes: usize,
    failures: usize,
    started: &Instant,
) -> Exit {
    match output
        .emit(detailed_run_event(
            json!({"type":"run_cancelled", "query_successes":successes,
            "query_errors":failures}),
            started,
        ))
        .and_then(|_| output.flush())
    {
        Ok(()) => Exit::Cancelled,
        Err(error) => output_failure(error),
    }
}

fn execute_query<W: Write>(
    repo: &IndexedRepository,
    args: &Arguments,
    input: &QueryInput,
    prepared: Option<&PreparedQuery>,
    query_id: u64,
    output: &mut EventWriter<W>,
    token: &CancellationToken,
) -> Exit {
    let scope = match args.limit {
        Some(limit) => ResultScope::Page {
            offset: args.offset,
            limit,
        },
        None => ResultScope::All {
            offset: args.offset,
        },
    };
    let scope_json = match args.limit {
        Some(limit) => json!({"type":"page", "offset":args.offset, "limit":limit}),
        None => json!({"type":"all", "offset":args.offset}),
    };
    if let Err(error) = output.emit(json!({
        "type":"query_start", "query_id":query_id, "input":input.original,
        "kind": if prepared.is_some() { "combined" } else { match input.kind { QueryKind::Fts => "fts", QueryKind::Structured => "structured" } },
        "line": input.line, "scope":scope_json, "count_requested":args.count,
    })) {
        return output_failure(error);
    }
    let started = Instant::now();
    let preparation_started = detailed_start();
    let parsed = match prepared {
        Some(prepared) => prepared
            .query
            .clone()
            .map(|query| (query, prepared.decoded_fts.clone())),
        None => input.parse(args),
    };
    let (query, decoded_fts) = match parsed {
        Ok(result) => result,
        Err(message) => {
            eprintln!("obj: query {query_id}: {message}");
            if let Err(error) = output
                .emit(json!({"type":"query_error", "query_id":query_id,
                "line":input.line, "error":message, "stage":"input",
                "emitted_rows":0, "observed_matches":0, "exact_total":null,
                "total_exact":false, "first_result_ms":null,
                "all_results_ms":ms(started.elapsed().as_nanos())}))
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            return Exit::Operation;
        }
    };
    let preparation_ns = detailed_elapsed_ns(preparation_started);
    let mut emitted = 0usize;
    let mut first_hit_ns = None;
    let mut sink_error_kind = None;
    let sink_before = output.sink_ns();
    let search_started = detailed_start();
    let mut sink = |event: StreamEvent<'_>| -> Result<StreamControl, String> {
        match event {
            StreamEvent::OrderedRows { rows, .. } => {
                for hit in rows {
                    if first_hit_ns.is_none() {
                        first_hit_ns = Some(started.elapsed().as_nanos());
                    }
                    if let Err(error) = output.emit_hit(repo, hit, query_id, emitted, args.offset) {
                        sink_error_kind = Some(error.kind());
                        return Err(error.to_string());
                    }
                    emitted += 1;
                }
                if let Err(error) = output.flush() {
                    sink_error_kind = Some(error.kind());
                    return Err(error.to_string());
                }
            }
            StreamEvent::PageReplacement { .. } => {
                return Err("unexpected replacement event".into());
            }
            StreamEvent::Progress { .. } => {}
        }
        Ok(StreamControl::Continue)
    };
    let result = repo.search_stream_scoped(
        &query,
        scope,
        StreamOptions {
            mode: StreamMode::OrderedRows,
            ..StreamOptions::default()
        },
        token,
        &mut sink,
    );
    let search_elapsed_ns = detailed_elapsed_ns(search_started);
    let sink_ns = output.sink_ns().saturating_sub(sink_before);
    let search_ns = search_elapsed_ns.saturating_sub(sink_ns);
    if sink_error_kind == Some(io::ErrorKind::BrokenPipe) {
        token.cancel();
        return Exit::PipeClosed;
    }
    if let Some(kind) = sink_error_kind {
        eprintln!("obj: output failed during query {query_id}: {kind:?}");
        return Exit::OutputError;
    }
    if let Err(error) = output.flush() {
        return output_failure(error);
    }
    match result {
        Ok(StreamOutcome::Complete(summary)) => {
            if summary.counters.source_unavailable_events > 0 {
                let message = format!(
                    "{} source records could not be checked; query output is incomplete",
                    summary.counters.source_unavailable_events
                );
                if let Err(error) = output
                    .emit(detailed_query_event(
                        json!({
                            "type":"query_error", "query_id":query_id, "line":input.line,
                            "error":message, "stage":"source", "partial":true,
                            "emitted_rows":emitted,
                            "observed_matches":summary.progress.observed_matches,
                            "exact_total":null, "total_exact":false,
                            "counters":counters_with_ms(&summary.counters),
                            "first_result_ms":first_hit_ns.map(ms),
                            "all_results_ms":ms(started.elapsed().as_nanos()),
                        }),
                        preparation_ns,
                        search_ns,
                        sink_ns,
                    ))
                    .and_then(|_| output.flush())
                {
                    return output_failure(error);
                }
                return Exit::Operation;
            }
            let event = detailed_query_event(
                json!({
                    "type":"query_complete", "query_id":query_id, "line":input.line,
                    "decoded_fts":decoded_fts, "complete":true,
                    "emitted_rows":emitted, "observed_matches":summary.progress.observed_matches,
                    "exact_total":summary.count.filter(|_| summary.counters.match_count_complete),
                    "total_exact":summary.count.is_some() && summary.counters.match_count_complete,
                    "counters":counters_with_ms(&summary.counters),
                    "all_results_ms":ms(started.elapsed().as_nanos()), "first_result_ms":first_hit_ns.map(ms),
                }),
                preparation_ns,
                search_ns,
                sink_ns,
            );
            if let Err(error) = output.emit(event).and_then(|_| output.flush()) {
                return output_failure(error);
            }
            Exit::Success
        }
        Ok(StreamOutcome::Cancelled(progress)) => {
            if let Err(error) = output
                .emit(detailed_query_event(
                    json!({"type":"query_cancelled", "query_id":query_id,
                "line":input.line, "emitted_rows":emitted,
                "observed_matches":progress.observed_matches, "exact_total":null,
                "total_exact":false, "first_result_ms":first_hit_ns.map(ms),
                "all_results_ms":ms(started.elapsed().as_nanos())}),
                    preparation_ns,
                    search_ns,
                    sink_ns,
                ))
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            Exit::Cancelled
        }
        Err(error) => {
            eprintln!("obj: query {query_id}: {error}");
            let progress = match &error {
                objekte_search_core::streaming::StreamError::Execution { progress, .. }
                | objekte_search_core::streaming::StreamError::Sink { progress, .. }
                | objekte_search_core::streaming::StreamError::Resource { progress, .. } => {
                    progress
                }
            };
            if let Err(write_error) = output
                .emit(detailed_query_event(
                    json!({"type":"query_error", "query_id":query_id,
                "line":input.line, "error":error.to_string(), "stage":"execution",
                "emitted_rows":emitted, "observed_matches":progress.observed_matches,
                "processed_candidates":progress.processed_candidates,
                "progress_final":false, "exact_total":null, "total_exact":false,
                "first_result_ms":first_hit_ns.map(ms),
                "all_results_ms":ms(started.elapsed().as_nanos())}),
                    preparation_ns,
                    search_ns,
                    sink_ns,
                ))
                .and_then(|_| output.flush())
            {
                return output_failure(write_error);
            }
            Exit::Operation
        }
    }
}

fn detailed_query_event(
    #[allow(unused_mut)] mut event: Value,
    preparation_ns: u128,
    search_ns: u128,
    sink_ns: u128,
) -> Value {
    #[cfg(feature = "detailed-profiling")]
    {
        let fields = event.as_object_mut().expect("query event is an object");
        fields.insert("preparation_ms".into(), ms(preparation_ns));
        fields.insert("search_ms".into(), ms(search_ns));
        fields.insert("sink_ms".into(), ms(sink_ns));
    }

    #[cfg(not(feature = "detailed-profiling"))]
    {
        let _ = (preparation_ns, search_ns, sink_ns);
    }
    event
}

fn detailed_run_event(#[allow(unused_mut)] mut event: Value, started: &Instant) -> Value {
    #[cfg(feature = "detailed-profiling")]
    {
        event["elapsed_ms"] = ms(started.elapsed().as_nanos());
    }

    #[cfg(not(feature = "detailed-profiling"))]
    {
        let _ = started;
    }
    event
}

fn index_target_label(choice: IndexTargetChoice) -> &'static str {
    match choice {
        IndexTargetChoice::Temporary => "temporary",
        IndexTargetChoice::Repository => "repository",
        IndexTargetChoice::Custom => "custom",
    }
}

fn output_failure(error: io::Error) -> Exit {
    if error.kind() == io::ErrorKind::BrokenPipe {
        Exit::PipeClosed
    } else {
        eprintln!("obj: output: {error}");
        Exit::OutputError
    }
}

#[cfg(test)]
#[path = "execution.test.rs"]
mod tests;
