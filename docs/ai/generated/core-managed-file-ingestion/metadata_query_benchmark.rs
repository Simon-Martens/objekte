//! Shared public-API metadata query workload for the repaired parent and final revision.

use std::error::Error;
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::files::FileCatalog;
use objekte_search_core::index::segment::SegmentPolicy;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::repository::configuration::{
    CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::schema::Schema;
use objekte_search_core::source::SourceFormat;

fn main() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(std::env::args().nth(1).ok_or("expected fixture root")?);
    std::fs::create_dir_all(root.join("data"))?;
    let mut selected_size = 0;
    for group in 0..64 {
        let mut bytes = Vec::new();
        for offset in 0..256 {
            let number = group * 256 + offset;
            let pad = if group == 63 && offset == 0 {
                ",\"pad\":\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\""
            } else {
                ""
            };
            bytes.extend_from_slice(
                format!(
                    "{{\"id\":\"r{number:05}\",\"year\":{},\"note\":\"Leipzig\"{pad}}}\n",
                    1800 + number % 16
                )
                .as_bytes(),
            );
        }
        if group == 63 {
            selected_size = bytes.len();
        }
        std::fs::write(root.join(format!("data/group-{group:02}.jsonl")), bytes)?;
    }
    let mut collection = CollectionDefinition::folder("records", "data", SourceFormat::JsonLines);
    collection.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    let options = RepositoryOptions {
        query_workers: 1,
        segment_policy: SegmentPolicy {
            target_records: NonZeroUsize::new(256).unwrap(),
            max_files: NonZeroUsize::new(1).unwrap(),
        },
        ..Default::default()
    };
    let mut builder = Index::builder().with_options(options);
    let registration = builder.collection("records", Schema::default())?;
    builder.register_file_catalog(FileCatalog::new(&root)?)?;
    builder.register_file_source(registration, collection.file_source())?;
    let index = builder.build()?;
    for group in 0..64 {
        index.add_file(&root.join(format!("data/group-{group:02}.jsonl")))?;
    }
    if index.status()?.live_records != 16_384 {
        return Err("unexpected indexed record count".into());
    }
    let view = index.view()?;
    let query = Query {
        collections: vec!["records".into()],
        fts: None,
        filter: Some(parse_filter(&format!("@file.size = {selected_size}"))?),
        options: QueryOptions {
            sort: Vec::new(),
            offset: 0,
            limit: 10,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let cold_start = Instant::now();
    let cold = view.search(&query)?;
    let cold_ns = cold_start.elapsed().as_nanos();
    if cold.count != Some(256) || cold.counters.files_opened != 0 || cold.counters.bytes_read != 0 {
        return Err(format!("unexpected cold result: {cold:?}").into());
    }
    let warm_start = Instant::now();
    for _ in 0..500 {
        let result = view.search(&query)?;
        if result.count != Some(256)
            || result.counters.files_opened != 0
            || result.counters.bytes_read != 0
        {
            return Err("unexpected warm result".into());
        }
        black_box(result.count);
    }
    let warm_ns = warm_start.elapsed().as_nanos();
    println!(
        "files=64 records=16384 selected_files=1 matched_records=256 segments={} cold_ns={cold_ns} warm_500_ns={warm_ns} content_opens=0 content_bytes=0",
        view.segment_summaries().len()
    );
    std::fs::remove_dir_all(root)?;
    Ok(())
}
