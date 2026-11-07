//! Optional repository collection files around the shared schema adapter.

use std::path::Path;

use objekte_search_core::repository::{CollectionDefinition, IdentityPolicy};
use objekte_search_core::schema_adapter::compile_schema;
use objekte_search_core::source::SourceFormat;

pub fn load_definitions(root: &Path) -> Result<Vec<CollectionDefinition>, String> {
    let directory = root.join(".objekte/collections");
    let mut paths = std::fs::read_dir(&directory)
        .map_err(|error| format!("{}: {error}", directory.display()))?
        .map(|entry| {
            entry
                .map(|entry| entry.path())
                .map_err(|error| error.to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    paths.retain(|path| {
        path.extension()
            .is_some_and(|extension| extension == "json")
    });
    paths.sort();
    let mut definitions = Vec::new();
    for path in paths {
        let bytes = std::fs::read(&path).map_err(|error| format!("{}: {error}", path.display()))?;
        let document: serde_json::Value = serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?;
        let meta = document
            .get("x-objekte")
            .ok_or_else(|| format!("{}: missing x-objekte", path.display()))?;
        let string = |key: &str| -> Result<&str, String> {
            meta.get(key)
                .and_then(|value| value.as_str())
                .ok_or_else(|| format!("{}: missing x-objekte.{key}", path.display()))
        };
        let name = string("name")?.to_owned();
        let source_path = string("path")?.into();
        let format = match string("format")? {
            "json" => SourceFormat::Json,
            "jsonl" => SourceFormat::JsonLines,
            other => {
                return Err(format!(
                    "{}: unsupported source format {other}",
                    path.display()
                ));
            }
        };
        let identity = match string("id")? {
            "filename" => IdentityPolicy::Filename,
            "generated" => IdentityPolicy::Generated,
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
        definitions.push(CollectionDefinition {
            name,
            source_path,
            format,
            identity,
            discovery: Default::default(),
            schema: compiled.schema,
            sort_fields: compiled.sort_fields,
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        });
    }
    Ok(definitions)
}

#[cfg(test)]
#[path = "schema.test.rs"]
mod tests;
