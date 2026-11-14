use super::*;
use crate::model::{ArrayValue, LogicalType, Number};
use crate::query::QueryOptions;
use crate::reference::{FieldDefinition, RegisteredBinding};
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::source::SourceFormat;
use crate::workload::ReturnMode;


#[derive(Debug)]
struct NullAsZero;


impl BindingOperation for NullAsZero {
    fn accepts(&self, _: &crate::model::LogicalType, _: crate::query::Operator, _: bool) -> bool {
        true
    }

    fn prepare_literal(
        &self,
        _: &crate::query::Literal,
        _: &crate::model::LogicalType,
    ) -> Result<Value, String> {
        Ok(Value::Null)
    }

    fn map_value(&self, value: &Value) -> Option<Value> {
        Some(if matches!(value, Value::Null) {
            Value::Number(Number::integer(0))
        } else {
            value.clone()
        })
    }

    fn map_text(&self, text: &str) -> String {
        text.into()
    }

    fn sort_compatible(&self) -> bool {
        true
    }
}


#[test]
fn min_max_transform_null_before_classifying_it() {
    let null = Value::Null;
    let two = Value::Number(Number::integer(2));
    let values = [&null, &two];
    for (reducer, expected) in [
        (SortReducer::Min, Number::integer(0)),
        (SortReducer::Max, Number::integer(2)),
        (SortReducer::First, Number::integer(0)),
        (SortReducer::Last, Number::integer(2)),
    ] {
        let mut sort = sorter("values[*]", Some(reducer));
        sort.transform = SortTransform::Operation(Arc::new(NullAsZero));
        assert_eq!(
            sort.reduce(&values, true).unwrap(),
            SortKey::Number(expected)
        );
    }
}

