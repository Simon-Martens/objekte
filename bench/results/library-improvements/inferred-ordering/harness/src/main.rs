use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::query::{Query, QueryOptions};
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{ReturnMode, SortField};

const RECORDS: usize = 4096;
const BLOCKS: usize = 5;
const REUSES: usize = 10;

fn fixture(root: &Path) -> Result<(), String> {
    let directory = root.join("data");
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let file = directory.join("records.jsonl");
    if file.exists() {
        return Ok(());
    }
    let mut bytes = String::with_capacity(RECORDS * 90);
    for slot in 0..RECORDS {
        bytes.push_str(&format!(
            "{{\"uuid\":\"00000000-0000-0000-0000-{slot:012}\",\"label\":\"{}\"}}\n",
            if slot % 2 == 0 { "Ähre" } else { "Zug" }
        ));
    }
    std::fs::write(file, bytes).map_err(|error| error.to_string())
}

fn query() -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField::simple("uuid", false)],
            limit: 20,
            offset: 100,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

fn main() -> Result<(), String> {
    let root = std::env::args().nth(1).ok_or("root path required")?;
    let root = Path::new(&root);
    fixture(root)?;
    let mut inference = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    inference.policy.max_distinct_values_per_path = 8192;
    inference.policy.max_distinct_string_bytes = 256 * 1024;
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        verified_cache_bytes: 0,
        query_workers: 1,
        build_workers: std::num::NonZeroUsize::new(1),
        inference,
        ..Default::default()
    };
    let request = query();
    println!("block,phase,iteration,elapsed_ns,records,returned,files_opened,record_parses");
    for block in 0..BLOCKS {
        let repo = Repository::open_with_options(
            root,
            vec![CollectionDefinition::folder(
                "data",
                "data",
                SourceFormat::JsonLines,
            )],
            options,
        )?;
        let plan = repo.explain(&request)?;
        if plan[0].sort_strategy != "inferred_posting_order_page" {
            return Err(format!(
                "inferred order unavailable: {}",
                plan[0].sort_strategy
            ));
        }
        for iteration in 0..=REUSES {
            let started = Instant::now();
            let result = black_box(repo.search(&request)?);
            let elapsed_ns = started.elapsed().as_nanos();
            if result.hits.len() != 20 || result.counters.files_opened != 0 {
                return Err("unexpected inferred order result or source read".into());
            }
            println!(
                "{block},{},{iteration},{elapsed_ns},{RECORDS},{},{},{}",
                if iteration == 0 { "first" } else { "reuse" },
                result.hits.len(),
                result.counters.files_opened,
                result.counters.record_parses,
            );
        }
    }
    Ok(())
}
