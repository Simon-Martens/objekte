use roaring::RoaringBitmap;

use super::{InferenceConfig, InferenceIndexStatus, InferenceMode, InferencePolicy};
use crate::index::bigram::BigramAlphabet;
use crate::index::memory::inference::{InferenceBuilder, ObservedStep};
use crate::inventory::ByteRange;
use crate::model::LogicalType;
use crate::model::{RecordSlot, Value};
use crate::query::options::{ReturnMode, SortField};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::query_execution::restrictions::AccessPathKind;
use crate::repository::Repository;
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
use crate::schema::{FieldDefinition, Schema};
use crate::source::{ExtractedRecord, SourceFormat, SourceReader};

fn value(text: &str) -> Value {
    crate::source::test_support::fixture_value_from_text(text)
}

fn temporary_root(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!("objekte-inference-{tag}-{}", std::process::id()))
}

fn write_fixture_documents(root: &std::path::Path, documents: impl IntoIterator<Item = Value>) {
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (number, document) in documents.into_iter().enumerate() {
        std::fs::write(
            root.join(format!("data/{number:03}.row")),
            document.fixture_text(),
        )
        .unwrap();
    }
}

fn open_category_repository(root: &std::path::Path, min_nonblank_occurrences: u64) -> Repository {
    crate::lifecycle::Index::build_repository_for_test(
        root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(512).unwrap(),
                max_files: std::num::NonZeroUsize::new(512).unwrap(),
            },
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    max_string_bytes: 64,
                    short_string_max_utf8_bytes: 4,
                    min_nonblank_occurrences,
                    min_nonblank_records: min_nonblank_occurrences,
                    minimum_repeated_percent: 0,
                    ..Default::default()
                },
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap()
}

