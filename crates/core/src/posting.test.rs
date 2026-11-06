use super::*;

#[test]
fn base_zero_merge_moves_postings_and_later_unique_keys_shift() {
    let field = FieldDefinition {
        path: vec!["tags".into()],
        kind: LogicalType::Array(Box::new(LogicalType::String)),
        required: false,
        nullable: true,
        indexed: true,
        multi: true,
        element_nullable: vec![],
    };
    let mut first = PostingIndex::new(&field);
    first
        .add(
            RecordSlot(0),
            &Value::from_json(serde_json::json!({"tags":["a","a"]})).unwrap(),
        )
        .unwrap();
    let key = ScalarKey::String("a".into());
    let original = first.values.get(&key).unwrap() as *const RoaringBitmap;
    let mut merged = PostingIndex::new(&field);
    merged.merge_shifted(first, 0).unwrap();
    assert_eq!(merged.values.get(&key).unwrap() as *const _, original);
    assert_eq!(
        merged.lengths[&Number::integer(2)]
            .iter()
            .collect::<Vec<_>>(),
        [0]
    );
    let mut later = PostingIndex::new(&field);
    later
        .add(
            RecordSlot(0),
            &Value::from_json(serde_json::json!({"tags":["b"]})).unwrap(),
        )
        .unwrap();
    merged.merge_shifted(later, 65).unwrap();
    assert_eq!(
        merged.values[&ScalarKey::String("b".into())]
            .iter()
            .collect::<Vec<_>>(),
        [65]
    );
    assert_eq!(
        merged.lengths[&Number::integer(1)]
            .iter()
            .collect::<Vec<_>>(),
        [65]
    );
}

