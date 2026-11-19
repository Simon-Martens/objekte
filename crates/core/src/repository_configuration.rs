//! Explicit schema loading and the configured `.objekte/collections` layout.
//!
//! This module resolves definitions and protected files. It never opens or
//! chooses an index cache and does not build an index.

use std::collections::HashSet;
use std::fs;
use std::path::{Component, Path, PathBuf};

use serde_json::Value;

use crate::repository::{CollectionDefinition, IdentityPolicy};
use crate::schema::json::{CompiledSchema, compile_schema};
use crate::source::SourceFormat;

/// Prepare collection definitions for the live configuration identity and
/// source traversal. Selected source paths are validated, the generated index
/// namespace is rejected as source data, and established index locations are
/// pruned. The function never stats, opens or creates an index file.
pub(crate) fn prepare_source_definitions(
    root: &Path,
    definitions: &[CollectionDefinition],
) -> Result<Vec<CollectionDefinition>, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("repository root {}: {error}", root.display()))?;
    if !root.is_dir() {
        return Err(format!(
            "repository root is not a directory: {}",
            root.display()
        ));
    }
    // Kept in the same order and spelling as the previous live identity so
    // unchanged inputs keep their configuration hash byte-for-byte.
    let owned_namespaces = [
        root.join(".objekte").join(".indexes"),
        root.join(".objekte").join("indexes"),
    ];
    let mut prepared = definitions.to_vec();
    for definition in &mut prepared {
        if crate::discovery::contains_generated_directory(&definition.source_path) {
            return Err(format!(
                "collection {} source selection {} selects or lies beneath a generated index namespace",
                definition.name,
                definition.source_path.display()
            ));
        }
        let selected = root.join(normalized_selection(&definition.source_path)?);
        let selected = match selected.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => selected,
            Err(error) => {
                return Err(format!(
                    "collection {} source selection {}: {error}",
                    definition.name,
                    selected.display()
                ));
            }
        };
        if selected
            .strip_prefix(&root)
            .is_ok_and(crate::discovery::contains_generated_directory)
        {
            return Err(format!(
                "collection {} source selection {} resolves inside a generated index namespace",
                definition.name,
                definition.source_path.display()
            ));
        }
        for owned in &owned_namespaces {
            if selected.starts_with(owned) {
                return Err(format!(
                    "collection {} selects the generated index namespace as source data",
                    definition.name
                ));
            }
            if let Ok(relative) = owned.strip_prefix(&selected) {
                if relative.as_os_str().is_empty() {
                    return Err("generated index namespace is the collection source root".into());
                }
                if !definition
                    .discovery
                    .exclude_directories
                    .iter()
                    .any(|existing| {
                        !existing.as_os_str().is_empty()
                            && existing
                                .components()
                                .all(|part| matches!(part, Component::Normal(_)))
                            && relative.starts_with(existing)
                    })
                {
                    definition
                        .discovery
                        .exclude_directories
                        .push(relative.to_path_buf());
                }
            }
        }
    }
    Ok(prepared)
}

fn normalized_selection(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Err(format!(
            "source selection must be relative: {}",
            path.display()
        ));
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => result.push(part),
            Component::ParentDir => {
                if !result.pop() {
                    return Err(format!("source selection escapes root: {}", path.display()));
                }
            }
            _ => return Err(format!("invalid source selection: {}", path.display())),
        }
    }
    Ok(result)
}

/// One explicitly named schema, including its canonical path for write guards.
pub struct ExplicitSchemaFile {
    pub path: PathBuf,
    pub compiled: CompiledSchema,
    /// Repository metadata was present but has no effect in explicit mode.
    pub ignored_repository_metadata: bool,
}

/// Load only `path`. Relative paths resolve against the process working
/// directory once; the returned path is canonical and is safe to protect
/// against cache destination aliases. No repository or cache is discovered.
pub fn load_explicit_schema(path: &Path, collection: &str) -> Result<ExplicitSchemaFile, String> {
    let path = canonical_file(path)?;
    let (bytes, mut document) = read_schema(&path)?;
    let ignored_repository_metadata = document.get("x-objekte").is_some();
    omit_unavailable_relations(&mut document, &HashSet::from([collection.to_owned()]));
    let mut compiled = compile_schema(&document, collection)
        .map_err(|error| format!("{}: {error}", path.display()))?;
    compiled.schema.set_source_fingerprint(&bytes);
    Ok(ExplicitSchemaFile {
        path,
        compiled,
        ignored_repository_metadata,
    })
}

/// Definitions from a concrete repository root and its schema files.
pub struct RepositoryConfiguration {
    pub root: PathBuf,
    pub definitions: Vec<CollectionDefinition>,
    pub schema_files: Vec<PathBuf>,
    pub schema_fields: usize,
}

/// Read the existing `.objekte/collections/*.schema.json` layout beneath
/// exactly `root`. Relative roots resolve against the process working
/// directory once. Cache discovery and cache I/O belong to a separate layer.
pub fn load_repository_configuration(root: &Path) -> Result<RepositoryConfiguration, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", root.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let directory = root.join(".objekte/collections");
    let mut schema_files = fs::read_dir(&directory)
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
        let (bytes, document) = read_schema(path)?;
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
                .and_then(Value::as_str)
                .unwrap_or("und")
                .to_owned(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        };
        definition.discovery.exclude = vec![".git".into(), "**/.git".into()];
        definition.discovery.exclude_directories = vec![PathBuf::from(".objekte")];
        definitions.push(definition);
    }
    Ok(RepositoryConfiguration {
        root,
        definitions,
        schema_files,
        schema_fields,
    })
}

fn canonical_file(path: &Path) -> Result<PathBuf, String> {
    let resolved = path
        .canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !resolved.is_file() {
        return Err(format!("{} is not a file", path.display()));
    }
    Ok(resolved)
}

fn read_schema(path: &Path) -> Result<(Vec<u8>, Value), String> {
    let bytes = fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let document =
        serde_json::from_slice(&bytes).map_err(|error| format!("{}: {error}", path.display()))?;
    Ok((bytes, document))
}

fn repository_metadata<'a>(metadata: &'a Value, key: &str, path: &Path) -> Result<&'a str, String> {
    metadata
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("{}: missing x-objekte.{key}", path.display()))
}

fn omit_unavailable_relations(document: &mut Value, collections: &HashSet<String>) {
    match document {
        Value::Object(fields) => {
            if fields
                .get("x-relation")
                .and_then(|relation| relation.get("collection"))
                .and_then(Value::as_str)
                .is_some_and(|target| !collections.contains(target))
            {
                fields.remove("x-relation");
            }
            for value in fields.values_mut() {
                omit_unavailable_relations(value, collections);
            }
        }
        Value::Array(values) => {
            for value in values {
                omit_unavailable_relations(value, collections);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
#[path = "repository_configuration.test.rs"]
mod tests;