fn category_records(short_index: usize) -> Vec<Value> {
    (0..513)
        .map(|number| {
            fixture_value!({ "category" => (if number == short_index { "b" } else { "category_alpha" }) })
        })
        .collect()
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

fn write_named_fixtures(root: &std::path::Path, records: &[(&str, Value)]) {
    let _ = std::fs::remove_dir_all(root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, value) in records {
        std::fs::write(root.join("data").join(name), value.fixture_text()).unwrap();
    }
}

fn build_default_auto_repository(root: &std::path::Path) -> Repository {
    crate::lifecycle::Index::build_repository_for_test(
        root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap()
}

fn result_ids(result: &crate::query_results::SearchResult) -> Vec<String> {
    let mut ids = result
        .hits
        .iter()
        .map(|hit| hit.record_id.to_string())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

#[test]
fn empty_string_posting_offers_keep_empty_traversals() {
    let root = temporary_root("empty-string-offer");
    write_named_fixtures(
        &root,
        &[
            (
                "A.row",
                fixture_value!({ "items" => { "name" => ("") }, "tag" => ("keep") }),
            ),
            (
                "B.row",
                fixture_value!({ "items" => [], "tag" => ("keep") }),
            ),
            (
                "C.row",
                fixture_value!({ "items" => { "name" => ("Ada") }, "tag" => ("keep") }),
            ),
            ("D.row", fixture_value!({ "tag" => ("other") })),
        ],
    );
    let repo = build_default_auto_repository(&root);
    for (filter, expected) in [
        ("items.name = \"\"", vec!["path:A.row", "path:B.row"]),
        ("items.name ?= \"\"", vec!["path:A.row"]),
        (
            "items.name = \"\" && tag = 'keep'",
            vec!["path:A.row", "path:B.row"],
        ),
    ] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(
            result_ids(&actual),
            result_ids(&reference),
            "{filter}: optimized/reference"
        );
        assert_eq!(result_ids(&actual), expected, "{filter}: expected IDs");
    }

    let scalar_root = temporary_root("simple-empty-string");
    write_named_fixtures(
        &scalar_root,
        &[
            ("A.row", fixture_value!({ "name" => ("") })),
            ("B.row", fixture_value!({ "name" => ("Ada") })),
            ("C.row", fixture_value!({ "other" => (true) })),
        ],
    );
    let scalar = build_default_auto_repository(&scalar_root);
    let mut request = query("name = \"\"");
    request.options.return_mode = ReturnMode::Locator;
    let actual = scalar.search(&request).unwrap();
    assert_eq!(result_ids(&actual), ["path:A.row"]);
    assert_eq!(actual.counters.files_opened, 0);
    assert_eq!(actual.counters.record_parses, 0);
    let plan = scalar.explain(&request).unwrap();
    assert!(
        plan.iter().all(|step| !step.remaining_verification),
        "simple empty string must use exact posting evidence: {plan:?}"
    );

    std::fs::remove_dir_all(root).unwrap();
    std::fs::remove_dir_all(scalar_root).unwrap();
}

#[test]
fn length_postings_follow_bound_quantification_and_root_traversal() {
    let root = temporary_root("length-proof");
    write_named_fixtures(
        &root,
        &[
            ("A.row", fixture_value!([{ "x" => [] }, { "x" => [(7)] }])),
            ("B.row", fixture_value!({ "x" => [(7)] })),
        ],
    );
    let repo = build_default_auto_repository(&root);
    for (filter, expected) in [
        ("x:length = 1", vec!["path:B.row"]),
        ("x:length ?= 1", vec!["path:A.row", "path:B.row"]),
    ] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(
            result_ids(&actual),
            result_ids(&reference),
            "{filter}: optimized/reference"
        );
        assert_eq!(result_ids(&actual), expected, "{filter}: expected IDs");
    }
    std::fs::remove_dir_all(root).unwrap();

    let extended = temporary_root("length-proof-extended");
    write_named_fixtures(
        &extended,
        &[
            ("A.row", fixture_value!([{ "x" => [] }, { "x" => [(7)] }])),
            ("B.row", fixture_value!({ "x" => [(7)] })),
            ("C.row", fixture_value!({ "x" => [] })),
            ("D.row", fixture_value!({})),
            ("E.row", fixture_value!([])),
            ("F.row", fixture_value!([[]])),
        ],
    );
    let repo = build_default_auto_repository(&extended);
    for (filter, expected) in [
        (
            "x:length = 1",
            vec!["path:B.row", "path:E.row", "path:F.row"],
        ),
        ("x:length ?= 1", vec!["path:A.row", "path:B.row"]),
        (
            "x:length != 1",
            vec!["path:C.row", "path:E.row", "path:F.row"],
        ),
        (
            "x:length <= 0",
            vec!["path:C.row", "path:E.row", "path:F.row"],
        ),
        ("x:length ?!= 1", vec!["path:A.row", "path:C.row"]),
    ] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(
            result_ids(&actual),
            result_ids(&reference),
            "{filter}: optimized/reference"
        );
        assert_eq!(result_ids(&actual), expected, "{filter}: expected IDs");
    }
    std::fs::remove_dir_all(extended).unwrap();

    let object = temporary_root("length-object-root");
    write_named_fixtures(
        &object,
        &[
            ("A.row", fixture_value!({ "x" => [] })),
            ("B.row", fixture_value!({ "x" => [(7)] })),
            ("C.row", fixture_value!({})),
        ],
    );
    let repo = build_default_auto_repository(&object);
    for (filter, expected) in [
        ("x:length = 0", vec!["path:A.row"]),
        ("x:length ?= 1", vec!["path:B.row"]),
    ] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(result_ids(&actual), result_ids(&reference));
        assert_eq!(result_ids(&actual), expected);
        assert_eq!(actual.counters.files_opened, 0, "{filter}");
        assert_eq!(actual.counters.record_parses, 0, "{filter}");
        assert!(
            repo.explain(&request)
                .unwrap()
                .iter()
                .all(|step| !step.remaining_verification),
            "{filter} must remain an exact length proof"
        );
    }
    std::fs::remove_dir_all(object).unwrap();

    let split = temporary_root("length-empty-traversal-segment");
    let documents = (0..513)
        .map(|number| {
            if number == 0 {
                fixture_value!({ "x" => [(7)] })
            } else if number == 512 {
                fixture_value!([])
            } else {
                fixture_value!({ "other" => (true) })
            }
        })
        .collect::<Vec<_>>();
    write_fixture_documents(&split, documents);
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &split,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(512).unwrap(),
                max_files: std::num::NonZeroUsize::new(512).unwrap(),
            },
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(repo.segments[0].len(), 2);
    let mut request = query("x:length = 1");
    request.options.return_mode = ReturnMode::Locator;
    let actual = repo.search(&request).unwrap();
    let reference = repo.search_reference(&request).unwrap();
    assert_eq!(result_ids(&actual), result_ids(&reference));
    assert_eq!(result_ids(&actual), ["path:000.row", "path:512.row"]);
    std::fs::remove_dir_all(split).unwrap();
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
fn partial_short_values_do_not_override_failed_general_dictionary_admission() {
    let root = temporary_root("mixed-short-general-rejection");
    write_fixture_documents(&root, category_records(512));
    let repo = open_category_repository(&root, 514);
    let path: Vec<String> = vec!["category".into()];
    assert_eq!(repo.segments[0].len(), 2);
    assert_eq!(
        repo.segments[0]
            .iter()
            .map(|segment| segment.index.records.len())
            .collect::<Vec<_>>(),
        [512, 1]
    );
    let report = repo
        .inference_report("data")
        .unwrap()
        .fields
        .iter()
        .find(|field| field.path == path)
        .unwrap();
    assert_eq!(report.index_status, InferenceIndexStatus::Built);
    assert!(report.reason.contains("short-string"), "{report:?}");
    let diagnostic = repo
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == path)
        .unwrap();
    assert!(!diagnostic.value_dictionary_complete, "{diagnostic:?}");
    assert!(diagnostic.short_string_candidates, "{diagnostic:?}");
    assert!(repo.segments[0].iter().any(|segment| {
        segment.index.inference.as_ref().is_some_and(|state| {
            state
                .short_string_coverage
                .get(&path)
                .is_some_and(|covered| !covered.is_empty())
                && state.indexes.get(&path).is_some_and(|index| {
                    index
                        .values
                        .contains_key(&crate::index::posting::ScalarKey::String("b".into()))
                })
        })
    }));
    for segment in repo.segments[0].iter() {
        assert!(!segment.index.inference.as_ref().unwrap().indexes[&path].value_complete);
    }

    for (filter, expected) in [("category = 'category_alpha'", 512), ("category = 'b'", 1)] {
        let mut request = query(filter);
        request.options.return_mode = ReturnMode::Locator;
        let result = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(result.hits, reference.hits, "{filter}");
        assert_eq!(result.count, Some(expected), "{filter}");
    }
    let mut long = query("category = 'category_alpha'");
    long.options.return_mode = ReturnMode::Locator;
    let result = repo.search(&long).unwrap();
    assert!(result.counters.record_parses > 0);
    assert!(repo.explain(&long).unwrap()[0].remaining_verification);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn short_dictionary_coverage_is_independent_of_segment_order() {
    let mut outcomes = Vec::new();
    for short_index in [0, 512] {
        let root = temporary_root(&format!("short-order-{short_index}"));
        write_fixture_documents(&root, category_records(short_index));
        let repo = open_category_repository(&root, 514);
        let path: Vec<String> = vec!["category".into()];
        let diagnostic = repo
            .inference_index_diagnostics("data")
            .unwrap()
            .into_iter()
            .find(|field| field.path == path)
            .unwrap();
        let mut counts = Vec::new();
        for filter in ["category = 'category_alpha'", "category = 'b'"] {
            let mut request = query(filter);
            request.options.return_mode = ReturnMode::Locator;
            let result = repo.search(&request).unwrap();
            let reference = repo.search_reference(&request).unwrap();
            assert_eq!(result.hits, reference.hits, "{short_index}: {filter}");
            counts.push(result.count);
        }
        outcomes.push((
            diagnostic.value_dictionary_complete,
            diagnostic.short_string_candidates,
            counts,
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
    assert_eq!(outcomes[0], outcomes[1]);
    assert_eq!(outcomes[0], (false, true, vec![Some(512), Some(1)]));
}

#[test]
fn complete_short_and_general_dictionaries_remain_exact_positive_controls() {
    let short_root = temporary_root("all-short-positive");
    write_fixture_documents(
        &short_root,
        (0..513).map(|_| fixture_value!({ "category" => ("b") })),
    );
    let short = open_category_repository(&short_root, 514);
    let path: Vec<String> = vec!["category".into()];
    let short_diagnostic = short
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == path)
        .unwrap();
    assert!(short_diagnostic.value_dictionary_complete);
    assert!(short_diagnostic.short_string_candidates);
    for segment in short.segments[0].iter() {
        assert!(segment.index.inference.as_ref().unwrap().indexes[&path].value_complete);
    }
    let mut request = query("category = 'b'");
    request.options.return_mode = ReturnMode::Locator;
    let result = short.search(&request).unwrap();
    assert_eq!(result.count, Some(513));
    assert_eq!(result.counters.record_parses, 0);

    let general_root = temporary_root("general-admission-positive");
    write_fixture_documents(&general_root, category_records(512));
    let general = open_category_repository(&general_root, 512);
    let general_diagnostic = general
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == path)
        .unwrap();
    assert!(general_diagnostic.value_dictionary_complete);
    let mut request = query("category = 'category_alpha'");
    request.options.return_mode = ReturnMode::Locator;
    let result = general.search(&request).unwrap();
    assert_eq!(result.count, Some(512));
    assert_eq!(result.counters.record_parses, 0);
    std::fs::remove_dir_all(short_root).unwrap();
    std::fs::remove_dir_all(general_root).unwrap();
}

#[test]
fn short_dictionary_coverage_respects_null_missing_and_uncovered_domains() {
    let exact_root = temporary_root("short-null-missing");
    let mut exact = vec![fixture_value!({ "category" => ("b") }); 510];
    exact.push(fixture_value!({ "category" => null }));
    exact.push(fixture_value!({ "other" => (true) }));
    exact.push(fixture_value!({ "category" => ("b") }));
    write_fixture_documents(&exact_root, exact);
    let repo = open_category_repository(&exact_root, 514);
    let path: Vec<String> = vec!["category".into()];
    let diagnostic = repo
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == path)
        .unwrap();
    assert!(diagnostic.value_dictionary_complete, "{diagnostic:?}");
    let mut request = query("category = 'b'");
    request.options.return_mode = ReturnMode::Locator;
    let result = repo.search(&request).unwrap();
    assert_eq!(result.count, Some(511));
    assert_eq!(result.counters.record_parses, 0);

    let partial_root = temporary_root("short-null-long");
    let mut partial = vec![fixture_value!({ "category" => ("b") }); 510];
    partial.push(fixture_value!({ "category" => null }));
    partial.push(fixture_value!({ "category" => ("category_alpha") }));
    partial.push(fixture_value!({ "category" => ("b") }));
    write_fixture_documents(&partial_root, partial);
    let partial = open_category_repository(&partial_root, 514);
    let diagnostic = partial
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == path)
        .unwrap();
    assert!(!diagnostic.value_dictionary_complete, "{diagnostic:?}");
    let mut request = query("category = 'category_alpha'");
    request.options.return_mode = ReturnMode::Locator;
    let result = partial.search(&request).unwrap();
    let reference = partial.search_reference(&request).unwrap();
    assert_eq!(result.hits, reference.hits);
    assert_eq!(result.count, Some(1));
    assert!(result.counters.record_parses > 0);
    std::fs::remove_dir_all(exact_root).unwrap();
    std::fs::remove_dir_all(partial_root).unwrap();
}

