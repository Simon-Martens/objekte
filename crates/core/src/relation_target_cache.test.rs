use super::*;
use crate::model::LogicalType;
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use crate::source::SourceFormat;
use crate::streaming::{CancellationToken, StreamControl, StreamError, StreamMode, StreamOptions};
use crate::workload::{BigramAlphabet, ReturnMode};

#[test]
fn prefetch_limits_keep_stream_resource_classification() {
    for message in [
        "target prefetch edge count overflow",
        "target prefetch exceeds edge visits",
        "target prefetch exceeds target slots",
        "target prefetch record exceeds size limit",
        "target prefetch exceeds retained bytes",
    ] {
        assert_eq!(limit_error(message.into(), false), message);
        assert!(limit_error(message.into(), true).starts_with("stream resource:"));
    }
    assert!(
        stream_target_slot_charge(100, Some(1024))
            .err()
            .unwrap()
            .starts_with("stream resource:")
    );
}

fn field(schema: &mut Schema, name: &str, kind: LogicalType, multi: bool) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: false,
            nullable: true,
            indexed: true,
            multi,
            element_nullable: if multi { vec![false] } else { vec![] },
        })
        .unwrap();
}

fn definitions() -> Vec<CollectionDefinition> {
    let mut targets = Schema::default();
    field(&mut targets, "id", LogicalType::Id, false);
    field(&mut targets, "title", LogicalType::String, false);
    let mut sources = Schema::default();
    field(&mut sources, "id", LogicalType::Id, false);
    field(&mut sources, "local_title", LogicalType::String, false);
    field(
        &mut sources,
        "links",
        LogicalType::Array(Box::new(LogicalType::Id)),
        true,
    );
    sources
        .register_relation(RelationDefinition {
            path: vec!["links".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::Many,
            reverse_alias: Some("sources".into()),
        })
        .unwrap();
    [
        ("sources", sources, SourceFormat::Json),
        ("targets", targets, SourceFormat::JsonLines),
    ]
    .into_iter()
    .map(|(name, schema, format)| CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    })
    .collect()
}

fn fixture(alphabet: BigramAlphabet) -> (std::path::PathBuf, Repository) {
    let root = std::env::temp_dir().join(format!(
        "objekte-target-cache-{}-{:?}-{}",
        std::process::id(),
        std::thread::current().id(),
        alphabet as u8,
    ));
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    std::fs::write(
        root.join("targets/records.jsonl"),
        "{\"id\":\"T0\",\"title\":\"Hymn\"}\n{\"id\":\"T1\",\"title\":\"Other\"}\n{broken}\n",
    )
    .unwrap();
    for index in 0..84 {
        let links = match index {
            80 => serde_json::json!(["T1"]),
            81 => serde_json::json!(["missing"]),
            82 => serde_json::json!(["T1", "T0"]),
            83 => serde_json::json!(["T0", "T1"]),
            _ => serde_json::json!(["T0", "T0"]),
        };
        std::fs::write(
            root.join(format!("sources/S{index:02}.json")),
            serde_json::json!({"id":format!("S{index:02}"),"local_title":"Hymn","links":links})
                .to_string(),
        )
        .unwrap();
    }
    std::fs::write(root.join("sources/broken.json"), "{broken}").unwrap();
    let repo = Repository::open_with_options(
        &root,
        definitions(),
        RepositoryOptions {
            bigram_alphabet: alphabet,
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            query_workers: 4,
            ..Default::default()
        },
    )
    .unwrap();
    (root, repo)
}

