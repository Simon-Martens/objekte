mod support;
use objekte_core::{CacheMode, Freshness, JsonFilter, JsonQuery, JsonQueryBackend, Repository};
use serde_json::json;
use std::{collections::BTreeMap, fs};
use support::*;

fn fixture(docs: &[(&str, &str)]) -> Fixture {
    let f = Fixture::new();
    for name in ["books", "people", "spare"] {
        let schema_path = f.root.join(format!(".editor/{name}/schema.json"));
        let mut schema: serde_json::Value =
            serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
        for field in [
            "x", "name", "start", "end", "tags", "authors", "nested", "a", "a.b", "empty",
            "nil\0key", "other", "n", "p", "missing",
        ] {
            schema["properties"]
                .as_object_mut()
                .unwrap()
                .entry(field)
                .or_insert_with(|| json!({}));
        }
        fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
        for entry in fs::read_dir(f.root.join("data").join(name)).unwrap() {
            fs::remove_file(entry.unwrap().path()).unwrap();
        }
    }
    for (id, raw) in docs {
        fs::write(f.root.join(format!("data/books/{id}.json")), raw).unwrap();
    }
    f
}

fn request(filter: &str) -> JsonQuery {
    JsonQuery {
        filter: filter.into(),
        count: true,
        limit: 1000,
        ..Default::default()
    }
}

fn ids_for(r: &mut Repository, request: &JsonQuery) -> Vec<String> {
    let result = r.query_json(request).unwrap();
    assert_eq!(result.total, Some(result.records.len()));
    result.records.into_iter().map(|r| r.id).collect()
}

fn check(f: &Fixture, cases: &[(&str, &[&str])]) {
    let mut direct = f.repo(CacheMode::Disabled);
    let mut cached = f.repo(CacheMode::Memory);
    let mut grep = f.repo(CacheMode::Disabled);
    let mut bigram = f.repo(CacheMode::Disabled);
    for (filter, expected) in cases {
        let request = request(filter);
        assert_eq!(
            ids_for(
                &mut direct,
                &JsonQuery {
                    backend: JsonQueryBackend::Direct,
                    ..request.clone()
                }
            ),
            *expected,
            "direct: {filter}"
        );
        assert_eq!(
            ids_for(
                &mut cached,
                &JsonQuery {
                    backend: JsonQueryBackend::Bigram,
                    ..request.clone()
                }
            ),
            *expected,
            "bigram: {filter}"
        );
        assert_eq!(
            ids_for(
                &mut grep,
                &JsonQuery {
                    backend: JsonQueryBackend::Grep,
                    ..request.clone()
                }
            ),
            *expected,
            "grep: {filter}"
        );
        assert_eq!(
            ids_for(
                &mut bigram,
                &JsonQuery {
                    backend: JsonQueryBackend::Bigram,
                    ..request
                }
            ),
            *expected,
            "bigram: {filter}"
        );
    }
}

