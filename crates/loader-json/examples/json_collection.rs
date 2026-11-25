//! Search a JSON folder with an optional explicit JSON Schema file.

use std::path::PathBuf;

use objekte_loader_json::configuration::load_optional_schema;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::configuration::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::{SourceFormat, registry::SourceRegistry};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root =
        PathBuf::from(args.next().ok_or("usage: json_collection ROOT [SCHEMA]")?).canonicalize()?;
    let schema = args.next().map(PathBuf::from);
    let schema = schema.map(|path| path.canonicalize()).transpose()?;

    let mut sources = SourceRegistry::default();
    objekte_loader_json::register(&mut sources)?;
    let mut collection = CollectionDefinition::folder("documents", "", SourceFormat::Json);
    if let Some(explicit) = load_optional_schema(schema.as_deref(), &collection.name)? {
        collection.schema = explicit.compiled.schema;
        collection.sort_fields = explicit.compiled.sort_fields;
        if let Some(relative) = schema
            .as_deref()
            .and_then(|path| path.strip_prefix(&root).ok())
        {
            collection.discovery.exclude_files.push(relative.to_owned());
        }
    }
    let index = Index::start_build(
        &root,
        vec![collection],
        RepositoryOptions::default(),
        sources,
    )?
    .wait_complete()?;
    println!("{} records", index.status()?.live_records);
    Ok(())
}