fn query(filter: &str, count: bool, limit: usize) -> Query {
    Query {
        collections: vec!["sources".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit,
            offset: 0,
            count,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

#[test]
fn source_dependent_comparisons_share_projected_targets_without_changing_truth() {
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let (root, repo) = fixture(alphabet);
        let expected = (0..80)
            .map(|index| format!("S{index:02}"))
            .chain(["S82".into(), "S83".into()])
            .collect::<Vec<_>>();
        let cases = [
            ("local_title ?= links.title", expected.clone()),
            (
                "links[0].title ?= local_title",
                (0..80)
                    .map(|index| format!("S{index:02}"))
                    .chain(["S83".into()])
                    .collect(),
            ),
            (
                "links[1].title ?= local_title",
                (0..80)
                    .map(|index| format!("S{index:02}"))
                    .chain(["S82".into()])
                    .collect(),
            ),
            ("!(local_title ?= links.title)", vec!["S80".into()]),
        ];
        for (filter, expected_ids) in cases {
            let request = query(filter, true, 100);
            let actual = repo.search(&request).unwrap();
            let reference = repo.search_reference(&request).unwrap();
            assert_eq!(actual.hits, reference.hits, "{filter}");
            assert_eq!(actual.count, reference.count, "{filter}");
            assert_eq!(
                actual
                    .hits
                    .iter()
                    .map(|hit| &hit.record_id)
                    .cloned()
                    .collect::<Vec<_>>(),
                expected_ids
            );
            assert_eq!(actual.counters.relation_target_file_opens, 1, "{filter}");
            assert!(actual.counters.relation_target_reads <= 2, "{filter}");
            assert!(actual.counters.relation_bytes_read > 0, "{filter}");
            assert!(
                actual.counters.relation_verified_targets_reused >= 79,
                "{filter}"
            );
        }
        let fallback = query("@file.status = 1 || local_title ?= links.title", true, 100);
        let actual = repo.search(&fallback).unwrap();
        let reference = repo.search_reference(&fallback).unwrap();
        assert_eq!(actual.hits, reference.hits);
        assert_eq!(actual.count, Some(83));
        let page = query("local_title ?= links.title", false, 5);
        let actual = repo.search(&page).unwrap();
        assert_eq!(actual.hits, repo.search_reference(&page).unwrap().hits);
        assert_eq!(actual.counters.relation_target_reads, 2);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn unpositioned_backlinks_visit_reverse_hits_while_positions_keep_identity_rank() {
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let (root, repo) = fixture(alphabet);
        for (filter, max_probes) in [
            (
                "@record.id = 'T1' && @backlinks.sources.local_title ?= 'Hymn'",
                3,
            ),
            (
                "@record.id = 'T1' && @backlinks.sources[0].local_title ?= 'Hymn'",
                84,
            ),
            (
                "@record.id = 'T1' && @backlinks.sources[1].local_title ?= 'Hymn'",
                84,
            ),
        ] {
            let mut request = query(filter, true, 100);
            request.collections = vec!["targets".into()];
            let actual = repo.search(&request).unwrap();
            let reference = repo.search_reference(&request).unwrap();
            assert_eq!(actual.hits, reference.hits, "{filter}");
            assert_eq!(actual.count, Some(1), "{filter}");
            assert_eq!(actual.hits[0].record_id, "T1");
            assert!(
                actual.counters.relation_backlink_probe_visits <= max_probes,
                "{filter}"
            );
            if filter.contains("sources[") {
                assert!(
                    actual.counters.relation_backlink_probe_visits >= 81,
                    "{filter}"
                );
            } else {
                assert!(
                    actual.counters.relation_backlink_probe_visits <= 3,
                    "{filter}"
                );
            }
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn high_fanout_stream_prefetch_reports_resource_limit_before_grouping() {
    let root = std::env::temp_dir().join(format!(
        "objekte-target-stream-limit-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    let mut targets = String::new();
    let mut links = Vec::new();
    for index in 0..100 {
        let id = format!("T{index:03}");
        targets.push_str(&format!("{{\"id\":\"{id}\",\"title\":\"Hymn\"}}\n"));
        links.push(id);
    }
    std::fs::write(root.join("targets/records.jsonl"), targets).unwrap();
    std::fs::write(
        root.join("sources/one.json"),
        serde_json::json!({"id":"S0","local_title":"Hymn","links":links}).to_string(),
    )
    .unwrap();
    let repo = Repository::open_with_options(
        &root,
        definitions(),
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let request = query("local_title ?= links.title", true, 1);
    assert_eq!(repo.search(&request).unwrap().count, Some(1));
    let result = repo.search_stream(
        &request,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            max_event_bytes: 1024,
            max_buffered_bytes: 4096,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |_| Ok(StreamControl::Continue),
    );
    assert!(matches!(result, Err(StreamError::Resource { .. })));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn stream_prefetch_checks_remaining_allowance_before_later_file_read() {
    let root = std::env::temp_dir().join(format!(
        "objekte-target-stream-remaining-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    let target = |id: &str| {
        serde_json::json!({
            "id": id, "title": "Hymn", "filler": "x".repeat(600)
        })
        .to_string()
            + "\n"
    };
    let first = target("T0");
    std::fs::write(root.join("targets/A.jsonl"), &first).unwrap();
    std::fs::write(root.join("targets/B.jsonl"), target("T1")).unwrap();
    std::fs::write(
        root.join("sources/one.json"),
        r#"{"id":"S0","local_title":"Hymn","links":["T0","T1"]}"#,
    )
    .unwrap();
    let repo = Repository::open_with_options(
        &root,
        definitions(),
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let request = query("local_title ?= links.title", true, 1);
    assert_eq!(repo.search(&request).unwrap().count, Some(1));
    std::fs::remove_file(root.join("targets/B.jsonl")).unwrap();
    let prefetch_allowance = 2 * STREAM_TARGET_SLOT_BYTES as usize + first.len() + 100;
    let result = repo.search_stream(
        &request,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            max_event_bytes: 1024,
            max_buffered_bytes: prefetch_allowance * 4,
            ..Default::default()
        },
        &CancellationToken::new(),
        &mut |_| Ok(StreamControl::Continue),
    );
    assert!(matches!(result, Err(StreamError::Resource { message, .. })
        if message.contains("target record exceeds read allowance")));
    std::fs::remove_dir_all(root).unwrap();
}
