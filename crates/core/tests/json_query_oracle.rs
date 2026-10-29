//! Independent source-file oracle for the published JSON query workload.
//!
//! The oracle deliberately uses serde_json, directory enumeration, and explicit
//! predicates. It does not call the filter parser, evaluator, picker, or cache.

mod support;

use objekte_core::{CacheMode, JsonQuery, JsonQueryBackend, Options, Repository};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, fs, path::Path};
use support::Fixture;


/// A named filter from the same workload used by the warm benchmark.
#[derive(Clone, Deserialize)]
struct Case {
    name: String,
    filter: String,
}


fn workload() -> Vec<Case> {
    serde_json::from_str(include_str!("../../bench/workloads/musenalm-bigram.json")).unwrap()
}

fn field_text<'a>(record: &'a Value, field: &str) -> Option<&'a str> {
    record.get(field)?.as_str()
}

fn field_number(record: &Value, field: &str) -> Option<f64> {
    record.get(field)?.as_number()?.as_f64()
}

fn contains_ascii(record: &Value, field: &str, needle: &str) -> bool {
    field_text(record, field).is_some_and(|text| {
        text.to_ascii_lowercase()
            .contains(&needle.to_ascii_lowercase())
    })
}

fn text_is(record: &Value, field: &str, expected: &str) -> bool {
    field_text(record, field) == Some(expected)
}

fn title_starts_with(record: &Value, prefix: &str) -> bool {
    field_text(record, "preferred_title").is_some_and(|text| {
        text.to_ascii_lowercase()
            .starts_with(&prefix.to_ascii_lowercase())
    })
}

fn title_matches_m_one_nd(record: &Value) -> bool {
    field_text(record, "preferred_title").is_some_and(|title| {
        let chars = title.chars().collect::<Vec<_>>();
        chars.windows(4).any(|part| {
            part[0].eq_ignore_ascii_case(&'m')
                && part[2].eq_ignore_ascii_case(&'n')
                && part[3].eq_ignore_ascii_case(&'d')
        })
    })
}

fn title_matches_m_any_nd(record: &Value) -> bool {
    field_text(record, "preferred_title").is_some_and(|title| {
        let title = title.to_ascii_lowercase();
        title.starts_with('m') && title.ends_with("nd")
    })
}

fn has_array_text(record: &Value, field: &str, expected: &str) -> bool {
    record
        .get(field)
        .and_then(Value::as_array)
        .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(expected)))
}

fn number_is(record: &Value, field: &str, expected: f64) -> bool {
    field_number(record, field) == Some(expected)
}

fn year_is_not_1800(record: &Value) -> bool {
    record.get("year").is_some_and(|value| match value {
        Value::Number(number) => number.as_f64() != Some(1800.0),
        Value::Array(_) | Value::Object(_) => false,
        _ => true,
    })
}

