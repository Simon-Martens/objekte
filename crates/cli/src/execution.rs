//! One repository opening and sequential, ordered query execution.

use std::collections::HashSet;
use std::io::{self, BufWriter, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::discovery::SymlinkPolicy;
use objekte_search_core::inference::{InferenceConfig, InferenceIndexStatus, InferenceMode};
use objekte_search_core::inventory::RecordClass;
use objekte_search_core::path_repr;
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotAction,
    SnapshotMode, SnapshotOpenPolicy,
};
use objekte_search_core::schema_adapter::{CompiledSchema, compile_schema};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::storage::SnapshotSaveStats;
use objekte_search_core::streaming::{
    CancellationToken, ResultScope, StreamControl, StreamEvent, StreamMode, StreamOptions,
    StreamOutcome,
};
use objekte_search_core::workload::BigramAlphabet;
use serde_json::json;

use objekte_cli::arguments::{Alphabet, Arguments, ResolvedPaths};
use objekte_cli::output::{EventWriter, counters_with_ms, milliseconds_value as ms};
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
        let configured = configure_definitions(args, &paths)?;
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
            configured.definitions,
            options,
            policy,
            initial_search,
            initial_filter,
        ))
    })();
    let (root, definitions, options, policy, initial_search, initial_filter) = match configured {
        Ok(configured) => configured,
        Err(error) => {
            eprintln!("obj: {error}");
            return Exit::Arguments;
        }
    };
    let ui = objekte_cli::tui::TuiOptions {
        single_collection: args.repo.is_none(),
        initial_search,
        initial_filter,
        offset: args.offset,
        limit: args.limit,
        interruption: Some(token.clone()),
        snapshot_requested: args.write_index.is_some(),
        loading_label: Some(
            match (args.load_index.is_some(), args.write_index.is_some()) {
                (true, false) => "Loading search index",
                (true, true) => "Loading or building search index",
                _ => "Building search index",
            }
            .to_owned(),
        ),
    };
    let write = args.write_index.is_some();
    let result = objekte_cli::tui::run_with_loader(
        move || {
            open_repository(&root, definitions, options, policy, write).map(|(repo, _, _)| repo)
        },
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
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    omit_unavailable_relations(
        &mut document,
        &std::collections::HashSet::from(["files".to_owned()]),
    );
    let mut compiled = compile_schema(&document, "files")
        .map_err(|error| format!("{}: {error}", path.display()))?;
    compiled.schema.set_source_fingerprint(&bytes);
    Ok(Some((compiled, document.get("x-objekte").is_some())))
}

struct ConfiguredDefinitions {
    definitions: Vec<CollectionDefinition>,
    schema_files: Vec<PathBuf>,
    schema_fields: usize,
    ignored_repository_metadata: bool,
}

fn configure_definitions(
    args: &Arguments,
    paths: &ResolvedPaths,
) -> Result<ConfiguredDefinitions, String> {
    if args.repo.is_some() {
        return load_repository_definitions(&paths.root, paths.index_directory.as_deref());
    }
    let schema = load_schema_configuration(paths)?;
    let schema_fields = schema
        .as_ref()
        .map_or(0, |(compiled, _)| compiled.schema.fields().len());
    let ignored_repository_metadata = schema.as_ref().is_some_and(|(_, ignored)| *ignored);
    Ok(ConfiguredDefinitions {
        definitions: vec![make_definition(
            args,
            paths,
            schema.map(|(compiled, _)| compiled),
        )],
        schema_files: paths.schema_file.iter().cloned().collect(),
        schema_fields,
        ignored_repository_metadata,
    })
}

