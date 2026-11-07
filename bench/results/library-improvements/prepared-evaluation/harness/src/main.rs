use std::alloc::{GlobalAlloc, Layout, System};
use std::collections::HashSet;
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use objekte_search_core::inference::InferenceConfig;
use objekte_search_core::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SourceRevision,
};
use objekte_search_core::model::{FileId, LogicalType, RecordId, RecordSlot, Value};
use objekte_search_core::query::{CaseSensitivity, FtsMode, Query, parse_filter};
use objekte_search_core::reference::{
    BoundExpr, FieldDefinition, RecordContext, Schema, bind_filter, eval_filter,
};
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SearchResult, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{ReturnMode, WorkloadCase};

struct CountedSystem;

static ALLOCATION_CALLS: AtomicU64 = AtomicU64::new(0);
static REQUESTED_BYTES: AtomicU64 = AtomicU64::new(0);

// The wrapper delegates to the standard allocator and counts requested sizes.
// It is confined to this standalone benchmark, not linked into the library.
unsafe impl GlobalAlloc for CountedSystem {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        REQUESTED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        REQUESTED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATION_CALLS.fetch_add(1, Ordering::Relaxed);
        REQUESTED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: CountedSystem = CountedSystem;

#[derive(Clone, Copy)]
struct Sample {
    calls: u64,
    requested_bytes: u64,
    elapsed_ns: u128,
}

fn measured(mut operation: impl FnMut()) -> Vec<Sample> {
    let mut samples = Vec::with_capacity(9);
    for _ in 0..9 {
        ALLOCATION_CALLS.store(0, Ordering::Relaxed);
        REQUESTED_BYTES.store(0, Ordering::Relaxed);
        let start = Instant::now();
        operation();
        let elapsed_ns = start.elapsed().as_nanos();
        let calls = ALLOCATION_CALLS.load(Ordering::Relaxed);
        let requested_bytes = REQUESTED_BYTES.load(Ordering::Relaxed);
        samples.push(Sample {
            calls,
            requested_bytes,
            elapsed_ns,
        });
    }
    samples
}

fn row(
    label: &str,
    kind: &str,
    iterations: usize,
    signature: &str,
    samples: &[Sample],
) -> serde_json::Value {
    let median = |select: fn(&Sample) -> u128| {
        let mut values = samples.iter().map(select).collect::<Vec<_>>();
        values.sort_unstable();
        values[values.len() / 2]
    };
    serde_json::json!({
        "case": label,
        "kind": kind,
        "iterations_per_sample": iterations,
        "signature": signature,
        "median_calls": median(|sample| sample.calls as u128),
        "median_requested_bytes": median(|sample| sample.requested_bytes as u128),
        "median_elapsed_ns": median(|sample| sample.elapsed_ns),
        "samples": samples.iter().map(|sample| serde_json::json!({
            "calls": sample.calls,
            "requested_bytes": sample.requested_bytes,
            "elapsed_ns": sample.elapsed_ns,
        })).collect::<Vec<_>>(),
    })
}

fn field(schema: &mut Schema, path: &str, kind: LogicalType, multi: bool) {
    schema
        .register_field(FieldDefinition {
            path: path.split('.').map(str::to_owned).collect(),
            kind,
            required: false,
            nullable: true,
            indexed: false,
            multi,
            element_nullable: Vec::new(),
        })
        .unwrap();
}

fn direct_context<'a>(
    value: &'a Value,
    file: &'a FileEntry,
    record: &'a RecordEntry,
) -> RecordContext<'a> {
    RecordContext {
        value,
        source_bytes: None,
        file,
        record,
        collection_name: "direct",
        repository_root: Path::new("/tmp"),
    }
}

fn direct_rows() -> Vec<serde_json::Value> {
    let mut schema = Schema::default();
    field(&mut schema, "name", LogicalType::String, false);
    field(
        &mut schema,
        "tags",
        LogicalType::Array(Box::new(LogicalType::String)),
        true,
    );
    let value = Value::from_json(serde_json::json!({
        "name": "Hymn", "tags": ["blue", "black", "red"]
    }))
    .unwrap();
    let revision = SourceRevision {
        modified_ns: 0,
        size: 1,
    };
    let file = FileEntry {
        id: FileId(0),
        relative_path: "direct.json".into(),
        revision,
        created_ns: None,
        collections: Vec::new(),
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    };
    let record = RecordEntry {
        slot: RecordSlot(0),
        id: RecordId("direct".into()),
        file: FileId(0),
        range: ByteRange { start: 0, end: 1 },
        revision,
        class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
    };
    let context = direct_context(&value, &file, &record);
    [
        ("scalar_equality", "name = 'Hymn'"),
        ("scalar_wildcard", "name ~ 'Hy%n'"),
        ("array_wildcard", "tags ?~ 'b%ue'"),
    ]
    .into_iter()
    .map(|(label, text)| {
        let bound: BoundExpr = bind_filter(&parse_filter(text).unwrap(), &schema).unwrap();
        let mut evidence = HashSet::with_capacity(8);
        for _ in 0..10 {
            evidence.clear();
            assert!(eval_filter(&bound, &context, &mut evidence).unwrap());
        }
        let samples = measured(|| {
            for _ in 0..1000 {
                evidence.clear();
                black_box(eval_filter(&bound, &context, &mut evidence).unwrap());
            }
        });
        row(label, "direct_bound_filter", 1000, "true", &samples)
    })
    .collect()
}

