//! Explicit schema loading and the configured `.objekte/collections` layout.
//!
//! This module resolves definitions and protected files. It never opens or
//! chooses an index cache and does not build an index.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::repository::{CollectionDefinition, IdentityPolicy};
use crate::schema_adapter::{compile_schema, CompiledSchema};
use crate::source::SourceFormat;


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


/// Definitions from a concrete repository root, with source schema/config
/// files to protect from cache writes. `schema_files` retains the selected
/// layout paths; `protected_files` contains their canonical targets.
pub struct RepositoryConfiguration {
    pub root: PathBuf,
    pub definitions: Vec<CollectionDefinition>,
    pub schema_files: Vec<PathBuf>,
    pub protected_files: Vec<PathBuf>,
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
    let mut protected_files = Vec::with_capacity(schema_files.len());
    for path in &schema_files {
        let protected = canonical_file(path)?;
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
        protected_files.push(protected);
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
    protected_files.sort();
    protected_files.dedup();
    Ok(RepositoryConfiguration {
        root,
        definitions,
        schema_files,
        protected_files,
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