#[test]
fn presence_null_array_dedup_and_cardinality() {
    let field = FieldDefinition {
        path: vec!["values".into()],
        kind: LogicalType::Array(Box::new(LogicalType::String)),
        required: false,
        nullable: true,
        indexed: true,
        multi: true,
        element_nullable: Vec::new(),
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
                element_nullable: Vec::new(),
            })
            .unwrap();
        let repo = Repository::open_with_options(
            &root,
            vec![CollectionDefinition {
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
                optimized.count.unwrap(),
                reference.count.unwrap(),
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

#[test]
fn nested_array_lengths_follow_quantifiers_with_and_without_postings() {
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;
    let root = std::env::temp_dir().join(format!("objekte-nested-length-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (id, json) in [
        (
            "mixed",
            r#"{"groups":[{"tags":["a"]},{"tags":["a","b","c"]}]}"#,
        ),
        (
            "reversed",
            r#"{"groups":[{"tags":["a","b","c"]},{"tags":["a"]}]}"#,
        ),
        ("triple", r#"{"groups":[{"tags":["a","b","c"]}]}"#),
        ("empty", r#"{"groups":[]}"#),
        ("empty_tags", r#"{"groups":[{"tags":[]}]}"#),
        ("null_tags", r#"{"groups":[{"tags":null}]}"#),
        ("missing", r#"{}"#),
    ] {
        std::fs::write(root.join(format!("data/{id}.json")), json).unwrap();
    }
    for indexed in [false, true] {
        let mut schema = crate::reference::Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["groups".into(), "tags".into()],
                kind: LogicalType::Array(Box::new(LogicalType::String)),
                required: false,
                nullable: true,
                indexed,
                multi: true,
                element_nullable: vec![],
            })
            .unwrap();
        let repo = Repository::open_with_options(
            &root,
            vec![CollectionDefinition {
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
            }],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..Default::default()
            },
        )
        .unwrap();
        if indexed {
            use crate::reference::{BoundExpr, bind_filter};
            let collection = &repo.collections()[0];
            let posting = &collection.postings.fields[&vec!["groups".into(), "tags".into()]];
            let domain = RoaringBitmap::from_iter(0..collection.generation.records.len() as u32);
            let BoundExpr::Term(universal) = bind_filter(
                &parse_filter("groups.tags:length = 3").unwrap(),
                &collection.definition.schema,
            )
            .unwrap() else {
                panic!("term")
            };
            assert!(posting.estimate_exact(&universal, domain.len()).is_none());
            assert!(posting.exact_set(&universal, &domain).is_none());
            assert!(
                posting
                    .exact_set_restricted(&universal, &domain, &domain)
                    .is_none()
            );
            let BoundExpr::Term(existential) = bind_filter(
                &parse_filter("groups.tags:length ?= 3").unwrap(),
                &collection.definition.schema,
            )
            .unwrap() else {
                panic!("term")
            };
            assert_eq!(posting.exact_set(&existential, &domain).unwrap().len(), 3);
            assert_eq!(
                posting
                    .exact_set_restricted(&existential, &domain, &domain)
                    .unwrap()
                    .len(),
                3
            );
        }
        for (filter, expected) in [
            ("groups.tags:length = 3", vec!["empty", "triple"]),
            (
                "groups.tags:length ?= 3",
                vec!["mixed", "reversed", "triple"],
            ),
            ("groups.tags:length != 3", vec!["empty", "empty_tags"]),
            (
                "groups.tags:length ?!= 3",
                vec!["empty_tags", "mixed", "reversed"],
            ),
            ("groups.tags:length >= 3", vec!["empty", "triple"]),
            (
                "groups.tags:length ?>= 3",
                vec!["mixed", "reversed", "triple"],
            ),
            ("groups.tags:length < 3", vec!["empty", "empty_tags"]),
            (
                "groups.tags:length ?< 3",
                vec!["empty_tags", "mixed", "reversed"],
            ),
            (
                "!(groups.tags:length ?= 3)",
                vec!["empty", "empty_tags", "missing", "null_tags"],
            ),
            (
                "groups.tags:length ?= 3 && groups.tags:length ?!= 3",
                vec!["mixed", "reversed"],
            ),
            (
                "groups.tags:length = 3 || groups.tags:length ?!= 3",
                vec!["empty", "empty_tags", "mixed", "reversed", "triple"],
            ),
        ] {
            let query = Query {
                collections: vec!["data".into()],
                fts: None,
                filter: Some(parse_filter(filter).unwrap()),
                options: QueryOptions {
                    sort: vec![],
                    limit: 10,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: true,
                },
            };
            let result = repo.search(&query).unwrap();
            let actual = result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{indexed} {filter}");
            assert_eq!(result.count, Some(expected.len()));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn narrow_ordered_ranges_visit_only_matching_dictionary_keys() {
    let mut values = BTreeMap::new();
    values.insert(
        ScalarKey::String("before".into()),
        RoaringBitmap::from_iter([20_000]),
    );
    for n in 0..10_000 {
        values.insert(
            ScalarKey::Number(Number::integer(n)),
            RoaringBitmap::from_iter([n as u32]),
        );
    }
    values.insert(ScalarKey::Bool(false), RoaringBitmap::from_iter([20_001]));
    let key = ScalarKey::Number(Number::integer(9_990));
    let (upper, visits) = scalar_range_postings(&values, &key, Operator::Gt, None);
    assert_eq!(visits, 9);
    assert_eq!(
        upper.iter().collect::<Vec<_>>(),
        (9_991..10_000).collect::<Vec<_>>()
    );
    let (inclusive, visits) = scalar_range_postings(&values, &key, Operator::Ge, None);
    assert_eq!(visits, 10);
    assert!(inclusive.contains(9_990) && !inclusive.contains(20_001));
    let (lower, visits) = scalar_range_postings(
        &values,
        &ScalarKey::Number(Number::integer(10)),
        Operator::Lt,
        None,
    );
    assert_eq!(visits, 10);
    assert_eq!(
        lower.iter().collect::<Vec<_>>(),
        (0..10).collect::<Vec<_>>()
    );
    let (edge, visits) = scalar_range_postings(
        &values,
        &ScalarKey::Number(Number::integer(0)),
        Operator::Lt,
        None,
    );
    assert!(edge.is_empty());
    assert_eq!(visits, 0);
    let (typed, visits) =
        scalar_range_postings(&values, &ScalarKey::String("a".into()), Operator::Gt, None);
    assert_eq!(typed.iter().collect::<Vec<_>>(), [20_000]);
    assert_eq!(visits, 1);
    let restricted = RoaringBitmap::from_iter([9_998]);
    let (restricted_upper, visits) =
        scalar_range_postings(&values, &key, Operator::Gt, Some(&restricted));
    assert_eq!(visits, 9);
    assert_eq!(restricted_upper.iter().collect::<Vec<_>>(), [9_998]);

    let lengths = (0..10_000)
        .map(|n| (Number::integer(n), RoaringBitmap::from_iter([n as u32])))
        .collect::<BTreeMap<_, _>>();
    let (long, visits) = range_postings(&lengths, &Number::integer(9_990), Operator::Gt, None);
    assert_eq!(visits, 9);
    assert_eq!(
        long.iter().collect::<Vec<_>>(),
        (9_991..10_000).collect::<Vec<_>>()
    );

    use crate::query::parse_filter;
    use crate::reference::{BoundExpr, Schema, bind_filter};
    let field = FieldDefinition {
        path: vec!["n".into()],
        kind: LogicalType::Number,
        required: true,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    };
    let mut schema = Schema::default();
    schema.register_field(field.clone()).unwrap();
    let mut index = PostingIndex::new(&field);
    for n in 0..10_000 {
        let value = Value::Number(Number::integer(n));
        index
            .add_values(RecordSlot(n as u32), Some(&[&value]))
            .unwrap();
    }
    index.finish_stats();
    let BoundExpr::Term(term) = bind_filter(&parse_filter("n > 9990").unwrap(), &schema).unwrap()
    else {
        panic!("expected term")
    };
    assert_eq!(index.estimate_exact(&term, 10_000), Some((10.0, 9)));
    let domain = RoaringBitmap::from_iter(0..10_000);
    assert_eq!(
        index
            .exact_set(&term, &domain)
            .unwrap()
            .iter()
            .collect::<Vec<_>>(),
        (9_991..10_000).collect::<Vec<_>>()
    );
    let restricted = RoaringBitmap::from_iter([9_998]);
    assert_eq!(
        index
            .exact_set_restricted(&term, &domain, &restricted)
            .unwrap(),
        restricted
    );
}

#[test]
fn ordered_posting_answers_bounds_and_array_lengths_exactly() {
    use crate::query::parse_filter;
    use crate::reference::{BoundExpr, Schema, bind_filter};

    let number = FieldDefinition {
        path: vec!["n".into()],
        kind: LogicalType::Number,
        required: false,
        nullable: true,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    };
    let array = FieldDefinition {
        path: vec!["a".into()],
        kind: LogicalType::Array(Box::new(LogicalType::Number)),
        required: false,
        nullable: false,
        indexed: true,
        multi: true,
        element_nullable: vec![false],
    };
    let mut schema = Schema::default();
    schema.register_field(number.clone()).unwrap();
    schema.register_field(array.clone()).unwrap();
    let mut numbers = PostingIndex::new(&number);
    let mut arrays = PostingIndex::new(&array);
    for (slot, json) in [
        serde_json::json!({"n":null, "a":[]}),
        serde_json::json!({"n":1, "a":[1]}),
        serde_json::json!({"n":2, "a":[1,2]}),
        serde_json::json!({"n":3, "a":[1,2,3]}),
        serde_json::json!({"a":[1,2,3,4]}),
    ]
    .into_iter()
    .enumerate()
    {
        let record = Value::from_json(json).unwrap();
        numbers.add(RecordSlot(slot as u32), &record).unwrap();
        arrays.add(RecordSlot(slot as u32), &record).unwrap();
    }
    let domain = RoaringBitmap::from_iter(0..5);
    for (filter, expected) in [
        ("n > 2", vec![3]),
        ("n >= 2", vec![2, 3]),
        ("n < 2", vec![1]),
        ("n <= 2", vec![1, 2]),
        ("a:length > 2", vec![3, 4]),
        ("a:length >= 2", vec![2, 3, 4]),
        ("a:length < 2", vec![0, 1]),
        ("a:length <= 2", vec![0, 1, 2]),
        ("a:length != 2", vec![0, 1, 3, 4]),
    ] {
        let BoundExpr::Term(term) = bind_filter(&parse_filter(filter).unwrap(), &schema).unwrap()
        else {
            panic!("expected term")
        };
        let index = if filter.starts_with('n') {
            &numbers
        } else {
            &arrays
        };
        let exact = index.exact_set(&term, &domain).unwrap();
        assert_eq!(exact.iter().collect::<Vec<_>>(), expected, "{filter}");
        let restricted = RoaringBitmap::from_iter([2, 3]);
        let restricted_exact = index
            .exact_set_restricted(&term, &domain, &restricted)
            .unwrap();
        assert_eq!(
            restricted_exact,
            &exact & &restricted,
            "restricted {filter}"
        );
    }
}
