//! Search folder-defined collections without loading .objekte schemas.
//!
//! cargo run --release -p objekte-search-core --example musenalm_without_schema -- \
//!   --root examples/musenalm --query Goethe
//! Add --folder NAME=RELATIVE_PATH to define collections explicitly.

use std::path::PathBuf;

use objekte_search_core::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions};
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;

fn main() {
    if let Err(error) = run() {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let mut folders = Vec::<(String, PathBuf)>::new();
    let mut literal = "Goethe".to_owned();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => root = args.next().ok_or("--root needs a path")?.into(),
            "--query" => literal = args.next().ok_or("--query needs text")?,
            "--folder" => {
                let value = args.next().ok_or("--folder needs NAME=RELATIVE_PATH")?;
                let (name, path) = value
                    .split_once('=')
                    .ok_or("--folder needs NAME=RELATIVE_PATH")?;
                folders.push((name.to_owned(), path.into()));
            }
            "--help" | "-h" => {
                println!(
                    "Usage: musenalm_without_schema [--root PATH] [--folder NAME=RELATIVE_PATH]... [--query TEXT]"
                );
                return Ok(());
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }
    if folders.is_empty() {
        let data = root.join("data");
        for entry in
            std::fs::read_dir(&data).map_err(|error| format!("{}: {error}", data.display()))?
        {
            let entry = entry.map_err(|error| error.to_string())?;
            if entry
                .file_type()
                .map_err(|error| error.to_string())?
                .is_dir()
            {
                let name = entry.file_name().to_string_lossy().into_owned();
                folders.push((name.clone(), PathBuf::from("data").join(name)));
            }
        }
    }
    folders.sort_by(|a, b| a.0.cmp(&b.0));
    let names = folders.iter().map(|(name, _)| name.clone()).collect();
    let definitions = folders
        .into_iter()
        .map(|(name, path)| CollectionDefinition::folder(name, path, SourceFormat::Json))
        .collect();
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..Default::default()
        },
    )?;
    let result = repo.search(&Query {
        collections: names,
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal,
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    })?;
    println!("{} matches", result.counters.total_matches);
    for hit in result.hits {
        println!("{}: {}", hit.collection, hit.record_id);
    }
    Ok(())
}