fn load_repository_definitions(
    root: &Path,
    index_directory: Option<&Path>,
) -> Result<ConfiguredDefinitions, String> {
    let directory = root.join(".objekte/collections");
    let mut schema_files = std::fs::read_dir(&directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    schema_files.retain(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.ends_with(".schema.json"))
    });
    schema_files.sort();
    if schema_files.is_empty() {
        return Err(format!(
            "{} contains no collection schemas",
            directory.display()
        ));
    }
    let mut documents = Vec::with_capacity(schema_files.len());
    let mut names = HashSet::new();
    for path in &schema_files {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        let document: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let metadata = document
            .get("x-objekte")
            .ok_or_else(|| format!("{}: missing x-objekte", path.display()))?;
        let name = repository_metadata(metadata, "name", path)?.to_owned();
        if !names.insert(name.clone()) {
            return Err(format!(
                "{}: duplicate collection name {name}",
                path.display()
            ));
        }
        documents.push((path.clone(), bytes, document, name));
    }
    let mut definitions = Vec::with_capacity(documents.len());
    let mut schema_fields = 0usize;
    for (path, bytes, mut document, name) in documents {
        omit_unavailable_relations(&mut document, &names);
        let metadata = document.get("x-objekte").unwrap();
        let source_path = PathBuf::from(repository_metadata(metadata, "path", &path)?);
        let format = match repository_metadata(metadata, "format", &path)? {
            "json" => SourceFormat::Json,
            "jsonl" => SourceFormat::JsonLines,
            "json_decoded_scalars" => SourceFormat::JsonDecodedScalars,
            other => {
                return Err(format!(
                    "{}: unsupported source format {other}",
                    path.display()
                ));
            }
        };
        let identity = match repository_metadata(metadata, "id", &path)? {
            "filename" => IdentityPolicy::Filename,
            "generated" => IdentityPolicy::Generated,
            "relative_path" => IdentityPolicy::RelativePath,
            other => {
                return Err(format!(
                    "{}: unsupported identity policy {other}",
                    path.display()
                ));
            }
        };
        let mut compiled = compile_schema(&document, &name)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        compiled.schema.set_source_fingerprint(&bytes);
        schema_fields = schema_fields.saturating_add(compiled.schema.fields().len());
        let mut definition = CollectionDefinition {
            name,
            source_path,
            format,
            identity,
            discovery: Default::default(),
            schema: compiled.schema,
            sort_fields: compiled.sort_fields,
            sort_locale: document
                .get("x-language")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("und")
                .to_owned(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        };
        definition.discovery.exclude = vec![".git".into(), "**/.git".into()];
        definition.discovery.exclude_directories = vec![PathBuf::from(".objekte")];
        if let Some(index_directory) = index_directory
            && let Ok(relative) = index_directory.strip_prefix(root.join(&definition.source_path))
            && !relative.as_os_str().is_empty()
        {
            definition
                .discovery
                .exclude_directories
                .push(relative.to_owned());
        }
        definitions.push(definition);
    }
    Ok(ConfiguredDefinitions {
        definitions,
        schema_files,
        schema_fields,
        ignored_repository_metadata: false,
    })
}

fn repository_metadata<'a>(
    metadata: &'a serde_json::Value,
    key: &str,
    path: &Path,
) -> Result<&'a str, String> {
    metadata
        .get(key)
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{}: missing x-objekte.{key}", path.display()))
}

fn omit_unavailable_relations(
    document: &mut serde_json::Value,
    collections: &std::collections::HashSet<String>,
) {
    match document {
        serde_json::Value::Object(fields) => {
            if fields
                .get("x-relation")
                .and_then(|relation| relation.get("collection"))
                .and_then(serde_json::Value::as_str)
                .is_some_and(|target| !collections.contains(target))
            {
                fields.remove("x-relation");
            }
            for value in fields.values_mut() {
                omit_unavailable_relations(value, collections);
            }
        }
        serde_json::Value::Array(values) => {
            for value in values {
                omit_unavailable_relations(value, collections);
            }
        }
        _ => {}
    }
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
        (true, false) => SnapshotMode::Required,
        (false, true) => SnapshotMode::Disabled,
        (true, true) => SnapshotMode::IfValid,
    };
    SnapshotOpenPolicy::new(mode, paths.index_directory.clone())
}

