use objekte_loader_json::{JsonDecodedScalarsReader, JsonReader};
use objekte_loader_jsonl::JsonLinesReader;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{Number, Value};
use objekte_search_core::source::{ScanProjection, SourceReader};
use std::collections::HashSet;

#[test]
fn root_queries_match_reference_over_mixed_json_documents() {
    use objekte_search_core::query::options::ReturnMode;
    use objekte_search_core::query::{Query, QueryOptions, parse_filter};
    use objekte_search_core::repository::configuration::RepositoryOptions;

    let root = std::env::temp_dir().join(format!("objekte-json-roots-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, json) in [
        ("array", r#"["first",123,true,null,{"title":"Goethe"}]"#),
        ("string", r#""first""#),
        ("number", "123"),
        ("null", "null"),
        ("object", r#"{"title":"Goethe","values":["first",123]}"#),
        ("quoted", r#"{"@root":"real","a.b":"dot","":"empty"}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let mut sources = objekte_search_core::source::registry::SourceRegistry::default();
    objekte_loader_json::register(&mut sources).unwrap();
    assert!(sources.resolve(&objekte_loader_jsonl::JSON_LINES).is_err());
    let repo = Index::start_build(
        &root,
        vec![objekte_loader_json::folder("data", "data")],
        RepositoryOptions {
            ..Default::default()
        },
        sources,
    )
    .and_then(|task| task.wait_complete())
    .unwrap()
    .view()
    .unwrap();
    assert_eq!(
        repo.count_exact(&super::query(&["data"], ReturnMode::Locator))
            .unwrap(),
        6
    );
    for (filter, expected) in [
        ("@root = 'first'", vec!["path:string.json"]),
        ("@root = 123", vec!["path:number.json"]),
        ("@root :null", vec!["path:null.json"]),
        ("@root[0] = 'first'", vec!["path:array.json"]),
        ("@root[0] = @root[0]", vec!["path:array.json"]),
        (
            "@root ?= 'first'",
            vec!["path:array.json", "path:string.json"],
        ),
        ("@root[*].title ~ 'Goethe'", vec!["path:array.json"]),
        (
            "@root.title ~ 'Goethe'",
            vec!["path:array.json", "path:object.json"],
        ),
        ("@root[\"a.b\"] = 'dot'", vec!["path:quoted.json"]),
        ("@root[\"\"] = 'empty'", vec!["path:quoted.json"]),
        ("[\"@root\"] = 'real'", vec!["path:quoted.json"]),
    ] {
        let query = Query {
            collections: vec!["data".into()],
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
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        let ids = |result: &objekte_search_core::query_results::SearchResult| {
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&optimized), expected, "{filter}");
        assert_eq!(ids(&optimized), ids(&reference), "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn declared_schema_keeps_scalar_root_as_diagnosed_fallback() {
    use objekte_search_core::inventory::RecordClass;
    use objekte_search_core::model::LogicalType;
    use objekte_search_core::repository::configuration::RepositoryOptions;
    use objekte_search_core::schema::FieldDefinition;

    let root = std::env::temp_dir().join(format!("objekte-declared-root-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/root.json"), br#"["first", 2]"#).unwrap();
    let mut definition = objekte_loader_json::folder("data", "data");
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["title".into()],
            kind: LogicalType::String,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let repo = Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
        objekte_cli::loader::installed_sources(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap()
    .view()
    .unwrap();
    let result = repo
        .search(&super::query(
            &["data"],
            objekte_search_core::query::options::ReturnMode::Locator,
        ))
        .unwrap();
    assert_eq!(result.hits.len(), 1);
    let hit = &result.hits[0];
    assert_eq!(hit.record_class, RecordClass::RawFallback);
    let issue = repo
        .diagnostic(hit.file.as_ref().unwrap().file_id, hit.diagnostic_ref)
        .unwrap();
    assert!(issue.message.contains("root must be an object"));
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn exceptional_numbers_do_not_demote_array_roots_or_dynamic_predicates() {
    use objekte_search_core::inventory::RecordClass;
    use objekte_search_core::query::options::ReturnMode;
    use objekte_search_core::query::{Query, QueryOptions, parse_filter};
    use objekte_search_core::repository::configuration::{IdentityPolicy, RepositoryOptions};

    let root =
        std::env::temp_dir().join(format!("objekte-exact-json-numbers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(
        root.join("data/array.json"),
        b" [ \"first\", 1e1000000, true ] \n",
    )
    .unwrap();
    std::fs::write(root.join("data/object.json"), br#"{"n":-1e-1000000}"#).unwrap();
    let mut definition = objekte_loader_json::decoded_scalar_folder("data", "data");
    definition.identity = IdentityPolicy::RelativePath;
    let repo = Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
        objekte_cli::loader::installed_sources(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap()
    .view()
    .unwrap();
    let result = repo
        .search(&super::query(&["data"], ReturnMode::Locator))
        .unwrap();
    assert_eq!(result.hits.len(), 2);
    assert!(
        result
            .hits
            .iter()
            .all(|hit| hit.record_class == RecordClass::Logical)
    );
    for (filter, expected) in [
        ("@root[0] = 'first'", vec!["path:array.json"]),
        ("@root[1] = 1e1000000", vec!["path:array.json"]),
        ("n < 0", vec!["path:object.json"]),
        ("n = -1e-1000000", vec!["path:object.json"]),
    ] {
        let query = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        let optimized = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        let ids = |result: &objekte_search_core::query_results::SearchResult| {
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&optimized), expected, "{filter}");
        assert_eq!(ids(&optimized), ids(&reference), "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn trusted_builtin_query_readers_match_validating_results_on_valid_json() {
    let bytes = br#"{"title":"old","title":{"nested":"new"},"n":1e0,"array":["\u00df",true,null]}"#;
    let readers: [&dyn SourceReader; 3] =
        [&JsonReader, &JsonDecodedScalarsReader, &JsonLinesReader];
    let projections = [
        ScanProjection::full(true),
        ScanProjection {
            top_level: Some(HashSet::from(["title".into(), "n".into()])),
            collect_text: false,
            resolved_text_only: false,
        },
        ScanProjection {
            top_level: Some(HashSet::from(["title".into()])),
            collect_text: true,
            resolved_text_only: true,
        },
    ];
    for reader in readers {
        for projection in &projections {
            let validating = reader.parse_for_query(bytes, projection).unwrap();
            let trusted = reader.parse_for_query_trusted(bytes, projection).unwrap();
            assert_eq!(validating.value, trusted.value, "{}", reader.name());
            assert_eq!(
                validating
                    .text
                    .iter()
                    .map(|segment| (&segment.path, &segment.text))
                    .collect::<Vec<_>>(),
                trusted
                    .text
                    .iter()
                    .map(|segment| (&segment.path, &segment.text))
                    .collect::<Vec<_>>(),
                "{}",
                reader.name(),
            );
        }
    }
}

#[test]
fn trusted_builtin_projection_omits_search_time_validation_of_skipped_content() {
    let bytes = br#"{"wanted":5,"ignored":[1,]} trailing"#;
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["wanted".into()])),
        collect_text: false,
        resolved_text_only: false,
    };
    for reader in [
        &JsonReader as &dyn SourceReader,
        &JsonDecodedScalarsReader,
        &JsonLinesReader,
    ] {
        assert!(
            reader.parse_for_query(bytes, &projection).is_err(),
            "{}",
            reader.name()
        );
        let record = reader.parse_for_query_trusted(bytes, &projection).unwrap();
        let Value::Object(fields) = record.value else {
            panic!("projected JSON root is not an object");
        };
        assert_eq!(
            fields.get("wanted"),
            Some(&Value::Number(Number::integer(5))),
            "{}",
            reader.name(),
        );
    }
}

#[test]
fn decoded_scalar_fts_matches_tokens_and_preserves_duplicate_name_rules() {
    use objekte_search_core::index::bigram::BigramAlphabet;
    use objekte_search_core::query::options::ReturnMode;
    use objekte_search_core::query::{
        CaseSensitivity, FtsField, FtsMode, FtsQuery, Query, QueryOptions,
    };
    use objekte_search_core::repository::configuration::{IdentityPolicy, RepositoryOptions};

    let root = std::env::temp_dir().join(format!("objekte-scalar-fts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, json) in [
        (
            "one",
            r#"{"needle_only":"x","value":"a\n","num":1.0,"title":"old","title":"new"}"#,
        ),
        ("two", r#"[true,null,1e0,"ß"]"#),
        ("three", r#"{"title":"not here","num":-0}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let cases: &[(&str, Option<&str>, &[&str])] = &[
        ("needle_only", None, &[]),
        ("1.0", None, &["path:one.json"]),
        ("1e0", None, &["path:two.json"]),
        ("-0", None, &["path:three.json"]),
        ("true", None, &["path:two.json"]),
        ("null", None, &["path:two.json"]),
        ("a\n", None, &["path:one.json"]),
        ("old", None, &["path:one.json"]),
        ("old", Some("title"), &[]),
        ("new", Some("title"), &["path:one.json"]),
        ("1e0", Some("@root"), &["path:two.json"]),
    ];
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let mut definition = objekte_loader_json::decoded_scalar_folder("data", "data");
        definition.identity = IdentityPolicy::RelativePath;
        let repo = Index::start_build(
            &root,
            vec![definition],
            RepositoryOptions {
                bigram_alphabet: alphabet,
                ..Default::default()
            },
            objekte_cli::loader::installed_sources(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap()
        .view()
        .unwrap();
        for &(literal, selected, expected) in cases {
            let query = Query {
                collections: vec!["data".into()],
                fts: Some(FtsQuery {
                    case_sensitivity: CaseSensitivity::Sensitive,
                    mode: FtsMode::Literal,
                    literal: literal.into(),
                    pattern_parameter: None,
                    fields: selected
                        .into_iter()
                        .map(|field| FtsField {
                            path: objekte_search_core::model::FieldPath::parse(field).unwrap(),
                            binding: None,
                            query_path: None,
                        })
                        .collect(),
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
            let optimized = repo.search(&query).unwrap();
            let ids = |result: &objekte_search_core::query_results::SearchResult| {
                result
                    .hits
                    .iter()
                    .map(|hit| hit.record_id.clone())
                    .collect::<Vec<_>>()
            };
            assert_eq!(
                ids(&optimized),
                expected,
                "{literal:?} {selected:?} {alphabet:?}"
            );
            if selected.is_none() {
                let reference = repo.search_reference(&query).unwrap();
                assert_eq!(
                    ids(&optimized),
                    ids(&reference),
                    "{literal:?} {selected:?} {alphabet:?}"
                );
            }
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn build_json_borrows_logical_strings_after_temporal_conversion() {
    use objekte_search_core::model::LogicalType;
    use objekte_search_core::schema::{FieldDefinition, Schema};

    let source = objekte_cli::loader::installed_sources()
        .resolve(&objekte_loader_json::JSON_DOCUMENTS)
        .unwrap();
    let bytes =
        r#"{"date":"2026-01-02","title":"aßb","nested":{"names":["ab","cd",""]}}"#.as_bytes();
    let mut record = source.reader().parse_for_build_outcome(bytes).unwrap();
    assert!(record.text.is_empty());
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["date".into()],
            kind: LogicalType::Date,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    objekte_search_core::lifecycle::initial::test_hooks::assert_equivalent_logical_bigrams(
        &schema,
        &mut record,
        &["ab", "cd", "", "aßb"],
    );
}

#[test]
fn query_projection_skips_unneeded_fields_and_preserves_selected_values() {
    let bytes = br#"{"ignored":{"deep":[41,42]},"needed":{"name":"found"},"other":"hidden"}"#;
    let projection = ScanProjection {
        top_level: Some(HashSet::from(["needed".to_owned()])),
        collect_text: true,
        resolved_text_only: false,
    };
    for format in [
        objekte_loader_json::JSON_DOCUMENTS,
        objekte_loader_jsonl::JSON_LINES,
    ] {
        let source = objekte_cli::loader::installed_sources()
            .resolve(&format)
            .unwrap();
        let selected = source.reader().parse_for_query(bytes, &projection).unwrap();
        assert_eq!(
            selected.value,
            objekte_loader_json::value_from_json(serde_json::json!({"needed":{"name":"found"}}))
                .unwrap()
        );
        assert_eq!(selected.text.len(), 1);
        assert_eq!(selected.text[0].path, ["needed", "name"]);
        assert_eq!(selected.text[0].text, "found");
        let full = source
            .reader()
            .parse_for_query(bytes, &ScanProjection::full(false))
            .unwrap();
        assert!(matches!(full.value, Value::Object(ref fields) if fields.len() == 3));
        assert!(full.text.is_empty());
        assert!(
            source
                .reader()
                .parse_for_query(br#"{"needed":"ok","ignored":[1,]}"#, &projection)
                .is_err()
        );
    }
}
