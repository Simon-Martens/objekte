//! Build a native index, search it, archive it explicitly, mutate it, and
//! archive again. Pass a writable temporary root as the sole argument.

use std::path::PathBuf;

use objekte_search_core::lifecycle::Index;
use objekte_search_core::query::{Query, QueryOptions};
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;

fn all_query() -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 100,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: explicit_archive <writable-root>")?,
    );
    std::fs::create_dir_all(root.join("data"))?;
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","title":"native first"}"#,
    )?;
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    let index = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )?;

    println!(
        "native search: {}",
        index.view()?.search(&all_query())?.count.unwrap_or(0)
    );
    let report = index.archive()?;
    println!(
        "archived {} components: {} bytes",
        report.converted_components, report.newly_serialized_bytes
    );
    println!(
        "archived search: {}",
        index.view()?.search(&all_query())?.count.unwrap_or(0)
    );

    std::fs::write(
        root.join("data/b.json"),
        br#"{"id":"b","title":"native mutation"}"#,
    )?;
    index.add_file(&root.join("data/b.json"))?;
    println!(
        "mixed search: {}",
        index.view()?.search(&all_query())?.count.unwrap_or(0)
    );
    index.archive()?;
    println!(
        "final search: {}",
        index.view()?.search(&all_query())?.count.unwrap_or(0)
    );
    index.shutdown()?;
    Ok(())
}