fn open_repository(
    root: &Path,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
    policy: SnapshotOpenPolicy,
    write_requested: bool,
) -> Result<(Repository, u128, Option<SnapshotSaveStats>), String> {
    let directory = policy
        .directory
        .clone()
        .unwrap_or_else(|| root.join(".objekte"));
    let open_started = Instant::now();
    let repo = Repository::open_with_snapshot_policy(root, definitions, options, policy)?;
    let open_ns = open_started.elapsed().as_nanos();
    if write_requested && !repo.startup().restored {
        let path =
            objekte_search_core::storage::snapshot_path_in(&directory, options.bigram_alphabet);
        let saved = repo.save_snapshot_with_stats(&path)?;
        return Ok((repo, open_ns, Some(saved)));
    }
    Ok((repo, open_ns, None))
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
        "collection": (args.repo.is_none()).then_some("files"),
        "collections": collection_names,
        "recursive": if args.repo.is_some() { false } else { !args.no_recurse },
        "hidden": args.repo.is_none(),
        "symlinks": "skip",
        "extension": (args.repo.is_none()).then_some(".json"),
        "alphabet": match args.index_alphabet { Alphabet::Ascii => "ascii", Alphabet::Unicode => "unicode" },
        "text_policy": if args.repo.is_some() { "configured" } else { "decoded_scalar_values" },
        "schema_fields": configured.schema_fields,
        "schema_file": paths.schema_file.as_deref().map(path_repr::query_path),
        "schema_files": configured.schema_files.iter().map(|path| path_repr::query_path(path)).collect::<Vec<_>>(),
        "schema_repository_metadata_ignored": configured.ignored_repository_metadata,
        "inference": if args.auto_index { "auto_index" } else { "disabled" },
        "input_mode": input_mode,
        "index_operation": index_operation(args),
        "index_directory": paths.index_directory.as_deref().map(path_repr::query_path),
    })) {
        return output_failure(error);
    }
    let options = repository_options(args);
    let policy = snapshot_policy(args, &paths);
    let (repo, startup_ns, snapshot_save) = match open_repository(
        &paths.root,
        configured.definitions,
        options,
        policy,
        args.write_index.is_some(),
    ) {
        Ok(result) => result,
        Err(message) => {
            eprintln!("obj: {message}");
            if let Err(error) = output
                .emit(
                    json!({"type":"run_error", "stage":"startup", "error":message,
                "elapsed_ms":ms(started.elapsed().as_nanos())}),
                )
                .and_then(|_| output.flush())
            {
                return output_failure(error);
            }
            return Exit::Arguments;
        }
    };
    let mut source = match QuerySource::open(args) {
        Ok(source) => source,
        Err(message) => {
            eprintln!("obj: {message}");
            if let Err(error) = output
                .emit(
                    json!({"type":"run_error", "stage":"query_input", "error":message,
                "elapsed_ms":ms(started.elapsed().as_nanos())}),
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
                    "elapsed_ms":ms(started.elapsed().as_nanos())}))
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
            "elapsed_ms":ms(started.elapsed().as_nanos())}),
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
    let automatic_index_paths = if args.diagnostic {
        repo.inference_index_diagnostics("files")
    } else {
        None
    };
    let mut event = json!({
        "type": terminal,
        "query_successes": successes,
        "query_errors": failures,
        "files": repo.files().files.len(),
        "records": records,
        "fallback_records": fallback,
        "build_parses": repo.build_parses(),
        "build_failed_parses": repo.build_failed_parses(),
        "startup_ms": ms(startup_ns),
        "build_ms": ms(stats.build_ns as u128),
        "discovery_ms": ms(stats.build_discovery_ns as u128),
        "extraction_index_ms": ms(stats.build_extraction_index_ns as u128),
        "finalization_ms": ms(stats.build_finalization_ns as u128),
        "snapshot_decode_ms": ms(stats.snapshot_decode_ns as u128),
        "reconciliation_ms": ms(stats.reconciliation_ns as u128),
        "snapshot_write_ms": ms(snapshot_save.as_ref().map_or(0, |save| save.total_ns) as u128),
        "snapshot_action": snapshot_action(stats.snapshot_action, snapshot_save.is_some(), stats.snapshot_restore_error.is_some()),
        "snapshot_restore_error": stats.snapshot_restore_error,
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
        "elapsed_ms": ms(started.elapsed().as_nanos()),
    });
    if let Some(paths) = automatic_index_paths {
        event["automatic_index_paths"] = json!(paths);
    }
    if args.repo.is_some() && args.diagnostic {
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
    if args.diagnostic
        && let Some(save) = &snapshot_save
    {
        event["snapshot_save_phases"] = json!({
            "total_ms": ms(save.total_ns as u128),
            "directory_ms": ms(save.directory_ns as u128),
            "header_ms": ms(save.header_ns as u128),
            "inventory_ms": ms(save.inventory_ns as u128),
            "write_ms": ms(save.write_ns as u128),
            "sync_ms": ms(save.sync_ns as u128),
            "rename_ms": ms(save.rename_ns as u128),
            "bytes_written": save.bytes_written,
            "sections": save.sections.iter().map(|section| json!({
                "id": section.id,
                "serialize_ms": ms(section.serialize_ns as u128),
                "checksum_ms": ms(section.checksum_ns as u128),
                "write_ms": ms(section.write_ns as u128),
                "payload_bytes": section.payload_bytes,
            })).collect::<Vec<_>>(),
        });
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
        .emit(json!({"type":"run_cancelled", "query_successes":successes,
            "query_errors":failures, "elapsed_ms":ms(started.elapsed().as_nanos())}))
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
                "total_exact":false, "query_wall_ms":ms(started.elapsed().as_nanos())}))
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
                "counters":counters_with_ms(&summary.counters), "preparation_ms":ms(preparation_ns),
                "search_ms":ms(search_ns), "sink_ms":ms(sink_ns),
                "query_wall_ms":ms(started.elapsed().as_nanos()), "first_hit_ms":first_hit_ns.map(ms),
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
                "total_exact":false, "preparation_ms":ms(preparation_ns),
                "search_ms":ms(search_ns), "sink_ms":ms(sink_ns),
                "query_wall_ms":ms(started.elapsed().as_nanos())}))
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
                "preparation_ms":ms(preparation_ns), "search_ms":ms(search_ns), "sink_ms":ms(sink_ns),
                "query_wall_ms":ms(started.elapsed().as_nanos())}))
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

fn snapshot_action(action: SnapshotAction, saved: bool, rebuilt: bool) -> &'static str {
    match (action, saved, rebuilt) {
        (SnapshotAction::Restored, _, _) => "restored",
        (SnapshotAction::BuiltInMemory, true, true) => "rebuilt_and_written",
        (SnapshotAction::BuiltInMemory, true, false) => "built_and_written",
        (SnapshotAction::BuiltInMemory, false, _) => "built_in_memory",
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
