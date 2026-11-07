use std::hint::black_box;
use std::path::Path;
use std::time::Instant;

use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{ReturnMode, SortField};

const RECORDS: usize = 12_288;
const BLOCKS: usize = 5;

fn fixture(root: &Path) -> Result<(), String> {
    let data = root.join("data");
    std::fs::create_dir_all(&data).map_err(|error| error.to_string())?;
    let mut bytes = String::with_capacity(RECORDS * 100);
    for slot in 0..RECORDS {
        bytes.push_str(&format!(
            "{{\"kind\":\"yes\",\"body\":\"needle {slot:05}\"}}\n"
        ));
    }
    std::fs::write(data.join("records.jsonl"), bytes).map_err(|error| error.to_string())
}

fn definition() -> Result<CollectionDefinition, String> {
    let mut schema = Schema::default();
    for (name, indexed) in [("kind", true), ("body", false)] {
        schema.register_field(FieldDefinition {
            path: vec![name.into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed,
            multi: false,
            element_nullable: vec![],
        })?;
    }
    Ok(CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    })
}

fn query(sort: bool) -> Result<Query, String> {
    Ok(Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter("kind = 'yes'")?),
        options: QueryOptions {
            sort: if sort {
                vec![SortField::simple("body", false)]
            } else {
                vec![]
            },
            limit: 0,
            offset: 37,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    })
}

fn main() -> Result<(), String> {
    let root = std::env::args().nth(1).ok_or("fixture path required")?;
    let root = Path::new(&root);
    fixture(root)?;
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        verified_cache_bytes: 0,
        query_workers: 1,
        build_workers: std::num::NonZeroUsize::new(1),
        ..Default::default()
    };
    let exact = query(false)?;
    let sorted = query(true)?;
    println!(
        "block,case,elapsed_ns,matches,files_opened,record_parses,sort_key_evaluations,peak_payload_bytes"
    );
    for block in 0..BLOCKS {
        let repo = Repository::open_with_options(root, vec![definition()?], options)?;
        for (name, request) in [("count", &exact), ("count_sorted", &sorted)] {
            let started = Instant::now();
            let result = black_box(repo.search(request)?);
            let elapsed_ns = started.elapsed().as_nanos();
            if result.count != Some(RECORDS) || !result.hits.is_empty() {
                return Err(format!("unexpected {name} result"));
            }
            println!(
                "{block},{name},{elapsed_ns},{RECORDS},{},{},{},{}",
                result.counters.files_opened,
                result.counters.record_parses,
                result.counters.sort_key_evaluations,
                result.counters.peak_retained_payload_bytes,
            );
        }
        let started = Instant::now();
        let selection = black_box(repo.select_exact("data", &sorted)?);
        let elapsed_ns = started.elapsed().as_nanos();
        if selection.len() != RECORDS as u64 {
            return Err("unexpected selection size".into());
        }
        println!("{block},selection,{elapsed_ns},{RECORDS},,,,");
    }
    Ok(())
}
