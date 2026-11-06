use super::*;

#[test]
fn bulk_final_trees_match_incremental_merge_across_sparse_batches() {
    use crate::reference::Schema;
    use std::hash::{DefaultHasher, Hash, Hasher};

    let mut integer_hash = DefaultHasher::new();
    Number::Integer(2).hash(&mut integer_hash);
    let mut float_hash = DefaultHasher::new();
    Number::Float(2.0).hash(&mut float_hash);
    assert_eq!(Number::Integer(2), Number::Float(2.0));
    assert_eq!(integer_hash.finish(), float_hash.finish());

    let mut schema = Schema::default();
    for (name, kind, multi) in [
        (
            "numbers",
            LogicalType::Array(Box::new(LogicalType::Number)),
            true,
        ),
        ("label", LogicalType::String, false),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: true,
                indexed: true,
                multi,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let mut first = PostingCollection::new(&schema);
    for (slot, record) in [
        serde_json::json!({"numbers":[2,2,3],"label":"repeat"}),
        serde_json::json!({"numbers":null,"label":null}),
        serde_json::json!({}),
    ]
    .into_iter()
    .enumerate()
    {
        first
            .add(RecordSlot(slot as u32), &Value::from_json(record).unwrap())
            .unwrap();
    }
    let mut second = PostingCollection::new(&schema);
    second
        .add(
            RecordSlot(0),
            &Value::from_json(serde_json::json!({"numbers":[2,4],"label":"repeat"})).unwrap(),
        )
        .unwrap();
    let numeric = second.fields.get_mut(&vec!["numbers".into()]).unwrap();
    let duplicate = numeric
        .values
        .remove(&ScalarKey::Number(Number::Integer(2)))
        .unwrap();
    numeric
        .values
        .insert(ScalarKey::Number(Number::Float(2.0)), duplicate);
    let length = numeric.lengths.remove(&Number::Integer(2)).unwrap();
    numeric.lengths.insert(Number::Float(2.0), length);
    let mut third = PostingCollection::new(&schema);
    third
        .add(
            RecordSlot(0),
            &Value::from_json(serde_json::json!({"numbers":[5],"label":"unique"})).unwrap(),
        )
        .unwrap();

    let mut incremental = PostingCollection::new(&schema);
    let mut bulk = PostingCollectionBuilder::new(&schema);
    let mut custom =
        CustomPostingIndexBuilder::new(schema.fields.get(&vec!["numbers".into()]).unwrap(), true);
    for (base, batch, complete) in [
        (0, first, true),
        (3, PostingCollection::new(&schema), true), // empty physical batch
        (4, PostingCollection::new(&schema), true), // fallback-only batch
        (65_536, second, false),
        (65_537, third, true),
    ] {
        let custom_index = batch.fields[&vec!["numbers".into()]].clone();
        custom
            .merge_shifted(
                CustomPostingIndex {
                    index: custom_index,
                    complete,
                },
                base,
            )
            .unwrap();
        incremental.merge_shifted(batch.clone(), base).unwrap();
        bulk.merge_shifted(batch, base).unwrap();
    }
    let mut final_bulk = bulk.finish();
    let final_custom = custom.finish();
    incremental.finish_stats();
    final_bulk.finish_stats();
    let allowed = RoaringBitmap::from_iter([0, 1, 2, 65_536, 65_537]);
    for collection in [&incremental, &final_bulk] {
        collection.validate_eligible(65_538, 5).unwrap();
        collection.validate_domain(&allowed).unwrap();
    }
    for (path, old) in &incremental.fields {
        let new = &final_bulk.fields[path];
        assert_eq!(old.path, new.path);
        assert_eq!(old.kind, new.kind);
        assert_eq!(old.multi, new.multi);
        assert_eq!(old.presence, new.presence);
        assert_eq!(old.nulls, new.nulls);
        assert_eq!(old.values, new.values);
        assert_eq!(old.lengths, new.lengths);
        assert_eq!(old.covered_records, new.covered_records);
        assert_eq!(old.stats.present_count, new.stats.present_count);
        assert_eq!(old.stats.null_count, new.stats.null_count);
        assert_eq!(old.stats.distinct_count, new.stats.distinct_count);
        assert_eq!(old.stats.min_key, new.stats.min_key);
        assert_eq!(old.stats.max_key, new.stats.max_key);
    }
    let numbers = &final_bulk.fields[&vec!["numbers".into()]];
    assert_eq!(numbers.values.len(), 4);
    assert_eq!(
        numbers.values[&ScalarKey::Number(Number::Integer(2))]
            .iter()
            .collect::<Vec<_>>(),
        [0, 65_536]
    );
    assert_eq!(
        numbers.lengths[&Number::Integer(2)]
            .iter()
            .collect::<Vec<_>>(),
        [65_536]
    );
    assert_eq!(numbers.nulls.iter().collect::<Vec<_>>(), [1]);
    assert!(!numbers.presence.contains(2));
    assert_eq!(final_custom.index.values, numbers.values);
    assert!(!final_custom.complete);
    let ordered = numbers.values.keys().cloned().collect::<Vec<_>>();
    assert_eq!(
        ordered,
        vec![
            ScalarKey::Number(Number::Integer(2)),
            ScalarKey::Number(Number::Integer(3)),
            ScalarKey::Number(Number::Integer(4)),
            ScalarKey::Number(Number::Integer(5)),
        ]
    );
    let old_numbers = &incremental.fields[&vec!["numbers".into()]];
    let key = ScalarKey::Number(Number::Integer(3));
    let (old_range, _) = range_postings(&old_numbers.values, &key, Operator::Gt, None);
    let (new_range, _) = range_postings(&numbers.values, &key, Operator::Gt, None);
    assert_eq!(old_range, new_range);
    assert_eq!(new_range.iter().collect::<Vec<_>>(), [65_536, 65_537]);
    let ordered_facets = |index: &PostingIndex| {
        index
            .values
            .iter()
            .map(|(value, slots)| (value.clone(), slots.len()))
            .collect::<Vec<_>>()
    };
    assert_eq!(ordered_facets(old_numbers), ordered_facets(numbers));
    assert_eq!(ordered_facets(numbers)[0].1, 2);
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
        let repo = Repository::open_with_options(
            &root,
            vec![definition.clone()],
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
        if indexed {
            let options = RepositoryOptions {
                snapshot_mode: SnapshotMode::ReadWrite,
                ..Default::default()
            };
            let built =
                Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
            let restored = Repository::open_with_options(&root, vec![definition], options).unwrap();
            assert!(restored.startup().restored);
            let path = vec!["tags".into()];
            let fresh = &built.collections()[0].postings.fields[&path];
            let loaded = &restored.collections()[0].postings.fields[&path];
            assert_eq!(fresh.values, loaded.values);
            assert_eq!(fresh.lengths, loaded.lengths);
            assert_eq!(fresh.presence, loaded.presence);
            assert_eq!(fresh.nulls, loaded.nulls);
            let query = Query {
                collections: vec!["data".into()],
                fts: None,
                filter: Some(parse_filter("tags ?= 'a'").unwrap()),
                options: QueryOptions {
                    sort: vec![],
                    limit: 20,
                    offset: 0,
                    count: true,
                    return_mode: ReturnMode::Locator,
                    evidence: false,
                },
            };
            assert_eq!(
                built.search(&query).unwrap().hits,
                restored.search(&query).unwrap().hits
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
