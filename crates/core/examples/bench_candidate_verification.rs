//! Compare decoded-scalar extraction in memory with complete warm searches.

use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashSet;
use std::fs;
use std::fs::OpenOptions;
use std::hint::black_box;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use objekte_search_core::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SourceRevision,
};
use objekte_search_core::model::{CollectionId, FileId, RecordId, RecordSlot};
use objekte_search_core::query::{CaseSensitivity, FtsMode, Query};
use objekte_search_core::reference::{
    RecordContext, Schema, bind_filter, bind_fts, eval_bound_fts, eval_filter,
};
use objekte_search_core::repository::{
    CollectionDefinition, Payload, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::{
    JsonDecodedScalarsReader, ScanProjection, SourceFormat, SourceReader,
};
use objekte_search_core::workload::{ReturnMode, WorkloadCase};
use serde_json::json;
use sha2::{Digest, Sha256};


/// Counts gross allocation work only during the separate instrumented pass.
struct CountingAllocator;


static COUNTING_ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATION_COUNT: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;


unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let allocation = unsafe { System.alloc(layout) };
        if !allocation.is_null() && COUNTING_ENABLED.load(Ordering::Relaxed) {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        allocation
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let allocation = unsafe { System.alloc_zeroed(layout) };
        if !allocation.is_null() && COUNTING_ENABLED.load(Ordering::Relaxed) {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        allocation
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let allocation = unsafe { System.realloc(pointer, layout, new_size) };
        if !allocation.is_null() && COUNTING_ENABLED.load(Ordering::Relaxed) {
            ALLOCATION_COUNT.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        allocation
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}


/// A fixed predicate and extraction shape shared by both benchmark levels.
struct Case {
    id: &'static str,
    fts: Option<&'static str>,
    fts_case_insensitive: bool,
    filter: Option<&'static str>,
    projection: ScanProjection,
}


impl Case {
    fn query(&self, limit: usize) -> Result<Query, String> {
        Query::from_workload(&WorkloadCase {
            id: self.id.into(),
            collection: "records".into(),
            fts: self.fts.map(str::to_owned),
            fts_case_sensitivity: if self.fts_case_insensitive {
                CaseSensitivity::Insensitive
            } else {
                CaseSensitivity::Sensitive
            },
            fts_mode: FtsMode::Literal,
            filter: self.filter.map(str::to_owned),
            fts_fields: Vec::new(),
            sort: Vec::new(),
            limit,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        })
    }
}


fn cases() -> Vec<Case> {
    let fields = |names: &[&str]| Some(names.iter().map(|name| (*name).into()).collect());
    vec![
        Case {
            id: "fts_good",
            fts: Some("good"),
            fts_case_insensitive: false,
            filter: None,
            projection: ScanProjection::full(true),
        },
        Case {
            id: "fts_good_insensitive",
            fts: Some("GOOD"),
            fts_case_insensitive: true,
            filter: None,
            projection: ScanProjection::full(true),
        },
        Case {
            id: "fts_accent_insensitive",
            fts: Some("É"),
            fts_case_insensitive: true,
            filter: None,
            projection: ScanProjection::full(true),
        },
        Case {
            id: "filter_votes_rating",
            fts: None,
            fts_case_insensitive: false,
            filter: Some("helpful_vote > 3 && rating = 5"),
            projection: ScanProjection {
                top_level: fields(&["helpful_vote", "rating"]),
                collect_text: false,
                resolved_text_only: false,
            },
        },
        Case {
            id: "filter_text_contains",
            fts: None,
            fts_case_insensitive: false,
            filter: Some("text ~ 'good'"),
            projection: ScanProjection {
                top_level: fields(&["text"]),
                collect_text: false,
                resolved_text_only: false,
            },
        },
        Case {
            id: "fts_and_filter",
            fts: Some("good"),
            fts_case_insensitive: false,
            filter: Some("helpful_vote > 3 && rating = 5"),
            projection: ScanProjection {
                top_level: fields(&["helpful_vote", "rating"]),
                collect_text: true,
                resolved_text_only: false,
            },
        },
    ]
}

fn sample_dir(root: &Path) -> Result<Vec<(String, Vec<u8>)>, String> {
    let mut paths = fs::read_dir(root.join("data"))
        .map_err(|error| error.to_string())?
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
    paths
        .into_iter()
        .map(|path| {
            let name = path
                .file_name()
                .ok_or("record filename missing")?
                .to_string_lossy()
                .into_owned();
            Ok((name, fs::read(path).map_err(|error| error.to_string())?))
        })
        .collect()
}

fn median(values: &mut [u128]) -> u128 {
    values.sort_unstable();
    values[values.len() / 2]
}

fn digest<'a>(members: impl Iterator<Item = &'a str>) -> String {
    let mut hash = Sha256::new();
    for member in members {
        hash.update((member.len() as u64).to_le_bytes());
        hash.update(member.as_bytes());
    }
    format!("{:x}", hash.finalize())
}

fn corpus_digest(records: &[(String, Vec<u8>)]) -> String {
    let mut hash = Sha256::new();
    for (name, bytes) in records {
        hash.update((name.len() as u64).to_le_bytes());
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

fn in_memory(
    case: &Case,
    query: &Query,
    records: &[(String, Vec<u8>)],
    repetitions: usize,
    trusted: bool,
) -> Result<serde_json::Value, String> {
    let schema = Schema::default();
    let filter = query
        .filter
        .as_ref()
        .map(|filter| bind_filter(filter, &schema))
        .transpose()?;
    let fts = query
        .fts
        .as_ref()
        .map(|fts| bind_fts(fts, &schema))
        .transpose()?;
    let file = FileEntry {
        id: FileId(0),
        relative_path: PathBuf::from("data/record.json").into(),
        revision: SourceRevision {
            modified_ns: 0,
            size: 0,
        },
        created_ns: None,
        collections: vec![CollectionId(0)],
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    };
    let record = RecordEntry {
        slot: RecordSlot(0),
        id: RecordId("record".into()),
        file: FileId(0),
        range: ByteRange { start: 0, end: 0 },
        revision: file.revision,
        class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
    };
    let reader = JsonDecodedScalarsReader;
    let scan = || -> Result<Vec<&str>, String> {
        let mut members = Vec::new();
        for (name, bytes) in records {
            let extracted = if trusted {
                reader.parse_for_query_trusted(bytes, &case.projection)?
            } else {
                reader.parse_for_query(bytes, &case.projection)?
            };
            let context = RecordContext {
                value: &extracted.value,
                source_bytes: Some(bytes),
                record: &record,
                file: &file,
                collection_name: "records",
                repository_root: Path::new("."),
            };
            let mut evidence = HashSet::new();
            let fts_match = fts.as_ref().map_or(Ok(true), |fts| {
                eval_bound_fts(fts, &extracted.text, &context, &mut evidence)
            })?;
            let filter_match = if fts_match {
                filter.as_ref().map_or(Ok(true), |filter| {
                    eval_filter(filter, &context, &mut evidence)
                })?
            } else {
                false
            };
            if fts_match && filter_match {
                members.push(name.as_str());
            }
        }
        Ok(members)
    };
    let mut times = Vec::with_capacity(repetitions);
    let mut final_members = Vec::new();
    for repetition in 0..=repetitions {
        let started = Instant::now();
        let members = scan()?;
        black_box(&members);
        let elapsed = started.elapsed().as_nanos();
        if repetition > 0 {
            times.push(elapsed);
            final_members = members;
        }
    }
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    COUNTING_ENABLED.store(true, Ordering::Relaxed);
    let instrumented_members = scan();
    COUNTING_ENABLED.store(false, Ordering::Relaxed);
    let instrumented_members = instrumented_members?;
    if instrumented_members != final_members {
        return Err(format!("{}: allocation pass changed membership", case.id));
    }
    let allocations = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
    let min = *times.iter().min().ok_or("no in-memory samples")?;
    let max = *times.iter().max().ok_or("no in-memory samples")?;
    let median = median(&mut times);
    Ok(json!({
        "case": case.id,
        "level": if trusted {
            "in_memory_trusted_parse_extract_evaluate"
        } else {
            "in_memory_validating_parse_extract_evaluate"
        },
        "records": records.len(),
        "matches": final_members.len(),
        "member_sha256": digest(final_members.into_iter()),
        "times_ns": times,
        "min_ns": min,
        "median_ns": median,
        "max_ns": max,
        "allocations": allocations,
        "allocated_bytes": allocated_bytes,
        "allocations_per_candidate": allocations as f64 / records.len() as f64,
        "allocated_bytes_per_candidate": allocated_bytes as f64 / records.len() as f64,
    }))
}

fn repository_search(
    repo: &Repository,
    case: &Case,
    query: &Query,
    repetitions: usize,
) -> Result<serde_json::Value, String> {
    let mut times = Vec::with_capacity(repetitions);
    let mut final_result = None;
    for repetition in 0..=repetitions {
        let started = Instant::now();
        let result = repo.search(query)?;
        let elapsed = started.elapsed().as_nanos();
        if repetition > 0 {
            times.push(elapsed);
            final_result = Some(result);
        }
    }
    let result = final_result.ok_or("no search samples")?;
    ALLOCATION_COUNT.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    COUNTING_ENABLED.store(true, Ordering::Relaxed);
    let instrumented_result = repo.search(query);
    COUNTING_ENABLED.store(false, Ordering::Relaxed);
    let instrumented_result = instrumented_result?;
    if instrumented_result.count != result.count
        || digest(
            instrumented_result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str()),
        ) != digest(result.hits.iter().map(|hit| hit.record_id.as_str()))
    {
        return Err(format!("{}: allocation pass changed membership", case.id));
    }
    let allocations = ALLOCATION_COUNT.load(Ordering::Relaxed);
    let allocated_bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
    let min = *times.iter().min().ok_or("no search samples")?;
    let max = *times.iter().max().ok_or("no search samples")?;
    let median = median(&mut times);
    Ok(json!({
        "case": case.id,
        "level": "warm_repository_search",
        "matches": result.count,
        "returned_records": result.hits.len(),
        "member_sha256": digest(result.hits.iter().map(|hit| {
            hit.record_id.strip_prefix("path:").unwrap_or(hit.record_id.as_str())
        })),
        "counters": result.counters,
        "times_ns": times,
        "min_ns": min,
        "median_ns": median,
        "max_ns": max,
        "allocations": allocations,
        "allocated_bytes": allocated_bytes,
    }))
}

fn borrowed_verification_cases(limit: usize) -> Result<Vec<(Case, Query)>, String> {
    let descriptions = [
        (
            "value_identity",
            Some("good"),
            false,
            Some("helpful_vote > 3 && rating = 5"),
            None,
            ReturnMode::Locator,
        ),
        (
            "value_folded",
            Some("GOOD"),
            true,
            Some("helpful_vote > 3 && rating = 5"),
            None,
            ReturnMode::Locator,
        ),
        (
            "source_identity",
            Some("good"),
            false,
            None,
            None,
            ReturnMode::Locator,
        ),
        (
            "source_folded",
            Some("GOOD"),
            true,
            None,
            None,
            ReturnMode::Locator,
        ),
        (
            "source_filter",
            None,
            false,
            Some("text ~ 'good'"),
            None,
            ReturnMode::Locator,
        ),
        (
            "source_combined",
            Some("good"),
            false,
            Some("text ~ 'good'"),
            None,
            ReturnMode::Locator,
        ),
        (
            "scoped_duplicates",
            Some("good"),
            false,
            None,
            Some("text"),
            ReturnMode::Locator,
        ),
        (
            "high_hit_logical",
            Some("good"),
            false,
            None,
            None,
            ReturnMode::Logical,
        ),
    ];
    descriptions
        .into_iter()
        .map(|(id, fts, insensitive, filter, field, mode)| {
            let case = Case {
                id,
                fts,
                fts_case_insensitive: insensitive,
                filter,
                projection: ScanProjection::full(true),
            };
            let mut query = case.query(limit)?;
            if let Some(field) = field {
                query.fts.as_mut().expect("field requires FTS").fields =
                    vec![objekte_search_core::query::FtsField {
                        path: objekte_search_core::model::FieldPath::parse(field)?,
                        query_path: None,
                        binding: None,
                    }];
            }
            query.options.return_mode = mode;
            Ok((case, query))
        })
        .collect()
}

fn write_oracle(
    path: &Path,
    repo: &Repository,
    cases: &[Case],
    record_count: usize,
) -> Result<(), String> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| error.to_string())?;
    let mut writer = BufWriter::new(file);
    for case in cases {
        for mode in [ReturnMode::Locator, ReturnMode::Bytes, ReturnMode::Logical] {
            let mut query = case.query(record_count)?;
            query.options.return_mode = mode.clone();
            let result = repo.search(&query)?;
            writeln!(
                writer,
                "{}",
                json!({
                    "kind": "query",
                    "case": case.id,
                    "return_mode": mode,
                    "exact_count": result.count,
                    "emitted_rows": result.hits.len(),
                    "source_diagnostics": result.counters.source_diagnostics,
                })
            )
            .map_err(|error| error.to_string())?;
            for (position, hit) in result.hits.into_iter().enumerate() {
                let payload = match hit.payload {
                    None => serde_json::Value::Null,
                    Some(Payload::Bytes(bytes)) => json!({
                        "kind": "bytes",
                        "source_json": String::from_utf8(bytes)
                            .map_err(|error| error.to_string())?,
                    }),
                    Some(Payload::Logical(value)) => json!({
                        "kind": "logical",
                        "value": value,
                    }),
                    Some(Payload::LogicalUnavailable { diagnostic_ref }) => json!({
                        "kind": "logical_unavailable",
                        "diagnostic_ref": diagnostic_ref.0,
                    }),
                };
                writeln!(
                    writer,
                    "{}",
                    json!({
                        "kind": "hit",
                        "case": case.id,
                        "return_mode": mode,
                        "position": position,
                        "collection": hit.collection,
                        "record_id": hit.record_id,
                        "matched_fields": hit.matched_fields,
                        "file_status": hit.file_status.label(),
                        "record_class": format!("{:?}", hit.record_class),
                        "diagnostic_ref": hit.diagnostic_ref.0,
                        "payload": payload,
                    })
                )
                .map_err(|error| error.to_string())?;
            }
        }
    }
    writer.flush().map_err(|error| error.to_string())
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    let mut repetitions = 7usize;
    let mut workers = 4usize;
    let mut oracle_output = None;
    let mut borrowed_focused = false;
    let mut source_format = SourceFormat::JsonDecodedScalars;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--root" => root = Some(PathBuf::from(args.next().ok_or("--root needs path")?)),
            "--repetitions" => {
                repetitions = args
                    .next()
                    .ok_or("--repetitions needs count")?
                    .parse()
                    .map_err(|error| format!("invalid repetitions: {error}"))?
            }
            "--workers" => {
                workers = args
                    .next()
                    .ok_or("--workers needs count")?
                    .parse()
                    .map_err(|error| format!("invalid workers: {error}"))?
            }
            "--oracle-output" => {
                oracle_output = Some(PathBuf::from(
                    args.next().ok_or("--oracle-output needs path")?,
                ))
            }
            "--borrowed-focused" => borrowed_focused = true,
            "--format" => {
                source_format = match args.next().as_deref() {
                    Some("json") => SourceFormat::Json,
                    Some("decoded") => SourceFormat::JsonDecodedScalars,
                    _ => return Err("--format requires json or decoded".into()),
                }
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    if repetitions == 0 || workers == 0 {
        return Err("repetitions and workers must be positive".into());
    }
    let root = root.ok_or("--root is required")?;
    let records = sample_dir(&root)?;
    let bytes = records.iter().map(|(_, bytes)| bytes.len()).sum::<usize>();
    let corpus_sha256 = corpus_digest(&records);
    let started = Instant::now();
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "records",
            "data",
            source_format,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            query_workers: workers,
            verified_cache_bytes: 0,
            ..RepositoryOptions::default()
        },
    )?;
    println!(
        "{}",
        json!({
            "level": "setup",
            "root": root,
            "records": records.len(),
            "source_bytes": bytes,
            "corpus_sha256": corpus_sha256,
            "build_ns": started.elapsed().as_nanos(),
            "build_parses": repo.build_parses(),
            "workers": workers,
            "verified_cache_bytes": 0,
        })
    );
    if borrowed_focused {
        for (case, query) in borrowed_verification_cases(records.len())? {
            println!("{}", repository_search(&repo, &case, &query, repetitions)?);
        }
        return Ok(());
    }
    let cases = cases();
    for case in &cases {
        let query = case.query(records.len())?;
        let memory = in_memory(case, &query, &records, repetitions, false)?;
        let trusted = in_memory(case, &query, &records, repetitions, true)?;
        let complete = repository_search(&repo, case, &query, repetitions)?;
        if memory["matches"] != complete["matches"]
            || memory["member_sha256"] != complete["member_sha256"]
            || trusted["matches"] != complete["matches"]
            || trusted["member_sha256"] != complete["member_sha256"]
        {
            return Err(format!(
                "{}: in-memory and repository results differ",
                case.id
            ));
        }
        println!("{memory}");
        println!("{trusted}");
        println!("{complete}");
    }
    if let Some(path) = oracle_output {
        write_oracle(&path, &repo, &cases, records.len())?;
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("candidate verification benchmark: {error}");
        std::process::exit(1);
    }
}
