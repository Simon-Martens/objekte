//! Shared CLI repository configuration and source discovery.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use objekte_search_core::discovery::SymlinkPolicy;
use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::repository_configuration::{
    load_explicit_schema, load_repository_configuration,
};
use objekte_search_core::schema_adapter::CompiledSchema;
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::BigramAlphabet;

use crate::arguments::{Alphabet, Arguments, IndexStorageChoice, ResolvedPaths};

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
        return Ok(ConfiguredDefinitions {
            definitions: loaded.definitions,
            schema_files: loaded.schema_files,
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

/// Exclude an application-owned index directory from source discovery, even
/// when a custom base is located inside a collection source folder.
fn exclude_managed_directory(
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
        let managed = if args.index_storage == IndexStorageChoice::Custom {
            directory.join("objekte-indexes")
        } else {
            directory.to_owned()
        };
        exclude_managed_directory(&mut definition, source_root, &managed);
    }
    definition
}

/// Translate command opening options into the core's index and inference policy.
pub fn repository_options(args: &Arguments) -> RepositoryOptions {
    RepositoryOptions {
        // CLI batch commands and the TUI both enter through this loader. One
        // bounded lifecycle store is shared across their index revisions.
        verified_cache_bytes: 64 * 1024 * 1024,
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

#[cfg(test)]
#[path = "loader.test.rs"]
mod tests;
