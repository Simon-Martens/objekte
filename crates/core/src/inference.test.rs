use roaring::RoaringBitmap;

use super::{
    InferenceBuilder, InferenceConfig, InferenceIndexStatus, InferenceMode, InferencePolicy,
    ObservedStep,
};
use crate::executor::AccessPathKind;
use crate::inventory::ByteRange;
use crate::model::LogicalType;
use crate::model::{RecordSlot, Value};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions, SnapshotMode};
use crate::source::{ExtractedRecord, SourceFormat, SourceReader, SourceRegistry};
use crate::workload::{BigramAlphabet, ReturnMode, SortField};

fn value(json: &str) -> Value {
    Value::from_json(serde_json::from_str(json).unwrap()).unwrap()
}

fn temporary_root(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("objekte-inference-{tag}-{}", std::process::id()))
}

fn query(filter: &str) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 100,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    }
}

#[test]
fn documented_inference_defaults_match_the_exported_policy() {
    let policy = InferencePolicy::default();
    assert_eq!(policy.max_batch_bytes, 16 * 1024 * 1024);
    assert_eq!(policy.max_total_bytes, 512 * 1024 * 1024);
    assert_eq!(policy.max_repository_bytes, 768 * 1024 * 1024);
    assert_eq!(policy.short_string_max_utf8_bytes, 64);
    assert_eq!(
        policy.max_short_string_index_bytes_per_path,
        24 * 1024 * 1024
    );
    assert_eq!(
        InferenceConfig {
            mode: InferenceMode::AutoIndex,
            policy,
        }
        .max_build_jobs(32),
        16
    );
}

