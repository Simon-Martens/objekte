//! One repository opening and sequential, ordered query execution.

use std::io::{self, BufWriter, IsTerminal, Write};
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::discovery::SymlinkPolicy;
use objekte_search_core::inference::{InferenceConfig, InferenceIndexStatus, InferenceMode};
use objekte_search_core::inventory::RecordClass;
use objekte_search_core::path_repr;
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotAction, SnapshotMode,
    SnapshotOpenPolicy,
};
use objekte_search_core::schema_adapter::{CompiledSchema, compile_schema};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::streaming::{
    CancellationToken, ResultScope, StreamControl, StreamEvent, StreamMode, StreamOptions,
    StreamOutcome,
};
use objekte_search_core::workload::BigramAlphabet;
use serde_json::json;

use objekte_cli::arguments::{Alphabet, Arguments, ResolvedPaths};
use objekte_cli::output::EventWriter;
use objekte_cli::query_input::{QueryInput, QueryKind, QuerySource};

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
        let schema = load_schema_configuration(&paths)?;
        let definition = make_definition(args, &paths, schema.map(|(compiled, _)| compiled));
        let options = repository_options(args);
        let policy = snapshot_policy(args, &paths);
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
            definition,
            options,
            policy,
            initial_search,
            initial_filter,
        ))
    })();
    let (root, definition, options, policy, initial_search, initial_filter) = match configured {
        Ok(configured) => configured,
        Err(error) => {
            eprintln!("obj: {error}");
            return Exit::Arguments;
        }
    };
    let ui = objekte_tui::TuiOptions {
        single_collection: true,
        initial_search,
        initial_filter,
        offset: args.offset,
        limit: args.limit,
        interruption: Some(token.clone()),
        loading_label: Some(
            match (args.load_index.is_some(), args.write_index.is_some()) {
                (true, false) => "Loading search index",
                (true, true) => "Loading or building search index",
                _ => "Building search index",
            }
            .to_owned(),
        ),
    };
    let result = objekte_tui::run_with_loader(
        move || Repository::open_with_snapshot_policy(&root, vec![definition], options, policy),
        ui,
    );
    match result {
        Ok(()) => Exit::Success,
        Err(error)
            if error
                .downcast_ref::<io::Error>()
                .is_some_and(|error| error.kind() == io::ErrorKind::Interrupted) =>
        {
            Exit::Cancelled
        }
        Err(error) => {
            eprintln!("obj: {error}");
            Exit::Operation
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

fn load_schema_configuration(
    paths: &ResolvedPaths,
) -> Result<Option<(CompiledSchema, bool)>, String> {
    let Some(path) = paths.schema_file.as_deref() else {
        return Ok(None);
    };
    let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    let mut compiled = compile_schema(&document, "files")
        .map_err(|error| format!("{}: {error}", path.display()))?;
    for relation in compiled.schema.relations().values() {
        if relation.target_collection != "files" {
            return Err(format!(
                "{}: relation target {} is unavailable in the single-collection CLI",
                path.display(),
                relation.target_collection
            ));
        }
    }
    compiled.schema.set_source_fingerprint(&bytes);
    Ok(Some((compiled, document.get("x-objekte").is_some())))
}

fn make_definition(
    args: &Arguments,
    paths: &ResolvedPaths,
    schema: Option<CompiledSchema>,
) -> CollectionDefinition {
    let mut definition =
        CollectionDefinition::folder("files", PathBuf::new(), SourceFormat::JsonDecodedScalars);
    if let Some(compiled) = schema {
        definition.schema = compiled.schema;
        definition.sort_fields = compiled.sort_fields;
    }
    definition.discovery.recursive = !args.no_recurse;
    definition.discovery.hidden = true;
    definition.discovery.symlinks = SymlinkPolicy::Skip;
    definition.discovery.exclude = vec![".git".into(), "**/.git".into()];
    definition.discovery.exclude_directories = vec![PathBuf::from(".objekte")];
    if let Some(schema_file) = paths.schema_file.as_deref()
        && let Ok(relative) = schema_file.strip_prefix(&paths.root)
    {
        definition.discovery.exclude_files.push(relative.to_owned());
    }
    if let Some(directory) = paths.index_directory.as_deref()
        && let Ok(relative) = directory.strip_prefix(&paths.root)
        && !relative.as_os_str().is_empty()
    {
        definition
            .discovery
            .exclude_directories
            .push(relative.to_owned());
    }
    definition
}

fn repository_options(args: &Arguments) -> RepositoryOptions {
    RepositoryOptions {
        bigram_alphabet: match args.index_alphabet {
            Alphabet::Ascii => BigramAlphabet::Ascii,
            Alphabet::Unicode => BigramAlphabet::Unicode,
        },
        inference: InferenceConfig {
            mode: if args.auto_index {
                InferenceMode::AutoIndex
            } else {
                InferenceMode::Disabled
            },
            ..Default::default()
        },
        ..RepositoryOptions::default()
    }
}

fn snapshot_policy(args: &Arguments, paths: &ResolvedPaths) -> SnapshotOpenPolicy {
    let mode = match (args.load_index.is_some(), args.write_index.is_some()) {
        (false, false) => SnapshotMode::Disabled,
        (true, false) => SnapshotMode::ReadOnly,
        (false, true) => SnapshotMode::Rebuild,
        (true, true) => SnapshotMode::ReadWrite,
    };
    SnapshotOpenPolicy::new(mode, paths.index_directory.clone())
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
    let schema_configuration = match load_schema_configuration(&paths) {
        Ok(configuration) => configuration,
        Err(message) => {
            eprintln!("obj: {message}");
            return Exit::Arguments;
        }
    };
    let schema_fields = schema_configuration
        .as_ref()
        .map_or(0, |(compiled, _)| compiled.schema.fields().len());
    let input_mode = if args.query_file.is_some() {
        "query_file"
    } else if args.fts.is_some() {
        "fts"
    } else if args.query.is_some() {
        "positional"
    } else {
        "index_only"
    };
    if let Err(error) = output.emit(json!({
        "type": "run_start",
        "executable_version": env!("CARGO_PKG_VERSION"),
        "core_model_version": objekte_search_core::MODEL_VERSION,
        "root": path_repr::query_path(&paths.root),
        "collection": "files",
        "recursive": !args.no_recurse,
        "hidden": true,
        "symlinks": "skip",
        "extension": ".json",
        "alphabet": match args.index_alphabet { Alphabet::Ascii => "ascii", Alphabet::Unicode => "unicode" },
        "text_policy": "decoded_scalar_values",
        "schema_fields": schema_fields,
        "schema_file": paths.schema_file.as_deref().map(path_repr::query_path),
        "schema_repository_metadata_ignored": schema_configuration.as_ref().is_some_and(|(_, ignored)| *ignored),
        "inference": if args.auto_index { "auto_index" } else { "disabled" },
        "input_mode": input_mode,
        "index_operation": index_operation(args),
        "index_directory": paths.index_directory.as_deref().map(path_repr::query_path),
    })) {
        return output_failure(error);
    }
    let definition = make_definition(
        args,
        &paths,
        schema_configuration.map(|(compiled, _)| compiled),
    );
    let options = repository_options(args);
    let policy = snapshot_policy(args, &paths);
    let open_started = Instant::now();
    let repo =
        match Repository::open_with_snapshot_policy(&paths.root, vec![definition], options, policy)
        {
            Ok(repo) => repo,
            Err(message) => {
                eprintln!("obj: {message}");
                if let Err(error) = output
                    .emit(
                        json!({"type":"run_error", "stage":"startup", "error":message,
                "elapsed_ns":started.elapsed().as_nanos()}),
                    )
                    .and_then(|_| output.flush())
                {
                    return output_failure(error);
                }
                return Exit::Arguments;
            }
        };
    let startup_ns = open_started.elapsed().as_nanos();
    let mut source = match QuerySource::open(args) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("obj: {message}");
            if let Err(error) = output
                .emit(
                    json!({"type":"run_error", "stage":"query_input", "error":message,
                "elapsed_ns":started.elapsed().as_nanos()}),
                )
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            return Exit::Operation;
        }
    };
    let mut successes = 0usize;
    let mut failures = 0usize;
    let mut query_id = 0u64;
    loop {
        if token.is_cancelled() {
            return cancel_run(&mut output, successes, failures, &started);
        }
        let input = match source.next_input() {
            Ok(Some(input)) => input,
            Ok(None) => break,
            Err(message) => {
                eprintln!("obj: {message}");
                if let Err(error) = output
                    .emit(json!({"type":"run_error", "stage":"query_input",
                    "error":message, "query_successes":successes, "query_errors":failures,
                    "elapsed_ns":started.elapsed().as_nanos()}))
                    .and_then(|_| output.flush())
                {
                    return output_failure(error);
                }
                return Exit::Operation;
            }
        };
        query_id += 1;
        let query_exit = execute_query(&repo, args, &input, query_id, &mut output, token);
        match query_exit {
            Exit::Success => successes += 1,
            Exit::Operation => failures += 1,
            Exit::Cancelled => {
                return cancel_run(&mut output, successes, failures, &started);
            }
            Exit::Arguments => return Exit::Arguments,
            Exit::PipeClosed => return Exit::PipeClosed,
            Exit::OutputError => return Exit::OutputError,
        }
    }
    if token.is_cancelled() {
        return cancel_run(&mut output, successes, failures, &started);
    }
    if source.is_file()
        && source.yielded() == 0
        && args.load_index.is_none()
        && args.write_index.is_none()
    {
        let message = "query file contains no queries";
        eprintln!("obj: {message}");
        if let Err(error) = output
            .emit(
                json!({"type":"run_error", "stage":"query_input", "error":message,
            "elapsed_ns":started.elapsed().as_nanos()}),
            )
            .and_then(|_| output.flush())
        {
            return output_failure(error);
        }
        return Exit::Operation;
    }
    let terminal = if failures == 0 {
        "run_complete"
    } else {
        "run_error"
    };
    let stats = repo.startup();
    let records: usize = repo
        .collections()
        .iter()
        .map(|item| item.generation.records.len())
        .sum();
    let fallback: usize = repo
        .collections()
        .iter()
        .flat_map(|item| &item.generation.records)
        .filter(|record| record.class == RecordClass::RawFallback)
        .count();
    let event = json!({
        "type": terminal,
        "query_successes": successes,
        "query_errors": failures,
        "files": repo.files().files.len(),
        "records": records,
        "fallback_records": fallback,
        "build_parses": repo.build_parses(),
        "build_failed_parses": repo.build_failed_parses(),
        "startup_ns": startup_ns,
        "build_ns": stats.build_ns,
        "discovery_ns": stats.build_discovery_ns,
        "extraction_index_ns": stats.build_extraction_index_ns,
        "finalization_ns": stats.build_finalization_ns,
        "snapshot_decode_ns": stats.snapshot_decode_ns,
        "reconciliation_ns": stats.reconciliation_ns,
        "snapshot_write_ns": stats.snapshot_write_ns,
        "snapshot_action": snapshot_action(stats.snapshot_action),
        "snapshot_restore_error": stats.snapshot_restore_error,
        "inference": if args.auto_index { "auto_index" } else { "disabled" },
        "automatic_index_summary": repo.inference_report("files").map(|report| json!({
            "admitted_paths": report.admitted_paths,
            "short_string_fields": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::Built && field.short_string_distinct_values > 0).count(),
            "short_string_complete_fields": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::Built && field.short_string_distinct_values > 0 && field.short_string_unresolved_records == 0).count(),
            "short_string_partial_fields": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::Built && field.short_string_distinct_values > 0 && field.short_string_unresolved_records > 0).count(),
            "budget_rejections": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::BudgetExceeded).count(),
            "incompatible_fields": report.fields.iter().filter(|field| field.index_status == InferenceIndexStatus::Incompatible).count(),
            "retained_bytes_estimate": report.retained_bytes_estimate,
        })),
        "elapsed_ns": started.elapsed().as_nanos(),
    });
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
        .emit(json!({"type":"run_cancelled", "query_successes":successes,
            "query_errors":failures, "elapsed_ns":started.elapsed().as_nanos()}))
        .and_then(|_| output.flush())
    {
        Ok(()) => Exit::Cancelled,
        Err(error) => output_failure(error),
    }
}

