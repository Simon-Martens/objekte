use super::prelude::*;
use super::{fixture_schema, with_record};
use crate::schema::validation::{ValidationNode, converted_kind};
#[test]
fn prepared_contains_preserves_plain_wildcard_and_escaped_percent_semantics() {
    for (pattern, cases) in [
        ("abc", vec![("zabcx", true), ("abxc", false)]),
        ("", vec![("", true), ("anything", true)]),
        ("a%b", vec![("ab", true), ("axxxb", true), ("ba", false)]),
        ("aba%aba", vec![("abaaba", true), ("ababa", false)]),
        ("a\\%b", vec![("a%b", true), ("axxb", false)]),
        ("%%", vec![("", true), ("anything", true)]),
        ("a\\\\%b", vec![("a\\%b", true), ("a%b", false)]),
    ] {
        let prepared = PreparedContains::new(pattern);
        for (text, expected) in cases {
            assert_eq!(
                prepared.matches(text, pattern),
                expected,
                "{pattern:?} {text:?}"
            );
            assert_eq!(
                contains_pattern(text, pattern),
                expected,
                "dynamic {pattern:?} {text:?}"
            );
        }
    }
    assert!(matches!(
        PreparedContains::new("abc"),
        PreparedContains::Literal(_)
    ));
    assert!(matches!(
        PreparedContains::new("a\\%b"),
        PreparedContains::Unescaped(_)
    ));
    assert!(matches!(
        PreparedContains::new("a%b"),
        PreparedContains::Wildcards(_)
    ));
}

#[test]
fn bound_contains_reuses_prepared_pattern_across_scalar_and_array_values() {
    for (filter, document, expected) in [
        ("name ~ 'a%b'", serde_json::json!({"name":"axxb"}), true),
        ("name !~ 'a%b'", serde_json::json!({"name":"axxb"}), false),
        ("name ~ 'a\\%b'", serde_json::json!({"name":"a%b"}), true),
        (
            "name:lower ~ 'str%se'",
            serde_json::json!({"name":"Straße"}),
            true,
        ),
        (
            "values ?~ 'a%b'",
            serde_json::json!({"values":["no", "axxb"]}),
            true,
        ),
        ("values ?~ 'a%b'", serde_json::json!({"values":[]}), false),
    ] {
        PREPARED_CONTAINS_BUILDS.with(|count| count.set(0));
        let bound = bind_filter(&parse_filter(filter).unwrap(), &fixture_schema()).unwrap();
        let BoundExpr::Term(term) = &bound else {
            panic!("one term")
        };
        assert!(term.prepared_contains.is_some());
        assert_eq!(PREPARED_CONTAINS_BUILDS.with(|count| count.get()), 1);
        let value = Value::from_json(document).unwrap();
        with_record(value, |ctx| {
            for _ in 0..32 {
                assert_eq!(
                    eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                    expected
                );
            }
        });
        assert_eq!(PREPARED_CONTAINS_BUILDS.with(|count| count.get()), 1);
    }
}

#[test]
fn incremental_indexed_value_paths_match_complete_rebuild_in_any_registration_order() {
    fn field(path: &[&str], kind: LogicalType, required: bool) -> FieldDefinition {
        FieldDefinition {
            path: path.iter().map(|part| (*part).into()).collect(),
            kind,
            required,
            nullable: !required,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        }
    }

    fn complete_tree(schema: &Schema) -> ValidationNode {
        let mut root = ValidationNode::default();
        for field in schema.fields.values().filter(|field| {
            converted_kind(&field.kind) || matches!(field.kind, LogicalType::Array(_))
        }) {
            if field.path.is_empty() {
                root.field = Some(field.clone());
            }
            let mut node = &mut root;
            for depth in 0..field.path.len() {
                node = node.children.entry(field.path[depth].clone()).or_default();
                if let Some(ancestor) = schema.fields.get(&field.path[..=depth]) {
                    node.field = Some(ancestor.clone());
                }
            }
        }
        root
    }

    fn assert_same_nodes(actual: &ValidationNode, expected: &ValidationNode) {
        match (&actual.field, &expected.field) {
            (Some(actual), Some(expected)) => {
                assert_eq!(actual.path, expected.path);
                assert_eq!(actual.kind, expected.kind);
                assert_eq!(actual.required, expected.required);
                assert_eq!(actual.nullable, expected.nullable);
                assert_eq!(actual.multi, expected.multi);
                assert_eq!(actual.element_nullable, expected.element_nullable);
            }
            (None, None) => {}
            _ => panic!("indexed value field differs"),
        }
        assert_eq!(actual.children.len(), expected.children.len());
        for (name, expected_child) in &expected.children {
            assert_same_nodes(&actual.children[name], expected_child);
        }
    }

    let fields = [
        field(&[], LogicalType::Object, false),
        field(&["details"], LogicalType::Object, false),
        field(&["details", "published"], LogicalType::Date, true),
        field(
            &["details", "history"],
            LogicalType::Array(Box::new(LogicalType::Object)),
            false,
        ),
        field(&["details", "history", "at"], LogicalType::Datetime, true),
        field(&["details", "history", "label"], LogicalType::String, false),
        field(&["other", "count"], LogicalType::Number, false),
        field(
            &["other", "days"],
            LogicalType::Array(Box::new(LogicalType::Date)),
            false,
        ),
        field(
            &["other", "tags"],
            LogicalType::Array(Box::new(LogicalType::String)),
            false,
        ),
    ];
    for order in [
        (0..fields.len()).collect::<Vec<_>>(),
        (0..fields.len()).rev().collect(),
        vec![2, 4, 7, 5, 3, 1, 6, 8, 0],
    ] {
        let mut schema = Schema::default();
        for index in order {
            schema.register_field(fields[index].clone()).unwrap();
            assert_same_nodes(&schema.indexed_value_paths, &complete_tree(&schema));
        }
    }
}