#[test]
fn auto_index_retains_structural_facts_after_value_admission_fails() {
    let root = temporary_root("structural-facts");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        ("00", r#"{"arr":[],"text":""}"#),
        ("01", r#"{"arr":[{}],"text":"long unique text one"}"#),
        ("02", r#"{"arr":[{},{}],"text":"long unique text two"}"#),
        ("03", r#"{"arr":[{},{},{}],"text":" "}"#),
        ("04", r#"{"text":null}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let make = |max_keys| {
        Repository::open_with_options(
            &root,
            vec![CollectionDefinition::folder(
                "data",
                "data",
                SourceFormat::Json,
            )],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                verified_cache_bytes: 0,
                inference: InferenceConfig {
                    mode: InferenceMode::AutoIndex,
                    policy: InferencePolicy {
                        max_distinct_values_per_path: max_keys,
                        max_string_bytes: 4,
                        max_short_string_index_bytes_per_path: 1,
                        ..Default::default()
                    },
                },
                ..Default::default()
            },
        )
        .unwrap()
    };
    let repo = make(4);
    let inference = repo.collections()[0].inference.as_ref().unwrap();
    let arr = &inference.indexes[&vec!["arr".into()]];
    let text = &inference.indexes[&vec!["text".into()]];
    assert!(arr.length_complete);
    assert!(!arr.value_complete);
    assert!(text.empty_string_complete);
    assert!(!text.value_complete);
    for filter in ["arr:length = 0", "arr:length >= 2", "text = \"\""] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        let optimized = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(optimized.count, reference.count, "{filter}");
        assert_eq!(optimized.hits, reference.hits, "{filter}");
        assert_eq!(optimized.counters.files_opened, 0, "{filter}");
        assert_eq!(optimized.counters.record_parses, 0, "{filter}");
    }
    let bounded = make(1);
    let inference = bounded.collections()[0].inference.as_ref().unwrap();
    assert!(
        !inference
            .indexes
            .get(&vec!["arr".into()])
            .is_some_and(|index| index.length_complete)
    );
    let mut request = query("arr:length >= 2");
    request.options.return_mode = ReturnMode::Locator;
    let optimized = bounded.search(&request).unwrap();
    let reference = bounded.search_reference(&request).unwrap();
    assert_eq!(optimized.count, reference.count);
    assert_eq!(optimized.hits, reference.hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn equal_declared_and_observed_postings_have_the_same_plan_and_proof() {
    let root = temporary_root("posting-provenance-parity");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, document) in [
        ("00", r#"{"score":1}"#),
        ("01", r#"{"score":2}"#),
        ("02", r#"{"score":3}"#),
        ("03", r#"{"score":null}"#),
        ("04", r#"{"other":true}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), document).unwrap();
    }
    let mut declared_definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    declared_definition.schema.register_field(FieldDefinition {
        path: vec!["score".into()],
        kind: LogicalType::Number,
        required: false,
        nullable: true,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    }).unwrap();
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        verified_cache_bytes: 0,
        ..Default::default()
    };
    let declared = Repository::open_with_options(&root, vec![declared_definition], options.clone()).unwrap();
    let observed = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder("data", "data", SourceFormat::Json)],
        RepositoryOptions {
            inference: InferenceConfig { mode: InferenceMode::AutoIndex, ..Default::default() },
            ..options
        },
    ).unwrap();
    let path = vec!["score".into()];
    let left = &declared.collections()[0].postings.fields[&path];
    let right = &observed.collections()[0].inference.as_ref().unwrap().indexes[&path];
    assert_eq!(left.kind, right.kind);
    assert_eq!(left.multi, right.multi);
    assert_eq!(left.presence, right.presence);
    assert_eq!(left.nulls, right.nulls);
    assert_eq!(left.values, right.values);
    assert_eq!(left.lengths, right.lengths);
    assert_eq!(left.value_complete, right.value_complete);
    assert_eq!(left.length_complete, right.length_complete);
    assert_eq!(left.empty_string_complete, right.empty_string_complete);
    assert_eq!(left.covered_records, right.covered_records);
    for filter in [
        "score = 2", "score >= 2", "score:exists = true", "score:null = true",
    ] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        request.options.evidence = false;
        let a = declared.search(&request).unwrap();
        let b = observed.search(&request).unwrap();
        assert_eq!(a.hits, b.hits, "{filter}");
        assert_eq!(a.count, b.count, "{filter}");
        assert_eq!(a.counters.files_opened, 0, "{filter}");
        assert_eq!(b.counters.files_opened, 0, "{filter}");
        let a_plan = declared.explain(&request).unwrap();
        let b_plan = observed.explain(&request).unwrap();
        let a_access = &a_plan[0].terms[0].selected;
        let b_access = &b_plan[0].terms[0].selected;
        assert_eq!(a_access.kind, b_access.kind, "{filter}");
        assert_eq!(a_access.exact, b_access.exact, "{filter}");
        assert_eq!(a_access.estimated_output, b_access.estimated_output, "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn conjunct_costing_reuses_inferred_term_estimates_without_candidate_builds() {
    let root = temporary_root("planning-conjuncts");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for slot in 0..32 {
        let object = (0..16)
            .map(|field| (format!("f{field}"), serde_json::json!(slot % 4)))
            .collect::<serde_json::Map<_, _>>();
        std::fs::write(
            root.join(format!("data/{slot:02}.json")),
            serde_json::Value::Object(object).to_string(),
        )
        .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    for conjuncts in [1, 4, 16] {
        let filter = (0..conjuncts)
            .map(|field| format!("f{field} = 0"))
            .collect::<Vec<_>>()
            .join(" && ");
        let request = query(&filter);
        let estimate = &repo.explain(&request).unwrap()[0];
        assert!(estimate.candidates_estimated);
        assert_eq!(estimate.planning.candidate_bitmap_builds, 0);
        let analyzed = repo.explain_analyze(&request).unwrap().1;
        let trace = &analyzed[0];
        assert_eq!(trace.planning.term_estimation_calls, conjuncts);
        assert_eq!(trace.planning.candidate_bitmap_builds, conjuncts);
        if conjuncts > 1 {
            assert!(trace.planning.reused_term_estimates >= conjuncts);
        }
        assert!(trace.planning.dictionary_keys_visited <= conjuncts * 2);
        assert_eq!(
            repo.search(&request).unwrap().hits,
            repo.search_reference(&request).unwrap().hits
        );
    }
    let disjunction = (0..16)
        .map(|field| format!("f{field} = 0"))
        .collect::<Vec<_>>()
        .join(" || ");
    let request = query(&disjunction);
    let estimate = &repo.explain(&request).unwrap()[0];
    assert_eq!(estimate.planning.candidate_bitmap_builds, 0);
    let analyzed = repo.explain_analyze(&request).unwrap().1;
    let trace = &analyzed[0];
    assert_eq!(trace.planning.term_estimation_calls, 16);
    assert_eq!(trace.planning.candidate_bitmap_builds, 16);
    assert_eq!(
        repo.search(&request).unwrap().hits,
        repo.search_reference(&request).unwrap().hits
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn short_strings_keep_ordered_membership_and_explicit_residual_coverage() {
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let uuid = "550e8400-e29b-41d4-a716-446655440000";
    let long_between = format!("A{}", "\0".repeat(64));
    let inputs = vec![
        serde_json::json!({"code": uuid}),
        serde_json::json!({"code": "A"}),
        serde_json::json!({"code": long_between}),
        serde_json::json!({"code": ["B", "long".repeat(17)]}),
        serde_json::json!({"code": "ü".repeat(32)}),
        serde_json::json!({"code": "ü".repeat(33)}),
        serde_json::json!({"code": null}),
        serde_json::json!({"other": "A"}),
        serde_json::json!("root"),
        serde_json::json!(["x", "x"]),
    ];
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    for (slot, input) in inputs.into_iter().enumerate() {
        batch
            .observe_record(RecordSlot(slot as u32), &Value::from_json(input).unwrap())
            .unwrap();
    }
    let mut coordinator = InferenceBuilder::new(config, false).unwrap();
    coordinator.merge_shifted(batch, 0).unwrap();
    let logical: RoaringBitmap = (0..10).collect();
    let state = coordinator
        .finish(&logical, 10, &Schema::default())
        .unwrap();
    let code = &state.indexes[&vec!["code".into()]];
    assert_eq!(code.kind, LogicalType::String);
    for short in [uuid, "A", "B"] {
        assert!(
            code.values
                .contains_key(&crate::posting::ScalarKey::String(short.into()))
        );
    }
    assert!(
        code.values
            .contains_key(&crate::posting::ScalarKey::String("ü".repeat(32)))
    );
    assert!(
        !code
            .values
            .contains_key(&crate::posting::ScalarKey::String("ü".repeat(33)))
    );
    assert_eq!(
        state.short_string_coverage[&vec!["code".into()]],
        [0, 1, 4].into_iter().collect()
    );
    assert_eq!(
        state.short_string_coverage[&Vec::<String>::new()],
        [8, 9].into_iter().collect()
    );
    assert_eq!(
        state.indexes[&Vec::<String>::new()].values[&crate::posting::ScalarKey::String("x".into())]
            .len(),
        1
    );
    let report = state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["code"])
        .unwrap();
    assert_eq!(report.short_string_covered_records, 3);
    assert_eq!(report.short_string_unresolved_records, 7);
    assert_eq!(report.index_status, InferenceIndexStatus::Built);
}

#[test]
fn high_cardinality_short_identifiers_do_not_use_enum_threshold() {
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    for slot in 0..128 {
        let input = serde_json::json!({"code": format!("550e8400-e29b-41d4-a716-{slot:012}")});
        batch
            .observe_record(RecordSlot(slot), &Value::from_json(input).unwrap())
            .unwrap();
    }
    let mut coordinator = InferenceBuilder::new(config, false).unwrap();
    coordinator.merge_shifted(batch, 0).unwrap();
    let state = coordinator
        .finish(&(0..128).collect(), 128, &Schema::default())
        .unwrap();
    let code = &state.indexes[&vec!["code".into()]];
    assert_eq!(code.values.len(), 128);
    assert_eq!(state.short_string_coverage[&vec!["code".into()]].len(), 128);
    assert_eq!(
        state
            .report
            .fields
            .iter()
            .find(|field| field.path == ["code"])
            .unwrap()
            .index_status,
        InferenceIndexStatus::Built
    );
}

#[test]
fn default_batch_budget_keeps_multi_field_short_string_index_complete() {
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    for slot in 0..512 {
        let input = serde_json::json!({
            "code": format!("550e8400-e29b-41d4-a716-{slot:012}"),
            "name": format!("name-{slot}"),
            "year": 1800 + slot,
            "group": format!("g{}", slot % 8),
            "flag": slot % 2 == 0,
            "nested": {"label": format!("label-{slot}")},
        });
        batch
            .observe_record(RecordSlot(slot), &Value::from_json(input).unwrap())
            .unwrap();
    }
    let mut coordinator = InferenceBuilder::new(config, false).unwrap();
    coordinator.merge_shifted(batch, 0).unwrap();
    let state = coordinator
        .finish(&(0..512).collect(), 512, &Schema::default())
        .unwrap();
    assert!(state.report.complete);
    assert_eq!(state.indexes[&vec!["code".into()]].values.len(), 512);
    assert_eq!(state.short_string_coverage[&vec!["code".into()]].len(), 512);
}

#[test]
fn default_batch_budget_keeps_dense_numeric_records_inferred() {
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let mut fields = serde_json::Map::new();
    for number in 0..350 {
        fields.insert(format!("field_{number:03}"), serde_json::json!(number));
    }
    fields.insert("year".into(), serde_json::json!(1800));
    let record = Value::from_json(serde_json::Value::Object(fields)).unwrap();
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    for slot in 0..512 {
        batch.observe_record(RecordSlot(slot), &record).unwrap();
    }
    let mut coordinator = InferenceBuilder::new(config, false).unwrap();
    coordinator.merge_shifted(batch, 0).unwrap();
    let state = coordinator
        .finish(&(0..512).collect(), 512, &Schema::default())
        .unwrap();
    assert!(state.report.complete);
    assert_eq!(
        state
            .report
            .fields
            .iter()
            .find(|field| field.path == ["year"])
            .unwrap()
            .index_status,
        InferenceIndexStatus::Built
    );
    assert_eq!(
        state.indexes[&vec!["year".into()]]
            .values
            .values()
            .next()
            .unwrap()
            .len(),
        512
    );
}

#[test]
fn selective_short_string_queries_keep_long_and_mixed_remainders() {
    let root = temporary_root("short-query");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for slot in 0..128 {
        let text = serde_json::json!({"code": format!("id-{slot:04}")}).to_string();
        std::fs::write(root.join(format!("data/{slot:03}.json")), text).unwrap();
    }
    let long = format!("A{}", "\0".repeat(64));
    for (name, input) in [
        ("a", serde_json::json!({"code":"A"})),
        ("long", serde_json::json!({"code":long})),
        ("mixed", serde_json::json!({"code":["Z", long]})),
        ("missing", serde_json::json!({"other":"A"})),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), input.to_string()).unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let mut exact = query("code = \"id-0036\"");
    exact.options.return_mode = ReturnMode::Locator;
    let result = repo.search(&exact).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["path:036.json"]
    );
    assert_eq!(result.hits, repo.search_reference(&exact).unwrap().hits);
    assert!(result.counters.record_parses < 20);
    let mut one_character = query("code = \"A\"");
    one_character.options.return_mode = ReturnMode::Locator;
    let plan = repo.explain(&one_character).unwrap();
    assert!(
        plan.iter()
            .flat_map(|trace| &trace.terms)
            .any(|term| term.selected.kind == AccessPathKind::Posting),
        "{plan:?}"
    );
    assert_eq!(
        repo.search(&one_character).unwrap().hits,
        repo.search_reference(&one_character).unwrap().hits
    );

    let mut range = query("code < \"B\"");
    range.options.return_mode = ReturnMode::Locator;
    let result = repo.search(&range).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["path:a.json", "path:long.json"]
    );
    assert_eq!(result.hits, repo.search_reference(&range).unwrap().hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mixed_short_strings_match_reference_after_snapshot_restore() {
    use crate::repository::{SnapshotAction, SnapshotOpenPolicy};
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "objekte-inference-matrix-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let long_between = format!("A{}", "\0".repeat(64));
    let inputs = [
        serde_json::json!({"code":"A"}),
        serde_json::json!({"code":"B"}),
        serde_json::json!({"code":"Z".repeat(65)}),
        serde_json::json!({"code":["A", "Z".repeat(65)]}),
        serde_json::json!({"code":["A", "A"]}),
        serde_json::json!({"code":[]}),
        serde_json::json!({"code":null}),
        serde_json::json!({"other":"A"}),
        serde_json::json!({"code":7}),
        serde_json::json!({"code":long_between}),
        serde_json::json!({"code":"ü".repeat(32)}),
        serde_json::json!({"code":"ü".repeat(33)}),
        serde_json::json!({"code":"urn:uuid:550e8400-e29b-41d4-a716-446655440000"}),
    ];
    for (slot, value) in inputs.iter().enumerate() {
        std::fs::write(root.join(format!("data/{slot:02}.json")), value.to_string()).unwrap();
    }
    std::fs::write(root.join("data/bad.json"), "{").unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    let filters = [
        "code ?= 'A'",
        "code = 'A'",
        "code != 'A'",
        "code ?!= 'A'",
        "code > 'A'",
        "code >= 'A'",
        "code < 'B'",
        "code <= 'B'",
        "code:null = true",
        "code:exists = false",
        "!(code ?= 'A')",
        "code ?= 'A' || code:exists = false",
        "code ?= 'A' && code > 'A'",
        "@record.id = 'path:00.json'",
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        };
        let snapshot = root.join(format!("cache-{alphabet:?}"));
        let open = |mode| {
            Repository::open_with_snapshot_policy(
                &root,
                vec![definition.clone()],
                options.clone(),
                SnapshotOpenPolicy::new(mode, Some(snapshot.clone())),
            )
        };
        let built = open(SnapshotMode::Disabled).unwrap();
        assert_eq!(built.build_parses(), 13);
        built
            .save_snapshot(&crate::storage::snapshot_path_in(&snapshot, alphabet))
            .unwrap();
        let restored = open(SnapshotMode::Required).unwrap();
        assert_eq!(restored.startup().snapshot_action, SnapshotAction::Restored);
        assert_eq!(restored.build_parses(), 0);
        for filter in filters {
            let mut request = query(filter);
            request.options.return_mode = ReturnMode::Locator;
            let expected = built.search_reference(&request);
            let fresh = built.search(&request);
            let loaded = restored.search(&request);
            match (expected, fresh, loaded) {
                (Ok(expected), Ok(fresh), Ok(loaded)) => {
                    assert_eq!(fresh.hits, expected.hits, "fresh {filter}");
                    assert_eq!(loaded.hits, expected.hits, "restored {filter}");
                    assert_eq!(fresh.count, expected.count, "count {filter}");
                }
                (Err(_), Err(_), Err(_)) => {}
                (reference, fresh, loaded) => {
                    panic!("route disagreement for {filter}: {reference:?}, {fresh:?}, {loaded:?}")
                }
            }
        }
        let ids = |filter: &str| {
            built
                .search(&query(filter))
                .unwrap()
                .hits
                .into_iter()
                .map(|hit| hit.record_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids("code ?= 'A'"),
            ["path:00.json", "path:03.json", "path:04.json"]
        );
        assert!(
            ids("code > 'A'").contains(&"path:09.json".to_owned()),
            "long value must survive the residual"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn empty_ancestor_reconciles_late_descendant_and_null_statistics() {
    assert_ne!(ObservedStep::Field("0".into()), ObservedStep::ArrayIndex(0));
    let config = InferenceConfig {
        mode: InferenceMode::ReportOnly,
        ..Default::default()
    };
    let mut first = InferenceBuilder::new(config, true).unwrap();
    first
        .observe_record(RecordSlot(0), &value(r#"{"items":[]}"#))
        .unwrap();
    first
        .observe_record(
            RecordSlot(1),
            &value(r#"{"items":[{"title":"A"},{"title":"A"}]}"#),
        )
        .unwrap();
    let mut second = InferenceBuilder::new(config, true).unwrap();
    second
        .observe_record(RecordSlot(0), &value(r#"{"items":[{"title":null}]}"#))
        .unwrap();
    second
        .observe_record(RecordSlot(1), &value(r#"{"items":[{}]}"#))
        .unwrap();
    let mut combined = InferenceBuilder::new(config, false).unwrap();
    combined.merge_shifted(first, 0).unwrap();
    combined.merge_shifted(second, 2).unwrap();
    let logical: RoaringBitmap = (0..4).collect();
    let state = combined.finish(&logical, 5, &Schema::default()).unwrap();
    let field = state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["items", "title"])
        .unwrap();
    assert_eq!(field.eligible_records, 4);
    assert_eq!(field.unavailable_records, 1);
    assert_eq!(field.present_records, 3);
    assert_eq!(field.missing_records, 1);
    assert_eq!(field.null_occurrence_records, 1);
    assert_eq!(field.exact_null_records, 1);
    assert_eq!(field.empty_selection_records, 1);
    assert_eq!(field.value_occurrences, 2);
    assert_eq!(field.nonblank_occurrences, 2);
    assert_eq!(field.nonblank_records, 1);
    assert_eq!(field.index_status, InferenceIndexStatus::NotRequested);
    assert!(state.indexes.is_empty());
}

#[test]
fn admitted_nested_state_posting_excludes_empty_ancestor_from_exists() {
    use crate::reference::BoundExpr;

    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    for slot in 0..24 {
        let record = if slot < 12 {
            value(r#"{"items":[]}"#)
        } else if slot < 22 {
            value(r#"{"items":[{"title":"A"}]}"#)
        } else {
            value(r#"{"items":[{"title":null}]}"#)
        };
        batch.observe_record(RecordSlot(slot), &record).unwrap();
    }
    let mut combined = InferenceBuilder::new(config, false).unwrap();
    combined.merge_shifted(batch, 0).unwrap();
    let logical = RoaringBitmap::from_iter(0..24);
    let schema = Schema::default();
    let state = combined.finish(&logical, 24, &schema).unwrap();
    let index = &state.indexes[&vec!["items".into(), "title".into()]];
    assert_eq!(index.presence.len(), 24);
    assert_eq!(index.empty_selections.len(), 12);
    assert!(index.structural.is_some());
    for (filter, expected) in [
        ("items.title:exists = true", (12..24).collect::<Vec<_>>()),
        ("items.title:exists = false", (0..12).collect::<Vec<_>>()),
        ("items.title:? = true", (12..22).collect::<Vec<_>>()),
        ("items.title:null = true", vec![22, 23]),
        ("items.title:string = true", (12..22).collect::<Vec<_>>()),
    ] {
        let BoundExpr::Term(term) = crate::reference::bind_filter(
            &parse_filter(filter).unwrap(), &schema,
        ).unwrap() else { panic!("term") };
        assert_eq!(index.exact_set(&term, &logical).unwrap().iter().collect::<Vec<_>>(),
            expected, "{filter}");
        assert_eq!(index.estimate_exact(&term, 24).unwrap().1,
            expected.len() as u64, "{filter}");
    }
}

#[test]
fn terminal_array_null_observation_is_not_exact_null() {
    let config = InferenceConfig {
        mode: InferenceMode::ReportOnly,
        ..Default::default()
    };
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    batch
        .observe_record(RecordSlot(0), &value(r#"{"tags":[null]}"#))
        .unwrap();
    let mut combined = InferenceBuilder::new(config, false).unwrap();
    combined.merge_shifted(batch, 0).unwrap();
    let state = combined
        .finish(&[0].into_iter().collect(), 1, &Schema::default())
        .unwrap();
    let field = state
        .report
        .fields
        .iter()
        .find(|field| field.path == ["tags"])
        .unwrap();
    assert_eq!(field.null_occurrence_records, 1);
    assert_eq!(field.exact_null_records, 0);
    assert_eq!(field.array_records, 1);
}

#[test]
fn nested_array_depth_counts_arrays_below_the_terminal_path() {
    let config = InferenceConfig {
        mode: InferenceMode::ReportOnly,
        ..Default::default()
    };
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    batch
        .observe_record(RecordSlot(0), &value(r#"{"two":[[1]],"three":[[[1]]]}"#))
        .unwrap();
    let mut combined = InferenceBuilder::new(config, false).unwrap();
    combined.merge_shifted(batch, 0).unwrap();
    let state = combined
        .finish(&[0].into_iter().collect(), 1, &Schema::default())
        .unwrap();
    for (name, depth) in [("two", 2), ("three", 3)] {
        let field = state
            .report
            .fields
            .iter()
            .find(|field| field.path == [name])
            .unwrap();
        assert_eq!(field.array_nesting, depth);
    }
}

#[test]
fn inference_batch_admission_does_not_depend_on_worker_count() {
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    assert_eq!(config.max_build_jobs(128), 16);
    assert!(
        config.max_build_jobs(128) * config.policy.max_batch_bytes
            <= config.policy.max_repository_bytes - config.policy.max_total_bytes
    );
    let array = (0..1000)
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join(",");
    let record = value(&format!(r#"{{"numbers":[{array}]}}"#));
    let mut reports = Vec::new();
    for jobs in [2, 128] {
        let mut batch = InferenceBuilder::with_job_count(config, true, jobs).unwrap();
        batch.observe_record(RecordSlot(0), &record).unwrap();
        let mut combined = InferenceBuilder::new(config, false).unwrap();
        combined.merge_shifted(batch, 0).unwrap();
        let state = combined
            .finish(&[0].into_iter().collect(), 1, &Schema::default())
            .unwrap();
        assert!(state.report.complete);
        assert_eq!(
            state.report.fields[0].index_status,
            InferenceIndexStatus::Built
        );
        reports.push(serde_json::to_value(state.report).unwrap());
    }
    assert_eq!(reports[0], reports[1]);
}

#[test]
fn multiple_collections_share_the_coordinator_budget_deterministically() {
    let root = temporary_root("collection-budget");
    let _ = std::fs::remove_dir_all(&root);
    for collection in ["one", "many"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
        for slot in 0..10 {
            let json = if collection == "one" {
                r#"{"flag":true}"#
            } else {
                r#"{"a":true,"b":true,"c":true}"#
            };
            std::fs::write(root.join(format!("{collection}/{slot:02}.json")), json).unwrap();
        }
    }
    let mut config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    config.policy.max_batch_bytes = 1024;
    config.policy.max_total_bytes = 2048;
    config.policy.max_repository_bytes = 4096;
    assert_eq!(config.max_build_jobs(128), 2);
    let definitions = vec![
        CollectionDefinition::folder("one", "one", SourceFormat::Json),
        CollectionDefinition::folder("many", "many", SourceFormat::Json),
    ];
    let mut observed = Vec::new();
    let available = std::thread::available_parallelism().map_or(1, |count| count.get());
    for workers in [1, available] {
        let repo = Repository::open_with_options(
            &root,
            definitions.clone(),
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                build_workers: std::num::NonZeroUsize::new(workers),
                inference: config,
                ..Default::default()
            },
        )
        .unwrap();
        let one = repo.inference_report("one").unwrap();
        let many = repo.inference_report("many").unwrap();
        assert!(one.complete);
        assert!(!many.complete);
        assert_eq!(one.admitted_paths, 1);
        assert_eq!(many.admitted_paths, 0);
        assert!(
            one.retained_bytes_estimate + many.retained_bytes_estimate
                <= config.policy.max_total_bytes
        );
        let mut request = query("flag = true");
        request.collections = vec!["one".into()];
        assert_eq!(
            repo.search(&request).unwrap().hits,
            repo.search_reference(&request).unwrap().hits
        );
        observed.push((
            serde_json::to_value(one).unwrap(),
            serde_json::to_value(many).unwrap(),
        ));
    }
    assert_eq!(observed[0], observed[1]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_manual_summaries_keep_inferred_indexes_across_workers_and_batches() {
    let root = temporary_root("bounded-manual-workers");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for index in 0..513 {
        std::fs::write(
            root.join(format!("data/{index:04}.json")),
            format!(r#"{{"manual":"m{index:04}","blocked":"b{index:04}","kind":"a"}}"#),
        )
        .unwrap();
    }
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["manual".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    definition
        .schema
        .prohibit_auto_index(vec!["blocked".into()])
        .unwrap();
    let mut config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    config.policy.max_distinct_nonblank_strings = 2;
    let available = std::thread::available_parallelism().map_or(1, |count| count.get());
    let mut outcomes = Vec::new();
    for workers in [1, available] {
        let repo = Repository::open_with_options(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                build_workers: std::num::NonZeroUsize::new(workers),
                inference: config,
                ..Default::default()
            },
        )
        .unwrap();
        let report = repo.inference_report("data").unwrap();
        assert!(report.complete);
        assert_eq!(report.admitted_paths, 1);
        let stats = &repo.startup().inference_build;
        assert_eq!(stats.observed_records, 513);
        assert_eq!(stats.merged_batches, 2);
        assert_eq!(stats.retained_manual_summary_bytes, 0);
        let indexes = &repo.collections()[0].inference.as_ref().unwrap().indexes;
        assert_eq!(indexes.len(), 1);
        outcomes.push((
            serde_json::to_value(report).unwrap(),
            bincode::serialize(&indexes[&vec!["kind".into()]]).unwrap(),
        ));
    }
    assert_eq!(outcomes[0], outcomes[1]);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn modes_preserve_answers_and_autoindex_reduces_source_parses() {
    let root = temporary_root("modes");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for index in 0..24 {
        let tag = if index < 12 {
            "A"
        } else if index == 23 {
            ""
        } else {
            "B"
        };
        let flag = if index == 23 {
            "null"
        } else if index == 3 {
            "true"
        } else {
            "false"
        };
        let score = if index == 3 { 99 } else { 0 };
        std::fs::write(
            root.join(format!("data/{index:02}.json")),
            format!(r#"{{"flag":{flag},"score":{score},"tag":"{tag}"}}"#),
        )
        .unwrap();
    }
    std::fs::write(root.join("data/bad.json"), "{").unwrap();
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut answers = Vec::new();
        let mut scans = Vec::new();
        for mode in [
            InferenceMode::Disabled,
            InferenceMode::ReportOnly,
            InferenceMode::AutoIndex,
        ] {
            let options = RepositoryOptions {
                bigram_alphabet: alphabet,
                snapshot_mode: SnapshotMode::IfValid,
                verified_cache_bytes: 0,
                inference: InferenceConfig {
                    mode,
                    ..Default::default()
                },
                ..Default::default()
            };
            let repo =
                Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
            assert_eq!(repo.build_parses(), 24);
            assert_eq!(repo.build_failed_parses(), 1);
            if mode == InferenceMode::Disabled {
                assert!(repo.collections[0].inference.is_none());
                assert_eq!(repo.startup().inference_build.observed_records, 0);
                assert_eq!(repo.startup().inference_build.merged_batches, 0);
            } else {
                let report = repo.inference_report("data").unwrap();
                assert!(report.complete);
                assert_eq!(report.eligible_records, 24);
                assert_eq!(report.unavailable_records, 1);
                assert_eq!(report.observed_paths, report.fields.len());
                assert_eq!(
                    report.admitted_paths,
                    repo.collections[0]
                        .inference
                        .as_ref()
                        .unwrap()
                        .indexes
                        .len()
                );
                assert_eq!(
                    report.retained_bytes_estimate,
                    report.summary_bytes_estimate + report.index_bytes_estimate
                );
                assert_eq!(
                    repo.collections[0]
                        .inference
                        .as_ref()
                        .unwrap()
                        .indexes
                        .is_empty(),
                    mode == InferenceMode::ReportOnly
                );
            }
            let request = query("flag = true");
            let result = repo.search(&request).unwrap();
            assert_eq!(result.hits.len(), 1);
            assert_eq!(result.hits[0].record_id, "path:03.json");
            assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
            answers.push(result.hits.clone());
            scans.push(result.counters.record_parses);
            repo.configure_verified_cache(1024 * 1024).unwrap();
            assert_eq!(repo.search(&request).unwrap().hits, result.hits);
            assert_eq!(repo.search(&request).unwrap().hits, result.hits);
            if mode == InferenceMode::ReportOnly {
                repo.save_default_snapshot_for_test();
                let reopened =
                    Repository::open_with_options(&root, vec![definition.clone()], options)
                        .unwrap();
                assert!(reopened.startup().restored);
                assert!(
                    reopened.collections[0]
                        .inference
                        .as_ref()
                        .unwrap()
                        .indexes
                        .is_empty()
                );
            }
            if mode == InferenceMode::AutoIndex {
                let report = repo.inference_report("data").unwrap();
                assert_eq!(
                    report
                        .fields
                        .iter()
                        .find(|field| field.path == ["flag"])
                        .unwrap()
                        .index_status,
                    InferenceIndexStatus::Built
                );
                assert_eq!(
                    report
                        .fields
                        .iter()
                        .find(|field| field.path == ["tag"])
                        .unwrap()
                        .index_status,
                    InferenceIndexStatus::Built
                );
                for filter in [
                    "score >= 99",
                    "tag = 'A'",
                    "flag:exists = true",
                    "flag:null = true",
                    "flag = false",
                ] {
                    let request = query(filter);
                    assert_eq!(
                        repo.search(&request).unwrap().hits,
                        repo.search_reference(&request).unwrap().hits,
                        "{filter}"
                    );
                }
                let plan = repo.explain(&request).unwrap();
                assert!(
                    plan.iter()
                        .flat_map(|trace| &trace.terms)
                        .any(|term| term.selected.kind == AccessPathKind::Posting)
                );
                let fallback_or = query("flag = true || @file.status = 1");
                let selected = repo.search(&fallback_or).unwrap();
                assert_eq!(selected.hits.len(), 2);
                assert_eq!(
                    selected.hits,
                    repo.search_reference(&fallback_or).unwrap().hits
                );
                let blank = query("tag = ''");
                assert_eq!(
                    repo.search(&blank).unwrap().hits,
                    repo.search_reference(&blank).unwrap().hits
                );
                let parameterized = query("score = $value");
                for value in [
                    Value::Number(crate::model::Number::integer(99)),
                    Value::String("99".into()),
                ] {
                    let params = std::collections::HashMap::from([("value".into(), value)]);
                    assert_eq!(
                        repo.search_with_parameters(&parameterized, &params)
                            .unwrap()
                            .hits,
                        repo.search_reference(&parameterized.with_parameters(&params).unwrap())
                            .unwrap()
                            .hits
                    );
                }
                repo.save_default_snapshot_for_test();
                let reopened =
                    Repository::open_with_options(&root, vec![definition.clone()], options)
                        .unwrap();
                assert!(reopened.startup().restored);
                assert_eq!(reopened.search(&request).unwrap().hits, result.hits);
                let one_worker = Repository::open_with_options(
                    &root,
                    vec![definition.clone()],
                    RepositoryOptions {
                        snapshot_mode: SnapshotMode::Disabled,
                        build_workers: std::num::NonZeroUsize::new(1),
                        ..options
                    },
                )
                .unwrap();
                assert_eq!(
                    serde_json::to_value(repo.inference_report("data").unwrap()).unwrap(),
                    serde_json::to_value(one_worker.inference_report("data").unwrap()).unwrap(),
                );
                let disabled = Repository::open_with_options(
                    &root,
                    vec![definition.clone()],
                    RepositoryOptions {
                        inference: InferenceConfig::default(),
                        ..options
                    },
                )
                .unwrap();
                assert!(!disabled.startup().restored);
                assert!(disabled.collections[0].inference.is_none());
            }
        }
        assert!(answers.windows(2).all(|pair| pair[0] == pair[1]));
        assert!(
            scans[2] < scans[0],
            "inferred posting should avoid broad source parsing: {scans:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn budget_exhaustion_never_publishes_partial_postings() {
    let mut config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    config.policy.max_paths = 1;
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    batch
        .observe_record(RecordSlot(0), &value(r#"{"a":true,"b":1}"#))
        .unwrap();
    let mut combined = InferenceBuilder::new(config, false).unwrap();
    combined.merge_shifted(batch, 0).unwrap();
    let state = combined
        .finish(&[0].into_iter().collect(), 1, &Schema::default())
        .unwrap();
    assert!(!state.report.complete);
    assert!(state.indexes.is_empty());
    assert!(
        state
            .report
            .fields
            .iter()
            .all(|field| field.index_status == InferenceIndexStatus::Incomplete)
    );
}

#[test]
fn mixed_shapes_and_explicit_optouts_decline_inferred_postings() {
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    for (slot, json) in [
        r#"{"mixed":true,"shape":1,"blocked":true,"declared":true}"#,
        r#"{"mixed":42,"shape":[1],"blocked":false,"declared":false}"#,
    ]
    .into_iter()
    .enumerate()
    {
        batch
            .observe_record(RecordSlot(slot as u32), &value(json))
            .unwrap();
    }
    let mut schema = Schema::default();
    for (name, indexed) in [("blocked", false), ("declared", true)] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind: LogicalType::Bool,
                required: false,
                nullable: false,
                indexed,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    schema.prohibit_auto_index(vec!["blocked".into()]).unwrap();
    let mut combined = InferenceBuilder::new(config, false).unwrap();
    combined.merge_shifted(batch, 0).unwrap();
    let state = combined.finish(&(0..2).collect(), 2, &schema).unwrap();
    let status = |path: &str| {
        state
            .report
            .fields
            .iter()
            .find(|field| field.path == [path])
            .unwrap()
            .index_status
    };
    assert_eq!(status("mixed"), InferenceIndexStatus::Incompatible);
    assert_eq!(status("shape"), InferenceIndexStatus::Built);
    assert_eq!(status("blocked"), InferenceIndexStatus::ExplicitOptOut);
    assert_eq!(status("declared"), InferenceIndexStatus::Redundant);
    assert_eq!(state.indexes.len(), 1);
    let shape = &state.indexes[&vec!["shape".into()]];
    assert!(shape.length_complete);
    assert!(!shape.value_complete);
}

#[test]
fn high_cardinality_and_long_text_drop_only_their_own_candidates() {
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    for slot in 0..24 {
        let json = format!(
            r#"{{"flag":{},"unique":"value-{slot}","long":"{}"}}"#,
            slot == 0,
            "x".repeat(80)
        );
        batch
            .observe_record(RecordSlot(slot), &value(&json))
            .unwrap();
    }
    let mut combined = InferenceBuilder::new(config, false).unwrap();
    combined.merge_shifted(batch, 0).unwrap();
    let state = combined
        .finish(&(0..24).collect(), 24, &Schema::default())
        .unwrap();
    let status = |path: &str| {
        state
            .report
            .fields
            .iter()
            .find(|field| field.path == [path])
            .unwrap()
            .index_status
    };
    assert_eq!(status("flag"), InferenceIndexStatus::Built);
    assert_eq!(status("unique"), InferenceIndexStatus::Built);
    assert_eq!(status("long"), InferenceIndexStatus::Built);
    assert_eq!(state.indexes.len(), 3);
    let long = &state.indexes[&vec!["long".into()]];
    assert!(long.empty_string_complete);
    assert!(!long.value_complete);
    assert!(long.values.is_empty());
}

struct PipeReader;

impl SourceReader for PipeReader {
    fn name(&self) -> &'static str {
        "inference_pipe"
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        Ok(vec![ByteRange::checked(
            0,
            file.len() as u64,
            file.len() as u64,
        )?])
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let (flag, title) = text.split_once('|').ok_or("missing delimiter")?;
        Ok(ExtractedRecord {
            value: Value::Object(std::collections::BTreeMap::from([
                ("flag".into(), Value::Bool(flag == "true")),
                ("title".into(), Value::String(title.into())),
            ])),
            // The reader deliberately omits title from raw text. Inference
            // observes its logical value, independent of raw FTS coverage.
            text: Vec::new(),
        })
    }
}

#[test]
fn registered_reader_observations_do_not_depend_on_raw_text_coverage() {
    let root = temporary_root("reader");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.pipe"), "true|hidden title").unwrap();
    std::fs::write(root.join("data/b.pipe"), "false|other title").unwrap();
    let source = || {
        let mut registry = SourceRegistry::builtin();
        registry
            .register("pipe", 1, "pipe", std::sync::Arc::new(PipeReader))
            .unwrap();
        registry
    };
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::registered("pipe"));
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        ..Default::default()
    };
    let repo = Repository::open_with_sources(&root, vec![definition], options, source()).unwrap();
    assert_eq!(repo.build_parses(), 2);
    assert_eq!(repo.inference_report("data").unwrap().eligible_records, 2);
    let request = query("flag = true");
    assert_eq!(
        repo.search(&request).unwrap().hits,
        repo.search_reference(&request).unwrap().hits
    );
    assert!(
        repo.explain(&request)
            .unwrap()
            .iter()
            .flat_map(|trace| &trace.terms)
            .any(|term| term.selected.kind == AccessPathKind::Posting)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inferred_multivalue_candidates_preserve_universal_empty_selection() {
    let root = temporary_root("empty-selection");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, json) in [
        ("a", r#"{"items":[]}"#),
        ("b", r#"{"items":[{"flag":true}]}"#),
        ("c", r#"{"items":[{"flag":false}]}"#),
        ("d", r#"{"items":[{}]}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        repo.inference_report("data")
            .unwrap()
            .fields
            .iter()
            .find(|field| field.path == ["items", "flag"])
            .unwrap()
            .index_status,
        InferenceIndexStatus::Built
    );
    for (filter, expected) in [
        ("items.flag = true", vec!["path:a.json", "path:b.json"]),
        ("items.flag ?= true", vec!["path:b.json"]),
    ] {
        let request = query(filter);
        let result = repo.search(&request).unwrap();
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
        assert!(
            repo.explain(&request)
                .unwrap()
                .iter()
                .flat_map(|trace| &trace.terms)
                .any(|term| term.selected.kind == AccessPathKind::Posting)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn derived_existence_and_null_use_exact_inferred_postings() {
    let root = temporary_root("derived");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for slot in 0..24 {
        let json = match slot {
            3 => r#"{"flag":true}"#,
            5 => r#"{"nullable":null}"#,
            6 => r#"{"nullable":false}"#,
            7 => r#"{"items":{"flag":true}}"#,
            _ => r#"{"other":false}"#,
        };
        std::fs::write(root.join(format!("data/{slot:02}.json")), json).unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    for (filter, expected) in [
        ("flag:exists = true", vec!["path:03.json"]),
        ("flag:? = true", vec!["path:03.json"]),
        ("nullable:null = true", vec!["path:05.json"]),
        ("nullable:? = true", vec!["path:06.json"]),
        ("items.flag:exists = true", vec!["path:07.json"]),
    ] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        let result = repo.search(&request).unwrap();
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{filter}",
        );
        assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
        assert_eq!(result.counters.record_parses, 0, "{filter}");
        assert!(
            repo.explain(&request)
                .unwrap()
                .iter()
                .flat_map(|trace| &trace.terms)
                .any(|term| term.selected.kind == AccessPathKind::Posting),
            "{filter}"
        );
    }
    let mut explicit = query("items[*].flag:exists = true");
    explicit.options.return_mode = ReturnMode::Locator;
    assert!(repo.search(&explicit).unwrap().hits.is_empty());
    assert_eq!(
        repo.search(&explicit).unwrap().hits,
        repo.search_reference(&explicit).unwrap().hits
    );
    assert!(
        !repo
            .explain(&explicit)
            .unwrap()
            .iter()
            .flat_map(|trace| &trace.terms)
            .any(|term| term.selected.kind == AccessPathKind::Posting)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn complete_scalar_numeric_ranges_use_exact_ordered_inferred_postings() {
    let root = temporary_root("numeric-range-proof");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, document) in [
        ("00", r#"{"year":1799}"#),
        ("01", r#"{"year":1800}"#),
        ("02", r#"{"year":1805}"#),
        ("03", r#"{"year":1810}"#),
        ("04", r#"{"year":null}"#),
        ("05", r#"{"other":[1800,1810]}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), document).unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    for (filter, expected) in [
        ("year = 1800", vec!["path:01.json"]),
        ("year > 1800", vec!["path:02.json", "path:03.json"]),
        (
            "year >= 1800 && year < 1810",
            vec!["path:01.json", "path:02.json"],
        ),
        ("year < 1800", vec!["path:00.json"]),
        ("year <= 1800", vec!["path:00.json", "path:01.json"]),
    ] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        request.options.evidence = false;
        let result = repo.search(&request).unwrap();
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{filter}"
        );
        assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
        assert_eq!(result.count, Some(expected.len()), "{filter}");
        assert_eq!(result.counters.record_parses, 0, "{filter}");
        let trace = repo.explain_analyze(&request).unwrap().1;
        assert!(trace[0].filter_exact, "{filter}");
        assert_eq!(trace[0].candidates, expected.len() as u64, "{filter}");
        assert!(
            trace[0].terms.iter().all(|term| {
                term.selected.kind == AccessPathKind::Posting
                    && term.selected.exact
                    && !term.residual_verification
            }),
            "{filter}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn numeric_range_keeps_residual_for_empty_ancestor_selections() {
    let root = temporary_root("numeric-empty-ancestor");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/one.json"), r#"{"items":{"year":1800}}"#).unwrap();
    std::fs::write(root.join("data/empty.json"), r#"{"items":[]}"#).unwrap();
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let mut request = query("items.year > 1799");
    request.options.return_mode = ReturnMode::Locator;
    request.options.evidence = false;
    let result = repo.search(&request).unwrap();
    assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
    assert_eq!(result.hits.len(), 2);
    assert!(repo.explain(&request).unwrap()[0].remaining_verification);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn complete_short_strings_and_booleans_prove_predicates_without_source_reads() {
    let root = temporary_root("exact-short-bool");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, document) in [
        ("00", r#"{"code":"A1","flag":true}"#),
        ("01", r#"{"code":"B2","flag":false}"#),
        ("02", r#"{"code":"a1","flag":true}"#),
        ("03", r#"{"code":"","flag":null}"#),
        ("04", r#"{"other":17}"#),
        ("05", r#"{"code":"A10","flag":false}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), document).unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    for (filter, expected) in [
        ("code = 'A1'", vec!["path:00.json"]),
        (
            "code != 'A1'",
            vec![
                "path:01.json",
                "path:02.json",
                "path:03.json",
                "path:05.json",
            ],
        ),
        ("code >= 'A10' && code < 'B2'", vec!["path:05.json"]),
        ("code ~ 'A1'", vec!["path:00.json", "path:05.json"]),
        (
            "code !~ 'A1'",
            vec!["path:01.json", "path:02.json", "path:03.json"],
        ),
        ("flag = true", vec!["path:00.json", "path:02.json"]),
        ("flag != false", vec!["path:00.json", "path:02.json"]),
        ("flag > false", vec!["path:00.json", "path:02.json"]),
        ("flag < true", vec!["path:01.json", "path:05.json"]),
        (
            "flag:exists = true",
            vec![
                "path:00.json",
                "path:01.json",
                "path:02.json",
                "path:03.json",
                "path:05.json",
            ],
        ),
        ("flag:null = true", vec!["path:03.json"]),
    ] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        request.options.evidence = false;
        let result = repo.search(&request).unwrap();
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{filter}"
        );
        assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
        assert_eq!(result.count, Some(expected.len()), "{filter}");
        assert_eq!(result.counters.record_parses, 0, "{filter}");
        assert!(
            repo.explain(&request).unwrap()[0]
                .terms
                .iter()
                .all(|term| term.selected.kind == AccessPathKind::Posting
                    && term.selected.exact),
            "{filter}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn inferred_scalar_order_matches_reference_without_sort_source_reads() {
    let root = temporary_root("inferred-sort-order");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, document) in [
        ("00", r#"{"year":3,"code":"z","flag":false}"#),
        ("01", r#"{"year":1,"code":"ä","flag":true}"#),
        ("02", r#"{"year":3,"code":"a","flag":true}"#),
        ("03", r#"{"year":null,"code":null,"flag":null}"#),
        ("04", r#"{"other":"missing"}"#),
        ("05", "{"),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), document).unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    for field in ["year", "code", "flag"] {
        for descending in [false, true] {
            for count in [false, true] {
                let request = Query {
                    collections: vec!["data".into()],
                    fts: None,
                    filter: None,
                    options: QueryOptions {
                        sort: vec![SortField::simple(field, descending)],
                        limit: 2,
                        offset: 1,
                        count,
                        return_mode: ReturnMode::Locator,
                        evidence: false,
                    },
                };
                let result = repo.search(&request).unwrap();
                let reference = repo.search_reference(&request).unwrap();
                assert_eq!(result.hits, reference.hits, "{field} {descending} {count}");
                assert_eq!(
                    result.count, reference.count,
                    "{field} {descending} {count}"
                );
                assert_eq!(
                    result.counters.record_parses, 0,
                    "{field} {descending} {count}"
                );
                assert_eq!(
                    result.counters.files_opened, 0,
                    "{field} {descending} {count}"
                );
                let before_builds = crate::run::inferred_order_builds_for_test();
                let estimate = repo.explain(&request).unwrap();
                assert_eq!(
                    estimate[0].sort_strategy, "inferred_posting_order_page",
                    "{field} {descending} {count}"
                );
                assert!(estimate[0].sort_strategy_estimated);
                assert_eq!(crate::run::inferred_order_builds_for_test(), before_builds);
            }
        }
    }
    let mut filtered = query("year >= 2");
    filtered.options.sort = vec![SortField::simple("year", true)];
    filtered.options.limit = 1;
    filtered.options.return_mode = ReturnMode::Locator;
    filtered.options.evidence = false;
    let result = repo.search(&filtered).unwrap();
    assert_eq!(result.hits, repo.search_reference(&filtered).unwrap().hits);
    assert_eq!(result.count, Some(2));
    assert_eq!(result.counters.record_parses, 0);
    assert_eq!(result.counters.files_opened, 0);
    repo.configure_verified_cache(1024 * 1024).unwrap();
    for _ in 0..2 {
        let warm = repo.search(&filtered).unwrap();
        assert_eq!(warm.hits, result.hits);
        assert_eq!(warm.count, result.count);
        assert_eq!(warm.counters.record_parses, 0);
        assert_eq!(warm.counters.files_opened, 0);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn partial_short_string_posting_does_not_claim_complete_sort_order() {
    let root = temporary_root("partial-short-sort");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, code) in [("short", "A".to_owned()), ("long", "z".repeat(65))] {
        std::fs::write(
            root.join(format!("data/{name}.json")),
            serde_json::json!({"code":code}).to_string(),
        )
        .unwrap();
    }
    let repo = Repository::open_with_options(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .unwrap();
    let request = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField::simple("code", false)],
            limit: 2,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(
        repo.search(&request).unwrap().hits,
        repo.search_reference(&request).unwrap().hits
    );
    assert_eq!(
        repo.explain(&request).unwrap()[0].sort_strategy,
        "computed_top_k_full_key_evaluation"
    );
    std::fs::remove_dir_all(root).unwrap();
}
