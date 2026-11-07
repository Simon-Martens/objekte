use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::repository::{CollectionDefinition, Repository, RepositoryOptions, SnapshotMode};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::ReturnMode;

const RECORDS: usize = 8192;
const ITERATIONS: usize = 200;
const BLOCKS: usize = 5;

fn fixture(root: &Path) -> Result<(), String> {
    let directory = root.join("data");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let file = directory.join("records.jsonl");
    if file.exists() {
        return Ok(());
    }
    let mut bytes = String::new();
    for slot in 0..RECORDS {
        bytes.push('{');
        for field in 0..16 {
            if field > 0 {
                bytes.push(',');
            }
            bytes.push_str(&format!("\"f{field}\":{}", slot % 4));
        }
        bytes.push_str("}\n");
    }
    std::fs::write(file, bytes).map_err(|error| error.to_string())
}

fn main() -> Result<(), String> {
    let root = std::env::args().nth(1).ok_or("root path required")?;
    let root = Path::new(&root);
    fixture(root)?;
    let repo = Repository::open_with_options(
        root,
        vec![CollectionDefinition::folder("data", "data", SourceFormat::JsonLines)],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            query_workers: 1,
            build_workers: std::num::NonZeroUsize::new(1),
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )?;
    let queries = [1, 4, 16].map(|terms| {
        let expression = (0..terms)
            .map(|field| format!("f{field} = 0"))
            .collect::<Vec<_>>()
            .join(" && ");
        Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(&expression).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 1,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        }
    });
    for query in &queries {
        assert_eq!(repo.search(query)?.count, Some(RECORDS / 4));
    }
    println!("terms,block,iterations,total_ns,ns_per_search");
    for block in 0..BLOCKS {
        for (case, query) in queries.iter().enumerate() {
            let start = Instant::now();
            for _ in 0..ITERATIONS {
                black_box(repo.search(query)?);
            }
            let total_ns = start.elapsed().as_nanos();
            println!("{},{block},{ITERATIONS},{total_ns},{}", [1, 4, 16][case], total_ns / ITERATIONS as u128);
        }
    }
    Ok(())
}
