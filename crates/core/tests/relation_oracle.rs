use std::collections::HashMap;

use objekte_search_core::model::{LogicalType, Value};
use objekte_search_core::parameters::parse_parameter_json;
use objekte_search_core::query::{CaseSensitivity, FtsMode, Query, RegexOptions};
use objekte_search_core::reference::{FieldDefinition, Schema};
use objekte_search_core::relation::{RelationCardinality, RelationDefinition};
use objekte_search_core::repository::{
    CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
};
use objekte_search_core::source::SourceFormat;
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
        r#"{"id":"A","title":"Hymn","issued":"2024-02-29"}"#,
        r#"{"id":"B","title":"Festival","issued":"2024-03-01"}"#,
        r#"{"id":"C","title":"Hymn","issued":"2024-02-01"}"#,
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
            snapshot_mode: SnapshotMode::ReadWrite,
            verified_cache_bytes: 1 << 20,
            ..Default::default()
        };
        let built = Repository::open_with_options(&root, definitions(), options).unwrap();
        let restored = Repository::open_with_options(&root, definitions(), options).unwrap();
        assert!(restored.startup().restored);
        for repository in [&built, &restored] {
            for (filter, _field, _predicate, _quantifier, _negated, expected) in &cases {
                for mode in [ReturnMode::Locator, ReturnMode::Bytes, ReturnMode::Logical] {
                    let request = query("mentions", filter, mode);
                    let actual = repository.search(&request).unwrap();
                    let reference = repository.search_reference(&request).unwrap();
                    assert_eq!(sorted_ids(&actual), *expected, "{filter}");
                    assert_eq!(actual.count, Some(expected.len()), "{filter}");
                    assert_eq!(actual.hits, reference.hits, "{filter}");
                    assert_eq!(actual.count, reference.count, "{filter}");
                }
            }
            let mut selected = query("mentions", "entry = $target", ReturnMode::Locator);
            selected.fts = Some(objekte_search_core::query::FtsQuery {
                pattern_parameter: Some("pattern".into()),
                literal: String::new(),
                fields: vec![objekte_search_core::query::FtsField {
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
            assert!(repository.search(&selected).is_err());
            assert!(
                repository
                    .search_with_parameters(&selected, &HashMap::new())
                    .is_err()
            );
            let mut wrong_type = params.clone();
            wrong_type.insert("pattern".into(), Value::Bool(true));
            assert!(
                repository
                    .search_with_parameters(&selected, &wrong_type)
                    .is_err()
            );
            let actual = repository
                .search_with_parameters(&selected, &params)
                .unwrap();
            let reference = repository
                .search_reference_with_parameters(&selected, &params)
                .unwrap();
            assert_eq!(sorted_ids(&actual), ["m1"]);
            assert_eq!(actual.hits, reference.hits);
            assert_eq!(actual.count, Some(1));
            let mut alternate = params.clone();
            alternate.insert("pattern".into(), Value::String("^Fest.*$".into()));
            assert_eq!(
                repository
                    .search_with_parameters(&selected, &alternate)
                    .unwrap()
                    .count,
                Some(0)
            );
            assert!(
                repository
                    .search_with_parameters(&selected, &params)
                    .unwrap()
                    .counters
                    .verified_result_hit
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