fn definition(name: &str, schema: Schema) -> CollectionDefinition {
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: Vec::new(),
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn fixture() -> (PathBuf, Vec<CollectionDefinition>, u64) {
    let root = std::env::temp_dir().join(format!("objekte-q6-alloc-{}", std::process::id()));
    for name in ["entries", "mentions"] {
        std::fs::create_dir_all(root.join(name)).unwrap();
    }
    let entries = (0..32)
        .map(|index| {
            format!(
                r#"{{"id":"E{index:03}","title":"{}"}}"#,
                if index % 2 == 0 { "Hymn" } else { "Festival" }
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let mentions = (0..128).map(|index| format!(
        r#"{{"id":"M{index:03}","entry":"E{:03}","title":"{}","local_title":"{}","tags":["blue","black","red"]}}"#,
        index % 32,
        if index % 4 == 0 { "Hymn" } else { "Other" },
        if index % 2 == 0 { "Hymn" } else { "Festival" },
    )).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(root.join("entries/data.jsonl"), &entries).unwrap();
    std::fs::write(root.join("mentions/data.jsonl"), &mentions).unwrap();
    let corpus_fingerprint = entries
        .bytes()
        .chain(mentions.bytes())
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ byte as u64).wrapping_mul(0x100000001b3)
        });
    let mut entry_schema = Schema::default();
    field(&mut entry_schema, "id", LogicalType::Id, false);
    field(&mut entry_schema, "title", LogicalType::String, false);
    let mut mention_schema = Schema::default();
    field(&mut mention_schema, "id", LogicalType::Id, false);
    field(&mut mention_schema, "entry", LogicalType::Id, false);
    field(&mut mention_schema, "title", LogicalType::String, false);
    field(
        &mut mention_schema,
        "local_title",
        LogicalType::String,
        false,
    );
    field(
        &mut mention_schema,
        "tags",
        LogicalType::Array(Box::new(LogicalType::String)),
        true,
    );
    mention_schema
        .register_relation(RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "entries".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("mentions".into()),
        })
        .unwrap();
    (
        root,
        vec![
            definition("entries", entry_schema),
            definition("mentions", mention_schema),
        ],
        corpus_fingerprint,
    )
}

fn query(
    label: &str,
    filter: Option<&str>,
    fts: Option<&str>,
    fields: &[&str],
    evidence: bool,
) -> Query {
    Query::from_workload(&WorkloadCase {
        id: label.into(),
        collection: "mentions".into(),
        fts: fts.map(str::to_owned),
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Literal,
        filter: filter.map(str::to_owned),
        fts_fields: fields.iter().map(|field| (*field).to_owned()).collect(),
        sort: Vec::new(),
        limit: 12,
        offset: 0,
        count: true,
        return_mode: ReturnMode::Locator,
        evidence,
    })
    .unwrap()
}

fn signature(result: &SearchResult) -> String {
    let rows = result
        .hits
        .iter()
        .map(|hit| (&hit.record_id, &hit.matched_fields))
        .collect::<Vec<_>>();
    serde_json::to_string(&(result.count, rows)).unwrap()
}

fn repository_rows() -> (Vec<serde_json::Value>, u64) {
    let (root, definitions, corpus_fingerprint) = fixture();
    let repository = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            prepared_cache_bytes: 4 * 1024 * 1024,
            verified_cache_bytes: 0,
            query_workers: 1,
            build_workers: Some(NonZeroUsize::new(1).unwrap()),
            inference: InferenceConfig::default(),
            ..Default::default()
        },
    )
    .unwrap();
    let cases = [
        (
            "fts_local_membership",
            None,
            Some("Hymn"),
            vec!["local_title", "title"],
            false,
        ),
        (
            "fts_local_evidence",
            None,
            Some("Hymn"),
            vec!["local_title", "title"],
            true,
        ),
        (
            "fts_mixed_membership",
            None,
            Some("Hymn"),
            vec!["local_title", "entry.title"],
            false,
        ),
        (
            "fts_mixed_evidence",
            None,
            Some("Hymn"),
            vec!["local_title", "entry.title"],
            true,
        ),
        (
            "filter_mixed_membership",
            Some("local_title = entry.title || title = 'Hymn'"),
            None,
            vec![],
            false,
        ),
        (
            "filter_mixed_evidence",
            Some("local_title = entry.title || title = 'Hymn'"),
            None,
            vec![],
            true,
        ),
    ];
    let rows = cases
        .into_iter()
        .map(|(label, filter, fts, fields, evidence)| {
            let request = query(label, filter, fts, &fields, evidence);
            let reference = repository.search_reference(&request).unwrap();
            let expected = signature(&reference);
            for _ in 0..3 {
                let warm = repository.search(&request).unwrap();
                assert_eq!(signature(&warm), expected, "{label}");
            }
            let samples = measured(|| {
                black_box(repository.search(&request).unwrap());
            });
            row(label, "warmed_repository_search", 1, &expected, &samples)
        })
        .collect();
    drop(repository);
    std::fs::remove_dir_all(root).unwrap();
    (rows, corpus_fingerprint)
}

fn main() {
    let direct = direct_rows();
    let (repository, corpus_fingerprint) = repository_rows();
    let code_sha = std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    let code_sha = String::from_utf8(code_sha.stdout)
        .unwrap()
        .trim()
        .to_owned();
    println!(
        "{}",
        serde_json::json!({
            "code_sha": code_sha,
            "corpus_fingerprint_fnv64": format!("{corpus_fingerprint:016x}"),
            "allocator": "System wrapper; alloc/alloc_zeroed/realloc calls and requested bytes; deallocation excluded",
            "samples_per_case": 9,
            "direct": direct,
            "repository": repository,
        })
    );
}
