//! Shared CLI repository configuration, discovery, and snapshot opening.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::discovery::SymlinkPolicy;
use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode, SnapshotOpenPolicy,
};
use objekte_search_core::repository_configuration::{
    load_explicit_schema, load_repository_configuration,
};
use objekte_search_core::schema_adapter::CompiledSchema;
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
    let loaded = load_explicit_schema(path, "files")?;
    Ok(Some((loaded.compiled, loaded.ignored_repository_metadata)))
}


/// Resolved collection definitions and schema metadata shared by CLI frontends.
pub struct ConfiguredDefinitions {
    pub definitions: Vec<CollectionDefinition>,
    pub schema_files: Vec<PathBuf>,
    /// Canonical schema/configuration targets to protect from snapshot writes.
    pub protected_files: Vec<PathBuf>,
    pub schema_fields: usize,
    pub ignored_repository_metadata: bool,
}


/// Apply single-folder, repeated-path, or configured-repository rules.
pub fn configure_definitions(
    args: &Arguments,
    paths: &ResolvedPaths,
) -> Result<ConfiguredDefinitions, String> {
    if args.repo.is_some() {
        let loaded = load_repository_configuration(&paths.root)?;
        let mut definitions = loaded.definitions;
        if let Some(directory) = paths.index_directory.as_deref() {
            for definition in &mut definitions {
                let source_root = paths.root.join(&definition.source_path);
                exclude_cache_directory(definition, &source_root, directory);
            }
        }
        return Ok(ConfiguredDefinitions {
            definitions,
            schema_files: loaded.schema_files,
            protected_files: loaded.protected_files,
            schema_fields: loaded.schema_fields,
            ignored_repository_metadata: false,
        });
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
            protected_files: Vec::new(),
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
        protected_files: paths.schema_file.iter().cloned().collect(),
        schema_fields,
        ignored_repository_metadata,
    })
}

/// Preserve the legacy CLI snapshot policy: a custom cache directory is not
/// part of source discovery, including JSON files added after a cache write.
fn exclude_cache_directory(
    definition: &mut CollectionDefinition,
    source_root: &Path,
    directory: &Path,
) {
    let source_root = source_root
        .canonicalize()
        .unwrap_or_else(|_| source_root.to_owned());
    let Ok(relative) = directory.strip_prefix(&source_root) else {
        return;
    };
    if !relative.as_os_str().is_empty() {
        definition
            .discovery
            .exclude_directories
            .push(relative.to_owned());
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
    if let Some(directory) = paths.index_directory.as_deref() {
        exclude_cache_directory(&mut definition, source_root, directory);
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

#[cfg(test)]
#[path = "loader.test.rs"]
mod tests;
