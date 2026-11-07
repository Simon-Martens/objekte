use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use objekte_search_core::inference::{InferenceConfig, InferenceIndexStatus, InferenceMode};
use objekte_search_core::model::{LogicalType, Number, Value};
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::streaming::{
    CancellationToken, StreamControl, StreamEvent, StreamMode, StreamOptions, StreamOutcome,
};
use objekte_search_core::workload::{ReturnMode, SortField};

static NEXT_ROOT: AtomicUsize = AtomicUsize::new(0);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "objekte-optional-interactions-{}-{}",
            std::process::id(),
            NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        for number in 0..32 {
            let title = if number == 1 {
                serde_json::json!("needle")
            } else if number == 3 {
                serde_json::json!([])
            } else {
                serde_json::json!("ordinary")
            };
            let unrelated = if number == 2 {
                serde_json::json!([])
            } else if number % 2 == 0 {
                serde_json::json!(number)
            } else {
                serde_json::json!("mixed")
            };
            let value = serde_json::json!({
                "score": if number < 24 { 7 } else { 9 },
                "rank": 31 - number,
                "title": title,
                "unrelated": unrelated,
            });
            std::fs::write(
                root.join(format!("data/r{number:02}.json")),
                value.to_string(),
            )
            .unwrap();
        }
        std::fs::write(root.join("data/broken.json"), "{").unwrap();
        Self(root)
    }

    fn definition(&self) -> CollectionDefinition {
        let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
        definition.identity = IdentityPolicy::Filename;
        definition
    }

    fn open(&self, mode: InferenceMode, snapshot_mode: SnapshotMode) -> Repository {
        Repository::open_with_options(
            &self.0,
            vec![self.definition()],
            RepositoryOptions {
                inference: InferenceConfig {
                    mode,
                    ..Default::default()
                },
                snapshot_mode,
                query_workers: 4,
                verified_cache_bytes: 1024 * 1024,
                ..Default::default()
            },
        )
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).ok();
    }
}

fn query(filter: &str, sort: Vec<SortField>, return_mode: ReturnMode) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort,
            limit: 40,
            offset: 0,
            count: true,
            return_mode,
            evidence: true,
        },
    }
}

fn ids(result: &objekte_search_core::repository::SearchResult) -> Vec<String> {
    result
        .hits
        .iter()
        .map(|hit| hit.record_id.clone())
        .collect()
}

