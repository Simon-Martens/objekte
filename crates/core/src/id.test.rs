use crate::model::{FieldPath, LogicalType, Value};
use crate::posting::ScalarKey;
use crate::posting_values::PostingValuesRequest;
use crate::query::{CaseSensitivity, FtsField, FtsQuery, Query, QueryOptions, parse_filter};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::{
    CollectionDefinition, IdentityPolicy, Payload, Repository, RepositoryOptions, SnapshotMode,
};
use crate::source::SourceFormat;
use crate::workload::{ReturnMode, SortField};

fn definition() -> CollectionDefinition {
    let mut schema = Schema::default();
    for (path, kind, multi) in [
        ("id", LogicalType::Id, false),
        ("entry", LogicalType::Id, false),
        ("refs", LogicalType::Array(Box::new(LogicalType::Id)), true),
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
            r#"{"id":"A","entry":"TargetA","refs":["TargetA","TargetA"]}"#,
        ),
        ("b", r#"{"id":"a","entry":"targeta","refs":[]}"#),
        ("c", r#"{"id":"C","entry":"Missing"}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), json).unwrap();
    }
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::ReadWrite,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, vec![definition()], options).unwrap();
    assert!(!built.startup().restored);
    let restored = Repository::open_with_options(&root, vec![definition()], options).unwrap();
    assert!(restored.startup().restored);
    for repo in [&built, &restored] {
        for (filter, ids) in [
            ("entry = 'TargetA'", vec!["A"]),
            ("entry = 'targeta'", vec!["a"]),
            ("entry != 'TargetA'", vec!["C", "a"]),
            ("refs ?= 'TargetA'", vec!["A"]),
            ("@record.id = 'A'", vec!["A"]),
            ("@record.id:lower = 'a'", vec!["A", "a"]),
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
                case_sensitivity: CaseSensitivity::Sensitive,
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
fn id_conversion_rejects_invalid_source_identifiers_but_metadata_can_find_generated_ids() {
    let schema = definition().schema;
    for text in ["", "~generated:1:2:3", "~fallback:0"] {
        let mut value = Value::from_json(serde_json::json!({"id":"A","entry":text})).unwrap();
        assert!(
            schema
                .coerce_record(&mut value)
                .unwrap_err()
                .contains("invalid intrinsic record ID")
        );
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
            snapshot_mode: SnapshotMode::Disabled,
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
