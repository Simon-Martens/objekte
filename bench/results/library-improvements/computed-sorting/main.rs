use std::hint::black_box;
use std::path::PathBuf;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{Query, QueryOptions};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Payload, Repository, RepositoryOptions, SearchResult,
    SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::workload::{ReturnMode, SortExpression, SortField};
use sha2::{Digest, Sha256};

fn hash_file(hash: &mut Sha256, relative: &str, bytes: &[u8]) {
    hash.update(relative.as_bytes());
    hash.update([0]);
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn hash_slice(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

fn answer_sha256(result: &SearchResult) -> String {
    let mut hash = Sha256::new();
    hash.update((result.count.expect("count requested") as u64).to_le_bytes());
    hash.update((result.hits.len() as u64).to_le_bytes());
    for hit in &result.hits {
        hash_slice(&mut hash, hit.collection.as_bytes());
        hash_slice(&mut hash, hit.record_id.as_bytes());
        hash_slice(&mut hash, hit.file_path.to_string_lossy().as_bytes());
        hash.update(hit.byte_start.to_le_bytes());
        hash.update(hit.byte_end.to_le_bytes());
        hash.update((hit.matched_fields.len() as u64).to_le_bytes());
        for field in &hit.matched_fields {
            hash_slice(&mut hash, field.as_bytes());
        }
        match &hit.payload {
            Some(Payload::Bytes(bytes)) => hash_slice(&mut hash, bytes),
            _ => panic!("byte payload requested"),
        }
    }
    format!("{:x}", hash.finalize())
}

fn field(schema: &mut Schema, name: &str, kind: LogicalType) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
}

fn fixture(
    mode: &str,
    layout: &str,
    reuse: &str,
    rows: usize,
) -> (PathBuf, Vec<CollectionDefinition>, String) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let parent = PathBuf::from(
        std::env::var("Q4_FIXTURE_PARENT").expect("Q4_FIXTURE_PARENT must be worktree local"),
    );
    let root = parent.join(format!("objekte-q4-bench-{}-{nonce}", std::process::id()));
    let source = root.join("sources");
    std::fs::create_dir_all(&source).unwrap();
    let payload = "x".repeat(4096);
    let target_count = if reuse == "high" { 8 } else { rows };
    let mut hash = Sha256::new();
    let mut lines = Vec::with_capacity(rows);
    for index in 0..rows {
        let title = format!("title-{:04}", (index * 37) % (rows / 4).max(1));
        let row = if mode == "related" {
            format!(
                "{{\"id\":\"S{index:04}\",\"entry\":\"T{:04}\",\"payload\":\"{payload}\"}}\n",
                index % target_count
            )
        } else {
            format!("{{\"id\":\"S{index:04}\",\"title\":\"{title}\",\"payload\":\"{payload}\"}}\n")
        };
        lines.push(row);
    }
    if layout == "many" {
        for (index, line) in lines.iter().enumerate() {
            let relative = format!("sources/{index:04}.jsonl");
            hash_file(&mut hash, &relative, line.as_bytes());
            std::fs::write(root.join(relative), line).unwrap();
        }
    } else {
        let all = lines.concat();
        hash_file(&mut hash, "sources/all.jsonl", all.as_bytes());
        std::fs::write(source.join("all.jsonl"), all).unwrap();
    }
    let mut source_definition =
        CollectionDefinition::folder("sources", "sources", SourceFormat::JsonLines);
    source_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    field(&mut source_definition.schema, "id", LogicalType::Id);
    if mode == "related" {
        field(&mut source_definition.schema, "entry", LogicalType::Id);
        source_definition
            .schema
            .register_relation(RelationDefinition {
                path: vec!["entry".into()],
                target_collection: "targets".into(),
                cardinality: RelationCardinality::One,
                reverse_alias: None,
            })
            .unwrap();
        let target_path = root.join("targets");
        std::fs::create_dir_all(&target_path).unwrap();
        for index in 0..target_count {
            let relative = format!("targets/{index:04}.json");
            let contents = format!(
                "{{\"id\":\"T{index:04}\",\"title\":\"target-{:04}\"}}",
                (index * 29) % target_count
            );
            hash_file(&mut hash, &relative, contents.as_bytes());
            std::fs::write(root.join(relative), contents).unwrap();
        }
        let mut target_definition =
            CollectionDefinition::folder("targets", "targets", SourceFormat::Json);
        target_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        field(&mut target_definition.schema, "id", LogicalType::Id);
        field(&mut target_definition.schema, "title", LogicalType::String);
        (
            root,
            vec![source_definition, target_definition],
            format!("{:x}", hash.finalize()),
        )
    } else {
        (
            root,
            vec![source_definition],
            format!("{:x}", hash.finalize()),
        )
    }
}