#[test]
fn missing_is_distinct_from_explicit_null_across_backends() {
    let f = fixture(&[
        ("a", r#"{"x":null}"#),
        ("b", r#"{}"#),
        ("c", r#"{"x":1}"#),
        ("bad", "not JSON"),
    ]);
    check(
        &f,
        &[
            ("missing(x)", &["b"]),
            ("exists(x)", &["a", "c"]),
            ("x = null", &["a"]),
        ],
    );
}

#[test]
fn undeclared_fields_search_all_collections_or_an_explicit_scope() {
    let f = fixture(&[
        ("a", r#"{"custom":{"tag":"moon"},"score":"high"}"#),
        ("b", r#"{"custom":{"tag":"sun"},"score":42}"#),
    ]);
    fs::write(
        f.root.join("data/people/p3.json"),
        r#"{"name":"P","custom":{"tag":"moon"}}"#,
    )
    .unwrap();
    fs::write(
        f.root.join("data/spare/s2.json"),
        r#"{"name":"S","custom":{"tag":"moon"}}"#,
    )
    .unwrap();

    for backend in [
        JsonQueryBackend::Auto,
        JsonQueryBackend::Bigram,
        JsonQueryBackend::Direct,
        JsonQueryBackend::Grep,
    ] {
        let mut repo = f.repo(CacheMode::Memory);
        let mut query = request("custom.tag = 'moon'");
        query.backend = backend;
        let all = repo.query_json(&query).unwrap();
        assert_eq!(all.total, Some(3), "{backend:?}");
        assert_eq!(
            all.records
                .iter()
                .map(|record| (record.collection.as_str(), record.id.as_str()))
                .collect::<Vec<_>>(),
            [("books", "a"), ("people", "p3"), ("spare", "s2")],
            "{backend:?}"
        );

        query.collections = Some(vec!["people".into()]);
        let scoped = repo.query_json(&query).unwrap();
        assert_eq!(scoped.total, Some(1), "{backend:?}");
        assert_eq!(scoped.records[0].collection, "people");
        assert_eq!(scoped.records[0].id, "p3");

        query.collections = Some(vec!["books".into()]);
        query.filter = "score = 'high'".into();
        let schema_incompatible = repo.query_json(&query).unwrap();
        assert_eq!(schema_incompatible.total, Some(1), "{backend:?}");
        assert_eq!(schema_incompatible.records[0].id, "a");

        query.collections = None;
        query.filter = "exists(custom.tag)".into();
        assert_eq!(repo.query_json(&query).unwrap().total, Some(4));
    }

    let mut repo = f.repo(CacheMode::Memory);
    let count = repo.query_json(&request("score = 42")).unwrap();
    assert_eq!(count.total, Some(1));
    assert_eq!(count.records[0].id, "b");
    // Books still uses its declared numeric index; the other collections scan.
    assert_eq!(count.metrics.grep_scans, 2);

    let mut scoped = request("score = 42");
    scoped.collections = Some(vec!["books".into()]);
    assert_eq!(repo.query_json(&scoped).unwrap().metrics.grep_scans, 0);
}

#[test]
fn undeclared_scalar_field_still_checks_later_collections_for_cardinality_errors() {
    let f = fixture(&[("a", r#"{"score":42}"#)]);
    fs::write(
        f.root.join("data/people/p3.json"),
        r#"{"name":"P","score":[1,2]}"#,
    )
    .unwrap();
    let mut repo = f.repo(CacheMode::Memory);
    let mut query = request("score = 42");
    query.count = false;
    query.limit = 1;
    assert!(
        repo.query_json(&query)
            .unwrap_err()
            .to_string()
            .contains("multiple values")
    );
}

#[test]
fn grep_prefilter_keeps_json_unicode_escapes_and_boolean_branches() {
    let f = fixture(&[
        ("escaped_key", r#"{"\u0078":"L\u0069ebe"}"#),
        ("escaped_punctuation", r#"{"x":"N\u006f. \u0031\u0031."}"#),
        ("escaped_value", r#"{"x":"LOVE"}"#),
        ("other", r#"{"other":"moon"}"#),
    ]);
    check(
        &f,
        &[
            ("x = 'Liebe'", &["escaped_key"]),
            ("x = 'No. 11.'", &["escaped_punctuation"]),
            ("x ~ 'No. 11.'", &["escaped_punctuation"]),
            ("x ~ :ignorecase('liebe')", &["escaped_key"]),
            ("x ~ :ignorecase('lo%e')", &["escaped_value"]),
            (
                "x ~ :ignorecase('liebe') || other = 'moon'",
                &["escaped_key", "other"],
            ),
            (
                "x ~ '%' || other = 'moon'",
                &[
                    "escaped_key",
                    "escaped_punctuation",
                    "escaped_value",
                    "other",
                ],
            ),
        ],
    );
}

#[test]
fn contains_is_case_sensitive_unless_ignorecase_is_requested() {
    let f = fixture(&[
        ("a", r#"{"x":"G\u006fethe","p":"goethe","tags":["other"]}"#),
        ("b", r#"{"x":"goethe","p":"Goethe","tags":["GOETHE"]}"#),
        ("c", r#"{"x":"GOETHE","p":"goethe","tags":["else"]}"#),
        ("d", r#"{"x":"Äpfel","p":"ä","tags":[]}"#),
        ("e", r#"{"x":"äpfel","p":"Ä","tags":[]}"#),
        ("f", r#"{"x":"Straße","p":"STRASSE","tags":[]}"#),
        ("g", r#"{"x":"STRASSE","p":"straße","tags":[]}"#),
    ]);
    check(
        &f,
        &[
            ("x ~ 'Goethe'", &["a"]),
            ("x ~ 'goethe'", &["b"]),
            ("x ~ :ignorecase('Goethe')", &["a", "b", "c"]),
            ("x ~ :ignorecase('GOE%')", &["a", "b", "c"]),
            ("x !~ 'Goethe'", &["b", "c", "d", "e", "f", "g"]),
            ("x !~ :ignorecase('Goethe')", &["d", "e", "f", "g"]),
            ("x ?~ :ignorecase('Goethe')", &["a", "b", "c"]),
            ("tags ?~ :ignorecase('Goethe')", &["b"]),
            ("x ~ :ignorecase(p)", &["a", "b", "c", "d", "e", "f", "g"]),
            ("x ~ 'Ä'", &["d"]),
            ("x ~ :ignorecase('Ä')", &["d", "e"]),
            ("x ~ 'STRASSE'", &["g"]),
            ("x ~ :ignorecase('STRASSE')", &["f", "g"]),
        ],
    );
    let plain = JsonFilter::compile("x ~ 'Goethe'", &BTreeMap::new()).unwrap();
    let folded = JsonFilter::compile(
        "x ~ :ignorecase($term)",
        &BTreeMap::from([("term".into(), json!("Goethe"))]),
    )
    .unwrap();
    assert_ne!(plain.hash(), folded.hash());
    assert!(!plain.matches(&json!({"x":"goethe"})).unwrap());
    assert!(folded.matches(&json!({"x":"goethe"})).unwrap());
}

#[test]
fn actual_json_types_are_distinct_even_when_the_schema_disagrees() {
    let f = fixture(&[
        ("false", r#"{"x":false}"#),
        ("missing", "{}"),
        ("null", r#"{"x":null}"#),
        ("number", r#"{"x":1800}"#),
        ("object", r#"{"x":{"value":1800}}"#),
        ("one", r#"{"x":1}"#),
        ("string", r#"{"x":"1800"}"#),
        ("true", r#"{"x":true}"#),
        ("zero", r#"{"x":0}"#),
    ]);
    check(
        &f,
        &[
            ("x = 1800", &["number"]),
            ("x == '1800'", &["string"]),
            (
                "x != 1800",
                &["false", "null", "one", "string", "true", "zero"],
            ),
            (
                "x != '1800'",
                &["false", "null", "number", "one", "true", "zero"],
            ),
            ("x = true", &["true"]),
            ("x = 1", &["one"]),
            ("x = false", &["false"]),
            ("x > 1", &["number"]),
            ("x >= true", &[]),
            ("x < null", &[]),
            ("x >= '1700'", &["string"]),
            ("x = null", &["null"]),
            (
                "x != null",
                &["false", "number", "one", "string", "true", "zero"],
            ),
            (
                "exists(x)",
                &[
                    "false", "null", "number", "object", "one", "string", "true", "zero",
                ],
            ),
            ("missing != 0", &[]),
            ("x ~ '800'", &["string"]),
            ("x !~ '800'", &[]),
        ],
    );
}

#[test]
fn exact_decimal_numbers_preserve_precision_across_backends() {
    let f = fixture(&[
        ("a", r#"{"x":9007199254740992}"#),
        ("b", r#"{"x":9007199254740993}"#),
        ("c", r#"{"x":0.123456789012345678901234567890}"#),
        ("d", r#"{"x":0.123456789012345678901234567891}"#),
        ("huge", r#"{"x":1e500}"#),
        ("tiny", r#"{"x":1e-500}"#),
        ("negative", r#"{"x":-1e500}"#),
        ("zero", r#"{"x":-0.0}"#),
        ("equivalent", r#"{"x":1.8e3}"#),
        ("integer", r#"{"x":1800.0}"#),
    ]);
    let schema_path = f.root.join(".editor/books/schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["properties"]["x"] = json!({"type":"number","index":true});
    fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    check(
        &f,
        &[
            ("x = 9007199254740993", &["b"]),
            ("x = 9007199254740992 || x = 9007199254740993", &["a", "b"]),
            ("x > 9007199254740992", &["b", "huge"]),
            ("x = 0.123456789012345678901234567890", &["c"]),
            ("x > 0.123456789012345678901234567890 && x < 1", &["d"]),
            ("x = 1e500", &["huge"]),
            ("x = 1e-500", &["tiny"]),
            ("x = 1e500 || x = 1e-500", &["huge", "tiny"]),
            ("x < 0", &["negative"]),
            ("x = 0", &["zero"]),
            ("x = 1800", &["equivalent", "integer"]),
            ("x = 18e2", &["equivalent", "integer"]),
            ("x > 1e999999999999999999999", &[]),
        ],
    );
}

#[test]
fn arrays_nested_paths_existence_and_any_not_equal_have_explicit_semantics() {
    let f = fixture(&[
        ("empty", r#"{"tags":[],"authors":[]}"#),
        ("missing", "{}"),
        (
            "mixed",
            r#"{"tags":["poet","editor"],"authors":[{"name":"A","years":[1800,1900]},{"name":"B"}]}"#,
        ),
        ("poet", r#"{"tags":["poet"],"authors":[{"name":null}]}"#),
        ("scalar", r#"{"tags":"poet","authors":{"name":"A"}}"#),
    ]);
    check(
        &f,
        &[
            ("tags ?= 'poet'", &["mixed", "poet", "scalar"]),
            ("tags ?!= 'poet'", &["mixed"]),
            ("tags ?!~ 'poet'", &["mixed"]),
            ("exists(tags)", &["empty", "mixed", "poet", "scalar"]),
            ("authors.name ?= 'A'", &["mixed", "scalar"]),
            ("authors[*].name ?= 'A'", &["mixed"]),
            ("authors.years ?>= 1850", &["mixed"]),
            ("authors[0].name = null", &["poet"]),
            ("exists(authors.name)", &["mixed", "poet", "scalar"]),
        ],
    );
}

#[test]
fn ambiguous_scalar_paths_error_even_outside_the_page_or_boolean_branch() {
    let f = fixture(&[
        ("a", r#"{"tags":["poet"]}"#),
        ("z", r#"{"tags":["poet","editor"]}"#),
    ]);
    for mode in [CacheMode::Memory, CacheMode::Disabled] {
        let mut r = f.repo(mode);
        for text in [
            "tags='poet'",
            "exists(missing) && tags='poet'",
            "exists(tags) || tags='poet'",
        ] {
            let mut q = request(text);
            q.backend = if mode == CacheMode::Memory {
                JsonQueryBackend::Bigram
            } else {
                JsonQueryBackend::Direct
            };
            q.limit = 1;
            q.count = false;
            assert!(
                r.query_json(&q)
                    .unwrap_err()
                    .to_string()
                    .contains("multiple values"),
                "{text}"
            );
        }
    }
    let mut bigram = f.repo(CacheMode::Disabled);
    for text in [
        "tags='poet'",
        "exists(missing) && tags='poet'",
        "exists(tags) || tags='poet'",
    ] {
        let mut q = request(text);
        q.backend = JsonQueryBackend::Bigram;
        assert!(
            bigram
                .query_json(&q)
                .unwrap_err()
                .to_string()
                .contains("multiple values"),
            "{text}"
        );
    }
}

#[test]
fn bigram_query_backend_reuses_index_and_follows_edits() {
    let f = fixture(&[
        ("a", r#"{"x":"Moonlight","n":1800}"#),
        ("b", r#"{"x":"\u004doon","n":1900}"#),
        ("c", r#"{"x":"Sunshine","n":1800}"#),
        ("broken", "not json"),
    ]);
    let mut options = f.options(CacheMode::Disabled);
    options.freshness = Freshness::MetadataOnRequest;
    let mut bigram = Repository::load_repository(&f.root, options).unwrap();
    let mut direct = f.repo(CacheMode::Disabled);
    for filter in [
        "x ~ 'Moon'",
        "x = 'Moon'",
        "x ~ 'Moon' && n >= 1800",
        "n = 1800",
    ] {
        let mut q = request(filter);
        q.collections = Some(vec!["books".into()]);
        q.backend = JsonQueryBackend::Direct;
        let expected = ids_for(&mut direct, &q);
        q.backend = JsonQueryBackend::Bigram;
        assert_eq!(ids_for(&mut bigram, &q), expected, "{filter}");
        assert_eq!(ids_for(&mut bigram, &q), expected, "warm {filter}");
    }
    fs::write(
        f.root.join("data/books/c.json"),
        r#"{"x":"Moonrise","n":1800}"#,
    )
    .unwrap();
    fs::remove_file(f.root.join("data/books/a.json")).unwrap();
    fs::write(
        f.root.join("data/books/d.json"),
        r#"{"x":"Moonbeam","n":1800}"#,
    )
    .unwrap();
    let mut q = request("x ~ 'Moon' && n >= 1800");
    q.collections = Some(vec!["books".into()]);
    q.backend = JsonQueryBackend::Direct;
    let expected = ids_for(&mut direct, &q);
    q.backend = JsonQueryBackend::Bigram;
    assert_eq!(ids_for(&mut bigram, &q), expected);
}

#[test]
fn bigram_refresh_detects_new_scalar_path_cardinality_errors() {
    let f = fixture(&[("a", r#"{"x":"Moonlight"}"#), ("z", r#"{"x":"Sunshine"}"#)]);
    let mut options = f.options(CacheMode::Disabled);
    options.freshness = Freshness::MetadataOnRequest;
    let mut repo = Repository::load_repository(&f.root, options).unwrap();
    let mut q = request("x = 'Moonlight'");
    q.backend = JsonQueryBackend::Bigram;
    assert_eq!(ids_for(&mut repo, &q), ["a"]);

    fs::write(
        f.root.join("data/books/z.json"),
        r#"{"x":["Sunshine","Starlight"]}"#,
    )
    .unwrap();
    assert!(
        repo.query_json(&q)
            .unwrap_err()
            .to_string()
            .contains("multiple values")
    );
}

#[test]
fn bigram_typed_candidates_cover_numbers_booleans_enums_and_schema_invalid_values() {
    let f = fixture(&[
        ("a", r#"{"x":1800,"flag":true,"kind":"alpha"}"#),
        ("b", r#"{"x":1800.0,"flag":false,"kind":"beta"}"#),
        ("c", r#"{"x":1799,"flag":true,"kind":"zeta"}"#),
        ("d", r#"{"x":1801,"flag":"true","kind":"beta"}"#),
        ("e", r#"{"x":"1800","flag":false,"kind":"alpha"}"#),
    ]);
    let schema_path = f.root.join(".editor/books/schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["properties"]["x"] = json!({"type":"number","index":true});
    schema["properties"]["flag"] = json!({"type":"boolean","index":true});
    schema["properties"]["kind"] = json!({"type":"string","enum":["alpha","beta"],"index":true});
    fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    check(
        &f,
        &[
            ("x = 1800", &["a", "b"]),
            ("x > 1799 && x < 1801", &["a", "b"]),
            ("x = '1800'", &["e"]),
            ("flag = true", &["a", "c"]),
            ("flag = false", &["b", "e"]),
            ("flag = 'true'", &["d"]),
            ("kind = 'beta'", &["b", "d"]),
            ("kind > 'alpha'", &["b", "c", "d"]),
            ("kind = 'zeta'", &["c"]),
        ],
    );

    let mut options = f.options(CacheMode::Disabled);
    options.freshness = Freshness::MetadataOnRequest;
    let mut bigram = Repository::load_repository(&f.root, options).unwrap();
    let mut q = request("x = 1800 && flag = true && kind = 'alpha'");
    q.backend = JsonQueryBackend::Bigram;
    assert_eq!(ids_for(&mut bigram, &q), ["a"]);
    fs::write(
        f.root.join("data/books/a.json"),
        r#"{"x":1900,"flag":false,"kind":"beta"}"#,
    )
    .unwrap();
    fs::write(
        f.root.join("data/books/new.json"),
        r#"{"x":1800,"flag":true,"kind":"alpha"}"#,
    )
    .unwrap();
    assert_eq!(ids_for(&mut bigram, &q), ["new"]);
    fs::remove_file(f.root.join("data/books/b.json")).unwrap();
    assert_eq!(
        ids_for(
            &mut bigram,
            &JsonQuery {
                filter: "kind = 'beta'".into(),
                ..q
            }
        ),
        ["a", "d"]
    );
}

#[test]
fn numeric_or_unions_typed_candidates_without_losing_mixed_branches() {
    let docs = (0..80)
        .map(|number| {
            let name = if number == 3 || number == 54 {
                "special"
            } else {
                "ordinary"
            };
            (
                format!("r{number:03}"),
                json!({"x":number,"flag":number % 2 == 0,"name":name}).to_string(),
            )
        })
        .collect::<Vec<_>>();
    let borrowed = docs
        .iter()
        .map(|(id, raw)| (id.as_str(), raw.as_str()))
        .collect::<Vec<_>>();
    let f = fixture(&borrowed);
    let schema_path = f.root.join(".editor/books/schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["properties"]["x"] = json!({"type":"number","index":true});
    schema["properties"]["flag"] = json!({"type":"boolean","index":true});
    schema["properties"]["name"] = json!({"type":"string"});
    fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    check(
        &f,
        &[
            ("x = 2 || x = 50", &["r002", "r050"]),
            (
                "(x >= 70 && x <= 72) || x = 2",
                &["r002", "r070", "r071", "r072"],
            ),
            ("x = 2 || name ~ 'special'", &["r002", "r003", "r054"]),
            (
                "(x = 2 || name ~ 'special') && flag = true",
                &["r002", "r054"],
            ),
        ],
    );

    let mut memory = f.repo(CacheMode::Memory);
    let mut query = request("x = 2 || x = 50");
    query.collections = Some(vec!["books".into()]);
    let warm = memory.query_json(&query).unwrap();
    assert_eq!(warm.total, Some(2));
    let result = memory.query_json(&query).unwrap();
    assert_eq!(result.metrics.grep_scans, 0);
    assert!(
        result
            .query_plan
            .iter()
            .any(|step| step.contains("exact scalar"))
    );
    query.filter = "(x = 2 || name ~ 'special') && flag = true".into();
    let mixed = memory.query_json(&query).unwrap();
    assert_eq!(mixed.total, Some(2));
    assert_eq!(mixed.metrics.grep_scans, 40);
    query.filter = "x = 2 || name ~ 'special'".into();
    let unbounded = memory.query_json(&query).unwrap();
    assert_eq!(unbounded.total, Some(3));
    assert_eq!(unbounded.metrics.grep_scans, 80);
    f.edit("data/books/r050.json", |record| record["x"] = json!(3));
    memory.refresh("books").unwrap();
    query.filter = "x = 2 || x = 3".into();
    query.backend = JsonQueryBackend::Auto;
    let result = memory.query_json(&query).unwrap();
    assert_eq!(
        result
            .records
            .iter()
            .map(|record| record.id.as_str())
            .collect::<Vec<_>>(),
        ["r002", "r003", "r050"]
    );
    assert_eq!(result.metrics.grep_scans, 3);
}

#[test]
fn typed_or_still_checks_scalar_cardinality_in_every_source_file() {
    let f = fixture(&[("a", r#"{"x":2}"#), ("b", r#"{"x":[1,2]}"#)]);
    let schema_path = f.root.join(".editor/books/schema.json");
    let mut schema: serde_json::Value =
        serde_json::from_slice(&fs::read(&schema_path).unwrap()).unwrap();
    schema["properties"]["x"] = json!({"type":"number","index":true});
    fs::write(&schema_path, serde_json::to_vec(&schema).unwrap()).unwrap();
    for (mode, backend) in [
        (CacheMode::Memory, JsonQueryBackend::Bigram),
        (CacheMode::Memory, JsonQueryBackend::Bigram),
    ] {
        let mut query = request("x = 2 || x = 3");
        query.backend = backend;
        let error = f.repo(mode).query_json(&query).unwrap_err();
        assert!(error.to_string().contains("multiple values"), "{error}");
    }
}

#[test]
fn grouping_parameters_punctuation_and_unicode_contains_are_engine_independent() {
    let f = fixture(&[
        (
            "a",
            r#"{"x":1,"name":"Straße ÄÖÜ Σςσ %_","a.b":{"quote\"slash/":"yes"},"empty":"","nil\u0000key":"match"}"#,
        ),
        ("b", r#"{"x":2,"name":"other","a":{"b":"nested"}}"#),
        ("c", r#"{"x":3,"name":"O'Brien"}"#),
    ]);
    check(
        &f,
        &[
            ("x = 1 || x = 2 && name='missing'", &["a"]),
            ("(x = 1 || x = 2) && name='missing'", &[]),
            (r#"["a.b"]["quote\"slash/"]='yes'"#, &["a"]),
            ("a.b='nested'", &["b"]),
            ("name ~ 'STRASSE äöü σσσ'", &[]),
            ("name ~ :ignorecase('STRAßE ÄÖÜ Σςσ')", &["a"]),
            ("name ~ '%_'", &["a", "b", "c"]),
            (r#"name ~ '\\%\\_'"#, &["a"]),
            ("empty ~ ''", &["a"]),
            (r#"["nil\u0000key"]='match'"#, &["a"]),
            (r#"name = 'O\'Brien'"#, &["c"]),
        ],
    );
    let mut q = request("x=$minimum || name=$name");
    q.parameters.insert("minimum".into(), json!(2));
    q.parameters.insert("name".into(), json!("O'Brien"));
    for mode in [CacheMode::Disabled, CacheMode::Memory] {
        let backend = if mode == CacheMode::Memory {
            JsonQueryBackend::Bigram
        } else {
            JsonQueryBackend::Direct
        };
        assert_eq!(
            ids_for(
                &mut f.repo(mode),
                &JsonQuery {
                    backend,
                    ..q.clone()
                }
            ),
            ["b", "c"]
        );
    }
    let f = JsonFilter::compile(
        "name=$p",
        &BTreeMap::from([("p".into(), json!("' OR 1=1 --"))]),
    )
    .unwrap();
    assert!(!f.matches(&json!({"name":"other"})).unwrap());
    assert!(f.matches(&json!({"name":"' OR 1=1 --"})).unwrap());
}

#[test]
fn last_duplicate_key_wins_and_malformed_json_never_matches() {
    let f = fixture(&[
        ("a", r#"{"x":1,"x":2,"nested":{"v":0,"v":3}}"#),
        ("b", r#"{"x":"bad","\u0078":2}"#),
        ("broken", "{bad"),
        ("nullroot", "null"),
    ]);
    check(
        &f,
        &[
            ("x=2", &["a", "b"]),
            ("x=1", &[]),
            ("nested.v=3", &["a"]),
            ("exists(x)", &["a", "b"]),
        ],
    );
}

#[test]
fn invalid_requests_fail_before_creating_cache_or_enumerating_sources() {
    let f = fixture(&[]);
    let mut r = f.repo(CacheMode::Memory);
    for text in [
        "",
        "x=",
        "x=unknown()",
        "x===1",
        "@id='a'",
        "x=1; DROP TABLE records",
        "x ~ 1",
        "x = :ignorecase('a')",
        "x ~ :ignorecase(1)",
        "x ~ :wrong('a')",
        "x ~ :ignorecase('a'",
        "x=[]",
        "x={}",
        "x=$missing",
        "x[nope]=1",
        "x[-1]=1",
        "x=1 &&",
        "(x=1",
        "exists()",
        "x='unterminated",
    ] {
        assert!(r.query_json(&request(text)).is_err(), "{text}");
    }
    let deep = format!("{}x=1{}", "(".repeat(33), ")".repeat(33));
    assert!(r.query_json(&request(&deep)).is_err());
    let long = vec!["x=1"; 257].join(" || ");
    assert!(r.query_json(&request(&long)).is_err());
    let mut q = request("x=$p");
    q.parameters.insert("p".into(), json!([1]));
    assert!(r.query_json(&q).is_err());
    let mut q = request("x=1");
    q.backend = JsonQueryBackend::Bigram;
    q.collections = Some(vec!["unknown".into()]);
    assert!(r.query_json(&q).is_err());
    assert_eq!(r.metrics().enumerations, 0);
    assert!(!f.data.exists());
}

#[test]
fn repository_scope_paging_and_dependencies_include_unopened_collections() {
    let f = fixture(&[("same", r#"{"x":1}"#)]);
    f.write("data/people/same.json", &json!({"x":1}));
    f.write("data/spare/same.json", &json!({"x":1}));
    let mut r = f.repo(CacheMode::Memory);
    let mut q = request("x=1");
    q.backend = JsonQueryBackend::Bigram;
    q.limit = 1;
    q.offset = 1;
    let first = r.query_json(&q).unwrap();
    assert_eq!(first.records[0].collection, "people");
    assert_eq!(first.records[0].id, "same");
    assert_eq!(first.total, Some(3));
    assert_eq!(first.dependencies.collections, ["books", "people", "spare"]);
    assert_eq!(first.dependencies.paths.len(), 1);
    assert_eq!(first.metrics.source_reads, 4);
    assert!(first.metrics.bigram_open_nanos > 0);
    let next = r.query_json(&q).unwrap();
    assert_eq!(next.metrics.source_reads, 4);
    assert_eq!(next.metrics.enumerations, 0);
    assert_eq!(first.filter_hash, next.filter_hash);
    q.offset = 100;
    let empty_page = r.query_json(&q).unwrap();
    assert_eq!(empty_page.total, Some(3));
    assert!(empty_page.records.is_empty());
    q.count = false;
    assert_eq!(r.query_json(&q).unwrap().total, None);
    f.edit(".editor/repository.json", |v| {
        v["collections"].as_object_mut().unwrap().remove("spare");
    });
    drop(r);
    let mut r = f.repo(CacheMode::Memory);
    let all = r
        .query_json(&JsonQuery {
            backend: JsonQueryBackend::Bigram,
            ..request("x=1")
        })
        .unwrap();
    assert_eq!(all.total, Some(2));
    assert!(all.records.iter().all(|r| r.collection != "spare"));
}

#[test]
fn explicit_collection_scope_is_lazy_and_source_edits_are_reconciled_on_reopen() {
    let f = fixture(&[("a", r#"{"x":1}"#)]);
    f.write("data/people/b.json", &json!({"x":1}));
    let mut q = request("x=1");
    q.backend = JsonQueryBackend::Bigram;
    q.collections = Some(vec!["books".into()]);
    let mut r = f.repo(CacheMode::Memory);
    assert_eq!(r.query_json(&q).unwrap().metrics.source_reads, 2);
    assert_eq!(
        r.cache_status()
            .unwrap()
            .collections
            .iter()
            .find(|c| c.collection == "people")
            .unwrap()
            .state,
        "absent"
    );
    drop(r);
    f.write("data/books/a.json", &json!({"x":"1"}));
    let mut r = f.repo(CacheMode::Memory);
    let changed = r.query_json(&q).unwrap();
    assert_eq!(changed.metrics.source_reads, 1);
    assert_eq!(changed.total, Some(0));
    q.collections = Some(vec![]);
    assert!(r.query_json(&q).unwrap().records.is_empty());
}

#[test]
fn compiled_filter_parameters_remain_literal_data() {
    let attack = "' OR 1=1; DROP TABLE records; --";
    let f = fixture(&[("a", r#"{"x":"safe"}"#)]);
    f.write("data/books/b.json", &json!({"x":attack}));
    let mut q = request("x=$value");
    q.backend = JsonQueryBackend::Bigram;
    q.parameters.insert("value".into(), json!(attack));
    let result = f.repo(CacheMode::Memory).query_json(&q).unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].id, "b");
    let compiled = JsonFilter::compile(&q.filter, &q.parameters).unwrap();
    assert!(compiled.matches(&json!({"x":attack})).unwrap());
    assert!(!compiled.matches(&json!({"x":"safe"})).unwrap());
    q.parameters.insert("value".into(), json!(1800));
    let numeric = JsonFilter::compile(&q.filter, &q.parameters).unwrap();
    assert!(numeric.matches(&json!({"x":1800})).unwrap());
    assert!(!numeric.matches(&json!({"x":"1800"})).unwrap());
    assert_ne!(compiled.hash(), numeric.hash());
}

#[test]
fn generated_json_values_and_comparisons_match_direct_execution() {
    let f = fixture(&[]);
    let mut seed = 73_u64;
    for i in 0..80 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let x = match seed % 6 {
            0 => json!(null),
            1 => json!(seed.is_multiple_of(2)),
            2 => json!(format!("{}", seed % 10)),
            3 => json!(seed % 10),
            4 => json!((seed % 100) as f64 / 10.0),
            _ => json!("Straße %_"),
        };
        f.write(
            &format!("data/books/{i:03}.json"),
            &json!({"x":x,"nested":[{"v":x}, {}, {"v":null}]}),
        );
    }
    let mut direct = f.repo(CacheMode::Disabled);
    let mut cached = f.repo(CacheMode::Memory);
    for op in ["=", "!=", "<", "<=", ">", ">="] {
        for literal in ["null", "true", "false", "0", "1", "1.0", "2.5", "'1'"] {
            for field in ["x", "nested.v ?"] {
                let q = request(&format!("{field}{op}{literal}"));
                assert_eq!(
                    ids_for(
                        &mut direct,
                        &JsonQuery {
                            backend: JsonQueryBackend::Direct,
                            ..q.clone()
                        }
                    ),
                    ids_for(
                        &mut cached,
                        &JsonQuery {
                            backend: JsonQueryBackend::Bigram,
                            ..q.clone()
                        }
                    ),
                    "{}",
                    q.filter
                );
            }
        }
    }
}

#[test]
fn fields_modifiers_and_typed_parameters_work_on_both_sides() {
    let f = fixture(&[
        (
            "a",
            r#"{"start":1,"end":3,"title":"Alpha","other":"alpha","tags":["pre_one","pre_two"],"n":2}"#,
        ),
        (
            "b",
            r#"{"start":"1","end":3,"title":"ÄBC","other":"äbc","tags":["pre_one","other"],"n":2}"#,
        ),
        ("c", r#"{"start":4,"end":3,"tags":[],"n":0}"#),
        ("d", r#"{"start":true,"end":1,"tags":"pre_one","n":7}"#),
        ("e", r#"{"start":null,"end":null}"#),
        ("f", "{}"),
    ]);
    check(
        &f,
        &[
            ("start < end", &["a"]),
            ("end > start", &["a"]),
            ("start = end", &["e"]),
            ("start != end", &["a", "b", "c", "d"]),
            ("title:lower = other", &["a"]),
            ("title:lower = 'Äbc'", &["b"]),
            ("tags:length = n", &["a", "b", "c"]),
            ("0 = tags:length", &["c"]),
            ("tags:length > 0", &["a", "b"]),
            (r#"tags:each ~ 'pre\\_%'"#, &["a", "d"]),
            (r#"tags:each ?~ 'pre\\_%'"#, &["a", "b", "d"]),
            ("tags:each != 'other'", &["a", "d"]),
            ("tags:each = tags:each", &["d"]),
            ("1 < 2", &["a", "b", "c", "d", "e", "f"]),
            ("true = 1", &[]),
            ("'1' != 1", &["a", "b", "c", "d", "e", "f"]),
        ],
    );
    let filter = JsonFilter::compile(
        "start < end && tags:length = $n",
        &BTreeMap::from([("n".into(), json!(2))]),
    )
    .unwrap();
    assert_eq!(filter.paths().len(), 3);
    assert!(
        filter
            .matches(&json!({"start":1,"end":2,"tags":[1,2]}))
            .unwrap()
    );
    for bad in ["tags:unknown=1", "tags:lower:each='x'", "title ~ 1"] {
        assert!(JsonFilter::compile(bad, &BTreeMap::new()).is_err(), "{bad}");
    }
}

#[test]
fn wildcard_patterns_and_field_patterns_have_pocketbase_wrapping() {
    let f = fixture(&[
        ("a", r#"{"x":"Abc_def","p":"Abc%"}"#),
        ("b", r#"{"x":"xxAbcZdefyy","p":"Abc%"}"#),
        ("c", r#"{"x":"100% done","p":"100%"}"#),
        ("d", r#"{"x":"a\\b","p":"a"}"#),
        ("empty", r#"{"x":""}"#),
        ("null", r#"{"x":null}"#),
    ]);
    check(
        &f,
        &[
            ("x ~ 'Abc%'", &["a"]),
            ("x ~ '%Abc%'", &["a", "b"]),
            ("x ~ '%Abc_def%'", &["a", "b"]),
            ("x ~ 'Abc_def'", &["a"]),
            (r#"x ~ '%Abc\\_def%'"#, &["a"]),
            (r#"x ~ '100\\%'"#, &["c"]),
            (r#"x ~ 'a\\b'"#, &["d"]),
            ("x ~ p", &["a", "b", "c", "d"]),
            ("x !~ 'Abc%'", &["b", "c", "d", "empty"]),
            ("x ~ '%'", &["a", "b", "c", "d", "empty"]),
        ],
    );
}