// Each arm is a handwritten reading of one benchmark filter. Keeping this
// separate from the query language makes shared parser/evaluator bugs visible.
fn oracle_matches(case: &str, record: &Value) -> bool {
    match case {
        "title_almanach" => contains_ascii(record, "preferred_title", "almanach"),
        "title_liebe" => contains_ascii(record, "preferred_title", "Liebe"),
        "title_goethe" => contains_ascii(record, "preferred_title", "Goethe"),
        "title_schiller" => contains_ascii(record, "preferred_title", "Schiller"),
        "title_gedicht" => contains_ascii(record, "preferred_title", "Gedicht"),
        "title_mond" => contains_ascii(record, "preferred_title", "Mond"),
        "variant_liebe" => contains_ascii(record, "variant_title", "Liebe"),
        "title_statement_mond" => contains_ascii(record, "title_statement", "Mond"),
        "incipit_liebe" => contains_ascii(record, "incipit_statement", "Liebe"),
        "responsibility_goethe" => contains_ascii(record, "responsibility_statement", "Goethe"),
        "annotation_goethe" => contains_ascii(record, "annotation", "Goethe"),
        "publication_leipzig" => contains_ascii(record, "publication_statement", "Leipzig"),
        "type_gedicht_lied" => has_array_text(record, "musenalm_type", "Gedicht/Lied"),
        "type_text" => has_array_text(record, "musenalm_type", "Text"),
        "title_eq_mond" => text_is(record, "preferred_title", "Der Mond"),
        "title_eq_issue" => text_is(record, "preferred_title", "No. 11."),
        "title_prefix_lieb" => title_starts_with(record, "Lieb"),
        "title_like_mond" => contains_ascii(record, "preferred_title", "Mond"),
        "title_like_wildcard" => contains_ascii(record, "preferred_title", "M_nd"),
        "title_one_char_wildcard" => title_matches_m_one_nd(record),
        "title_regex_or" => {
            contains_ascii(record, "preferred_title", "Liebe")
                || contains_ascii(record, "preferred_title", "Mond")
        }
        "year_eq_1800" => number_is(record, "year", 1800.0),
        "year_lt_1800" => field_number(record, "year").is_some_and(|year| year < 1800.0),
        "pseudonym_true" => record.get("pseudonym") == Some(&json!(true)),
        "pseudonym_false" => record.get("pseudonym") == Some(&json!(false)),
        "numbering_ge_10" => field_number(record, "numbering").is_some_and(|number| number >= 10.0),
        "musenalm_id_eq" => number_is(record, "musenalm_id", 56317.0),
        "title_liebe_year" => {
            contains_ascii(record, "preferred_title", "Liebe") && number_is(record, "year", 1800.0)
        }
        "title_almanach_year" => {
            contains_ascii(record, "preferred_title", "almanach")
                && field_number(record, "year").is_some_and(|year| year >= 1800.0)
        }
        "title_goethe_pseudonym" => {
            contains_ascii(record, "preferred_title", "Goethe")
                && record.get("pseudonym") == Some(&json!(true))
        }
        "title_gedicht_old" => {
            contains_ascii(record, "preferred_title", "Gedicht")
                && field_number(record, "year").is_some_and(|year| year < 1800.0)
        }
        "type_gedicht_year" => {
            has_array_text(record, "musenalm_type", "Gedicht/Lied")
                && field_number(record, "year").is_some_and(|year| year >= 1800.0)
        }
        "title_like_regex_prefix" => title_matches_m_any_nd(record),
        "mond_numbering" => {
            contains_ascii(record, "preferred_title", "Mond")
                && field_number(record, "numbering").is_some_and(|number| number > 10.0)
        }
        "year_ne_1800" => year_is_not_1800(record),
        "title_not_mond" => field_text(record, "preferred_title")
            .is_some_and(|title| !title.to_ascii_lowercase().contains("mond")),
        "exists_title" => record.get("preferred_title").is_some(),
        "year_or" => number_is(record, "year", 1800.0) || number_is(record, "year", 1900.0),
        "text_or_year" => {
            contains_ascii(record, "preferred_title", "Mond") || number_is(record, "year", 1800.0)
        }
        "year_window" => {
            field_number(record, "year").is_some_and(|year| (1800.0..1805.0).contains(&year))
        }
        "year_and_bool" => {
            number_is(record, "year", 1800.0) && record.get("pseudonym") == Some(&json!(true))
        }
        "type_length" => record
            .get("musenalm_type")
            .and_then(Value::as_array)
            .is_some_and(|values| values.len() >= 2),
        "title_empty" => text_is(record, "preferred_title", ""),
        "year_null" => record.get("year").is_some_and(Value::is_null),
        other => panic!("missing independent oracle for {other}"),
    }
}

fn source_ids(directory: &Path, cases: &[Case]) -> (usize, BTreeMap<String, Vec<String>>) {
    let mut expected = cases
        .iter()
        .map(|case| (case.name.clone(), Vec::new()))
        .collect::<BTreeMap<_, _>>();
    let mut files = 0;
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.extension().is_none_or(|extension| extension != "json") {
            continue;
        }
        files += 1;
        let bytes = fs::read(&path).unwrap();
        let Ok(record) = serde_json::from_slice::<Value>(&bytes) else {
            continue;
        };
        let id = path.file_stem().unwrap().to_str().unwrap();
        for case in cases {
            if oracle_matches(&case.name, &record) {
                expected.get_mut(&case.name).unwrap().push(id.to_owned());
            }
        }
    }
    for ids in expected.values_mut() {
        ids.sort_unstable();
    }
    (files, expected)
}

fn assert_backend(
    repository: &mut Repository,
    collection: &str,
    case: &Case,
    backend: JsonQueryBackend,
    expected: &[String],
) {
    let result = repository
        .query_json(&JsonQuery {
            filter: case.filter.clone(),
            collections: Some(vec![collection.into()]),
            backend,
            count: true,
            limit: expected.len().max(1),
            ..Default::default()
        })
        .unwrap_or_else(|error| panic!("{} {backend:?}: {error}", case.name));
    assert_eq!(
        result.total,
        Some(expected.len()),
        "{} {backend:?} total",
        case.name
    );
    let actual = result
        .records
        .into_iter()
        .map(|record| record.id)
        .collect::<Vec<_>>();
    assert_eq!(actual, expected, "{} {backend:?} IDs", case.name);
}

