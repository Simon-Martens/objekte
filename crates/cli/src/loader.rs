//! Shared CLI repository configuration, discovery, and snapshot opening.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::discovery::SymlinkPolicy;
use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    SnapshotOpenPolicy,
};
use objekte_search_core::schema_adapter::{CompiledSchema, compile_schema};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::storage::SnapshotSaveStats;
use objekte_search_core::workload::BigramAlphabet;

use crate::arguments::{Alphabet, Arguments, ResolvedPaths};

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


/// Resolved collection definitions and schema metadata shared by CLI frontends.
pub struct ConfiguredDefinitions {
    pub definitions: Vec<CollectionDefinition>,
    pub schema_files: Vec<PathBuf>,
    pub schema_fields: usize,
    pub ignored_repository_metadata: bool,
}


/// Apply single-folder, repeated-path, or configured-repository rules.
pub fn configure_definitions(
    args: &Arguments,
    paths: &ResolvedPaths,
) -> Result<ConfiguredDefinitions, String> {
    if args.repo.is_some() {
        return load_repository_definitions(&paths.root, paths.index_directory.as_deref());
    }
    if paths.collection_paths.len() > 1 {
        let mut names = HashSet::new();
        let mut definitions = Vec::with_capacity(paths.collection_paths.len());
        for source in &paths.collection_paths {
            let name = source
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| format!("{} has no UTF-8 directory name", source.display()))?;
            if !names.insert(name.to_owned()) {
                return Err(format!(
                    "duplicate collection name {name} from --path; use directories with distinct names"
                ));
            }
            let relative = source
                .strip_prefix(&paths.root)
                .map_err(|error| error.to_string())?;
            definitions.push(make_definition(args, paths, name, relative, source, None));
        }
        return Ok(ConfiguredDefinitions {
            definitions,
            schema_files: Vec::new(),
            schema_fields: 0,
            ignored_repository_metadata: false,
        });
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
            "files",
            Path::new(""),
            &paths.root,
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
    name: &str,
    source_path: &Path,
    source_root: &Path,
    schema: Option<CompiledSchema>,
) -> CollectionDefinition {
    let mut definition = CollectionDefinition::folder(
        name,
        source_path.to_owned(),
        SourceFormat::JsonDecodedScalars,
    );
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
        && let Ok(relative) = schema_file.strip_prefix(source_root)
    {
        definition.discovery.exclude_files.push(relative.to_owned());
    }
    if let Some(directory) = paths.index_directory.as_deref()
        && let Ok(relative) = directory.strip_prefix(source_root)
        && !relative.as_os_str().is_empty()
    {
        definition
            .discovery
            .exclude_directories
            .push(relative.to_owned());
    }
    definition
}

/// Translate command opening options into the core's index and inference policy.
pub fn repository_options(args: &Arguments) -> RepositoryOptions {
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

/// Preserve the CLI's load-only, write-only, combined, and in-memory policies.
pub fn snapshot_policy(args: &Arguments, paths: &ResolvedPaths) -> SnapshotOpenPolicy {
    let mode = match (args.load_index.is_some(), args.write_index.is_some()) {
        (false, false) => SnapshotMode::Disabled,
        (true, false) => SnapshotMode::Required,
        (false, true) => SnapshotMode::Disabled,
        (true, true) => SnapshotMode::IfValid,
    };
    SnapshotOpenPolicy::new(mode, paths.index_directory.clone())
}

/// Open once and write a snapshot only when explicitly requested.
pub fn open_repository(
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