#[test]
fn small_selection_preserves_scalar_and_array_path_states() {
    let value = Value::from_json(serde_json::json!({
        "name": "Ada", "empty": [], "null": null,
        "people": [{"name": "A"}, {"name": "B"}]
    }))
    .unwrap();
    let path = |parts: &[&str]| {
        parts
            .iter()
            .map(|part| (*part).to_string())
            .collect::<Vec<_>>()
    };
    let scalar = data_values_small(&value, &path(&["name"])).unwrap();
    assert!(!scalar.spilled());
    assert!(matches!(scalar.as_slice(), [Value::String(name)] if name == "Ada"));
    assert!(data_values_small(&value, &path(&["absent"])).is_none());
    assert!(matches!(
        data_values_small(&value, &path(&["null"]))
            .unwrap()
            .as_slice(),
        [Value::Null]
    ));
    assert_eq!(
        data_values_small(&value, &path(&["empty", "name"]))
            .unwrap()
            .len(),
        0
    );
    let many = data_values_small(&value, &path(&["people", "name"])).unwrap();
    assert_eq!(many.len(), 2);
    assert_eq!(
        many.as_slice(),
        data_values(&value, &path(&["people", "name"])).unwrap()
    );
}
use crate::query::parse_filter;

#[test]
fn scalar_metadata_comparisons_preserve_null_and_timestamp_rules() {
    with_record(Value::Null, |ctx| {
        for (expression, expected) in [
            ("@file.modified >= '1970-01-01T00:00:00Z'", true),
            ("@file.modified > '1970-01-01T00:00:00Z'", false),
            ("@file.modified < '1971-01-01T00:00:00Z'", true),
            ("@file.created:null = true", true),
            ("@file.created = '1970-01-01T00:00:00Z'", false),
        ] {
            let bound = bind_filter(&parse_filter(expression).unwrap(), &fixture_schema()).unwrap();
            let BoundExpr::Term(term) = bound else {
                panic!("expected one metadata predicate");
            };
            assert_eq!(eval_term(&term, &ctx).unwrap(), expected, "{expression}");
        }
    });
}

#[test]
fn truth_table_arrays_null_and_case_folding() {
    let value = Value::from_json(serde_json::json!({"name":"Straße", "values":["a","b"], "people":[{"name":"Ada","aliases":["A","Ada"]},{"name":"Bob","aliases":["B"]}]})).unwrap();
    with_record(value, |ctx| {
        for (input, expected) in [
            ("values ?!= 'a'", true),
            ("!(values ?= 'a')", false),
            ("values:each = 'a'", false),
            ("values:length = 2", true),
            ("name:lower ~ 'STRASSE'", true),
            ("name:folded ~ 'strasse'", true),
            ("people.name ?= 'Ada'", true),
            ("people.aliases ?= 'Ada'", true),
            ("people.aliases ?= 'missing'", false),
            ("name:null = false", true),
            ("@file.created:null = true", true),
        ] {
            let bound = bind_filter(&parse_filter(input).unwrap(), &fixture_schema()).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{input}"
            );
        }
    });
}