#[test]
fn independent_serde_oracle_checks_four_backends_on_edge_cases() {
    let fixture = Fixture::new();
    let schema_path = fixture.root.join(".editor/books/schema.json");
    let mut schema: Value = serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    for (field, definition) in [
        ("preferred_title", json!({"type":"string"})),
        ("year", json!({"type":"number"})),
        ("numbering", json!({"type":"number"})),
        ("pseudonym", json!({"type":"boolean"})),
        ("musenalm_id", json!({"type":"integer"})),
        (
            "musenalm_type",
            json!({"type":"array","items":{"type":"string"}}),
        ),
    ] {
        schema["properties"][field] = definition;
    }
    fs::write(schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    let books = fixture.root.join("data/books");
    for entry in fs::read_dir(&books).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    for (id, raw) in [
        (
            "a",
            r#"{"preferred_title":"Liebe","year":1800,"pseudonym":true,"numbering":11,"musenalm_id":56317,"musenalm_type":["Gedicht/Lied","Text"]}"#,
        ),
        (
            "b",
            r#"{"preferred_title":"Münd","year":1900,"pseudonym":false,"numbering":0.1,"musenalm_type":[]}"#,
        ),
        (
            "c",
            r#"{"preferred_title":"N\u006f. \u0031\u0031.","year":1800.0,"pseudonym":false,"numbering":12,"musenalm_type":["Text"]}"#,
        ),
        (
            "d",
            r#"{"preferred_title":"Mond","year":null,"pseudonym":true,"musenalm_type":["other"]}"#,
        ),
        ("e", r#"{"preferred_title":"","musenalm_type":[]}"#),
        (
            "f",
            r#"{"preferred_title":"Der Mond","year":"1800","pseudonym":"true","numbering":"12","musenalm_type":["Gedicht/Lied"]}"#,
        ),
        ("g", "not json"),
    ] {
        fs::write(books.join(format!("{id}.json")), raw).unwrap();
    }
    let selected = [
        "title_liebe",
        "title_eq_issue",
        "title_like_wildcard",
        "title_like_regex_prefix",
        "title_not_mond",
        "title_empty",
        "year_eq_1800",
        "year_lt_1800",
        "year_or",
        "text_or_year",
        "year_and_bool",
        "year_ne_1800",
        "year_null",
        "pseudonym_true",
        "pseudonym_false",
        "numbering_ge_10",
        "musenalm_id_eq",
        "type_gedicht_lied",
        "type_length",
        "exists_title",
        "title_liebe_year",
    ];
    let mut cases = workload()
        .into_iter()
        .filter(|case| selected.contains(&case.name.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(cases.len(), selected.len());
    cases.push(Case {
        name: "title_one_char_wildcard".into(),
        filter: "preferred_title ~ '%M_nd%'".into(),
    });
    let (files, expected) = source_ids(&books, &cases);
    assert_eq!(files, 7);
    assert_eq!(expected["year_eq_1800"], ["a", "c"]);
    assert_eq!(expected["title_eq_issue"], ["c"]);
    assert!(expected["title_like_wildcard"].is_empty());
    assert_eq!(expected["title_one_char_wildcard"], ["b", "d", "f"]);
    assert_eq!(expected["year_null"], ["d"]);
    assert_eq!(expected["title_empty"], ["e"]);
    let mut memory = fixture.repo(CacheMode::Memory);
    let mut segments = fixture.repo(CacheMode::Segments);
    let mut direct = fixture.repo(CacheMode::Disabled);
    let mut grep = fixture.repo(CacheMode::Disabled);
    for case in &cases {
        let ids = &expected[&case.name];
        assert_backend(&mut memory, "books", case, JsonQueryBackend::Bigram, ids);
        assert_backend(
            &mut segments,
            "books",
            case,
            JsonQueryBackend::Segments,
            ids,
        );
        assert_backend(&mut direct, "books", case, JsonQueryBackend::Direct, ids);
        assert_backend(&mut grep, "books", case, JsonQueryBackend::Grep, ids);
    }
}

#[test]
#[ignore = "scans 133,948 Musenalm source files and compares complete IDs for all 43 benchmark filters"]
fn independent_serde_oracle_checks_every_musenalm_benchmark_filter() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let contents = root.join("data/contents");
    let cases = workload();
    assert_eq!(cases.len(), 43);
    let (files, expected) = source_ids(&contents, &cases);
    assert_eq!(files, 133_948);
    let temp = tempfile::tempdir().unwrap();
    let mut memory = Repository::load_repository(&root, Options::new(temp.path())).unwrap();
    let mut options = Options::new(temp.path());
    options.cache = CacheMode::Segments;
    let mut segments = Repository::load_repository(&root, options).unwrap();
    for case in &cases {
        let ids = &expected[&case.name];
        assert_backend(&mut memory, "contents", case, JsonQueryBackend::Bigram, ids);
        assert_backend(
            &mut segments,
            "contents",
            case,
            JsonQueryBackend::Segments,
            ids,
        );
        println!("{}: {} independently matched IDs", case.name, ids.len());
    }
}
