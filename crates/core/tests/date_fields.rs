use objekte_core::{CacheMode, JsonFilter, JsonQuery, JsonQueryBackend, Options, Repository};
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};


fn matches(filter: &str, record: Value) -> bool {
    JsonFilter::compile(filter, &BTreeMap::new())
        .unwrap()
        .matches(&record)
        .unwrap()
}


#[test]
fn date_strings_compare_as_the_start_of_their_period() {
    let offset = json!({"at": "2026-05-31T23:30:00-01:00"});
    assert!(matches("at = 2026-06", offset.clone()));
    assert!(!matches("at = 2026-05", offset.clone()));
    assert!(matches("at = 2026-06-01T00:30Z", offset));

    let day = json!({"at": "2026-05-10"});
    assert!(matches("at = 2026-05", day.clone()));
    assert!(matches("at = 2026-05-10", day.clone()));
    assert!(matches("at < 2026-05-10T12:00Z", day.clone()));
    assert!(matches("at > 2026-05-09", day.clone()));
    assert!(matches("at >= 2026-05-10", day.clone()));
    assert!(!matches("at >= 2026-05-11", day.clone()));
    assert!(!matches("at != 2026-05", day.clone()));
    // A literal on the left reverses the comparison.
    assert!(matches("2026-05-09 < at", day.clone()));
    assert!(!matches("2026-05-10 < at", day));

    let precise = json!({"at": "2026-05-10T10:00:00.000000001Z"});
    // `>` means at or after the end of the literal's period.
    assert!(!matches("at > 2026-05-10T10:00:00Z", precise.clone()));
    assert!(matches(
        "at > 2026-05-10T10:00:00.000000000Z",
        precise.clone()
    ));
    assert!(matches("at = 2026-05-10T10:00Z", precise));
    assert!(matches("at = 2026-05-01", json!({"at": "2026-05"})));
}

#[test]
fn values_outside_the_literal_grammar_keep_type_mismatch_semantics() {
    for value in [
        json!("soon"),
        json!("2026-05-10T10:00"),
        json!("2026-05-10 10:00Z"),
        json!("2026-02-30"),
        json!(2026),
        json!(null),
        json!(true),
    ] {
        let record = json!({"at": value});
        assert!(!matches("at = 2026-05", record.clone()), "{value}");
        assert!(!matches("at < 2026-05", record.clone()), "{value}");
        assert!(matches("at != 2026-05", record), "{value}");
    }
    assert!(!matches("at != 2026-05", json!({})));
    assert!(!matches(
        "at = 2026-05",
        json!({"at": {"date": "2026-05-01"}})
    ));
}

#[test]
fn arrays_use_existential_date_comparisons() {
    let record =
        json!({"dates": ["2025-01-01", "2026-05-02T08:00Z"], "nested": {"at": "2026-05-02"}});
    assert!(matches("dates ?= 2026-05", record.clone()));
    assert!(!matches("dates ?< 2024", record.clone()));
    // A bare year is a number, not a date literal.
    assert!(!matches("dates[0] < 2026", record.clone()));
    assert!(matches("dates[0] < 2026-01", record.clone()));
    assert!(matches("nested.at = 2026-05-02", record.clone()));
    assert!(
        JsonFilter::compile("dates = 2026-05", &BTreeMap::new())
            .unwrap()
            .matches(&record)
            .is_err()
    );
}

#[test]
fn date_literal_errors_name_the_allowed_operands() {
    for (filter, message) in [
        ("@size = 2026-05", "require a JSON field"),
        ("2026-05 = 2026-06", "require a JSON field"),
        ("at ~ 2026-05", "~ cannot compare"),
        ("at = 2026-13", "invalid UTC date/time"),
        ("at > 2026-05-01T12:00", "invalid UTC date/time"),
    ] {
        let error = JsonFilter::compile(filter, &BTreeMap::new())
            .err()
            .unwrap_or_else(|| panic!("{filter} compiled"))
            .to_string();
        assert!(error.contains(message), "{filter}: {error}");
    }
}


fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    let schemas = root.join(".objekte/collections");
    fs::create_dir_all(&schemas).unwrap();
    fs::write(
        schemas.join("events.schema.json"),
        serde_json::to_vec(&json!({
            "type":"object",
            "properties":{
                "at":{"type":"string","index":true},
                "day":{"type":"string"}
            },
            "x-objekte":{"name":"events","path":"data/events","type":"data","format":"json","id":"filename"}
        }))
        .unwrap(),
    )
    .unwrap();
    let folder = root.join("data/events");
    fs::create_dir_all(&folder).unwrap();
    for (id, body) in [
        (
            "a",
            json!({"at":"2026-05-01T00:00:00Z","day":"2026-05-01"}).to_string(),
        ),
        (
            "b",
            json!({"at":"2026-04-30T23:59:59.999Z","day":"2026-04-30"}).to_string(),
        ),
        (
            "c",
            json!({"at":"2026-05-31T22:00:00-03:00","day":"2026-06-01"}).to_string(),
        ),
        ("d", json!({"at":"later","day":null}).to_string()),
        ("e", json!({"day":"2026-05-15"}).to_string()),
        ("f", "{\"at\": \"2026-05-02".to_owned()),
    ] {
        fs::write(folder.join(format!("{id}.json")), body).unwrap();
    }
    temp
}

fn ids(root: &Path, cache: CacheMode, backend: JsonQueryBackend, filter: &str) -> Vec<String> {
    let mut options = Options::new();
    options.cache = cache;
    let mut repository = Repository::load_repository(root, options).unwrap();
    let result = repository
        .query_json(&JsonQuery {
            filter: format!("{filter} SORT BY @id"),
            collections: Some(vec!["events".into()]),
            backend,
            count: true,
            limit: 10,
            ..Default::default()
        })
        .unwrap();
    assert_eq!(result.total, Some(result.records.len()));
    result.records.into_iter().map(|record| record.id).collect()
}

#[test]
fn indexed_and_direct_backends_agree_on_date_fields() {
    let fixture = fixture();
    let root = fixture.path();
    for (filter, expected) in [
        ("at = 2026-05", vec!["a"]),
        ("at >= 2026-05", vec!["a", "c"]),
        ("at < 2026-05", vec!["b"]),
        ("at != 2026-05", vec!["b", "c", "d"]),
        ("day = 2026-05", vec!["a", "e"]),
        ("day > 2026-05-01 && at >= 2026-06", vec!["c"]),
        ("exists(at) && at != 2026", vec!["a", "b", "c", "d"]),
    ] {
        let indexed = ids(root, CacheMode::Memory, JsonQueryBackend::Auto, filter);
        let direct = ids(root, CacheMode::Disabled, JsonQueryBackend::Direct, filter);
        assert_eq!(indexed, expected, "{filter}");
        assert_eq!(direct, expected, "{filter}");
    }
}