#[test]
fn missing_null_empty_truth_table() {
    for (input, expected) in [
        ("values:exists = true", true),
        ("values:null = false", true),
        ("values:each = 'x'", true),
        ("values ?= 'x'", false),
        ("values:length = 0", true),
        ("name:exists = false", true),
        ("name:null = false", true),
        ("name != 'x'", false),
    ] {
        let value = Value::from_json(serde_json::json!({"values": []})).unwrap();
        with_record(value, |ctx| {
            let bound = bind_filter(&parse_filter(input).unwrap(), &fixture_schema()).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{input}"
            );
        });
    }
    let value = Value::from_json(serde_json::json!({"name":null,"values":[null,"x"]})).unwrap();
    with_record(value, |ctx| {
        for (input, expected) in [
            ("name:exists = true", true),
            ("name:null = true", true),
            ("name = null", true),
            ("name != 'x'", false),
            ("values ?!= 'x'", false),
            ("values ?= 'x'", true),
        ] {
            let bound = bind_filter(&parse_filter(input).unwrap(), &fixture_schema()).unwrap();
            assert_eq!(
                eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                expected,
                "{input}"
            );
        }
    });
}

#[test]
fn numeric_string_is_rejected_before_indexed_or_scanned_inequality() {
    use crate::execution_options::ReturnMode;
    use crate::query::{Query, QueryOptions};
    use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
    use crate::source::SourceFormat;

    let root = std::env::temp_dir().join(format!("objekte-type-index-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let definition = |indexed| {
        let mut schema = Schema::default();
        for (name, kind, indexed) in [
            ("count", LogicalType::Number, indexed),
            ("other", LogicalType::String, false),
        ] {
            schema
                .register_field(FieldDefinition {
                    path: vec![name.into()],
                    kind,
                    required: true,
                    nullable: false,
                    indexed,
                    multi: false,
                    element_nullable: vec![],
                })
                .unwrap();
        }
        CollectionDefinition {
            name: "data".into(),
            source_path: "data".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        }
    };
    let options = RepositoryOptions {
        ..Default::default()
    };
    std::fs::write(root.join("data/bad.json"), r#"{"count":"7","other":"ok"}"#).unwrap();
    for indexed in [false, true] {
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition(indexed)],
            options,
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        assert_eq!(
            repo.files.files[0].status,
            crate::inventory::FileStatus::Broken
        );
        assert_eq!(
            repo.collections[0].generation.records[0].class,
            crate::inventory::RecordClass::RawFallback
        );
        assert!(
            repo.files.files[0].diagnostics[0]
                .message
                .contains("count: expected Number")
        );
    }
    std::fs::remove_file(root.join("data/bad.json")).unwrap();
    std::fs::write(root.join("data/seven.json"), r#"{"count":7,"other":"ok"}"#).unwrap();
    std::fs::write(root.join("data/eight.json"), r#"{"count":8,"other":"ok"}"#).unwrap();
    let query = Query {
        collections: vec![],
        fts: None,
        filter: Some(parse_filter("count != 7").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    for indexed in [false, true] {
        let repo = crate::lifecycle::Index::build_repository_for_test(
            &root,
            vec![definition(indexed)],
            options,
            crate::source::registry::SourceRegistry::builtin(),
        )
        .map_err(|error| error.to_string())
        .unwrap();
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(optimized.count.unwrap(), 1);
        assert_eq!(optimized.hits[0].record_id, "eight");
        assert_eq!(optimized.hits, reference.hits);
        let mut logical_query = query.clone();
        logical_query.options.return_mode = ReturnMode::Logical;
        let logical = repo.search(&logical_query).unwrap();
        assert!(
            matches!(&logical.hits[0].payload, Some(crate::query_results::Payload::Logical(Value::Object(fields))) if fields.get("count") == Some(&Value::Number(Number::integer(8))))
        );
    }
    let persisted = RepositoryOptions {
        ..Default::default()
    };
    let built = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition(true)],
        persisted,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    drop(built);
    let restored = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition(true)],
        persisted,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert_eq!(restored.search(&query).unwrap().hits[0].record_id, "eight");
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unhinted_paths_use_actual_types_and_collection_specific_bindings() {
    use crate::execution_options::ReturnMode;
    use crate::query::{FtsField, FtsQuery, Query, QueryOptions};
    use crate::repository::{CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions};
    use crate::source::SourceFormat;

    let root = std::env::temp_dir().join(format!("objekte-dynamic-fields-{}", std::process::id()));
    for (collection, id, json) in [
        (
            "alpha",
            "one",
            r#"{"count":7,"title":"Hello","when":"2026-01-01","tags":[1,2]}"#,
        ),
        ("alpha", "two", r#"{"count":8,"title":"Other","tags":[]}"#),
        (
            "beta",
            "three",
            r#"{"count":7,"title":"HELLO","when":"2026-01-01","tags":[1,2]}"#,
        ),
        ("beta", "four", r#"{"count":"7","title":"Else","tags":[]}"#),
    ] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
        std::fs::write(root.join(collection).join(format!("{id}.json")), json).unwrap();
    }
    let mut declared = Schema::default();
    declared
        .register_field(FieldDefinition {
            path: vec!["count".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let definitions = vec![
        CollectionDefinition {
            name: "alpha".into(),
            source_path: "alpha".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema: declared,
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        },
        CollectionDefinition {
            name: "beta".into(),
            source_path: "beta".into(),
            format: SourceFormat::Json,
            identity: IdentityPolicy::Filename,
            discovery: Default::default(),
            schema: Schema::default(),
            sort_fields: vec![],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        },
    ];
    let options = RepositoryOptions {
        ..Default::default()
    };
    let check = |repo: &Repository| {
        for (filter, expected) in [
            ("count = 7", vec![("alpha", "one"), ("beta", "three")]),
            (
                "title:lower ~ 'hello'",
                vec![("alpha", "one"), ("beta", "three")],
            ),
            ("tags ?= 2", vec![("alpha", "one"), ("beta", "three")]),
            ("tags:length = 0", vec![("alpha", "two"), ("beta", "four")]),
            (
                "when = '2026-01-01'",
                vec![("alpha", "one"), ("beta", "three")],
            ),
            (
                "missing:exists = false",
                vec![
                    ("alpha", "one"),
                    ("alpha", "two"),
                    ("beta", "four"),
                    ("beta", "three"),
                ],
            ),
            ("missing = 'x'", vec![]),
        ] {
            let query = Query {
                collections: vec![],
                fts: None,
                filter: Some(parse_filter(filter).unwrap()),
                options: QueryOptions {
                    sort: vec![],
                    limit: 20,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: true,
                },
            };
            let result = repo.search(&query).unwrap();
            let reference = repo.search_reference(&query).unwrap();
            let actual = result
                .hits
                .iter()
                .map(|hit| (hit.collection.as_str(), hit.record_id.as_str()))
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{filter}");
            assert_eq!(result.count.unwrap(), reference.count.unwrap());
            assert_eq!(result.hits, reference.hits);
        }
        let query = Query {
            collections: vec![],
            fts: Some(FtsQuery {
                pattern_parameter: None,
                mode: crate::query::FtsMode::Literal,
                case_sensitivity: crate::query::CaseSensitivity::Sensitive,
                literal: "hello".into(),
                fields: vec![FtsField {
                    query_path: None,
                    path: FieldPath::parse("title").unwrap(),
                    binding: Some("lower".into()),
                }],
            }),
            filter: None,
            options: QueryOptions {
                sort: vec![],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let result = repo.search(&query).unwrap();
        assert_eq!(result.count.unwrap(), 2);
        assert_eq!(result.hits, repo.search_reference(&query).unwrap().hits);
        let mut selected = query.clone();
        selected.fts.as_mut().unwrap().literal = "ell".into();
        selected.fts.as_mut().unwrap().fields[0].binding = None;
        let plain = repo.search(&selected).unwrap();
        assert_eq!(plain.count.unwrap(), 1);
        assert_eq!(plain.hits[0].record_id, "one");
        assert_eq!(plain.hits, repo.search_reference(&selected).unwrap().hits);
        let mut invalid = query.clone();
        invalid.fts.as_mut().unwrap().fields[0].binding = Some("unknown".into());
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unknown FTS binding")
        );
        let invalid = Query {
            collections: vec!["alpha".into()],
            fts: None,
            filter: Some(parse_filter("count = '7'").unwrap()),
            options: query.options.clone(),
        };
        assert!(repo.search(&invalid).unwrap_err().contains("incompatible"));
        let invalid = Query {
            collections: vec!["beta".into()],
            fts: None,
            filter: Some(parse_filter("title:unknown = 'x'").unwrap()),
            options: query.options.clone(),
        };
        assert!(
            repo.search(&invalid)
                .unwrap_err()
                .contains("unknown binding")
        );
    };
    let built = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions.clone(),
        options,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    check(&built);
    drop(built);
    let restored = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        options,
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    check(&restored);
    std::fs::remove_dir_all(root).unwrap();
}
