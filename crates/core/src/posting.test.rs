use super::*;

#[test]
fn presence_null_array_dedup_and_cardinality() {
    let field = FieldDefinition {
        path: vec!["values".into()],
        kind: LogicalType::Array(Box::new(LogicalType::String)),
        required: false,
        nullable: true,
        indexed: true,
        multi: true,
    };
    let mut index = PostingIndex::new(&field);
    for (slot, json) in [
        serde_json::json!({}),
        serde_json::json!({"values":null}),
        serde_json::json!({"values":[]}),
        serde_json::json!({"values":["a","a","b"]}),
    ]
    .into_iter()
    .enumerate()
    {
        index
            .add(RecordSlot(slot as u32), &Value::from_json(json).unwrap())
            .unwrap();
    }
    index.finish_stats();
    index.validate(4).unwrap();
    assert_eq!(index.presence.iter().collect::<Vec<_>>(), [1, 2, 3]);
    assert_eq!(index.nulls.iter().collect::<Vec<_>>(), [1]);
    assert_eq!(
        index
            .lengths
            .get(&Number::integer(0))
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [2]
    );
    assert_eq!(
        index
            .lengths
            .get(&Number::integer(3))
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [3]
    );
    assert_eq!(
        index
            .values
            .get(&ScalarKey::String("a".into()))
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        [3]
    );
}

#[test]
fn nullable_arrays_keep_quantifiers_when_postings_are_available() {
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-array-null-{}", std::process::id()));
    let path = root.join("data");
    std::fs::create_dir_all(&path).unwrap();
    for (id, json) in [
        ("missing", "{}"),
        ("explicit", r#"{"tags":null}"#),
        ("empty", r#"{"tags":[]}"#),
        ("null_item", r#"{"tags":[null]}"#),
        ("all_null", r#"{"tags":[null,null]}"#),
        ("value", r#"{"tags":["a"]}"#),
        ("mixed", r#"{"tags":[null,"a"]}"#),
    ] {
        std::fs::write(path.join(format!("{id}.json")), json).unwrap();
    }
    for indexed in [false, true] {
        let mut schema = crate::reference::Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["tags".into()],
                kind: LogicalType::Array(Box::new(LogicalType::String)),
                required: false,
                nullable: true,
                indexed,
                multi: true,
            })
            .unwrap();
        let repo = Repository::open_with_options(
            &root,
            vec![CollectionDefinition {
                name: "data".into(),
                source_path: "data".into(),
                format: SourceFormat::Json,
                identity: IdentityPolicy::Filename,
                schema,
                sort_fields: vec![],
                sort_locale: "de".into(),
            }],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap();
        for (filter, expected) in [
            (
                "tags = null",
                vec!["all_null", "empty", "explicit", "null_item"],
            ),
            ("tags != null", vec!["empty", "value"]),
            ("tags:each != null", vec!["empty", "value"]),
            (
                "tags ?= null",
                vec!["all_null", "explicit", "mixed", "null_item"],
            ),
            ("tags ?!= null", vec!["mixed", "value"]),
            ("tags:null = true", vec!["explicit"]),
            (
                "tags:null = false",
                vec![
                    "all_null",
                    "empty",
                    "missing",
                    "mixed",
                    "null_item",
                    "value",
                ],
            ),
            ("!(tags = null)", vec!["missing", "mixed", "value"]),
            (
                "tags = null && tags:exists = true",
                vec!["all_null", "empty", "explicit", "null_item"],
            ),
            (
                "tags = null || tags ?!= null",
                vec![
                    "all_null",
                    "empty",
                    "explicit",
                    "mixed",
                    "null_item",
                    "value",
                ],
            ),
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
                    evidence: false,
                },
            };
            let optimized = repo.search(&query).unwrap();
            let reference = repo.search_reference(&query).unwrap();
            let mut actual = optimized
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>();
            actual.sort_unstable();
            assert_eq!(actual, expected, "indexed={indexed}, {filter}");
            assert_eq!(
                optimized.count, reference.count,
                "indexed={indexed}, {filter}"
            );
            assert_eq!(
                optimized.hits, reference.hits,
                "indexed={indexed}, {filter}"
            );
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
