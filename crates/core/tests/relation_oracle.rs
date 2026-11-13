use std::collections::HashMap;

use objekte_search_core::index_storage::{
    IndexResidency, IndexStorageLocation, IndexStoragePolicy,
};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{LogicalType, Value};
use objekte_search_core::parameters::parse_parameter_json;
use objekte_search_core::query::{CaseSensitivity, FtsMode, Query, RegexOptions};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions,
};
use objekte_search_core::source::{SourceFormat, SourceRegistry};
use objekte_search_core::workload::{BigramAlphabet, ReturnMode, WorkloadCase};
use serde_json::Value as JsonValue;

fn add_field(schema: &mut Schema, name: &str, kind: LogicalType, multi: bool) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: name == "id",
            nullable: name != "id",
            indexed: true,
            multi,
            element_nullable: vec![],
        })
        .unwrap();
}

fn definition(name: &str, schema: Schema) -> CollectionDefinition {
    CollectionDefinition {
        name: name.into(),
        source_path: name.into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn definitions() -> Vec<CollectionDefinition> {
    let mut entries = Schema::default();
    add_field(&mut entries, "id", LogicalType::Id, false);
    add_field(&mut entries, "title", LogicalType::String, false);
    add_field(&mut entries, "issued", LogicalType::Date, false);
    add_field(
        &mut entries,
        "aliases",
        LogicalType::Array(Box::new(LogicalType::String)),
        true,
    );
    let mut mentions = Schema::default();
    add_field(&mut mentions, "id", LogicalType::Id, false);
    add_field(&mut mentions, "title", LogicalType::String, false);
    add_field(&mut mentions, "entry", LogicalType::Id, false);
    add_field(
        &mut mentions,
        "contributors",
        LogicalType::Array(Box::new(LogicalType::Id)),
        true,
    );
    for (path, cardinality, alias) in [
        ("entry", RelationCardinality::One, "mentions"),
        ("contributors", RelationCardinality::Many, "contributions"),
    ] {
        mentions
            .register_relation(RelationDefinition {
                path: vec![path.into()],
                target_collection: "entries".into(),
                cardinality,
                reverse_alias: Some(alias.into()),
            })
            .unwrap();
    }
    vec![
        definition("entries", entries),
        definition("mentions", mentions),
    ]
}

fn fixture() -> (
    std::path::PathBuf,
    Vec<JsonValue>,
    HashMap<String, JsonValue>,
) {
    let root = std::env::temp_dir().join(format!(
        "objekte-independent-relation-oracle-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("entries")).unwrap();
    std::fs::create_dir_all(root.join("mentions")).unwrap();
    let entries = [
        r#"{"id":"A","title":"Hymn","issued":"2024-02-29","aliases":["Hymn","Other"]}"#,
        r#"{"id":"B","title":"Festival","issued":"2024-03-01","aliases":["Festival"]}"#,
        r#"{"id":"C","title":"Hymn","issued":"2024-02-01","aliases":["Other","Hymn"]}"#,
    ];
    let mentions = [
        r#"{"id":"m1","entry":"A","contributors":["A","B"],"title":"Festival"}"#,
        r#"{"id":"m2","entry":"B","contributors":["B"],"title":"Hymn"}"#,
        r#"{"id":"m3","entry":"Z","contributors":["Z"],"title":"Dangling"}"#,
        r#"{"id":"m4","entry":null,"contributors":[],"title":"Null"}"#,
        r#"{"id":"m5","title":"Missing"}"#,
        r#"{"id":"m6","entry":"C","contributors":["A","C"],"title":"Both"}"#,
    ];
    let mut targets = HashMap::new();
    for (index, text) in entries.iter().enumerate() {
        std::fs::write(root.join(format!("entries/{index}.json")), text).unwrap();
        let value: JsonValue = serde_json::from_str(text).unwrap();
        targets.insert(value["id"].as_str().unwrap().to_owned(), value);
    }
    let mut sources = Vec::new();
    for (index, text) in mentions.iter().enumerate() {
        std::fs::write(root.join(format!("mentions/{index}.json")), text).unwrap();
        sources.push(serde_json::from_str(text).unwrap());
    }
    (root, sources, targets)
}

#[derive(Clone, Copy)]
enum TargetTest {
    TitleIsHymn,
    TitleIsNotHymn,
    IssuedInFebruary,
}

#[derive(Clone, Copy)]
enum Quantifier {
    All,
    Any,
}

// This oracle reads the original JSON fixture and never calls the engine's
// binding, relation index, scanner, or exact comparison helpers.
fn oracle_relation(
    source: &JsonValue,
    field: &str,
    targets: &HashMap<String, JsonValue>,
    predicate: TargetTest,
    quantifier: Quantifier,
) -> Option<bool> {
    let Some(value) = source.get(field) else {
        return Some(false);
    };
    if value.is_null() {
        return Some(false);
    }
    let ids: Vec<_> = if let Some(items) = value.as_array() {
        items.iter().collect()
    } else {
        vec![value]
    };
    let mut saw_unavailable = false;
    for id in ids {
        let Some(id) = id.as_str() else {
            return None;
        };
        let next = targets.get(id).map(|target| match predicate {
            TargetTest::TitleIsHymn => target["title"].as_str() == Some("Hymn"),
            TargetTest::TitleIsNotHymn => target["title"].as_str() != Some("Hymn"),
            TargetTest::IssuedInFebruary => target["issued"]
                .as_str()
                .is_some_and(|date| date.starts_with("2024-02-")),
        });
        match (quantifier, next) {
            (Quantifier::Any, Some(true)) => return Some(true),
            (Quantifier::All, Some(false)) => return Some(false),
            (_, None) => saw_unavailable = true,
            _ => {}
        }
    }
    if saw_unavailable {
        None
    } else {
        Some(matches!(quantifier, Quantifier::All))
    }
}

fn query(collection: &str, filter: &str, return_mode: ReturnMode) -> Query {
    Query::from_workload(&WorkloadCase {
        id: "oracle".into(),
        collection: collection.into(),
        fts: None,
        fts_case_sensitivity: CaseSensitivity::Sensitive,
        fts_mode: FtsMode::Literal,
        filter: Some(filter.into()),
        fts_fields: vec![],
        sort: vec![],
        limit: 100,
        offset: 0,
        count: true,
        return_mode,
        evidence: true,
    })
    .unwrap()
}

fn sorted_ids(result: &objekte_search_core::repository::SearchResult) -> Vec<String> {
    let mut ids = result
        .hits
        .iter()
        .map(|hit| hit.record_id.clone())
        .collect::<Vec<_>>();
    ids.sort();
    ids
}

#[test]
fn independent_json_relation_oracle_covers_quantifiers_negation_periods_and_restore() {
    let (root, sources, targets) = fixture();
    let cases = [
        (
            "entry.title = 'Hymn'",
            "entry",
            TargetTest::TitleIsHymn,
            Quantifier::All,
            false,
            vec!["m1", "m6"],
        ),
        (
            "!(entry.title = 'Hymn')",
            "entry",
            TargetTest::TitleIsHymn,
            Quantifier::All,
            true,
            vec!["m2", "m4", "m5"],
        ),
        (
            "contributors.title ?= 'Hymn'",
            "contributors",
            TargetTest::TitleIsHymn,
            Quantifier::Any,
            false,
            vec!["m1", "m6"],
        ),
        (
            "contributors.title = 'Hymn'",
            "contributors",
            TargetTest::TitleIsHymn,
            Quantifier::All,
            false,
            vec!["m4", "m6"],
        ),
        (
            "contributors.title ?!= 'Hymn'",
            "contributors",
            TargetTest::TitleIsNotHymn,
            Quantifier::Any,
            false,
            vec!["m1", "m2"],
        ),
        (
            "entry.issued = 2024-02",
            "entry",
            TargetTest::IssuedInFebruary,
            Quantifier::All,
            false,
            vec!["m1", "m6"],
        ),
    ];
    for (filter, field, predicate, quantifier, negated, expected) in &cases {
        let mut oracle = sources
            .iter()
            .filter(|source| {
                let answer = oracle_relation(source, field, &targets, *predicate, *quantifier);
                (if *negated {
                    answer.map(|value| !value)
                } else {
                    answer
                }) == Some(true)
            })
            .map(|source| source["id"].as_str().unwrap().to_owned())
            .collect::<Vec<_>>();
        oracle.sort();
        assert_eq!(oracle, *expected, "fixture oracle for {filter}");
    }
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            verified_cache_bytes: 1 << 20,
            ..Default::default()
        };
        let built = Repository::open_with_options(&root, definitions(), options).unwrap();
        let policy = IndexStoragePolicy {
            location: IndexStorageLocation::RepositoryDirectory,
            residency: IndexResidency::OnDemand,
        };
        let first = Index::open_with_storage(
            &root,
            definitions(),
            options,
            policy.clone(),
            SourceRegistry::builtin(),
        )
        .unwrap();
        first.stop_sync_storage().unwrap();
        let restored = Index::open_with_storage(
            &root,
            definitions(),
            options,
            policy,
            SourceRegistry::builtin(),
        )
        .unwrap();
        assert!(restored.restored_from_cache());
        let view = restored.view().unwrap();
        for stored in [false, true] {
            let search = |query: &Query| {
                if stored {
                    view.search(query)
                } else {
                    built.search(query)
                }
            };
            let search_with_parameters = |query: &Query, params: &HashMap<String, Value>| {
                if stored {
                    view.search_with_parameters(query, params)
                } else {
                    built.search_with_parameters(query, params)
                }
            };
            for (filter, _field, _predicate, _quantifier, _negated, expected) in &cases {
                for mode in [ReturnMode::Locator, ReturnMode::Bytes, ReturnMode::Logical] {
                    let request = query("mentions", filter, mode);
                    let actual = search(&request).unwrap();
                    let reference = built.search_reference(&request).unwrap();
                    assert_eq!(sorted_ids(&actual), *expected, "{filter}");
                    assert_eq!(actual.count, Some(expected.len()), "{filter}");
                    assert_eq!(actual.hits, reference.hits, "{filter}");
                    assert_eq!(actual.count, reference.count, "{filter}");
                }
            }
            for (collection, path, expected) in [
                ("mentions", "contributors[0].title", vec!["m1", "m6"]),
                ("mentions", "contributors[1].title", vec!["m6"]),
                ("entries", "aliases[0]", vec!["A"]),
                ("entries", "aliases[1]", vec!["C"]),
                ("mentions", "entry.aliases[0]", vec!["m1"]),
                ("mentions", "entry.aliases[1]", vec!["m6"]),
            ] {
                for (literal, mode) in [
                    ("Hymn", FtsMode::Literal),
                    ("^Hy.*n$", FtsMode::Regex(RegexOptions::default())),
                ] {
                    let selected = Query::from_workload(&WorkloadCase {
                        id: "selected-position".into(),
                        collection: collection.into(),
                        fts: Some(literal.into()),
                        fts_case_sensitivity: CaseSensitivity::Sensitive,
                        fts_mode: mode,
                        filter: None,
                        fts_fields: vec![path.into()],
                        sort: vec![],
                        limit: 100,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: true,
                    })
                    .unwrap();
                    assert_eq!(
                        selected.fts.as_ref().unwrap().fields[0]
                            .query_path
                            .as_deref(),
                        Some(path)
                    );
                    let actual = search(&selected).unwrap();
                    let reference = built.search_reference(&selected).unwrap();
                    assert_eq!(sorted_ids(&actual), expected);
                    assert_eq!(actual.hits, reference.hits);
                    assert_eq!(actual.count, Some(expected.len()));
                    assert!(
                        actual
                            .hits
                            .iter()
                            .all(|hit| hit.matched_fields.contains(&path.to_owned()))
                    );
                    assert!(search(&selected).unwrap().counters.verified_result_hit);
                }
            }
            let mut selected = query("mentions", "entry = $target", ReturnMode::Locator);
            selected.fts = Some(objekte_search_core::query::FtsQuery {
                pattern_parameter: Some("pattern".into()),
                literal: String::new(),
                fields: vec![objekte_search_core::query::FtsField {
                    query_path: None,
                    path: objekte_search_core::model::FieldPath::parse("entry.title").unwrap(),
                    binding: None,
                }],
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: FtsMode::Regex(RegexOptions::default()),
            });
            let params = HashMap::from([
                (
                    "target".into(),
                    parse_parameter_json(r#"{"type":"id","value":"A"}"#).unwrap(),
                ),
                ("pattern".into(), Value::String("^Hy.*n$".into())),
            ]);
            assert!(search(&selected).is_err());
            assert!(search_with_parameters(&selected, &HashMap::new()).is_err());
            let mut wrong_type = params.clone();
            wrong_type.insert("pattern".into(), Value::Bool(true));
            assert!(search_with_parameters(&selected, &wrong_type).is_err());
            let actual = search_with_parameters(&selected, &params).unwrap();
            let reference = built
                .search_reference_with_parameters(&selected, &params)
                .unwrap();
            assert_eq!(sorted_ids(&actual), ["m1"]);
            assert_eq!(actual.hits, reference.hits);
            assert_eq!(actual.count, Some(1));
            let mut alternate = params.clone();
            alternate.insert("pattern".into(), Value::String("^Fest.*$".into()));
            assert_eq!(
                search_with_parameters(&selected, &alternate).unwrap().count,
                Some(0)
            );
            assert!(
                search_with_parameters(&selected, &params)
                    .unwrap()
                    .counters
                    .verified_result_hit
            );
        }
        restored.stop_sync_storage().unwrap();
        restored.wait_for_reconciliation().unwrap();
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explain_reports_bound_relation_and_temporal_semantics() {
    let (root, _, _) = fixture();
    let repo =
        Repository::open_with_options(&root, definitions(), RepositoryOptions::default()).unwrap();
    let mut request = query("mentions", "entry.issued = 2024-02", ReturnMode::Locator);
    request.fts = Some(objekte_search_core::query::FtsQuery {
        pattern_parameter: None,
        literal: "Hymn".into(),
        fields: vec![objekte_search_core::query::FtsField {
            path: objekte_search_core::model::FieldPath::parse("entry.title").unwrap(),
            binding: None,
            query_path: None,
        }],
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
    });
    let trace = repo.explain(&request).unwrap();
    assert_eq!(trace.len(), 1);
    assert_eq!(trace[0].fts_covered_target, "entry.title");
    assert_eq!(trace[0].fts_exact_literal.as_deref(), Some("Hymn"));
    let term = &trace[0].bound_terms[0];
    assert_eq!(term.field, "entry.issued");
    assert!(term.resolved_period.is_some());
    assert_eq!(term.relation_hops.len(), 1);
    assert_eq!(term.relation_hops[0].from_collection, "mentions");
    assert_eq!(term.relation_hops[0].to_collection, "entries");
    assert_eq!(term.relation_hops[0].direction, "Forward");
    let encoded = serde_json::to_value(&trace).unwrap();
    assert!(encoded[0]["bound_terms"][0]["resolved_period"].is_array());
    let parameterized = query("mentions", "entry = $target", ReturnMode::Locator);
    let mut parameters = HashMap::new();
    parameters.insert("target".to_owned(), Value::Id("A".into()));
    let bound_trace = repo
        .explain_with_parameters(&parameterized, &parameters)
        .unwrap();
    assert_eq!(
        bound_trace[0].bound_terms[0].bound_value,
        Some(Value::Id("A".into()))
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn false_and_branch_skips_expensive_relation_verification_in_reference_route() {
    let (root, _, _) = fixture();
    let repo =
        Repository::open_with_options(&root, definitions(), RepositoryOptions::default()).unwrap();
    let request = query(
        "mentions",
        "@record.id = 'm1' && entry.title = 'Hymn'",
        ReturnMode::Locator,
    );
    let answer = repo.search_reference(&request).unwrap();
    assert_eq!(sorted_ids(&answer), ["m1"]);
    assert_eq!(answer.counters.relation_target_reads, 1);
    std::fs::remove_dir_all(root).unwrap();
}
