//! Index JSON Lines with the shared build pool and JSONL chunk framing.

use std::num::NonZeroUsize;
use std::path::PathBuf;

use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::configuration::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::{SourceFormat, registry::SourceRegistry};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let root = PathBuf::from(args.next().ok_or("usage: parallel_jsonl ROOT [WORKERS]")?);
    let workers = args
        .next()
        .map(|value| value.to_string_lossy().parse::<NonZeroUsize>())
        .transpose()?;
    let mut sources = SourceRegistry::default();
    objekte_loader_jsonl::register(&mut sources)?;
    assert!(sources.resolve(&SourceFormat::Json).is_err());

    let collection = CollectionDefinition::folder("entries", "", SourceFormat::JsonLines);
    let options = RepositoryOptions {
        build_workers: workers,
        ..Default::default()
    };
    let index = Index::start_build(&root, vec![collection], options, sources)?.wait_complete()?;
    println!("{} records", index.status()?.live_records);
    Ok(())
}