#[test]
fn inference_modes_preserve_parameterized_dynamic_sort_stream() {
    let fixture = Fixture::new();
    let mut request = query(
        "score = $value",
        vec![SortField::simple("rank", false)],
        ReturnMode::Logical,
    );
    request.options.limit = 5;
    request.options.offset = 2;
    let parameters = HashMap::from([("value".into(), Value::Number(Number::integer(7)))]);
    let expected = ["r21", "r20", "r19", "r18", "r17"];
    let mut mode_pages = Vec::new();
    for mode in [
        InferenceMode::Disabled,
        InferenceMode::ReportOnly,
        InferenceMode::AutoIndex,
    ] {
        let repo = fixture.open(mode, SnapshotMode::Disabled);
        if mode == InferenceMode::Disabled {
            assert!(repo.inference_report("data").is_none());
        } else {
            let score = repo
                .inference_report("data")
                .unwrap()
                .fields
                .iter()
                .find(|field| field.path == ["score"])
                .unwrap();
            if mode == InferenceMode::AutoIndex {
                assert_eq!(score.index_status, InferenceIndexStatus::Built);
            }
        }

        let mut final_page = Vec::new();
        let mut replacements = 0;
        let outcome = repo
            .search_stream_with_parameters(
                &request,
                &parameters,
                StreamOptions {
                    mode: StreamMode::PageReplacements,
                    checkpoint_files: 1,
                    ..Default::default()
                },
                &CancellationToken::new(),
                &mut |event| {
                    if let StreamEvent::PageReplacement { rows, .. } = event {
                        replacements += 1;
                        final_page = rows.to_vec();
                    }
                    Ok(StreamControl::Continue)
                },
            )
            .unwrap();
        let StreamOutcome::Complete(summary) = outcome else {
            panic!("expected complete stream")
        };
        assert_eq!(summary.count, Some(24));
        assert!(replacements > 0);
        assert_eq!(
            final_page
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        let collected = repo.search_with_parameters(&request, &parameters).unwrap();
        assert_eq!(collected.count, Some(24));
        assert_eq!(ids(&collected), expected);
        assert!(collected.hits.iter().all(|hit| hit.payload.is_some()));
        if mode == InferenceMode::AutoIndex {
            assert_eq!(collected.counters.sort_key_evaluations, 0);
            assert_eq!(
                collected.counters.record_parses,
                collected.hits.len() as u64
            );
        } else {
            assert!(collected.counters.sort_key_evaluations >= 24);
        }
        assert_eq!(final_page, collected.hits);
        mode_pages.push(final_page);
    }
    assert!(mode_pages.windows(2).all(|pair| pair[0] == pair[1]));
}

#[test]
fn unrelated_empty_array_does_not_fabricate_universal_hit_or_lose_fallback_or() {
    let fixture = Fixture::new();
    for mode in [InferenceMode::Disabled, InferenceMode::AutoIndex] {
        let repo = fixture.open(mode, SnapshotMode::Disabled);
        let universal = repo
            .search(&query("title ~ 'needle'", vec![], ReturnMode::Locator))
            .unwrap();
        assert_eq!(ids(&universal), ["r01", "r03"]);
        assert!(!ids(&universal).contains(&"r02".to_owned()));
        let existential = repo
            .search(&query("title ?~ 'needle'", vec![], ReturnMode::Locator))
            .unwrap();
        assert_eq!(ids(&existential), ["r01"]);
        let fallback_or = repo
            .search(&query(
                "title ?~ 'needle' || @record.class = 1",
                vec![],
                ReturnMode::Locator,
            ))
            .unwrap();
        assert_eq!(fallback_or.count, Some(2));
        assert!(fallback_or.hits.iter().any(|hit| hit.record_id == "r01"));
        assert_eq!(
            fallback_or
                .hits
                .iter()
                .filter(|hit| hit.record_id.starts_with("~fallback:"))
                .count(),
            1
        );
    }
}

#[test]
fn reopening_without_inference_does_not_restore_inferred_postings() {
    let fixture = Fixture::new();
    let auto = fixture.open(InferenceMode::AutoIndex, SnapshotMode::ReadWrite);
    assert!(!auto.startup().restored);
    assert!(auto.inference_report("data").is_some());
    let baseline = auto
        .search(&query("score = 7", vec![], ReturnMode::Locator))
        .unwrap();
    assert_eq!(baseline.count, Some(24));
    drop(auto);

    let disabled = fixture.open(InferenceMode::Disabled, SnapshotMode::ReadWrite);
    assert!(!disabled.startup().restored);
    assert!(disabled.inference_report("data").is_none());
    let restored_answers = disabled
        .search(&query("score = 7", vec![], ReturnMode::Locator))
        .unwrap();
    assert_eq!(ids(&restored_answers), ids(&baseline));
}

#[test]
fn library_diagnostics_list_ordered_postings_without_source_access() {
    let fixture = Fixture::new();
    let repo = fixture.open(InferenceMode::AutoIndex, SnapshotMode::Disabled);
    let diagnostics = repo.inference_index_diagnostics("data").unwrap();
    assert!(
        diagnostics
            .windows(2)
            .all(|pair| pair[0].path <= pair[1].path)
    );
    for (path, keys) in [("rank", 32), ("score", 2)] {
        let field = diagnostics
            .iter()
            .find(|field| field.path == [path])
            .unwrap();
        assert_eq!(field.index_status, InferenceIndexStatus::Built);
        assert_eq!(field.kind, Some(LogicalType::Number));
        assert_eq!(field.value_keys, Some(keys));
        assert_eq!(field.multi, Some(false));
        assert_eq!(field.covered_records, Some(32));
    }
    let disabled = fixture.open(InferenceMode::Disabled, SnapshotMode::Disabled);
    assert!(disabled.inference_index_diagnostics("data").is_none());
}