fn query(mode: &str) -> Query {
    Query {
        collections: vec!["sources".into()],
        filter: None,
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::computed(
                SortExpression {
                    path: if mode == "related" {
                        "entry.title"
                    } else {
                        "title"
                    }
                    .into(),
                    binding: (mode == "local").then(|| "lower".into()),
                    reducer: None,
                },
                false,
            )],
            limit: 32,
            offset: 16,
            count: true,
            return_mode: ReturnMode::Bytes,
            evidence: false,
        },
    }
}

fn main() {
    let mode = std::env::args()
        .nth(1)
        .expect("usage: q4-bench local|related");
    assert!(matches!(mode.as_str(), "local" | "related"));
    let rows: usize = std::env::var("Q4_ROWS")
        .unwrap_or_else(|_| "256".into())
        .parse()
        .unwrap();
    let repetitions: usize = std::env::var("Q4_REPS")
        .unwrap_or_else(|_| "7".into())
        .parse()
        .unwrap();
    assert!(rows >= 48 && repetitions > 0);
    let sha = std::env::var("Q4_SHA").unwrap_or_else(|_| "unknown".into());
    let hash_only = std::env::var("Q4_FIXTURE_HASH_ONLY").is_ok_and(|value| value == "1");
    println!(
        "{}",
        serde_json::json!({
            "kind": "run", "sha": sha, "mode": mode, "rows": rows,
            "repetitions": repetitions, "rustc": std::env::var("Q4_RUSTC").unwrap_or_default(),
            "profile": std::env::var("Q4_PROFILE").unwrap_or_default()
        })
    );
    let reuse_cases: &[&str] = if mode == "related" {
        &["high", "low"]
    } else {
        &["none"]
    };
    for layout in ["many", "one"] {
        for &reuse in reuse_cases {
            let fixture_start = Instant::now();
            let (root, definitions, fixture_sha256) = fixture(&mode, layout, reuse, rows);
            let fixture_ns = fixture_start.elapsed().as_nanos() as u64;
            if hash_only {
                println!(
                    "{}",
                    serde_json::json!({
                        "kind": "fixture", "mode": mode, "layout": layout, "reuse": reuse,
                        "rows": rows, "fixture_sha256": fixture_sha256
                    })
                );
                std::fs::remove_dir_all(root).unwrap();
                continue;
            }
            for workers in [1, 2, 4, 8] {
                let build_start = Instant::now();
                let repo = Repository::open_with_options(
                    &root,
                    definitions.clone(),
                    RepositoryOptions {
                        query_workers: workers,
                        verified_cache_bytes: 0,
                        snapshot_mode: SnapshotMode::Disabled,
                        ..Default::default()
                    },
                )
                .unwrap();
                let build_ns = build_start.elapsed().as_nanos() as u64;
                println!(
                    "{}",
                    serde_json::json!({
                        "kind": "build", "sha": sha, "mode": mode, "layout": layout,
                        "reuse": reuse, "workers": workers, "fixture_ns": fixture_ns,
                        "build_ns": build_ns, "fixture_sha256": fixture_sha256
                    })
                );
                let request = query(&mode);
                let mut last = None;
                for repetition in 0..=repetitions {
                    let started = Instant::now();
                    let result = repo.search(&request).unwrap();
                    let elapsed_ns = started.elapsed().as_nanos() as u64;
                    let answer_sha256 = answer_sha256(&result);
                    black_box(result.hits.len());
                    println!(
                        "{}",
                        serde_json::json!({
                            "kind": "query", "sha": sha, "mode": mode, "layout": layout,
                            "reuse": reuse, "workers": workers,
                            "phase": if repetition == 0 { "first" } else { "repeat" },
                            "repetition": repetition, "elapsed_ns": elapsed_ns,
                            "count": result.count,
                            "answer_sha256": answer_sha256,
                            "sort_target_reads": result.counters.sort_target_reads,
                            "sort_target_reuse": result.counters.sort_target_reuse,
                            "peak_retained_sort_key_bytes": result.counters.peak_retained_sort_key_bytes,
                            "peak_retained_payload_bytes": result.counters.peak_retained_payload_bytes,
                            "bytes_read": result.counters.bytes_read
                        })
                    );
                    last = Some(result);
                }
                let reference = repo.search_reference(&request).unwrap();
                assert_eq!(last.unwrap().hits, reference.hits);
                drop(repo);
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