fn execute_query<W: Write>(
    repo: &Repository,
    args: &Arguments,
    input: &QueryInput,
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
        "kind": match input.kind { QueryKind::Fts => "fts", QueryKind::Structured => "structured" },
        "line": input.line, "scope":scope_json, "count_requested":args.count,
    })) {
        return output_failure(error);
    }
    let started = Instant::now();
    let preparation_started = Instant::now();
    let (query, decoded_fts) = match input.parse(args) {
        Ok(result) => result,
        Err(message) => {
            eprintln!("obj: query {query_id}: {message}");
            if let Err(error) = output
                .emit(json!({"type":"query_error", "query_id":query_id,
                "line":input.line, "error":message, "stage":"input",
                "emitted_rows":0, "observed_matches":0, "exact_total":null,
                "total_exact":false, "query_wall_ns":started.elapsed().as_nanos()}))
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            return Exit::Operation;
        }
    };
    let preparation_ns = preparation_started.elapsed().as_nanos();
    let mut emitted = 0usize;
    let mut first_hit_ns = None;
    let mut sink_error_kind = None;
    let sink_before = output.sink_ns();
    let search_started = Instant::now();
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
    let search_elapsed_ns = search_started.elapsed().as_nanos();
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
            let event = json!({
                "type":"query_complete", "query_id":query_id, "line":input.line,
                "decoded_fts":decoded_fts, "complete":true,
                "emitted_rows":emitted, "observed_matches":summary.progress.observed_matches,
                "exact_total":summary.count, "total_exact":summary.count.is_some(),
                "counters":summary.counters, "preparation_ns":preparation_ns,
                "search_ns":search_ns, "sink_ns":sink_ns,
                "query_wall_ns":started.elapsed().as_nanos(), "first_hit_ns":first_hit_ns,
            });
            if let Err(error) = output.emit(event).and_then(|_| output.flush()) {
                return output_failure(error);
            }
            Exit::Success
        }
        Ok(StreamOutcome::Cancelled(progress)) => {
            if let Err(error) = output
                .emit(json!({"type":"query_cancelled", "query_id":query_id,
                "line":input.line, "emitted_rows":emitted,
                "observed_matches":progress.observed_matches, "exact_total":null,
                "total_exact":false, "preparation_ns":preparation_ns,
                "search_ns":search_ns, "sink_ns":sink_ns,
                "query_wall_ns":started.elapsed().as_nanos()}))
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
                .emit(json!({"type":"query_error", "query_id":query_id,
                "line":input.line, "error":error.to_string(), "stage":"execution",
                "emitted_rows":emitted, "observed_matches":progress.observed_matches,
                "processed_candidates":progress.processed_candidates,
                "progress_final":false, "exact_total":null, "total_exact":false,
                "preparation_ns":preparation_ns, "search_ns":search_ns, "sink_ns":sink_ns,
                "query_wall_ns":started.elapsed().as_nanos()}))
                .and_then(|_| output.flush())
            {
                return output_failure(write_error);
            }
            Exit::Operation
        }
    }
}

fn index_operation(args: &Arguments) -> &'static str {
    match (args.load_index.is_some(), args.write_index.is_some()) {
        (false, false) => "memory_only",
        (true, false) => "load_only",
        (false, true) => "write_only",
        (true, true) => "load_and_write",
    }
}

fn snapshot_action(action: SnapshotAction) -> &'static str {
    match action {
        SnapshotAction::BuiltInMemory => "built_in_memory",
        SnapshotAction::BuiltAndWritten => "built_and_written",
        SnapshotAction::Restored => "restored",
        SnapshotAction::RebuiltAndWritten => "rebuilt_and_written",
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
