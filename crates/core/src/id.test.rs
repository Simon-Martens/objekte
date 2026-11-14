use crate::model::{FieldPath, LogicalType, RecordId, Value};
use crate::posting::ScalarKey;
use crate::posting_values::PostingValuesRequest;
use crate::query::{CaseSensitivity, FtsField, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Payload, Repository, RepositoryOptions,
};
use crate::source::SourceFormat;
use crate::workload::{ReturnMode, SortField};

fn definition() -> CollectionDefinition {
    let mut schema = Schema::default();
    for (path, kind, multi) in [
        ("id", LogicalType::Id, false),
        ("entry", LogicalType::Id, false),
        ("refs", LogicalType::Array(Box::new(LogicalType::Id)), true),
        ("name", LogicalType::String, false),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: path == "id",
                nullable: false,
                indexed: true,
                multi,
                element_nullable: vec![false],
            })
            .unwrap();
    }
    CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Intrinsic(vec!["id".into()]),
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("entry").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }
}

fn query(filter: &str) -> Query {
    Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    }
}

#[test]
fn typed_ids_remain_distinct_from_strings_through_postings_text_and_restore() {
    let root = std::env::temp_dir().join(format!("objekte-typed-id-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, json) in [
        (
            "a",
            r#"{"id":"A","entry":"TargetA","refs":["TargetA","TargetA"],"name":"a"}"#,
        ),
        ("b", r#"{"id":"a","entry":"targeta","refs":[],"name":"A"}"#),
        ("c", r#"{"id":"C","entry":"Missing","name":"x"}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let options = RepositoryOptions {
        verified_cache_bytes: 1024 * 1024,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, vec![definition()], options).unwrap();
    let restored = Repository::open_with_options(&root, vec![definition()], options).unwrap();
    for repo in [&built, &restored] {
        for (filter, ids) in [
            ("entry = 'TargetA'", vec!["A"]),
            ("entry = 'targeta'", vec!["a"]),
            ("entry != 'TargetA'", vec!["C", "a"]),
            ("refs ?= 'TargetA'", vec!["A"]),
            ("@record.id = 'A'", vec!["A"]),
            ("@record.id:lower = 'a'", vec!["A", "a"]),
            ("@record.id:lower = name:lower", vec!["A", "a"]),
        ] {
            let mut actual = repo
                .search(&query(filter))
                .unwrap()
                .hits
                .into_iter()
                .map(|hit| hit.record_id)
                .collect::<Vec<_>>();
            actual.sort();
            assert_eq!(actual, ids, "{filter}");
            assert_eq!(
                repo.search(&query(filter)).unwrap().hits,
                repo.search_reference(&query(filter)).unwrap().hits,
                "{filter}"
            );
        }
        let logical = repo.search(&query("entry = 'TargetA'")).unwrap();
        assert!(
            matches!(&logical.hits[0].payload, Some(Payload::Logical(Value::Object(fields)))
            if fields.get("entry") == Some(&Value::Id("TargetA".into())))
        );
        assert!(
            repo.search(&query("entry ~ 'Target'"))
                .unwrap_err()
                .contains("ID permits only equality")
        );
        for (filter, value, expected) in [
            ("entry = $id", Value::String("TargetA".into()), vec!["A"]),
            ("entry = $id", Value::Id("TargetA".into()), vec!["A"]),
            ("entry = $id", Value::Id("targeta".into()), vec!["a"]),
            ("@record.id = $id", Value::Id("A".into()), vec!["A"]),
            ("@record.id = $id", Value::Id("a".into()), vec!["a"]),
        ] {
            let params = std::collections::HashMap::from([("id".to_owned(), value)]);
            let request = query(filter);
            let actual = repo.search_with_parameters(&request, &params).unwrap();
            let reference = repo
                .search_reference_with_parameters(&request, &params)
                .unwrap();
            assert_eq!(actual.hits, reference.hits, "{filter}");
            assert_eq!(actual.count, reference.count, "{filter}");
            assert_eq!(
                actual
                    .hits
                    .iter()
                    .map(|hit| hit.record_id.as_str())
                    .collect::<Vec<_>>(),
                expected,
                "{filter}"
            );
        }
        let by_id = query("@record.id = $id");
        for (iteration, (value, expected)) in
            [("A", "A"), ("a", "a"), ("A", "A")].into_iter().enumerate()
        {
            let params =
                std::collections::HashMap::from([("id".to_owned(), Value::Id(value.into()))]);
            let result = repo.search_with_parameters(&by_id, &params).unwrap();
            assert_eq!(result.hits[0].record_id, expected);
            if iteration == 2 {
                assert!(result.counters.verified_result_hit);
                assert_eq!(result.count, Some(1));
            }
        }
        let tagged =
            crate::parameters::parse_parameter_json(r#"{"type":"id","value":"TargetA"}"#).unwrap();
        assert_eq!(tagged, Value::Id("TargetA".into()));
        let incompatible = std::collections::HashMap::from([("value".into(), tagged)]);
        assert!(
            repo.search_with_parameters(&query("name = $value"), &incompatible)
                .unwrap_err()
                .contains("incompatible")
        );
        assert!(
            repo.search_with_parameters(&query("entry = $value && name = $value"), &incompatible)
                .unwrap_err()
                .contains("incompatible")
        );

        let page = repo
            .posting_values(&PostingValuesRequest::new("data", "entry", 10))
            .unwrap();
        assert_eq!(
            page.values
                .iter()
                .map(|entry| entry.value.clone())
                .collect::<Vec<_>>(),
            [
                ScalarKey::Id("Missing".into()),
                ScalarKey::Id("TargetA".into()),
                ScalarKey::Id("targeta".into())
            ]
        );
        let refs = repo
            .posting_values(&PostingValuesRequest::new("data", "refs", 10))
            .unwrap();
        assert_eq!(refs.values.len(), 1);
        assert_eq!(refs.values[0].value, ScalarKey::Id("TargetA".into()));
        assert_eq!(refs.values[0].count, 1);
        let mut incompatible = PostingValuesRequest::new("data", "entry", 10);
        incompatible.lower = Some(crate::posting_values::PostingValueBound {
            value: ScalarKey::String("TargetA".into()),
            inclusive: true,
        });
        assert!(
            repo.posting_values(&incompatible)
                .unwrap_err()
                .contains("incompatible type")
        );
        let mut sorted = query("entry != 'never'");
        sorted.options.sort = vec![SortField {
            field: "entry".into(),
            descending: false,
            expression: None,
        }];
        assert_eq!(
            repo.search(&sorted)
                .unwrap()
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            ["C", "A", "a"]
        );

        let fts = Query {
            collections: vec!["data".into()],
            filter: None,
            fts: Some(FtsQuery {
                pattern_parameter: None,
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: crate::query::FtsMode::Literal,
                literal: "TargetA".into(),
                fields: vec![],
            }),
            options: QueryOptions {
                sort: vec![],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        assert_eq!(
            repo.search(&fts)
                .unwrap()
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            ["A"]
        );
        let mut selected_id = fts.clone();
        selected_id.fts.as_mut().unwrap().fields = vec![FtsField {
            query_path: None,
            path: FieldPath::parse("@record.id").unwrap(),
            binding: None,
        }];
        selected_id.fts.as_mut().unwrap().literal = "A".into();
        assert_eq!(
            repo.search(&selected_id)
                .unwrap()
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            ["A"]
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn id_conversion_rejects_empty_values_and_keeps_generated_record_identity_separate() {
    let schema = definition().schema;
    let mut empty = Value::from_json(serde_json::json!({"id":"A","entry":""})).unwrap();
    assert!(
        schema
            .coerce_record(&mut empty)
            .unwrap_err()
            .contains("ID is empty")
    );
    for text in ["~generated:1:2:3", "~fallback:0"] {
        let mut value = Value::from_json(serde_json::json!({"id":"A","entry":text})).unwrap();
        schema.coerce_record(&mut value).unwrap();
        let Value::Object(fields) = value else {
            panic!("expected object")
        };
        assert_eq!(fields["entry"], Value::Id(text.into()));
        assert!(RecordId::intrinsic(text).is_err());
    }
    let root = std::env::temp_dir().join(format!("objekte-generated-id-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), r#"{"id":"A","entry":"X"}"#).unwrap();
    let mut definition = definition();
    definition.identity = IdentityPolicy::Generated;
    let repo = Repository::open_with_options(
        &root,
        vec![definition],
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    let mut all = query("entry = 'X'");
    all.options.return_mode = ReturnMode::Locator;
    let id = repo.search(&all).unwrap().hits[0].record_id.clone();
    assert!(id.starts_with("~generated:"));
    let mut by_id = query("@record.id = 'placeholder'");
    by_id.filter = Some(parse_filter(&format!("@record.id = '{id}'")).unwrap());
    assert_eq!(repo.search(&by_id).unwrap().hits[0].record_id, id);
    std::fs::remove_dir_all(root).unwrap();
}