#[test]
fn global_threshold_admits_a_field_when_each_segment_is_locally_below_it() {
    let root = temporary_root("global-threshold-segments");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let records = 513usize;
    for number in 0..records {
        let body = if number == 0 {
            r#"("category"="category_alpha","items"=[],"shape"="direct","maybe"=[null,"mixed"],"code"="A")"#
                .to_owned()
        } else if number + 1 == records {
            r#"("category"="category_alpha","items"=[("name"="alpha")],"shape"=[1],"only_late"=true,"code"="A")"#
                .to_owned()
        } else {
            format!(
                r#"("category"="category_alpha","code"="{}")"#,
                if number.is_multiple_of(2) { "A" } else { "B" }
            )
        };
        std::fs::write(root.join(format!("data/{number:03}.row")), body).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(512).unwrap(),
                max_files: std::num::NonZeroUsize::new(512).unwrap(),
            },
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    min_nonblank_occurrences: records as u64,
                    min_nonblank_records: records as u64,
                    minimum_repeated_percent: 0,
                    short_string_max_utf8_bytes: 4,
                    ..Default::default()
                },
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(repo.segments[0].len(), 2);
    assert_eq!(
        repo.segments[0]
            .iter()
            .map(|segment| segment.index.records.len())
            .collect::<Vec<_>>(),
        [512, 1]
    );
    let report = repo.inference_report("data").unwrap();
    let field = report
        .fields
        .iter()
        .find(|field| field.path == ["category"])
        .unwrap()
        .clone();
    assert_eq!(field.index_status, InferenceIndexStatus::Built);
    assert_eq!(field.present_records, records as u64);
    let late = report
        .fields
        .iter()
        .find(|field| field.path == ["only_late"])
        .unwrap();
    assert_eq!(late.eligible_records, records as u64);
    assert_eq!(late.present_records, 1);
    assert_eq!(late.missing_records, (records - 1) as u64);
    let nested = report
        .fields
        .iter()
        .find(|field| field.path == ["items", "name"])
        .unwrap();
    assert_eq!(nested.eligible_records, records as u64);
    assert_eq!(nested.present_records, 2);
    assert_eq!(nested.missing_records, (records - 2) as u64);
    assert_eq!(nested.empty_selection_records, 1);
    let shape = report
        .fields
        .iter()
        .find(|field| field.path == ["shape"])
        .unwrap();
    assert_eq!(shape.scalar_records, 1);
    assert_eq!(shape.array_records, 1);
    let maybe = report
        .fields
        .iter()
        .find(|field| field.path == ["maybe"])
        .unwrap();
    assert_eq!(maybe.null_occurrence_records, 1);
    assert_eq!(maybe.exact_null_records, 0);
    assert_eq!(maybe.nonblank_records, 1);
    let code = report
        .fields
        .iter()
        .find(|field| field.path == ["code"])
        .unwrap();
    assert_eq!(code.short_string_distinct_values, 2);
    assert_eq!(code.short_string_covered_records, records as u64);
    let segment_index = &repo.segments[0]
        .iter()
        .find(|segment| segment.index.inference.is_some())
        .unwrap()
        .index
        .inference
        .as_ref()
        .unwrap()
        .indexes[&vec!["category".into()]];
    assert!(segment_index.value_complete);
    assert!(
        segment_index
            .values
            .contains_key(&crate::index::posting::ScalarKey::String(
                "category_alpha".into()
            ))
    );

    let mut request = query("category = 'category_alpha'");
    request.options.return_mode = ReturnMode::Locator;
    let result = repo.search(&request).unwrap();
    assert_eq!(result.count, Some(records));
    assert_eq!(result.counters.files_opened, 0);
    assert_eq!(result.counters.record_parses, 0);
    assert!(
        repo.explain(&request).unwrap()[0]
            .terms
            .iter()
            .any(|term| { term.selected.kind == AccessPathKind::Posting && term.selected.exact })
    );

    let diagnostics = repo.inference_index_diagnostics("data").unwrap();
    let shape = diagnostics
        .iter()
        .find(|field| field.path == ["shape"])
        .unwrap();
    assert!(shape.array_length_complete);
    assert!(!shape.value_dictionary_complete);
    let code = diagnostics
        .iter()
        .find(|field| field.path == ["code"])
        .unwrap();
    assert!(code.short_string_candidates);

    let report_only = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(1).unwrap(),
                max_files: std::num::NonZeroUsize::new(1).unwrap(),
            },
            inference: InferenceConfig {
                mode: InferenceMode::ReportOnly,
                policy: InferencePolicy {
                    min_nonblank_occurrences: records as u64,
                    min_nonblank_records: records as u64,
                    minimum_repeated_percent: 0,
                    short_string_max_utf8_bytes: 4,
                    ..Default::default()
                },
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let report_only_report = report_only.inference_report("data").unwrap();
    assert!(
        report_only_report
            .fields
            .iter()
            .all(|field| field.index_status == InferenceIndexStatus::NotRequested)
    );
    assert_eq!(
        report_only_report
            .fields
            .iter()
            .find(|field| field.path == ["items", "name"])
            .unwrap()
            .present_records,
        2
    );
    assert!(report_only.segments[0].iter().all(|segment| {
        segment
            .index
            .inference
            .as_ref()
            .is_none_or(|inference| inference.indexes.is_empty())
    }));
    let rejected = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(1).unwrap(),
                max_files: std::num::NonZeroUsize::new(1).unwrap(),
            },
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    min_nonblank_occurrences: records as u64 + 1,
                    min_nonblank_records: records as u64 + 1,
                    minimum_repeated_percent: 0,
                    short_string_max_utf8_bytes: 4,
                    ..Default::default()
                },
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let rejected_diagnostic = rejected
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == ["category"])
        .unwrap();
    assert!(!rejected_diagnostic.value_dictionary_complete);
    let rejected_result = rejected.search(&request).unwrap();
    let rejected_reference = rejected.search_reference(&request).unwrap();
    assert_eq!(rejected_result.hits, rejected_reference.hits);
    assert!(rejected_result.counters.files_opened > 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn global_distinct_limit_rejects_dictionaries_locally_admitted_in_both_segments() {
    let root = temporary_root("global-distinct-rejection");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let records = 1_024usize;
    for number in 0..records {
        let value = format!("category_global_{:02}", number % 65);
        std::fs::write(
            root.join(format!("data/{number:04}.row")),
            fixture_value!({ "category" => (value) }).fixture_text(),
        )
        .unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(512).unwrap(),
                max_files: std::num::NonZeroUsize::new(512).unwrap(),
            },
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    short_string_max_utf8_bytes: 4,
                    max_distinct_nonblank_strings: 64,
                    ..Default::default()
                },
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(repo.segments[0].len(), 2);
    let field = repo.inference_report("data").unwrap().fields[0].clone();
    assert_eq!(field.index_status, InferenceIndexStatus::Built);
    assert_eq!(field.distinct_nonblank_strings, None);
    assert!(field.distinct_nonblank_strings_truncated);
    let diagnostic = repo
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == ["category"])
        .unwrap();
    assert!(!diagnostic.value_dictionary_complete);
    assert!(diagnostic.empty_string_exact);
    assert!(diagnostic.structural_facts);
    for segment in repo.segments[0].iter() {
        let posting = &segment.index.inference.as_ref().unwrap().indexes[&vec!["category".into()]];
        assert!(!posting.value_complete);
        assert!(posting.values.is_empty());
        assert!(!posting.presence.is_empty());
    }
    let mut request = query("category = 'category_global_00'");
    request.options.return_mode = ReturnMode::Locator;
    let result = repo.search(&request).unwrap();
    let reference = repo.search_reference(&request).unwrap();
    assert_eq!(result.hits, reference.hits);
    assert_eq!(result.count, reference.count);

    let accepted = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(512).unwrap(),
                max_files: std::num::NonZeroUsize::new(512).unwrap(),
            },
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    short_string_max_utf8_bytes: 4,
                    max_distinct_nonblank_strings: 65,
                    ..Default::default()
                },
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let accepted_field = accepted
        .inference_report("data")
        .unwrap()
        .fields
        .iter()
        .find(|field| field.path == ["category"])
        .unwrap();
    assert_eq!(accepted_field.distinct_nonblank_strings, Some(65));
    assert!(!accepted_field.distinct_nonblank_strings_truncated);
    let accepted_diagnostic = accepted
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == ["category"])
        .unwrap();
    assert!(accepted_diagnostic.value_dictionary_complete);
    let accepted_result = accepted.search(&request).unwrap();
    assert_eq!(accepted_result.counters.record_parses, 0);
    assert_eq!(
        accepted_result.hits,
        accepted.search_reference(&request).unwrap().hits
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn publication_reconciles_pruned_values_lengths_and_statistics() {
    let root = temporary_root("publication-reconciliation");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..513usize {
        let items = match number {
            0 => r#""items"=[("value"=1),("value"=null)]"#,
            1 => r#""items"=("value"="text")"#,
            2 => r#""items"=("value"=2)"#,
            _ => r#""items"=("value"=2)"#,
        };
        let list = format!(r#" [{}]"#, vec!["1"; number].join(","));
        std::fs::write(
            root.join(format!("data/{number:03}.row")),
            format!("({items},\"list\"={list})"),
        )
        .unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            segment_policy: crate::index::segment::SegmentPolicy {
                target_records: std::num::NonZeroUsize::new(512).unwrap(),
                max_files: std::num::NonZeroUsize::new(512).unwrap(),
            },
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy: InferencePolicy {
                    max_distinct_values_per_path: 512,
                    ..Default::default()
                },
            },
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let segments = &repo.segments[0];
    let mut saw_null = false;
    let mut saw_pruned_lengths = false;
    for segment in segments.iter() {
        let Some(inference) = &segment.index.inference else {
            continue;
        };
        if let Some(posting) = inference.indexes.get(&vec!["items".into(), "value".into()]) {
            if posting
                .null_posting()
                .is_some_and(|nulls| !nulls.is_empty())
            {
                saw_null = true;
                assert!(!posting.value_complete);
                assert!(
                    !posting
                        .values
                        .keys()
                        .any(|key| { matches!(key, crate::index::posting::ScalarKey::Number(_)) })
                );
            }
            posting
                .validate_eligible(
                    segment.index.record_count(),
                    segment.index.logical_records.len() as usize,
                )
                .unwrap();
            assert_eq!(posting.stats.distinct_count, posting.distinct_values());
        }
        if let Some(posting) = inference.indexes.get(&vec!["list".into()]) {
            saw_pruned_lengths = true;
            assert!(!posting.length_complete);
            assert!(posting.lengths.is_empty());
            posting
                .validate_eligible(
                    segment.index.record_count(),
                    segment.index.logical_records.len() as usize,
                )
                .unwrap();
        }
    }
    assert!(saw_null);
    assert!(saw_pruned_lengths);
    for filter in [
        "items.value:null = true",
        "items.value:number = true",
        "list:length = 2",
    ] {
        let request = query(filter);
        let actual = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(actual.hits, reference.hits, "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn collection_length_admission_deduplicates_keys_across_segments() {
    let root = temporary_root("length-key-dedup");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..2 {
        std::fs::write(root.join(format!("data/{number:03}.row")), r#"("x"=[1])"#).unwrap();
    }
    let options = RepositoryOptions {
        segment_policy: crate::index::segment::SegmentPolicy {
            target_records: std::num::NonZeroUsize::new(512).unwrap(),
            max_files: std::num::NonZeroUsize::new(512).unwrap(),
        },
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            policy: InferencePolicy {
                max_distinct_values_per_path: 1,
                ..Default::default()
            },
        },
        verified_cache_bytes: 0,
        ..Default::default()
    };
    let mut request = query("x:length = 1");
    request.options.limit = 1;
    request.options.return_mode = ReturnMode::Locator;
    request.options.evidence = false;
    let single = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        options.clone(),
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(single.segments[0].len(), 1);
    assert_eq!(
        single.inference_report("data").unwrap().fields[0].index_status,
        InferenceIndexStatus::Built
    );
    let result = single.search(&request).unwrap();
    assert_eq!(result.hits, single.search_reference(&request).unwrap().hits);
    assert_eq!(result.counters.scanned_records, 0);
    assert_eq!(result.counters.record_parses, 0);
    drop(single);

    for number in 2..513 {
        std::fs::write(root.join(format!("data/{number:03}.row")), r#"("x"=[1])"#).unwrap();
    }
    let partitioned = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        options.clone(),
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(partitioned.segments[0].len(), 2);
    let report = partitioned.inference_report("data").unwrap();
    let field = report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(field.index_status, InferenceIndexStatus::Built);
    assert_eq!(field.array_records, 513);
    let result = partitioned.search(&request).unwrap();
    assert_eq!(
        result.hits,
        partitioned.search_reference(&request).unwrap().hits
    );
    assert_eq!(result.counters.scanned_records, 0);
    assert_eq!(result.counters.record_parses, 0);
    drop(partitioned);

    std::fs::write(root.join("data/512.row"), r#"("x"=[1,2])"#).unwrap();
    let over_budget = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        options,
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let report = over_budget.inference_report("data").unwrap();
    let field = report
        .fields
        .iter()
        .find(|field| field.path == ["x"])
        .unwrap();
    assert_eq!(field.index_status, InferenceIndexStatus::BudgetExceeded);
    let result = over_budget.search(&request).unwrap();
    assert_eq!(
        result.hits,
        over_budget.search_reference(&request).unwrap().hits
    );
    assert!(result.counters.scanned_records > 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn repeated_percent_boundary_is_inclusive_and_exact() {
    let root = temporary_root("global-repeat-boundary");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..10 {
        let value = if number < 7 { "long-a" } else { "long-b" };
        std::fs::write(
            root.join(format!("data/{number:02}.row")),
            format!(r#"("category"="{value}")"#),
        )
        .unwrap();
    }
    let open = |minimum_repeated_percent| {
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![crate::source::test_support::document_folder("data", "data")],
            RepositoryOptions {
                inference: InferenceConfig {
                    mode: InferenceMode::AutoIndex,
                    policy: InferencePolicy {
                        min_nonblank_occurrences: 10,
                        min_nonblank_records: 10,
                        minimum_repeated_percent,
                        short_string_max_utf8_bytes: 1,
                        ..Default::default()
                    },
                },
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
        .unwrap()
    };
    let accepted = open(80);
    let accepted_diagnostic = accepted
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == ["category"])
        .unwrap();
    assert!(accepted_diagnostic.value_dictionary_complete);
    let rejected = open(81);
    let rejected_diagnostic = rejected
        .inference_index_diagnostics("data")
        .unwrap()
        .into_iter()
        .find(|field| field.path == ["category"])
        .unwrap();
    assert!(!rejected_diagnostic.value_dictionary_complete);
    let mut request = query("category = 'long-a'");
    request.options.return_mode = ReturnMode::Locator;
    let result = rejected.search(&request).unwrap();
    let reference = rejected.search_reference(&request).unwrap();
    assert_eq!(result.hits, reference.hits);
    assert_eq!(result.count, Some(7));
    assert!(result.counters.files_opened > 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn segmented_observation_overflow_uses_residual_scanning() {
    let root = temporary_root("segmented-observation-overflow");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for number in 0..513usize {
        std::fs::write(
            root.join(format!("data/{number:03}.row")),
            format!(
                r#"("flag"={},"payload"="{}-{number}")"#,
                number.is_multiple_of(2),
                "x".repeat(64)
            ),
        )
        .unwrap();
    }
    let policy = InferencePolicy {
        max_batch_bytes: 1_024,
        max_total_bytes: 2_048,
        max_repository_bytes: 4_096,
        ..Default::default()
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                policy,
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let report = repo.inference_report("data").unwrap();
    assert!(!report.complete);
    assert!(
        report.fields.is_empty()
            || report.fields.iter().any(|field| {
                field.index_status == InferenceIndexStatus::Incomplete
                    || field.index_status == InferenceIndexStatus::BudgetExceeded
            })
    );
    assert!(repo.segments[0].iter().all(|segment| {
        segment
            .index
            .inference
            .as_ref()
            .is_none_or(|inference| inference.indexes.is_empty())
    }));
    let mut request = query("flag = true");
    request.options.return_mode = ReturnMode::Locator;
    let result = repo.search(&request).unwrap();
    let reference = repo.search_reference(&request).unwrap();
    assert_eq!(result.hits, reference.hits);
    assert_eq!(result.count, Some(257));
    assert!(result.counters.files_opened > 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mutation_keeps_query_parity_with_construction_generation_observations() {
    let root = temporary_root("mutation-stale-report");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let initial = root.join("data/initial.rows");
    let mut initial_source = String::new();
    for number in 0..24usize {
        initial_source.push_str(&format!(
            "(\"id\"=\"i{number}\",\"flag\"={},\"category\"=\"alpha\")\n",
            number.is_multiple_of(2)
        ));
    }
    std::fs::write(&initial, initial_source).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            crate::source::test_support::line_format(),
        )],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    let view = index.view().unwrap();
    assert!(view.inference_report("data").unwrap().complete);
    let mut request = query("flag = true");
    request.options.return_mode = ReturnMode::Locator;
    let before = view.search(&request).unwrap();
    assert_eq!(before.count, Some(12));
    assert_eq!(before.counters.files_opened, 0);
    drop(view);

    let added = root.join("data/added.rows");
    let mut added_source = String::new();
    for number in 0..24 {
        added_source.push_str(&format!(
            "(\"id\"=\"a{number}\",\"flag\"=true,\"category\"=\"beta\")\n"
        ));
    }
    std::fs::write(&added, added_source).unwrap();
    let added = index.add_file(&added).unwrap();
    let view = index.view().unwrap();
    let after_add = view.search(&request).unwrap();
    let reference = view.search_reference(&request).unwrap();
    assert_eq!(after_add.hits, reference.hits);
    assert_eq!(after_add.count, Some(36));
    drop(view);

    index.delete_file(added.file).unwrap();
    let view = index.view().unwrap();
    let after_delete = view.search(&request).unwrap();
    let reference = view.search_reference(&request).unwrap();
    assert_eq!(after_delete.hits, reference.hits);
    assert_eq!(after_delete.count, Some(12));
    drop(view);
    let task = index.start_compaction().unwrap();
    task.wait().unwrap();
    let view = index.view().unwrap();
    assert_eq!(
        view.search(&request).unwrap().hits,
        view.search_reference(&request).unwrap().hits
    );
    drop(view);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn auto_index_retains_selected_null_after_value_admission_fails() {
    let root = temporary_root("structural-facts");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        ("00", r#"("arr"=[],"text"="")"#),
        ("01", r#"("arr"=[()],"text"="long unique text one")"#),
        ("02", r#"("arr"=[(),()],"text"="long unique text two")"#),
        ("03", r#"("arr"=[(),(),()],"text"=" ")"#),
        ("04", r#"("text"=null)"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.row")), body).unwrap();
    }
    let make = |max_keys| {
        crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![crate::source::test_support::document_folder("data", "data")],
            RepositoryOptions {
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
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
        .unwrap()
    };
    let repo = make(4);
    let inference = repo.segments[0][0].index.inference.as_ref().unwrap();
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
        ("00", r#"("score"=1)"#),
        ("01", r#"("score"=2)"#),
        ("02", r#"("score"=3)"#),
        ("03", r#"("score"=null)"#),
        ("04", r#"("other"=true)"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.row")), document).unwrap();
    }
    let mut declared_definition = crate::source::test_support::document_folder("data", "data");
    declared_definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["score".into()],
            kind: LogicalType::Number,
            required: false,
            nullable: true,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let options = RepositoryOptions {
        verified_cache_bytes: 0,
        ..Default::default()
    };
    let declared = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![declared_definition],
        options.clone(),
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let observed = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..options
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let path = vec!["score".into()];
    let left = &declared.segments[0][0].index.postings.fields[&path];
    let right = &observed.segments[0][0]
        .index
        .inference
        .as_ref()
        .unwrap()
        .indexes[&path];
    assert_eq!(left.kind, right.kind);
    assert_eq!(left.multi, right.multi);
    assert_eq!(left.presence, right.presence);
    assert_eq!(left.null_posting(), right.null_posting());
    assert_eq!(left.values, right.values);
    assert_eq!(left.lengths, right.lengths);
    assert_eq!(left.value_complete, right.value_complete);
    assert_eq!(left.length_complete, right.length_complete);
    assert_eq!(left.empty_string_complete, right.empty_string_complete);
    assert_eq!(left.covered_records, right.covered_records);
    for filter in [
        "score = 2",
        "score >= 2",
        "score:exists = true",
        "score:null = true",
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
        assert_eq!(
            a_access.estimated_output, b_access.estimated_output,
            "{filter}"
        );
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
            .map(|field| (format!("f{field}"), fixture_value!((slot % 4))))
            .collect::<std::collections::BTreeMap<_, _>>();
        std::fs::write(
            root.join(format!("data/{slot:02}.row")),
            Value::Object(object).fixture_text(),
        )
        .unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
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
        // Segment planning may build local candidate sets without the former
        // collection-wide planner memoization.
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
    assert_eq!(trace.planning.term_estimation_calls, 16);
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
        fixture_value!({ "code" => (uuid) }),
        fixture_value!({ "code" => ("A") }),
        fixture_value!({ "code" => (long_between) }),
        fixture_value!({ "code" => [("B"), ("long".repeat(17))] }),
        fixture_value!({ "code" => ("ü".repeat(32)) }),
        fixture_value!({ "code" => ("ü".repeat(33)) }),
        fixture_value!({ "code" => null }),
        fixture_value!({ "other" => ("A") }),
        fixture_value!(("root")),
        fixture_value!([("x"), ("x")]),
    ];
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    for (slot, input) in inputs.into_iter().enumerate() {
        batch
            .observe_record(RecordSlot(slot as u32), &input)
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
                .contains_key(&crate::index::posting::ScalarKey::String(short.into()))
        );
    }
    assert!(
        code.values
            .contains_key(&crate::index::posting::ScalarKey::String("ü".repeat(32)))
    );
    assert!(
        !code
            .values
            .contains_key(&crate::index::posting::ScalarKey::String("ü".repeat(33)))
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
        state.indexes[&Vec::<String>::new()].values
            [&crate::index::posting::ScalarKey::String("x".into())]
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
        let input = fixture_value!({ "code" => (format!("550e8400-e29b-41d4-a716-{slot:012}")) });
        batch.observe_record(RecordSlot(slot), &input).unwrap();
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
        let input = fixture_value!({ "code" => (format!("550e8400-e29b-41d4-a716-{slot:012}")), "name" => (format!("name-{slot}")), "year" => (1800 + slot), "group" => (format!("g{}", slot % 8)), "flag" => (slot % 2 == 0), "nested" => { "label" => (format!("label-{slot}")) } });
        batch.observe_record(RecordSlot(slot), &input).unwrap();
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
    let mut fields = std::collections::BTreeMap::new();
    for number in 0..350 {
        fields.insert(format!("field_{number:03}"), fixture_value!((number)));
    }
    fields.insert("year".into(), fixture_value!((1800)));
    let record = Value::Object(fields);
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
        let text = fixture_value!({ "code" => (format!("id-{slot:04}")) }).fixture_text();
        std::fs::write(root.join(format!("data/{slot:03}.row")), text).unwrap();
    }
    let long = format!("A{}", "\0".repeat(64));
    for (name, input) in [
        ("a", fixture_value!({ "code" => ("A") })),
        ("long", fixture_value!({ "code" => (&long) })),
        ("mixed", fixture_value!({ "code" => [("Z"), (long)] })),
        ("missing", fixture_value!({ "other" => ("A") })),
    ] {
        std::fs::write(root.join(format!("data/{name}.row")), input.fixture_text()).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
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
        ["path:036.row"]
    );
    assert_eq!(result.hits, repo.search_reference(&exact).unwrap().hits);
    assert!(result.counters.record_parses < 20);
    let mut one_character = query("code = \"A\"");
    one_character.options.return_mode = ReturnMode::Locator;
    let plan = repo.explain(&one_character).unwrap();
    assert!(!plan.is_empty());
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
        ["path:a.row", "path:long.row"]
    );
    assert_eq!(result.hits, repo.search_reference(&range).unwrap().hits);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mixed_short_strings_match_reference_across_static_and_native_publication() {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "objekte-inference-matrix-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let long_between = format!("A{}", "\0".repeat(64));
    let inputs = [
        fixture_value!({ "code" => ("A") }),
        fixture_value!({ "code" => ("B") }),
        fixture_value!({ "code" => ("Z".repeat(65)) }),
        fixture_value!({ "code" => [("A"), ("Z".repeat(65))] }),
        fixture_value!({ "code" => [("A"), ("A")] }),
        fixture_value!({ "code" => [] }),
        fixture_value!({ "code" => null }),
        fixture_value!({ "other" => ("A") }),
        fixture_value!({ "code" => (7) }),
        fixture_value!({ "code" => (long_between) }),
        fixture_value!({ "code" => ("ü".repeat(32)) }),
        fixture_value!({ "code" => ("ü".repeat(33)) }),
        fixture_value!({ "code" => ("urn:uuid:550e8400-e29b-41d4-a716-446655440000") }),
    ];
    for (slot, value) in inputs.iter().enumerate() {
        std::fs::write(
            root.join(format!("data/{slot:02}.row")),
            value.fixture_text(),
        )
        .unwrap();
    }
    std::fs::write(root.join("data/bad.row"), "{").unwrap();
    let definition = crate::source::test_support::document_folder("data", "data");
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
        "@record.id = 'path:00.row'",
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
        let built = crate::lifecycle::Index::start_build(
            &root,
            vec![definition.clone()],
            options,
            crate::source::test_support::fixture_registry(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap();
        let built_view = built.view().unwrap();
        assert_eq!(built_view.build_parses(), 13);
        let restored = crate::lifecycle::Index::start_build(
            &root,
            vec![definition.clone()],
            options,
            crate::source::test_support::fixture_registry(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap();
        let restored_view = restored.view().unwrap();
        for filter in filters {
            let mut request = query(filter);
            request.options.return_mode = ReturnMode::Locator;
            let expected = built_view.search_reference(&request);
            let fresh = built_view.search(&request);
            let loaded = restored_view.search(&request);
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
            built_view
                .search(&query(filter))
                .unwrap()
                .hits
                .into_iter()
                .map(|hit| hit.record_id)
                .collect::<Vec<_>>()
        };
        assert_eq!(
            ids("code ?= 'A'"),
            ["path:00.row", "path:03.row", "path:04.row"]
        );
        assert!(
            ids("code > 'A'").contains(&"path:09.row".to_owned()),
            "long value must survive the residual"
        );
        drop(restored_view);
        restored.shutdown().unwrap();
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
        .observe_record(RecordSlot(0), &value(r#"("items"=[])"#))
        .unwrap();
    first
        .observe_record(
            RecordSlot(1),
            &value(r#"("items"=[("title"="A"),("title"="A")])"#),
        )
        .unwrap();
    let mut second = InferenceBuilder::new(config, true).unwrap();
    second
        .observe_record(RecordSlot(0), &value(r#"("items"=[("title"=null)])"#))
        .unwrap();
    second
        .observe_record(RecordSlot(1), &value(r#"("items"=[()])"#))
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
    use crate::query::binding::filter::BoundExpr;

    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        ..Default::default()
    };
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    for slot in 0..24 {
        let record = if slot < 12 {
            value(r#"("items"=[])"#)
        } else if slot < 22 {
            value(r#"("items"=[("title"="A")])"#)
        } else {
            value(r#"("items"=[("title"=null)])"#)
        };
        batch.observe_record(RecordSlot(slot), &record).unwrap();
    }
    let mut combined = InferenceBuilder::new(config, false).unwrap();
    combined.merge_shifted(batch, 0).unwrap();
    let logical = RoaringBitmap::from_iter(0..24);
    let schema = Schema::default();
    let state = combined.finish(&logical, 24, &schema).unwrap();
    let index = &state.indexes[&vec!["items".into(), "title".into()]];
    assert_eq!(index.presence.len(), 12);
    assert!(index.summary.is_some());
    for (filter, expected) in [
        ("items.title:exists = true", (12..24).collect::<Vec<_>>()),
        ("items.title:exists = false", (0..12).collect::<Vec<_>>()),
        ("items.title:? = true", (12..22).collect::<Vec<_>>()),
        ("items.title:null = true", vec![22, 23]),
        ("items.title:string = true", (12..22).collect::<Vec<_>>()),
    ] {
        let BoundExpr::Term(term) =
            crate::query::binding::filter::bind_filter(&parse_filter(filter).unwrap(), &schema)
                .unwrap()
        else {
            panic!("term")
        };
        assert_eq!(
            index
                .exact_set(&term, &logical)
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            expected,
            "{filter}"
        );
        assert!(index.estimate_exact(&term, 24).is_some(), "{filter}");
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
        .observe_record(RecordSlot(0), &value(r#"("tags"=[null])"#))
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
fn inference_admission_keeps_baseline_shape_and_budget_decisions() {
    let config = InferenceConfig {
        mode: InferenceMode::AutoIndex,
        policy: InferencePolicy {
            max_distinct_values_per_path: 1,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut builder = InferenceBuilder::new(config, false).unwrap();
    for (slot, fixture) in [
        r#"("only_null"=null,"nullable"=1,"budget"=1)"#,
        r#"("only_null"=null,"nullable"=null,"mixed"=1,"budget"=2)"#,
        r#"("mixed"=(),"budget"=3)"#,
        "()",
    ]
    .into_iter()
    .enumerate()
    {
        builder
            .observe_record(RecordSlot(slot as u32), &value(fixture))
            .unwrap();
    }
    let logical = RoaringBitmap::from_iter(0..4);
    let schema = Schema::default();
    let state = builder.finish(&logical, 4, &schema).unwrap();
    state.validate(config, &logical, 4, &schema).unwrap();
    let status = |path: &str| {
        state
            .report
            .fields
            .iter()
            .find(|field| field.path == [path])
            .unwrap()
            .index_status
    };
    assert_eq!(status("only_null"), InferenceIndexStatus::Incompatible);
    assert_eq!(status("mixed"), InferenceIndexStatus::Incompatible);
    assert_eq!(status("budget"), InferenceIndexStatus::BudgetExceeded);
    assert_eq!(state.indexes.len(), 1);
    let index = &state.indexes[&vec!["nullable".into()]];
    assert_eq!(index.kind, LogicalType::Number);
    assert_eq!(index.presence, RoaringBitmap::from_iter(0..2));
    assert_eq!(
        index.null_posting().unwrap(),
        &RoaringBitmap::from_iter([1])
    );
    assert!(index.value_complete);
    assert_eq!(status("nullable"), InferenceIndexStatus::Built);
}

#[test]
fn nested_array_depth_counts_arrays_below_the_terminal_path() {
    let config = InferenceConfig {
        mode: InferenceMode::ReportOnly,
        ..Default::default()
    };
    let mut batch = InferenceBuilder::new(config, true).unwrap();
    batch
        .observe_record(RecordSlot(0), &value(r#"("two"=[[1]],"three"=[[[1]]])"#))
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
    let record = value(&format!(r#"("numbers"=[{array}])"#));
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
        reports.push(bincode::serialize(&state.report).unwrap());
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
            let fixture = if collection == "one" {
                r#"("flag"=true)"#
            } else {
                r#"("a"=true,"b"=true,"c"=true)"#
            };
            std::fs::write(root.join(format!("{collection}/{slot:02}.row")), fixture).unwrap();
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
        crate::source::test_support::document_folder("one", "one"),
        crate::source::test_support::document_folder("many", "many"),
    ];
    let mut observed = Vec::new();
    let available = std::thread::available_parallelism().map_or(1, |count| count.get());
    for workers in [1, available] {
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            definitions.clone(),
            RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(workers),
                inference: config,
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
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
            bincode::serialize(&one).unwrap(),
            bincode::serialize(&many).unwrap(),
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
            root.join(format!("data/{index:04}.row")),
            format!(r#"("manual"="m{index:04}","blocked"="b{index:04}","kind"="a")"#),
        )
        .unwrap();
    }
    let mut definition = crate::source::test_support::document_folder("data", "data");
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
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                build_workers: std::num::NonZeroUsize::new(workers),
                inference: config,
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        let report = repo.inference_report("data").unwrap();
        assert!(report.complete);
        assert_eq!(report.admitted_paths, 1);
        let stats = &repo.startup().inference_build;
        assert_eq!(stats.observed_records, 513);
        assert_eq!(stats.merged_batches, 2);
        assert_eq!(stats.retained_manual_summary_bytes, 0);
        let indexes = &repo.segments[0][0]
            .index
            .inference
            .as_ref()
            .unwrap()
            .indexes;
        assert_eq!(indexes.len(), 1);
        let index = &indexes[&vec!["kind".into()]];
        outcomes.push((
            bincode::serialize(&report).unwrap(),
            index.presence.clone(),
            index
                .values
                .iter()
                .map(|(key, slots)| (key.clone(), slots.clone()))
                .collect::<Vec<_>>(),
            index
                .lengths
                .iter()
                .map(|(key, slots)| (key.clone(), slots.clone()))
                .collect::<Vec<_>>(),
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
            root.join(format!("data/{index:02}.row")),
            format!(r#"("flag"={flag},"score"={score},"tag"="{tag}")"#),
        )
        .unwrap();
    }
    std::fs::write(root.join("data/bad.row"), "{").unwrap();
    let definition = crate::source::test_support::document_folder("data", "data");
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
                verified_cache_bytes: 0,
                inference: InferenceConfig {
                    mode,
                    ..Default::default()
                },
                ..Default::default()
            };
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &root,
                vec![definition.clone()],
                options,
                crate::source::test_support::fixture_registry(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            assert_eq!(repo.build_parses(), 24);
            assert_eq!(repo.build_failed_parses(), 1);
            if mode == InferenceMode::Disabled {
                assert!(
                    repo.segments[0]
                        .iter()
                        .all(|segment| segment.index.inference.is_none())
                );
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
                    repo.segments[0][0]
                        .index
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
                    repo.segments[0].iter().all(|segment| {
                        segment
                            .index
                            .inference
                            .as_ref()
                            .is_none_or(|inference| inference.indexes.is_empty())
                    }),
                    mode == InferenceMode::ReportOnly
                );
            }
            let request = query("flag = true");
            let result = repo.search(&request).unwrap();
            assert_eq!(result.hits.len(), 1);
            assert_eq!(result.hits[0].record_id, "path:03.row");
            assert_eq!(result.hits, repo.search_reference(&request).unwrap().hits);
            answers.push(result.hits.clone());
            scans.push(result.counters.record_parses);
            repo.configure_verified_cache(1024 * 1024).unwrap();
            assert_eq!(repo.search(&request).unwrap().hits, result.hits);
            assert_eq!(repo.search(&request).unwrap().hits, result.hits);
            if mode == InferenceMode::ReportOnly {
                let reopened = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![definition.clone()],
                    options,
                    crate::source::test_support::fixture_registry(),
                )
                .map_err(|error| error.to_string())
                .unwrap();
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
                let reopened = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![definition.clone()],
                    options,
                    crate::source::test_support::fixture_registry(),
                )
                .map_err(|error| error.to_string())
                .unwrap();
                assert_eq!(reopened.search(&request).unwrap().hits, result.hits);
                let one_worker = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![definition.clone()],
                    RepositoryOptions {
                        build_workers: std::num::NonZeroUsize::new(1),
                        ..options
                    },
                    crate::source::test_support::fixture_registry(),
                )
                .map_err(|error| error.to_string())
                .unwrap();
                assert_eq!(
                    bincode::serialize(repo.inference_report("data").unwrap()).unwrap(),
                    bincode::serialize(one_worker.inference_report("data").unwrap()).unwrap(),
                );
                let disabled = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![definition.clone()],
                    RepositoryOptions {
                        inference: InferenceConfig::default(),
                        ..options
                    },
                    crate::source::test_support::fixture_registry(),
                )
                .map_err(|error| error.to_string())
                .unwrap();
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
        .observe_record(RecordSlot(0), &value(r#"("a"=true,"b"=1)"#))
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
    for (slot, fixture) in [
        r#"("mixed"=true,"shape"=1,"blocked"=true,"declared"=true)"#,
        r#"("mixed"=42,"shape"=[1],"blocked"=false,"declared"=false)"#,
    ]
    .into_iter()
    .enumerate()
    {
        batch
            .observe_record(RecordSlot(slot as u32), &value(fixture))
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
        let fixture = format!(
            r#"("flag"={},"unique"="value-{slot}","long"="{}")"#,
            slot == 0,
            "x".repeat(80)
        );
        batch
            .observe_record(RecordSlot(slot), &value(&fixture))
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
        let mut registry = crate::source::test_support::fixture_registry();
        registry
            .register("pipe", 1, "pipe", std::sync::Arc::new(PipeReader))
            .unwrap();
        registry
    };
    let definition = CollectionDefinition::folder("data", "data", SourceFormat::registered("pipe"));
    let options = RepositoryOptions {
        inference: InferenceConfig {
            mode: InferenceMode::AutoIndex,
            ..Default::default()
        },
        ..Default::default()
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        options,
        source(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
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
    for (name, fixture) in [
        ("a", r#"("items"=[])"#),
        ("b", r#"("items"=[("flag"=true)])"#),
        ("c", r#"("items"=[("flag"=false)])"#),
        ("d", r#"("items"=[()])"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.row")), fixture).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
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
        ("items.flag = true", vec!["path:a.row", "path:b.row"]),
        ("items.flag ?= true", vec!["path:b.row"]),
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
        assert!(!repo.explain(&request).unwrap().is_empty());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn derived_existence_and_null_use_exact_inferred_postings() {
    let root = temporary_root("derived");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for slot in 0..24 {
        let fixture = match slot {
            3 => r#"("flag"=true)"#,
            5 => r#"("nullable"=null)"#,
            6 => r#"("nullable"=false)"#,
            7 => r#"("items"=("flag"=true))"#,
            8 => r#"("only_null"=null)"#,
            _ => r#"("other"=false)"#,
        };
        std::fs::write(root.join(format!("data/{slot:02}.row")), fixture).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    for (filter, expected) in [
        ("flag:exists = true", vec!["path:03.row"]),
        ("flag:? = true", vec!["path:03.row"]),
        ("nullable:null = true", vec!["path:05.row"]),
        ("nullable:? = true", vec!["path:06.row"]),
        ("items.flag:exists = true", vec!["path:07.row"]),
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
    let mut null_only = query("only_null:null = true");
    null_only.options.return_mode = ReturnMode::Locator;
    let result = repo.search(&null_only).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["path:08.row"]
    );
    assert_eq!(result.hits, repo.search_reference(&null_only).unwrap().hits);
    assert!(result.counters.record_parses > 0);
    assert!(
        !repo
            .explain(&null_only)
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
        ("00", r#"("year"=1799)"#),
        ("01", r#"("year"=1800)"#),
        ("02", r#"("year"=1805)"#),
        ("03", r#"("year"=1810)"#),
        ("04", r#"("year"=null)"#),
        ("05", r#"("other"=[1800,1810])"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.row")), document).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    for (filter, expected) in [
        ("year = 1800", vec!["path:01.row"]),
        ("year > 1800", vec!["path:02.row", "path:03.row"]),
        (
            "year >= 1800 && year < 1810",
            vec!["path:01.row", "path:02.row"],
        ),
        ("year < 1800", vec!["path:00.row"]),
        ("year <= 1800", vec!["path:00.row", "path:01.row"]),
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
            "{filter}: {trace:?}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn numeric_range_keeps_residual_for_empty_ancestor_selections() {
    let root = temporary_root("numeric-empty-ancestor");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/one.row"), r#"("items"=("year"=1800))"#).unwrap();
    std::fs::write(root.join("data/empty.row"), r#"("items"=[])"#).unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
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
fn paired_comparison_keeps_missing_and_successful_empty_traversal_distinct() {
    let root = temporary_root("paired-empty-traversal");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/empty.row"), r#"("items"=[])"#).unwrap();
    std::fs::write(root.join("data/missing.row"), "()").unwrap();
    std::fs::write(root.join("data/value.row"), r#"("items"=[("flag"="A")])"#).unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let mut request = query("items[*].flag = items[*].flag");
    request.options.return_mode = ReturnMode::Locator;
    let optimized = repo.search(&request).unwrap();
    let reference = repo.search_reference(&request).unwrap();
    assert_eq!(optimized.hits, reference.hits);
    assert_eq!(
        optimized
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["path:empty.row", "path:value.row"]
    );
    assert!(
        repo.explain(&request)
            .unwrap()
            .iter()
            .all(|trace| trace.remaining_verification)
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn complete_short_strings_and_booleans_prove_predicates_without_source_reads() {
    let root = temporary_root("exact-short-bool");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, document) in [
        ("00", r#"("code"="A1","flag"=true)"#),
        ("01", r#"("code"="B2","flag"=false)"#),
        ("02", r#"("code"="a1","flag"=true)"#),
        ("03", r#"("code"="","flag"=null)"#),
        ("04", r#"("other"=17)"#),
        ("05", r#"("code"="A10","flag"=false)"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.row")), document).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    for (filter, expected) in [
        ("code = 'A1'", vec!["path:00.row"]),
        (
            "code != 'A1'",
            vec!["path:01.row", "path:02.row", "path:03.row", "path:05.row"],
        ),
        ("code >= 'A10' && code < 'B2'", vec!["path:05.row"]),
        ("code ~ 'A1'", vec!["path:00.row", "path:05.row"]),
        (
            "code !~ 'A1'",
            vec!["path:01.row", "path:02.row", "path:03.row"],
        ),
        ("flag = true", vec!["path:00.row", "path:02.row"]),
        ("flag != false", vec!["path:00.row", "path:02.row"]),
        ("flag > false", vec!["path:00.row", "path:02.row"]),
        ("flag < true", vec!["path:01.row", "path:05.row"]),
        (
            "flag:exists = true",
            vec![
                "path:00.row",
                "path:01.row",
                "path:02.row",
                "path:03.row",
                "path:05.row",
            ],
        ),
        ("flag:null = true", vec!["path:03.row"]),
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
        let plan = repo.explain(&request).unwrap();
        assert!(
            plan[0]
                .terms
                .iter()
                .all(|term| term.selected.kind == AccessPathKind::Posting && term.selected.exact),
            "{filter}: {plan:?}"
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
        ("00", r#"("year"=3,"code"="z","flag"=false)"#),
        ("01", r#"("year"=1,"code"="ä","flag"=true)"#),
        ("02", r#"("year"=3,"code"="a","flag"=true)"#),
        ("03", r#"("year"=null,"code"=null,"flag"=null)"#),
        ("04", r#"("other"="missing")"#),
        ("05", "{"),
    ] {
        std::fs::write(root.join(format!("data/{name}.row")), document).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            verified_cache_bytes: 0,
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
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
                let estimate = repo.explain(&request).unwrap();
                assert!(!estimate.is_empty());
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
    repo.configure_verified_cache(1024 * 1024).unwrap();
    for _ in 0..2 {
        let warm = repo.search(&filtered).unwrap();
        assert_eq!(warm.hits, result.hits);
        assert_eq!(warm.count, result.count);
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
            root.join(format!("data/{name}.row")),
            fixture_value!({ "code" => (code) }).fixture_text(),
        )
        .unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions {
            inference: InferenceConfig {
                mode: InferenceMode::AutoIndex,
                ..Default::default()
            },
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .map_err(|error| error.to_string())
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
    assert!(!repo.explain(&request).unwrap().is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

use crate::source::test_support::{FixtureText, fixture_value};
