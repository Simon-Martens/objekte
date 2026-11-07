use roaring::RoaringBitmap;

use super::{InferenceBuilder, InferenceConfig, InferenceIndexStatus, InferenceMode, ObservedStep};
use crate::executor::AccessPathKind;
use crate::inventory::ByteRange;
use crate::model::LogicalType;
use crate::model::{RecordSlot, Value};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions, SnapshotMode};
use crate::source::{ExtractedRecord, SourceFormat, SourceReader, SourceRegistry};
use crate::workload::{BigramAlphabet, ReturnMode};

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
                snapshot_mode: SnapshotMode::ReadWrite,
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
                        .any(|term| term.selected.kind == AccessPathKind::InferredPosting)
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
    assert_eq!(status("shape"), InferenceIndexStatus::Incompatible);
    assert_eq!(status("blocked"), InferenceIndexStatus::ExplicitOptOut);
    assert_eq!(status("declared"), InferenceIndexStatus::Redundant);
    assert!(state.indexes.is_empty());
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
    assert_eq!(status("unique"), InferenceIndexStatus::BelowThreshold);
    assert_eq!(status("long"), InferenceIndexStatus::BudgetExceeded);
    assert_eq!(state.indexes.len(), 1);
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
            .any(|term| term.selected.kind == AccessPathKind::InferredPosting)
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
                .any(|term| term.selected.kind == AccessPathKind::InferredPosting)
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}
