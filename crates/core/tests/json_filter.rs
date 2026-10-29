mod support;
use objekte_core::{CacheMode, JsonFilter, JsonQuery, Repository};
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
            "x", "name", "start", "end", "tags", "authors", "nested", "a",
            "a.b", "empty", "nil\0key", "other", "n", "p", "missing",
        ] {
            schema["properties"].as_object_mut().unwrap().entry(field).or_insert_with(|| json!({}));
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
    let mut sql = f.repo(CacheMode::Sqlite);
    for (filter, expected) in cases {
        let request = request(filter);
        assert_eq!(
            ids_for(&mut direct, &request),
            *expected,
            "direct: {filter}"
        );
        assert_eq!(ids_for(&mut sql, &request), *expected, "SQL: {filter}");
    }
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
fn exact_decimal_numbers_never_pass_through_sqlite_floats() {
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
    check(
        &f,
        &[
            ("x = 9007199254740993", &["b"]),
            ("x > 9007199254740992", &["b", "huge"]),
            ("x = 0.123456789012345678901234567890", &["c"]),
            ("x > 0.123456789012345678901234567890 && x < 1", &["d"]),
            ("x = 1e500", &["huge"]),
            ("x = 1e-500", &["tiny"]),
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
    for mode in [CacheMode::Sqlite, CacheMode::Disabled] {
        let mut r = f.repo(mode);
        for text in [
            "tags='poet'",
            "exists(missing) && tags='poet'",
            "exists(tags) || tags='poet'",
        ] {
            let mut q = request(text);
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
            ("name ~ 'STRAßE ÄÖÜ Σςσ'", &["a"]),
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
    for mode in [CacheMode::Disabled, CacheMode::Sqlite] {
        assert_eq!(ids_for(&mut f.repo(mode), &q), ["b", "c"]);
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
    let mut r = f.repo(CacheMode::Sqlite);
    for text in [
        "",
        "x=",
        "x=unknown()",
        "x===1",
        "@id='a'",
        "x=1; DROP TABLE records",
        "x ~ 1",
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
    let mut r = f.repo(CacheMode::Sqlite);
    let mut q = request("x=1");
    q.limit = 1;
    q.offset = 1;
    let first = r.query_json(&q).unwrap();
    assert_eq!(first.records[0].collection, "people");
    assert_eq!(first.records[0].id, "same");
    assert_eq!(first.total, Some(3));
    assert_eq!(first.dependencies.collections, ["books", "people", "spare"]);
    assert_eq!(first.dependencies.revisions.len(), 3);
    assert_eq!(first.dependencies.paths.len(), 1);
    assert_eq!(first.metrics.source_reads, 3);
    assert_eq!(first.metrics.index_builds, 0);
    let next = r.query_json(&q).unwrap();
    assert_eq!(next.metrics.source_reads, 0);
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
    let mut r = f.repo(CacheMode::Sqlite);
    let all = r.query_json(&request("x=1")).unwrap();
    assert_eq!(all.total, Some(2));
    assert!(all.records.iter().all(|r| r.collection != "spare"));
    let db = rusqlite::Connection::open(r.cache_directory().unwrap().join("cache.sqlite")).unwrap();
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM records WHERE collection='spare'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
}

#[test]
fn explicit_collection_scope_is_lazy_and_source_edits_are_reconciled_on_reopen() {
    let f = fixture(&[("a", r#"{"x":1}"#)]);
    f.write("data/people/b.json", &json!({"x":1}));
    let mut q = request("x=1");
    q.collections = Some(vec!["books".into()]);
    let mut r = f.repo(CacheMode::Sqlite);
    assert_eq!(r.query_json(&q).unwrap().metrics.source_reads, 1);
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
    let mut r = f.repo(CacheMode::Sqlite);
    let changed = r.query_json(&q).unwrap();
    assert_eq!(changed.metrics.source_reads, 1);
    assert_eq!(changed.total, Some(0));
    q.collections = Some(vec![]);
    assert!(r.query_json(&q).unwrap().records.is_empty());
}

#[test]
fn compiled_filter_parameters_and_sql_injection_remain_data() {
    let attack = "' OR 1=1; DROP TABLE records; --";
    let f = fixture(&[("a", r#"{"x":"safe"}"#)]);
    f.write("data/books/b.json", &json!({"x":attack}));
    let mut q = request("x=$value");
    q.parameters.insert("value".into(), json!(attack));
    let result = f.repo(CacheMode::Sqlite).query_json(&q).unwrap();
    assert_eq!(result.records.len(), 1);
    assert_eq!(result.records[0].id, "b");
    assert!(!result.sql.as_ref().unwrap().contains(attack));
    assert!(result.bindings.iter().any(|s| s == attack));
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
fn generated_json_values_and_comparisons_match_sql_execution() {
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
    let mut sql = f.repo(CacheMode::Sqlite);
    for op in ["=", "!=", "<", "<=", ">", ">="] {
        for literal in ["null", "true", "false", "0", "1", "1.0", "2.5", "'1'"] {
            for field in ["x", "nested.v ?"] {
                let q = request(&format!("{field}{op}{literal}"));
                assert_eq!(
                    ids_for(&mut direct, &q),
                    ids_for(&mut sql, &q),
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