#[test]
fn cross_collection_sort_rejects_different_binding_versions() {
    let root = std::env::temp_dir().join(format!(
        "objekte-sort-binding-version-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    let mut definitions = Vec::new();
    for (name, version) in [("a", 1), ("b", 2)] {
        std::fs::create_dir_all(root.join(name)).unwrap();
        std::fs::write(root.join(name).join("record.json"), r#"{"title":"Alpha"}"#).unwrap();
        let mut definition = CollectionDefinition::folder(name, name, SourceFormat::Json);
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec!["title".into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: false,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
        definition
            .schema
            .register_operation(
                vec!["title".into()],
                RegisteredBinding {
                    name: "normalized".into(),
                    version,
                    operation: Arc::new(NullAsZero),
                },
            )
            .unwrap();
        definitions.push(definition);
    }
    let repo = Repository::open_with_options(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
    )
    .unwrap();
    let query = Query {
        collections: Vec::new(),
        filter: None,
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::computed(
                SortExpression {
                    path: "title".into(),
                    binding: Some("normalized".into()),
                    reducer: None,
                },
                false,
            )],
            limit: 10,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert!(
        repo.search(&query)
            .unwrap_err()
            .contains("incompatible cross-collection sort")
    );
    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

fn sorter(path: &str, reducer: Option<SortReducer>) -> ComputedSort {
    ComputedSort {
        path: QueryPath::parse(path).unwrap(),
        related: None,
        reducer,
        transform: SortTransform::Identity,
        collator: make_collator(&SortConfig {
            locale: "und".into(),
            options: SortOptions::default(),
        })
        .unwrap(),
    }
}

#[test]
fn explicit_positions_and_reductions_keep_occurrence_order() {
    let prices = Value::Array(
        ArrayValue::new(
            vec![
                Value::Number(Number::integer(3)),
                Value::Number(Number::integer(1)),
                Value::Number(Number::integer(3)),
            ],
            None,
            false,
        )
        .unwrap(),
    );
    let mut object = std::collections::BTreeMap::new();
    object.insert("prices".into(), prices);
    let value = Value::Object(object);
    let mut selected = Vec::new();
    let mut expanded = false;
    assert!(select_values(
        &value,
        &QueryPath::parse("prices[1]").unwrap().steps,
        &mut selected,
        &mut expanded
    ));
    assert_eq!(
        sorter("prices[1]", None)
            .reduce(&selected, expanded)
            .unwrap(),
        SortKey::Number(Number::integer(1))
    );
    selected.clear();
    assert!(select_values(
        &value,
        &QueryPath::parse("prices[*]").unwrap().steps,
        &mut selected,
        &mut expanded
    ));
    assert!(
        sorter("prices[*]", None)
            .reduce(&selected, expanded)
            .is_err()
    );
    assert_eq!(
        sorter("prices[*]", Some(SortReducer::Min))
            .reduce(&selected, expanded)
            .unwrap(),
        SortKey::Number(Number::integer(1))
    );
    assert_eq!(
        sorter("prices[*]", Some(SortReducer::Max))
            .reduce(&selected, expanded)
            .unwrap(),
        SortKey::Number(Number::integer(3))
    );
    assert_eq!(
        sorter("prices[*]", Some(SortReducer::First))
            .reduce(&selected, expanded)
            .unwrap(),
        SortKey::Number(Number::integer(3))
    );
    assert_eq!(
        sorter("prices[*]", Some(SortReducer::Last))
            .reduce(&selected, expanded)
            .unwrap(),
        SortKey::Number(Number::integer(3))
    );
}

#[test]
fn empty_ancestor_is_missing_but_nonempty_missing_child_is_absent() {
    let mut empty = std::collections::BTreeMap::new();
    empty.insert(
        "items".into(),
        Value::Array(ArrayValue::new(vec![], None, false).unwrap()),
    );
    let path = QueryPath::parse("items.title");
    let mut selected = Vec::new();
    let mut expanded = false;
    let value = Value::Object(empty);
    assert!(select_values(
        &value,
        &path.unwrap().steps,
        &mut selected,
        &mut expanded
    ));
    assert!(selected.is_empty());
    assert_eq!(
        sorter("items.title", Some(SortReducer::Min))
            .reduce(&selected, expanded)
            .unwrap(),
        SortKey::Missing
    );
}

#[test]
fn untyped_sort_names_remain_literal_and_typed_expressions_select_bindings() {
    let untyped = expression(&SortField::simple("x:lower", false)).unwrap();
    assert_eq!(untyped.path, "x:lower");
    assert_eq!(untyped.binding, None);

    let typed = expression(&SortField::computed(
        SortExpression {
            path: "x".into(),
            binding: Some("lower".into()),
            reducer: None,
        },
        false,
    ))
    .unwrap();
    assert_eq!(typed.path, "x");
    assert_eq!(typed.binding.as_deref(), Some("lower"));

    let legacy_positional = expression(&SortField::simple("items[0]:lower", false)).unwrap();
    assert_eq!(legacy_positional.path, "items[0]");
    assert_eq!(legacy_positional.binding.as_deref(), Some("lower"));

    let quoted = expression(&SortField::simple("[\"a:b\"]", false)).unwrap();
    assert_eq!(quoted.path, "[\"a:b\"]");
    assert_eq!(quoted.binding, None);
}

fn q4_field(schema: &mut crate::reference::Schema, name: &str, kind: LogicalType) {
    schema
        .register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: false,
            nullable: true,
            indexed: false,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
}

#[test]
fn related_sort_reuses_targets_across_worker_counts_and_source_layouts() {
    for split_files in [false, true] {
        for high_reuse in [false, true] {
            let root = std::env::temp_dir().join(format!(
                "objekte-sort-target-workers-{}-{split_files}-{high_reuse}",
                std::process::id()
            ));
            let source = root.join("sources");
            let target = root.join("targets");
            std::fs::create_dir_all(&source).unwrap();
            std::fs::create_dir_all(&target).unwrap();
            let target_count = if high_reuse { 4 } else { 47 };
            for index in 0..target_count {
                std::fs::write(
                    target.join(format!("{index:03}.json")),
                    format!(
                        "{{\"id\":\"T{index:03}\",\"title\":\"Title-{:03}\"}}",
                        (index * 13) % target_count
                    ),
                )
                .unwrap();
            }
            let rows = (0..48)
                .map(|index| {
                    let entry = if index == 47 {
                        "dangling".to_string()
                    } else {
                        format!("T{:03}", index % target_count)
                    };
                    format!("{{\"id\":\"S{index:03}\",\"entry\":\"{entry}\"}}\n")
                })
                .collect::<Vec<_>>();
            if split_files {
                for (index, row) in rows.iter().enumerate() {
                    std::fs::write(source.join(format!("{index:03}.jsonl")), row).unwrap();
                }
            } else {
                std::fs::write(source.join("all.jsonl"), rows.concat()).unwrap();
            }
            let mut source_definition =
                CollectionDefinition::folder("sources", "sources", SourceFormat::JsonLines);
            source_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
            q4_field(&mut source_definition.schema, "id", LogicalType::Id);
            q4_field(&mut source_definition.schema, "entry", LogicalType::Id);
            source_definition
                .schema
                .register_relation(RelationDefinition {
                    path: vec!["entry".into()],
                    target_collection: "targets".into(),
                    cardinality: RelationCardinality::One,
                    reverse_alias: None,
                })
                .unwrap();
            let mut target_definition =
                CollectionDefinition::folder("targets", "targets", SourceFormat::Json);
            target_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
            q4_field(&mut target_definition.schema, "id", LogicalType::Id);
            q4_field(&mut target_definition.schema, "title", LogicalType::String);
            let query = Query {
                collections: vec!["sources".into()],
                filter: None,
                fts: None,
                options: QueryOptions {
                    sort: vec![SortField::computed(
                        SortExpression {
                            path: "entry.title".into(),
                            binding: None,
                            reducer: None,
                        },
                        false,
                    )],
                    limit: 15,
                    offset: 5,
                    count: true,
                    return_mode: ReturnMode::Logical,
                    evidence: false,
                },
            };
            for workers in [1, 2, 4, 8] {
                let repo = Repository::open_with_options(
                    &root,
                    vec![source_definition.clone(), target_definition.clone()],
                    RepositoryOptions {
                        query_workers: workers,
                        ..Default::default()
                    },
                )
                .unwrap();
                let actual = repo.search(&query).unwrap();
                let expected = repo.search_reference(&query).unwrap();
                assert_eq!(actual.count, Some(48));
                assert_eq!(actual.hits, expected.hits);
                assert_eq!(actual.counters.sort_target_reads, target_count as u64);
                assert_eq!(
                    actual.counters.sort_target_reuse,
                    (47 - target_count) as u64
                );
            }
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}

#[test]
fn related_first_last_keep_occurrence_and_dangling_order_in_parallel() {
    let root =
        std::env::temp_dir().join(format!("objekte-sort-target-order-{}", std::process::id()));
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    for (id, title) in [("T1", "Alpha"), ("T2", "Zulu"), ("T3", "Beta")] {
        std::fs::write(
            root.join("targets").join(format!("{id}.json")),
            format!("{{\"id\":\"{id}\",\"title\":\"{title}\"}}"),
        )
        .unwrap();
    }
    let links = [
        "[\"T2\",\"T1\"]",
        "[\"dangling\",\"T1\"]",
        "[\"T1\",\"dangling\"]",
        "[\"T3\"]",
        "[]",
    ];
    for index in 0..20 {
        std::fs::write(
            root.join("sources").join(format!("{index:03}.jsonl")),
            format!(
                "{{\"id\":\"S{index:03}\",\"links\":{}}}\n",
                links[index % links.len()]
            ),
        )
        .unwrap();
    }
    let mut source_definition =
        CollectionDefinition::folder("sources", "sources", SourceFormat::JsonLines);
    source_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    q4_field(&mut source_definition.schema, "id", LogicalType::Id);
    source_definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["links".into()],
            kind: LogicalType::Array(Box::new(LogicalType::Id)),
            required: false,
            nullable: true,
            indexed: false,
            multi: true,
            element_nullable: vec![false],
        })
        .unwrap();
    source_definition
        .schema
        .register_relation(RelationDefinition {
            path: vec!["links".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::Many,
            reverse_alias: None,
        })
        .unwrap();
    let mut target_definition =
        CollectionDefinition::folder("targets", "targets", SourceFormat::Json);
    target_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    q4_field(&mut target_definition.schema, "id", LogicalType::Id);
    q4_field(&mut target_definition.schema, "title", LogicalType::String);
    for reducer in [SortReducer::First, SortReducer::Last] {
        let query = Query {
            collections: vec!["sources".into()],
            filter: None,
            fts: None,
            options: QueryOptions {
                sort: vec![SortField::computed(
                    SortExpression {
                        path: "links.title".into(),
                        binding: None,
                        reducer: Some(reducer),
                    },
                    false,
                )],
                limit: 12,
                offset: 3,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        for workers in [1, 2, 4, 8] {
            let repo = Repository::open_with_options(
                &root,
                vec![source_definition.clone(), target_definition.clone()],
                RepositoryOptions {
                    query_workers: workers,
                    ..Default::default()
                },
            )
            .unwrap();
            let actual = repo.search(&query).unwrap();
            assert_eq!(actual.count, Some(20));
            assert_eq!(actual.hits, repo.search_reference(&query).unwrap().hits);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}
