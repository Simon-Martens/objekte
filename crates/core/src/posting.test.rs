use super::*;

#[test]
fn broad_id_union_matches_individual_lookups() {
    let field = FieldDefinition {
        path: vec!["ref".into()],
        kind: LogicalType::Id,
        required: false,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    };
    let mut index = PostingIndex::new(&field);
    for id in 0..100 {
        index.values.insert(
            ScalarKey::Id(format!("id-{id:03}")),
            RoaringBitmap::from_iter([id, id + 100]),
        );
    }
    for ids in [
        HashSet::from(["id-017".to_owned()]),
        (0..80).map(|id| format!("id-{id:03}")).collect(),
    ] {
        let (actual, lists, members) = index.union_id_postings(&ids).unwrap();
        let expected = ids.iter().fold(RoaringBitmap::new(), |mut all, id| {
            if let Some(slots) = index.values.get(&ScalarKey::Id(id.clone())) {
                all |= slots;
            }
            all
        });
        assert_eq!(actual, expected);
        assert_eq!(lists, ids.len() as u64);
        assert_eq!(members, ids.len() as u64 * 2);
    }
}

#[test]
fn sealed_broad_id_union_uses_checked_borrowed_keys() {
    use crate::posting_dictionary::{PostingPack, SealedPostingDictionary, append_dictionary};
    use crate::segment_format::{SectionKind, SegmentReader, SegmentWriter};
    use std::io::Cursor;

    let field = FieldDefinition {
        path: vec!["ref".into()],
        kind: LogicalType::Id,
        required: false,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    };
    let mut index = PostingIndex::new(&field);
    for id in 0..100 {
        index.values.insert(
            ScalarKey::Id(format!("id-{id:03}")),
            RoaringBitmap::from_iter([id, id + 100]),
        );
    }
    index.values.insert(
        ScalarKey::String("not-an-id".into()),
        RoaringBitmap::from_iter([201]),
    );
    index.finish_stats();
    let expected = index.values.clone();
    let mut dictionary = Vec::new();
    let mut pack = PostingPack::new();
    let values = append_dictionary(&mut dictionary, &mut pack, &index.values).unwrap();
    let lengths = append_dictionary(&mut dictionary, &mut pack, &index.lengths).unwrap();
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), 2).unwrap();
    writer
        .write_section(SectionKind::ScalarDictionaries, |target| {
            target
                .write_all(&dictionary)
                .map_err(|error| error.to_string())
        })
        .unwrap();
    writer
        .write_section(SectionKind::PostingPayloads, |target| {
            target
                .write_all(pack.bytes())
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let reader = Arc::new(
        SegmentReader::from_owned(Arc::from(writer.finish().unwrap().into_inner())).unwrap(),
    );
    let allowed = Arc::new(RoaringBitmap::from_iter(0..202));
    index
        .install_sealed(
            SealedPostingDictionary::open(
                Arc::clone(&reader),
                SectionKind::ScalarDictionaries,
                values,
                Arc::clone(&allowed),
            )
            .unwrap(),
            SealedPostingDictionary::open(
                reader,
                SectionKind::ScalarDictionaries,
                lengths,
                allowed,
            )
            .unwrap(),
        )
        .unwrap();
    for ids in [
        HashSet::from(["id-017".to_owned()]),
        (0..80).map(|id| format!("id-{id:03}")).collect(),
    ] {
        let (actual, lists, members) = index.union_id_postings(&ids).unwrap();
        let wanted = ids.iter().fold(RoaringBitmap::new(), |mut all, id| {
            if let Some(slots) = expected.get(&ScalarKey::Id(id.clone())) {
                all |= slots;
            }
            all
        });
        assert_eq!(actual, wanted);
        assert_eq!(lists, ids.len() as u64);
        assert_eq!(members, ids.len() as u64 * 2);
    }
}

#[test]
fn temporal_period_estimates_probe_bounded_keys_and_ranges_keep_exact_membership() {
    use crate::query::parse_filter;
    use crate::reference::{BoundExpr, Schema, bind_filter};

    let field = FieldDefinition {
        path: vec!["issued".into()],
        kind: LogicalType::Date,
        required: true,
        nullable: false,
        indexed: true,
        multi: false,
        element_nullable: Vec::new(),
    };
    let mut schema = Schema::default();
    schema.register_field(field.clone()).unwrap();
    let mut index = PostingIndex::new(&field);
    for ordinal in 1..=366 {
        let date = time::Date::from_ordinal_date(2024, ordinal).unwrap();
        let value = Value::Date(date);
        index
            .add_values(RecordSlot(u32::from(ordinal - 1)), Some(&[&value]))
            .unwrap();
    }
    index.finish_stats();
    let domain = RoaringBitmap::from_iter(0..366);
    for (operator, expected) in [
        ("=", 29),
        ("!=", 337),
        ("<", 31),
        ("<=", 60),
        (">", 306),
        (">=", 335),
    ] {
        let filter = format!("issued {operator} 2024-02");
        let BoundExpr::Term(term) = bind_filter(&parse_filter(&filter).unwrap(), &schema).unwrap()
        else {
            panic!("expected bound temporal term")
        };
        let mut visited = 0;
        assert!(
            index
                .estimate_exact_with_visits(&term, domain.len(), &mut visited)
                .is_some()
        );
        assert!(visited <= 33, "{filter}: {visited}");
        let matches = index.exact_set(&term, &domain).unwrap();
        assert_eq!(matches.len(), expected, "{filter}");
        let restricted = RoaringBitmap::from_iter([0, 59, 60, 365]);
        assert_eq!(
            index
                .exact_set_restricted(&term, &domain, &restricted)
                .unwrap(),
            &matches & &restricted,
            "{filter}"
        );
    }
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
fn mixed_multivalue_null_predicate_cannot_use_singleton_null_posting() {
    use crate::query::parse_filter;
    use crate::reference::{BoundExpr, Schema, bind_filter};

    let field = FieldDefinition {
        path: vec!["values".into()],
        kind: LogicalType::Array(Box::new(LogicalType::String)),
        required: false,
        nullable: true,
        indexed: true,
        multi: true,
        element_nullable: vec![true],
    };
    let mut schema = Schema::default();
    schema.register_field(field.clone()).unwrap();
    let mut posting = PostingIndex::new(&field);
    let null = Value::Null;
    let text = Value::String("one".into());
    posting
        .add_values(RecordSlot(0), Some(&[&null, &text]))
        .unwrap();
    posting.finish_stats();
    assert!(!posting.nulls.contains(0));
    let domain = RoaringBitmap::from_iter([0]);
    for filter in [
        "values:null = true",
        "values:null = false",
        "values:null != true",
    ] {
        let BoundExpr::Term(term) = bind_filter(&parse_filter(filter).unwrap(), &schema).unwrap()
        else {
            panic!("expected term");
        };
        assert!(posting.estimate_exact(&term, 1).is_none(), "{filter}");
        assert!(posting.exact_set(&term, &domain).is_none(), "{filter}");
        assert!(
            posting
                .exact_set_restricted(&term, &domain, &domain)
                .is_none(),
            "{filter}"
        );
    }
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
        {
            use crate::reference::{BoundExpr, bind_filter};
            let collection = &repo.collections()[0];
            let posting = &collection.postings.fields[&vec!["groups".into(), "tags".into()]];
            assert_eq!(posting.value_complete, indexed);
            assert!(posting.length_complete);
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
            let reference = repo.search_reference(&query).unwrap();
            let actual = result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>();
            assert_eq!(actual, expected, "{indexed} {filter}");
            assert_eq!(result.count, Some(expected.len()));
            assert_eq!(result.count, reference.count, "{indexed} {filter}");
            assert_eq!(result.hits, reference.hits, "{indexed} {filter}");
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

#[test]
fn declared_structural_facts_are_exact_without_a_value_dictionary() {
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::reference::{BoundExpr, Schema, bind_filter};
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-structural-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        (
            "00",
            r#"{"nums":[],"objects":[],"text":"","blocked":[],"score":1,"shape":{}}"#,
        ),
        (
            "01",
            r#"{"nums":[1],"objects":[{"x":1}],"text":" ","blocked":[1],"score":null,"shape":null}"#,
        ),
        ("02", r#"{"nums":[1,2],"objects":[{},{}],"text":"Ω"}"#),
        (
            "03",
            r#"{"nums":[1,2,3],"objects":[{},{},{}],"text":"\t\n"}"#,
        ),
        ("04", r#"{"nums":null,"objects":null,"text":null}"#),
        ("05", r#"{}"#),
        ("06", r#"{"text":""}"#),
        ("07", r#"{"text":"Ω","text":""}"#),
        ("08", r#"{"text":"","text":"Ω"}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let mut schema = Schema::default();
    for (name, kind, multi) in [
        (
            "nums",
            LogicalType::Array(Box::new(LogicalType::Number)),
            true,
        ),
        (
            "objects",
            LogicalType::Array(Box::new(LogicalType::Object)),
            true,
        ),
        ("text", LogicalType::String, false),
        ("score", LogicalType::Number, false),
        ("shape", LogicalType::Object, false),
        (
            "blocked",
            LogicalType::Array(Box::new(LogicalType::Number)),
            true,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: true,
                indexed: false,
                multi,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    schema.prohibit_auto_index(vec!["blocked".into()]).unwrap();
    let postings = PostingCollection::new(&schema);
    assert_eq!(postings.fields.len(), 6);
    assert!(
        postings.fields[&vec!["blocked".into()]]
            .structural
            .is_some()
    );
    assert!(!postings.fields[&vec!["blocked".into()]].length_complete);
    assert!(!postings.fields[&vec!["nums".into()]].value_complete);
    assert!(!postings.fields[&vec!["nums".into()]].values_indexed);
    assert!(!postings.fields[&vec!["score".into()]].values_indexed);
    assert!(postings.fields[&vec!["shape".into()]].structural.is_some());
    assert!(postings.fields[&vec!["objects".into()]].length_complete);
    assert!(postings.fields[&vec!["text".into()]].empty_string_complete);
    let mut wrong = postings.clone();
    wrong
        .add(
            RecordSlot(0),
            &Value::from_json(serde_json::json!({
                "nums": "not an array", "objects": 7, "text": []
            }))
            .unwrap(),
        )
        .unwrap();
    assert!(wrong.fields[&vec!["nums".into()]].lengths.is_empty());
    assert!(wrong.fields[&vec!["objects".into()]].lengths.is_empty());
    assert!(wrong.fields[&vec!["text".into()]].values.is_empty());
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
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let collection = &repo.collections()[0];
    let domain = &collection.logical_records;
    for filter in ["nums ?= 2", "text = \"Ω\"", "text != \"\""] {
        let BoundExpr::Term(term) = bind_filter(
            &parse_filter(filter).unwrap(),
            &collection.definition.schema,
        )
        .unwrap() else {
            panic!("term")
        };
        let index = &collection.postings.fields[&term.path.segments];
        assert!(
            index.estimate_exact(&term, domain.len()).is_none(),
            "{filter}"
        );
        assert!(index.exact_set(&term, domain).is_none(), "{filter}");
    }
    for (filter, exact) in [
        ("nums:length = 0", true),
        ("nums:length >= 2", true),
        ("objects:length > 0", true),
        ("objects:length = 3", true),
        ("text = \"\"", true),
        ("text = \"Ω\"", false),
        ("nums ?= 2", false),
        ("blocked:length = 0", false),
        ("blocked:array = true", true),
        ("score:number = true", true),
        ("score:number = false", true),
        ("score:null = true", true),
        ("shape:object = true", true),
        ("shape:object = false", true),
        ("shape:exists = false", true),
    ] {
        let request = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 50,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let optimized = repo.search(&request).unwrap();
        let reference = repo.search_reference(&request).unwrap();
        assert_eq!(optimized.count, reference.count, "{filter}");
        assert_eq!(optimized.hits, reference.hits, "{filter}");
        if exact {
            assert_eq!(optimized.counters.files_opened, 0, "{filter}");
            assert_eq!(optimized.counters.bytes_read, 0, "{filter}");
            assert_eq!(optimized.counters.record_parses, 0, "{filter}");
            assert_eq!(optimized.counters.scanned_records, 0, "{filter}");
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn indexed_field_state_matches_independent_source_oracle_without_verification() {
    use crate::executor::AccessPathKind;
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::reference::Schema;
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::{SourceFormat, SourceRegistry};
    use crate::storage::load_snapshot_checked;
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-state-posting-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, body) in [
        ("00", r#"{"u":"unindexed","legacy":[]}"#),
        ("01", r#"{"x":null,"u":"unindexed","legacy":[1]}"#),
        ("02", r#"{"x":"","u":"unindexed"}"#),
        ("03", r#"{"x":0,"u":"unindexed"}"#),
        ("04", r#"{"x":false,"u":"unindexed"}"#),
        ("05", r#"{"x":[],"u":"unindexed"}"#),
        ("06", r#"{"x":{},"u":"unindexed"}"#),
        ("07", r#"{"x":"hello","u":"unindexed"}"#),
        ("08", r#"{"x":[null,"hello"],"u":"unindexed"}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), body).unwrap();
    }
    let mut schema = Schema::default();
    for (name, kind, indexed) in [
        ("x", LogicalType::Unknown, true),
        (
            "legacy",
            LogicalType::Array(Box::new(LogicalType::Number)),
            false,
        ),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: true,
                indexed,
                multi: true,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let definition = CollectionDefinition {
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
    };
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        verified_cache_bytes: 0,
        ..Default::default()
    };
    let repo = Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
    assert!(
        repo.collections()[0].postings.fields[&vec!["x".into()]]
            .structural
            .is_some()
    );
    assert!(
        repo.collections()[0].postings.fields[&vec!["legacy".into()]]
            .structural
            .is_some()
    );
    assert!(
        !repo.collections()[0]
            .postings
            .fields
            .contains_key(&vec!["u".into()])
    );
    let request = |filter: &str| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 50,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    for (filter, expected) in [
        ("x:exists = false", 1),
        ("x:null = true", 1),
        ("x:? = false", 5),
        ("x:? != true", 5),
        ("x:string = true", 2),
        ("x:number = true", 1),
        ("x:array = true", 2),
        ("x:object = true", 1),
        ("x:object = false", 8),
    ] {
        let query = request(filter);
        let actual = repo.search(&query).unwrap();
        let reference = repo.search_reference(&query).unwrap();
        assert_eq!(actual.hits, reference.hits, "{filter}");
        assert_eq!(actual.count, Some(expected), "{filter}");
        assert_eq!(actual.counters.files_opened, 0, "{filter}");
        assert_eq!(actual.counters.record_parses, 0, "{filter}");
        assert!(
            repo.explain(&query).unwrap()[0]
                .terms
                .iter()
                .all(|term| term.selected.kind == AccessPathKind::Posting && term.selected.exact),
            "{filter}"
        );
    }
    for (filter, exact) in [
        ("u:string = true", false),
        ("legacy:array = true", false),
        ("legacy:length = 0", true),
    ] {
        let query = request(filter);
        let actual = repo.search(&query).unwrap();
        assert_eq!(
            actual.hits,
            repo.search_reference(&query).unwrap().hits,
            "{filter}"
        );
        if exact {
            assert_eq!(actual.counters.files_opened, 0, "{filter}");
        }
    }
    let snapshot = root.join("state.snapshot");
    repo.save_snapshot(&snapshot).unwrap();
    let restored = load_snapshot_checked(
        root.clone(),
        vec![definition],
        options,
        &SourceRegistry::builtin(),
        &snapshot,
    )
    .unwrap();
    for filter in [
        "x:null = true",
        "x:? = false",
        "x:string = true",
        "x:array = true",
    ] {
        let query = request(filter);
        assert_eq!(
            restored.search(&query).unwrap().hits,
            repo.search_reference(&query).unwrap().hits,
            "{filter}"
        );
        assert_eq!(
            restored.search(&query).unwrap().counters.files_opened,
            0,
            "{filter}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn mixed_selected_occurrences_have_independent_exact_state_facts() {
    use crate::query::parse_filter;
    use crate::reference::{BoundExpr, Schema, bind_filter};

    let field = FieldDefinition {
        path: vec!["x".into()],
        kind: LogicalType::Unknown,
        required: false,
        nullable: true,
        indexed: true,
        multi: true,
        element_nullable: Vec::new(),
    };
    let mut schema = Schema::default();
    schema.register_field(field.clone()).unwrap();
    let mut index = PostingIndex::structural(&field);
    let null = Value::Null;
    let text = Value::String("text".into());
    let empty = Value::String(String::new());
    let zero = Value::Number(Number::integer(0));
    let empty_array = Value::from_json(serde_json::json!([])).unwrap();
    let empty_object = Value::from_json(serde_json::json!({})).unwrap();
    let false_value = Value::Bool(false);
    for (slot, selected) in [
        Some(vec![&null, &text]),
        Some(vec![&empty, &zero]),
        Some(vec![]),
        Some(vec![&empty_array]),
        Some(vec![&empty_object]),
        Some(vec![&false_value]),
        Some(vec![&null]),
        None,
    ]
    .into_iter()
    .enumerate()
    {
        index
            .add_values(RecordSlot(slot as u32), selected.as_deref())
            .unwrap();
    }
    index.finish_stats();
    index.validate(8).unwrap();
    assert_eq!(index.nulls.iter().collect::<Vec<_>>(), [6]);
    assert_eq!(
        index.structural.as_ref().unwrap().postings[&StructuralKey::AnyNull]
            .iter()
            .collect::<Vec<_>>(),
        [0, 6]
    );
    assert_eq!(
        index.structural.as_ref().unwrap().postings[&StructuralKey::EmptyObject]
            .iter()
            .collect::<Vec<_>>(),
        [4]
    );
    let domain = RoaringBitmap::from_iter(0..8);
    let restricted = RoaringBitmap::from_iter([0, 2, 4, 7]);
    for (filter, expected) in [
        ("x:exists = false", vec![2, 7]),
        ("x:null = true", vec![0, 6]),
        ("x:null != true", vec![1, 2, 3, 4, 5, 7]),
        ("x:? = true", vec![0, 1, 5]),
        ("x:? = false", vec![2, 3, 4, 6, 7]),
        ("x:string = true", vec![0, 1]),
        ("x:number = true", vec![1]),
        ("x:array = true", vec![3]),
        ("x:object = true", vec![4]),
    ] {
        let BoundExpr::Term(term) = bind_filter(&parse_filter(filter).unwrap(), &schema).unwrap()
        else {
            panic!("expected term")
        };
        let actual = index.exact_set(&term, &domain).unwrap();
        assert_eq!(actual.iter().collect::<Vec<_>>(), expected, "{filter}");
        assert_eq!(
            index.estimate_exact(&term, domain.len()).unwrap().1,
            expected.len() as u64,
            "{filter}"
        );
        assert_eq!(
            index
                .exact_set_restricted(&term, &domain, &restricted)
                .unwrap(),
            &actual & &restricted,
            "{filter}"
        );
    }
}

#[test]
fn sparse_dynamic_keys_do_not_create_unselected_state_postings() {
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::reference::Schema;
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-state-sparse-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    for slot in 0..64 {
        std::fs::write(
            root.join(format!("data/{slot:02}.json")),
            format!(r#"{{"selected":{slot},"dynamic_{slot:02}":{slot}}}"#),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["selected".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
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
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let postings = &repo.collections()[0].postings.fields;
    assert_eq!(postings.len(), 1);
    assert!(postings[&vec!["selected".into()]].structural.is_some());
    let request = |filter: &str| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 64,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let indexed = repo.search(&request("selected:number = true")).unwrap();
    assert_eq!(indexed.count, Some(64));
    assert_eq!(indexed.counters.files_opened, 0);
    let unindexed = repo.search(&request("dynamic_17:number = true")).unwrap();
    assert_eq!(unindexed.count, Some(1));
    assert_eq!(unindexed.counters.files_opened, 64);
    assert_eq!(
        unindexed.hits,
        repo.search_reference(&request("dynamic_17:number = true"))
            .unwrap()
            .hits
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn structural_membership_tracks_file_replacement_move_delete_and_add() {
    use crate::lifecycle::Index;
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::reference::Schema;
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-state-mutation-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let a = root.join("data/a.json");
    let b = root.join("data/b.json");
    let moved = root.join("data/c.json");
    let added = root.join("data/d.json");
    std::fs::write(&a, br#"{"x":null,"shape":{}}"#).unwrap();
    std::fs::write(&b, br#"{"x":"hello","shape":null}"#).unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["x".into()],
            kind: LogicalType::Unknown,
            required: false,
            nullable: true,
            indexed: true,
            multi: true,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
        .register_field(FieldDefinition {
            path: vec!["shape".into()],
            kind: LogicalType::Object,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
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
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    let index = Index::from_repository(repo).unwrap();
    let initial = index.view().unwrap();
    let request = |filter: &str| Query {
        collections: vec!["data".into()],
        fts: None,
        filter: Some(parse_filter(filter).unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let check = |view: &crate::lifecycle::IndexView, expected: [usize; 6]| {
        for (filter, count) in [
            "x:null = true",
            "x:string = true",
            "x:array = true",
            "x:? = false",
            "shape:object = true",
            "shape:null = true",
        ]
        .into_iter()
        .zip(expected)
        {
            let found = view.search(&request(filter)).unwrap();
            assert_eq!(found.count, Some(count), "{filter}");
            assert_eq!(found.counters.files_opened, 0, "{filter}");
        }
    };
    check(&initial, [1, 1, 0, 1, 1, 1]);
    let a_handle = initial.file_handle_for_path(&a).unwrap();
    let b_handle = initial.file_handle_for_path(&b).unwrap();
    std::fs::write(&a, br#"{"x":[],"shape":null}"#).unwrap();
    let replaced = index.update_file(a_handle, &a).unwrap();
    check(&index.view().unwrap(), [0, 1, 1, 1, 0, 2]);
    check(&initial, [1, 1, 0, 1, 1, 1]);
    std::fs::rename(&b, &moved).unwrap();
    index.move_file(b_handle, &moved).unwrap();
    check(&index.view().unwrap(), [0, 1, 1, 1, 0, 2]);
    index.delete_file(replaced.file).unwrap();
    check(&index.view().unwrap(), [0, 1, 0, 0, 0, 1]);
    std::fs::write(&added, br#"{"x":0,"shape":{}}"#).unwrap();
    index.add_file(&added).unwrap();
    check(&index.view().unwrap(), [0, 1, 0, 0, 1, 1]);
    let number = index
        .view()
        .unwrap()
        .search(&request("x:number = true"))
        .unwrap();
    assert_eq!(number.count, Some(1));
    assert_eq!(number.counters.files_opened, 0);
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_mixed_values_match_source_oracle_for_state_composition() {
    use crate::query::{Query, QueryOptions, parse_filter};
    use crate::reference::Schema;
    use crate::repository::{
        CollectionDefinition, IdentityPolicy, Repository, RepositoryOptions, SnapshotMode,
    };
    use crate::source::SourceFormat;
    use crate::workload::ReturnMode;

    let root = std::env::temp_dir().join(format!("objekte-state-mixed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let values = [
        serde_json::json!(null),
        serde_json::json!(""),
        serde_json::json!("text"),
        serde_json::json!(0),
        serde_json::json!(17),
        serde_json::json!(false),
        serde_json::json!(true),
        serde_json::json!([]),
        serde_json::json!([null, "text"]),
        serde_json::json!({}),
        serde_json::json!({"nested": null}),
    ];
    let mut state = 0x8f2e_37a1_u64;
    for slot in 0..96 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let choice = (state % (values.len() as u64 + 1)) as usize;
        let record = if choice == values.len() {
            serde_json::json!({"y": slot % 3})
        } else {
            serde_json::json!({"x": values[choice], "y": slot % 3})
        };
        std::fs::write(
            root.join(format!("data/{slot:03}.json")),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["x".into()],
            kind: LogicalType::Unknown,
            required: false,
            nullable: true,
            indexed: true,
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
            verified_cache_bytes: 0,
            ..Default::default()
        },
    )
    .unwrap();
    for filter in [
        "x:exists = true",
        "x:exists = false",
        "x:null = true",
        "x:null = false",
        "x:? = true",
        "x:? = false",
        "x:string = true",
        "x:string = false",
        "x:number = true",
        "x:array = true",
        "x:object = true",
        "x:string = true && x:? = true",
        "x:null = true || x:array = true",
        "!(x:number = true) && x:exists = true",
    ] {
        let request = Query {
            collections: vec!["data".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 96,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        let indexed = repo.search(&request).unwrap();
        let source = repo.search_reference(&request).unwrap();
        assert_eq!(indexed.count, source.count, "{filter}");
        assert_eq!(indexed.hits, source.hits, "{filter}");
        assert_eq!(indexed.counters.files_opened, 0, "{filter}");
    }
    std::fs::remove_dir_all(root).unwrap();
}
