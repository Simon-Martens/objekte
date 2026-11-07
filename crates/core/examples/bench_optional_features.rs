//! A separate, compact workload for schema-free construction, inference,
//! dynamic sorting, and progressive delivery. The historical 150 queries are
//! deliberately kept in their own benchmark.
//!
//! Build before measuring:
//! cargo build --release -p objekte-search-core --example bench_optional_features
//! target/release/examples/bench_optional_features --root examples/musenalm \
//!   --inference disabled --snapshot-mode disabled > optional-disabled.json
//!
//! For snapshot measurements, use an isolated copy of the corpus. The
//! benchmark's legacy `rebuild` spelling explicitly saves after a fresh
//! build; `read-write` restores if valid and saves only after rebuilding.

use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::inference::{InferenceConfig, InferenceMode};
use objekte_search_core::model::FieldPath;
use objekte_search_core::query::{
    CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use objekte_search_core::repository::{
    CollectionDefinition, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::storage::snapshot_path;
use objekte_search_core::streaming::{
    CancellationToken, StreamControl, StreamEvent, StreamMode, StreamOptions, StreamOutcome,
};
use objekte_search_core::workload::{
    BigramAlphabet, ReturnMode, SortExpression, SortField, SortReducer,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};


struct Config {
    root: PathBuf,
    source_path: String,
    alphabet: BigramAlphabet,
    inference: InferenceMode,
    snapshot: SnapshotMode,
    write_snapshot: bool,
    build_workers: Option<NonZeroUsize>,
    query_workers: usize,
    verified_cache_bytes: usize,
    skip_queries: bool,
}


impl Default for Config {
    fn default() -> Self {
        Self {
            root: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm"),
            source_path: "data/entries".into(),
            alphabet: BigramAlphabet::Ascii,
            inference: InferenceMode::Disabled,
            snapshot: SnapshotMode::Disabled,
            write_snapshot: false,
            build_workers: None,
            query_workers: RepositoryOptions::default().query_workers,
            verified_cache_bytes: 0,
            skip_queries: false,
        }
    }
}


fn parse_args() -> Result<Config, String> {
    let mut config = Config::default();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--root" => config.root = PathBuf::from(args.next().ok_or("--root needs a path")?),
            "--source-path" => {
                config.source_path = args.next().ok_or("--source-path needs a path")?
            }
            "--alphabet" => {
                config.alphabet = match args.next().ok_or("--alphabet needs a value")?.as_str() {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            "--inference" => {
                config.inference = match args.next().ok_or("--inference needs a value")?.as_str() {
                    "disabled" => InferenceMode::Disabled,
                    "report" => InferenceMode::ReportOnly,
                    "auto" => InferenceMode::AutoIndex,
                    other => return Err(format!("unknown inference mode {other}")),
                }
            }
            "--snapshot-mode" => {
                (config.snapshot, config.write_snapshot) = match args.next().ok_or("--snapshot-mode needs a value")?.as_str()
                {
                    "disabled" => (SnapshotMode::Disabled, false),
                    "rebuild" => (SnapshotMode::Disabled, true),
                    "read-write" => (SnapshotMode::IfValid, true),
                    other => return Err(format!("unknown snapshot mode {other}")),
                }
            }
            "--build-workers" => {
                let value = args.next().ok_or("--build-workers needs a value")?;
                config.build_workers = if value == "auto" {
                    None
                } else {
                    Some(
                        NonZeroUsize::new(
                            value
                                .parse()
                                .map_err(|error: std::num::ParseIntError| error.to_string())?,
                        )
                        .ok_or("build workers must be positive")?,
                    )
                };
            }
            "--query-workers" => {
                config.query_workers = args
                    .next()
                    .ok_or("--query-workers needs a value")?
                    .parse()
                    .map_err(|error: std::num::ParseIntError| error.to_string())?
            }
            "--verified-cache-bytes" => {
                config.verified_cache_bytes = args
                    .next()
                    .ok_or("--verified-cache-bytes needs a value")?
                    .parse()
                    .map_err(|error: std::num::ParseIntError| error.to_string())?
            }
            "--skip-queries" => config.skip_queries = true,
            "--help" | "-h" => {
                println!(
                    "bench_optional_features --root PATH [--source-path data/entries] [--alphabet ascii|unicode] [--inference disabled|report|auto] [--snapshot-mode disabled|rebuild|read-write] [--build-workers auto|N] [--query-workers N] [--verified-cache-bytes N] [--skip-queries]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown option {other}")),
        }
    }
    if config.query_workers == 0 {
        return Err("query workers must be positive".into());
    }
    if config.source_path != "data/entries" && !config.skip_queries {
        return Err("the compact query cases require data/entries; use --skip-queries".into());
    }
    Ok(config)
}

fn resident_bytes(name: &str) -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find_map(|line| {
            line.strip_prefix(name)?
                .trim()
                .strip_suffix(" kB")?
                .trim()
                .parse::<u64>()
                .ok()
                .map(|kilobytes| kilobytes * 1024)
        })
}

fn computed(path: &str, reducer: Option<SortReducer>, descending: bool) -> SortField {
    SortField::computed(
        SortExpression {
            path: path.into(),
            binding: None,
            reducer,
        },
        descending,
    )
}


struct Case {
    id: &'static str,
    filter: Option<&'static str>,
    fts: Option<&'static str>,
    sort: Vec<SortField>,
    limit: usize,
    offset: usize,
    stream: StreamMode,
}


fn cases() -> Vec<Case> {
    vec![
        Case {
            id: "selective-title",
            filter: Some("preferred_title ~ 'Goethe'"),
            fts: None,
            sort: vec![],
            limit: 10,
            offset: 0,
            stream: StreamMode::OrderedRows,
        },
        Case {
            id: "broad-title",
            filter: Some("preferred_title ~ 'Taschen'"),
            fts: None,
            sort: vec![],
            limit: 10,
            offset: 0,
            stream: StreamMode::OrderedRows,
        },
        Case {
            id: "enum-state",
            filter: Some("edit_state = 'Edited'"),
            fts: None,
            sort: vec![],
            limit: 10,
            offset: 0,
            stream: StreamMode::OrderedRows,
        },
        Case {
            id: "numeric-range",
            filter: Some("year >= 1800 && year < 1850"),
            fts: None,
            sort: vec![],
            limit: 10,
            offset: 0,
            stream: StreamMode::OrderedRows,
        },
        Case {
            id: "array-empty",
            filter: Some("items:length = 0"),
            fts: None,
            sort: vec![],
            limit: 10,
            offset: 0,
            stream: StreamMode::OrderedRows,
        },
        Case {
            id: "array-membership",
            filter: Some("language ?= 'ger'"),
            fts: None,
            sort: vec![],
            limit: 10,
            offset: 0,
            stream: StreamMode::OrderedRows,
        },
        Case {
            id: "mixed-fts-filter",
            filter: Some("year >= 1800"),
            fts: Some("Goethe"),
            sort: vec![],
            limit: 10,
            offset: 0,
            stream: StreamMode::OrderedRows,
        },
        Case {
            id: "stored-size",
            filter: None,
            fts: None,
            sort: vec![SortField::simple("@file.size", true)],
            limit: 20,
            offset: 0,
            stream: StreamMode::OrderedRows,
        },
        Case {
            id: "stored-composite",
            filter: None,
            fts: None,
            sort: vec![
                SortField::simple("@file.size", true),
                SortField::simple("@file.modified", false),
            ],
            limit: 20,
            offset: 0,
            stream: StreamMode::OrderedRows,
        },
        Case {
            id: "dynamic-title",
            filter: None,
            fts: None,
            sort: vec![computed("preferred_title", None, false)],
            limit: 20,
            offset: 0,
            stream: StreamMode::PageReplacements,
        },
        Case {
            id: "dynamic-title-deep",
            filter: Some("year >= 1800"),
            fts: None,
            sort: vec![computed("preferred_title", None, false)],
            limit: 20,
            offset: 200,
            stream: StreamMode::PageReplacements,
        },
        Case {
            id: "dynamic-array-min",
            filter: None,
            fts: None,
            sort: vec![computed("language", Some(SortReducer::Min), false)],
            limit: 20,
            offset: 0,
            stream: StreamMode::PageReplacements,
        },
    ]
}

fn query(case: &Case) -> Result<Query, String> {
    Ok(Query {
        collections: vec!["entries".into()],
        fts: case.fts.map(|literal| FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: literal.into(),
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        filter: case.filter.map(parse_filter).transpose()?,
        options: QueryOptions {
            sort: case.sort.clone(),
            limit: case.limit,
            offset: case.offset,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    })
}

fn measure_case(repo: &Repository, case: Case, warm_cache: bool) -> Result<Value, String> {
    let query = query(&case)?;
    let started = Instant::now();
    let collected = repo.search(&query)?;
    let collected_ns = started.elapsed().as_nanos();
    let warm_collected = if warm_cache {
        let started = Instant::now();
        let result = repo.search(&query)?;
        if result.count != collected.count || result.hits != collected.hits {
            return Err(format!("{} warm collected result differs", case.id));
        }
        Some((started.elapsed().as_nanos(), result.counters))
    } else {
        None
    };
    let mut events = 0u64;
    let mut data_events = 0u64;
    let mut event_rows = 0u64;
    let mut event_bytes = 0u64;
    let mut callback_ns = 0u128;
    let mut first_data_ns = None;
    let mut first_full_page_ns = None;
    let mut visible_rows = 0usize;
    let mut visible_ids = Vec::new();
    let started = Instant::now();
    let outcome = repo
        .search_stream(
            &query,
            StreamOptions {
                mode: case.stream,
                batch_rows: 8,
                checkpoint_files: 8,
                ..Default::default()
            },
            &CancellationToken::new(),
            &mut |event| {
                let callback_start = Instant::now();
                events += 1;
                match event {
                    StreamEvent::OrderedRows { rows, .. } => {
                        if !rows.is_empty() {
                            data_events += 1;
                            first_data_ns.get_or_insert_with(|| started.elapsed().as_nanos());
                        }
                        visible_rows += rows.len();
                        visible_ids.extend(
                            rows.iter()
                                .map(|hit| (hit.collection.clone(), hit.record_id.clone())),
                        );
                        event_rows += rows.len() as u64;
                        event_bytes += rows
                            .iter()
                            .map(|hit| (hit.collection.len() + hit.record_id.len()) as u64)
                            .sum::<u64>();
                    }
                    StreamEvent::PageReplacement { rows, .. } => {
                        if !rows.is_empty() {
                            data_events += 1;
                            first_data_ns.get_or_insert_with(|| started.elapsed().as_nanos());
                        }
                        visible_rows = rows.len();
                        visible_ids = rows
                            .iter()
                            .map(|hit| (hit.collection.clone(), hit.record_id.clone()))
                            .collect();
                        event_rows += rows.len() as u64;
                        event_bytes += rows
                            .iter()
                            .map(|hit| (hit.collection.len() + hit.record_id.len()) as u64)
                            .sum::<u64>();
                    }
                    StreamEvent::Progress { .. } => {}
                }
                if visible_rows == case.limit {
                    first_full_page_ns.get_or_insert_with(|| started.elapsed().as_nanos());
                }
                callback_ns += callback_start.elapsed().as_nanos();
                Ok(StreamControl::Continue)
            },
        )
        .map_err(|error| error.to_string())?;
    let stream_ns = started.elapsed().as_nanos();
    let StreamOutcome::Complete(summary) = outcome else {
        return Err(format!("{} unexpectedly cancelled", case.id));
    };
    if summary.count != collected.count || summary.returned_rows != collected.hits.len() {
        return Err(format!(
            "{} collected and streamed result size differ",
            case.id
        ));
    }
    let collected_ids: Vec<_> = collected
        .hits
        .iter()
        .map(|hit| (hit.collection.clone(), hit.record_id.clone()))
        .collect();
    if visible_ids != collected_ids {
        return Err(format!("{} collected and streamed page differs", case.id));
    }
    Ok(json!({
        "id": case.id,
        "filter": case.filter,
        "fts": case.fts,
        "sort": case.sort,
        "limit": case.limit,
        "offset": case.offset,
        "stream_mode": match case.stream { StreamMode::OrderedRows => "ordered_rows", StreamMode::PageReplacements => "page_replacements" },
        "count": collected.count,
        "page": collected.hits.len(),
        "collected_ns": collected_ns,
        "warm_collected_ns": warm_collected.as_ref().map(|(elapsed, _)| elapsed),
        "warm_collected_counters": warm_collected.as_ref().map(|(_, counters)| counters),
        "stream_ns": stream_ns,
        "first_data_ns": first_data_ns,
        "first_full_page_ns": first_full_page_ns,
        "callback_ns": callback_ns,
        "events": events,
        "data_events": data_events,
        "event_rows": event_rows,
        "event_identity_bytes": event_bytes,
        "collected_counters": collected.counters,
        "stream_counters": summary.counters,
        "stream_progress": {"sequence": summary.progress.sequence, "peak_buffered_bytes": summary.progress.peak_buffered_bytes},
    }))
}

fn run() -> Result<(), String> {
    let config = parse_args()?;
    let collection_name = config
        .source_path
        .rsplit('/')
        .next()
        .ok_or("source path has no collection component")?;
    let mut definition =
        CollectionDefinition::folder(collection_name, &config.source_path, SourceFormat::Json);
    definition.sort_fields = ["@file.size", "@file.modified"]
        .into_iter()
        .map(FieldPath::parse)
        .collect::<Result<_, _>>()?;
    definition.schema.register_composite_sort(vec![
        SortField::simple("@file.size", true),
        SortField::simple("@file.modified", false),
    ])?;
    let options = RepositoryOptions {
        bigram_alphabet: config.alphabet,
        snapshot_mode: config.snapshot,
        build_workers: config.build_workers,
        query_workers: config.query_workers,
        verified_cache_bytes: config.verified_cache_bytes,
        inference: InferenceConfig {
            mode: config.inference,
            ..Default::default()
        },
        ..Default::default()
    };
    let started = Instant::now();
    let repo = Repository::open_with_options(&config.root, vec![definition], options)?;
    let readiness_ns = started.elapsed().as_nanos();
    let save_started = Instant::now();
    if config.write_snapshot && !repo.startup().restored {
        repo.save_snapshot(&snapshot_path(&config.root, config.alphabet))?;
    }
    let snapshot_write_ns = if config.write_snapshot && !repo.startup().restored {
        save_started.elapsed().as_nanos() as u64
    } else { 0 };
    let collection = &repo.collections()[0];
    let mut hash = Sha256::new();
    hash.update(bincode::serialize(&repo.files().files).map_err(|error| error.to_string())?);
    let corpus_sha256 = format!("{:x}", hash.finalize());
    let snapshot_bytes = if config.snapshot == SnapshotMode::Disabled && !config.write_snapshot {
        None
    } else {
        std::fs::metadata(snapshot_path(&config.root, config.alphabet))
            .ok()
            .map(|metadata| metadata.len())
    };
    let inference_report = repo.inference_report(collection_name);
    let rss_after_open = resident_bytes("VmRSS:");
    let peak_after_open = resident_bytes("VmHWM:");
    let mut rows = Vec::new();
    if !config.skip_queries {
        for case in cases() {
            rows.push(measure_case(&repo, case, config.verified_cache_bytes > 0)?);
        }
    }
    println!(
        "{}",
        json!({
            "workload": "optional-features-v1",
            "root": config.root.canonicalize().map_err(|error| error.to_string())?,
            "source_path": config.source_path,
            "collection": collection_name,
            "schema_fields": collection.definition.schema.fields().len(),
            "stored_sort_fields": ["@file.size", "@file.modified"],
            "corpus_sha256": corpus_sha256,
            "alphabet": config.alphabet,
            "inference": format!("{:?}", config.inference),
            "snapshot_mode": format!("{:?}", config.snapshot),
            "snapshot_restored": repo.startup().restored,
            "snapshot_bytes": snapshot_bytes,
            "verified_cache_bytes": config.verified_cache_bytes,
            "requested_build_workers": config.build_workers.map(NonZeroUsize::get),
            "resolved_build_workers": repo.startup().build_workers,
            "query_workers": config.query_workers,
            "readiness_ns": readiness_ns,
            "build_ns": repo.startup().build_ns,
            "discovery_ns": repo.startup().build_discovery_ns,
            "extraction_index_ns": repo.startup().build_extraction_index_ns,
            "finalization_ns": repo.startup().build_finalization_ns,
            "snapshot_write_ns": snapshot_write_ns,
            "snapshot_decode_ns": repo.startup().snapshot_decode_ns,
            "build_parses": repo.build_parses(),
            "failed_build_parses": repo.build_failed_parses(),
            "file_count": collection.generation.files.len(),
            "record_count": collection.generation.records.len(),
            "inference_report": inference_report,
            "structural_bitmap_bytes": collection.empty_array_records.serialized_size(),
            "rss_after_open_bytes": rss_after_open,
            "peak_rss_after_open_bytes": peak_after_open,
            "peak_rss_after_queries_bytes": resident_bytes("VmHWM:"),
            "cases": rows,
        })
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("optional benchmark error: {error}");
        std::process::exit(1);
    }
}
