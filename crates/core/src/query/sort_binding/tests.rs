use super::*;
use crate::execution_options::ReturnMode;
use crate::index::sorting::Placement;
use crate::model::{ArrayValue, LogicalType, Number};
use crate::query::QueryOptions;
use crate::relation::{RelationCardinality, RelationDefinition};
use crate::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use crate::schema::{FieldDefinition, operation::RegisteredBinding};
use crate::source::SourceFormat;

fn stored_string_sort_definition(
    name: &str,
    defaults: SortOptions,
    effective: Option<SortConfig>,
) -> CollectionDefinition {
    let mut definition = CollectionDefinition::folder(name, name, SourceFormat::Json);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    definition.sort_locale = "en".into();
    definition.sort_options = defaults;
    for (path, kind, nullable) in [
        ("id", LogicalType::Id, false),
        ("title", LogicalType::String, true),
    ] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: !nullable,
                nullable,
                indexed: false,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    let path = QueryPath::parse("title").unwrap().field;
    definition.sort_fields.push(path.clone());
    if let Some(effective) = effective {
        definition.sort_overrides.insert(path, effective);
    }
    definition
}

fn stored_title_query(collections: &[&str]) -> Query {
    Query {
        collections: collections.iter().map(|name| (*name).into()).collect(),
        filter: None,
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::simple("title", false)],
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

fn hit_ids(result: &crate::repository::SearchResult) -> Vec<&str> {
    result
        .hits
        .iter()
        .map(|hit| hit.record_id.as_str())
        .collect()
}

#[test]
fn reference_stored_sort_uses_numeric_and_null_override() {
    let root = std::env::temp_dir().join(format!(
        "objekte-sort-binding-collation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, document) in [
        ("ten", r#"{"id":"ten","title":"item10"}"#),
        ("two", r#"{"id":"two","title":"item2"}"#),
        ("null", r#"{"id":"null","title":null}"#),
    ] {
        std::fs::write(root.join(format!("data/{name}.json")), document).unwrap();
    }
    let defaults = SortOptions {
        numeric: false,
        nulls: Placement::Last,
        missing: Placement::Last,
        ..Default::default()
    };
    let effective = SortConfig {
        locale: "en".into(),
        options: SortOptions {
            numeric: true,
            nulls: Placement::First,
            missing: Placement::First,
            ..Default::default()
        },
    };
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![stored_string_sort_definition(
            "data",
            defaults,
            Some(effective),
        )],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let query = stored_title_query(&["data"]);

    let optimized = repo.search(&query).unwrap();
    let reference = repo.search_reference(&query).unwrap();
    assert_eq!(hit_ids(&optimized), ["null", "two", "ten"]);
    assert_eq!(hit_ids(&reference), hit_ids(&optimized));
    assert_eq!(optimized.count, Some(3));
    assert_eq!(reference.count, optimized.count);

    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn cross_collection_sort_uses_effective_override_compatibility() {
    let root = std::env::temp_dir().join(format!(
        "objekte-sort-binding-cross-collation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    for collection in ["a", "b"] {
        std::fs::create_dir_all(root.join(collection)).unwrap();
        std::fs::write(
            root.join(collection).join("ten.json"),
            format!(r#"{{"id":"{collection}10","title":"item10"}}"#),
        )
        .unwrap();
        std::fs::write(
            root.join(collection).join("two.json"),
            format!(r#"{{"id":"{collection}2","title":"item2"}}"#),
        )
        .unwrap();
    }
    let defaults_a = SortOptions {
        numeric: false,
        nulls: Placement::Last,
        missing: Placement::Last,
        ..Default::default()
    };
    let defaults_b = SortOptions {
        numeric: false,
        nulls: Placement::First,
        missing: Placement::First,
        ..Default::default()
    };
    let numeric = SortConfig {
        locale: "en".into(),
        options: SortOptions {
            numeric: true,
            nulls: Placement::First,
            missing: Placement::First,
            ..Default::default()
        },
    };
    let equal = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![
            stored_string_sort_definition("a", defaults_a, Some(numeric.clone())),
            stored_string_sort_definition("b", defaults_b, Some(numeric.clone())),
        ],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let query = stored_title_query(&["a", "b"]);
    let optimized = equal.search(&query).unwrap();
    let reference = equal.search_reference(&query).unwrap();
    assert_eq!(hit_ids(&optimized), ["a2", "b2", "a10", "b10"]);
    assert_eq!(hit_ids(&reference), hit_ids(&optimized));
    drop(equal);

    let nonnumeric = SortConfig {
        locale: "en".into(),
        options: defaults_a,
    };
    let differing = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![
            stored_string_sort_definition("a", defaults_a, Some(numeric)),
            stored_string_sort_definition("b", defaults_a, Some(nonnumeric)),
        ],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    assert!(
        differing
            .search(&query)
            .unwrap_err()
            .contains("incompatible cross-collection sort")
    );
    assert!(
        differing
            .search_reference(&query)
            .unwrap_err()
            .contains("incompatible cross-collection sort")
    );

    drop(differing);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn optimized_binding_defers_reference_sort_preparation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-sort-binding-reference-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), r#"{"title":"Alpha"}"#).unwrap();
    let mut definition = CollectionDefinition::folder("data", "data", SourceFormat::Json);
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
        .sort_fields
        .push(QueryPath::parse("title").unwrap().field);
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let query = Query {
        collections: vec!["data".into()],
        filter: None,
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::simple("title", false)],
            limit: 10,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };

    let before = BoundSort::reference_stored_sort_preparations();
    let optimized = BoundSort::bind(&repo, &query).unwrap();
    assert_eq!(
        BoundSort::reference_stored_sort_preparations(),
        before,
        "optimized stored ordering must not prepare a reference collator"
    );
    assert!(matches!(
        optimized.terms[0].as_ref().unwrap()[0].source,
        BoundSortSource::Stored(_)
    ));

    let reference = BoundSort::bind_reference(&repo, &query).unwrap();
    assert_eq!(
        BoundSort::reference_stored_sort_preparations(),
        before + 1,
        "reference binding prepares one source evaluator"
    );
    assert!(matches!(
        reference.terms[0].as_ref().unwrap()[0].source,
        BoundSortSource::Computed(_)
    ));

    drop(repo);
    std::fs::remove_dir_all(root).unwrap();
}

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
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        definitions,
        RepositoryOptions {
            ..Default::default()
        },
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
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
    let sort_config = SortConfig {
        locale: "und".into(),
        options: SortOptions::default(),
    };
    ComputedSort {
        path: QueryPath::parse(path).unwrap(),
        related: None,
        reducer,
        transform: SortTransform::Identity,
        collator: make_collator(&sort_config).unwrap(),
        sort_config,
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

fn q4_field(schema: &mut crate::schema::Schema, name: &str, kind: LogicalType) {
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
                let repo = crate::lifecycle::Index::build_repository_for_test(
                    &root,
                    vec![source_definition.clone(), target_definition.clone()],
                    RepositoryOptions {
                        query_workers: workers,
                        ..Default::default()
                    },
                    crate::source::registry::SourceRegistry::builtin(),
                )
                .map_err(|error| error.to_string())
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
fn related_stored_sort_uses_the_target_fields_effective_collation() {
    let root = std::env::temp_dir().join(format!(
        "objekte-related-target-collation-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("sources")).unwrap();
    std::fs::create_dir_all(root.join("targets")).unwrap();
    for (id, title) in [("T2", "item2"), ("T10", "item10")] {
        std::fs::write(
            root.join("targets").join(format!("{id}.json")),
            format!(r#"{{"id":"{id}","title":"{title}"}}"#),
        )
        .unwrap();
    }
    for (id, target) in [("S2", "T2"), ("S10", "T10")] {
        std::fs::write(
            root.join("sources").join(format!("{id}.json")),
            format!(r#"{{"id":"{id}","entry":"{target}"}}"#),
        )
        .unwrap();
    }

    let nonnumeric = SortOptions {
        numeric: false,
        ..Default::default()
    };
    let numeric = SortConfig {
        locale: "en".into(),
        options: SortOptions {
            numeric: true,
            ..Default::default()
        },
    };
    let target = stored_string_sort_definition("targets", nonnumeric, Some(numeric.clone()));
    let mut source = CollectionDefinition::folder("sources", "sources", SourceFormat::Json);
    source.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    for (path, kind) in [("id", LogicalType::Id), ("entry", LogicalType::Id)] {
        source
            .schema
            .register_field(FieldDefinition {
                path: vec![path.into()],
                kind,
                required: true,
                nullable: false,
                indexed: false,
                multi: false,
                element_nullable: vec![],
            })
            .unwrap();
    }
    source
        .schema
        .register_relation(RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "targets".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![source.clone(), target],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let request = |limit| Query {
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
            limit,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let bound = BoundSort::bind_reference(&repo, &request(1)).unwrap();
    let BoundSortSource::Computed(computed) = &bound.terms[0].as_ref().unwrap()[0].source else {
        panic!("related sort must use a computed source evaluator");
    };
    assert!(!computed.sort_config.options.numeric);
    let key_two = computed.scalar_key(&Value::String("item2".into())).unwrap();
    let key_ten = computed
        .scalar_key(&Value::String("item10".into()))
        .unwrap();
    assert!(matches!(
        (key_two, key_ten),
        (SortKey::String(two), SortKey::String(ten)) if ten < two
    ));
    let direct = stored_title_query(&["targets"]);
    assert_eq!(hit_ids(&repo.search(&direct).unwrap()), ["T2", "T10"]);
    for limit in [1, 20] {
        let expected = if limit == 1 {
            vec!["S10"]
        } else {
            vec!["S10", "S2"]
        };
        let optimized = repo.search(&request(limit)).unwrap();
        let reference = repo.search_reference(&request(limit)).unwrap();
        assert_eq!(optimized.count, Some(2));
        assert_eq!(reference.count, Some(2));
        assert_eq!(
            hit_ids(&optimized),
            expected,
            "reference returned {:?}",
            hit_ids(&reference)
        );
        assert_eq!(hit_ids(&reference), expected);
    }

    let optimized = repo.search(&direct).unwrap();
    let reference = repo.search_reference(&direct).unwrap();
    assert_eq!(hit_ids(&optimized), ["T2", "T10"]);
    assert_eq!(hit_ids(&reference), ["T2", "T10"]);
    assert_eq!(optimized.count, Some(2));
    assert_eq!(reference.count, Some(2));

    assert_eq!(
        repo.search(&request(20))
            .unwrap()
            .counters
            .sort_target_reads,
        2,
        "an incompatible stored key must fall back to computed target evaluation"
    );
    drop(repo);

    let compatible = stored_string_sort_definition("targets", numeric.options, None);
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &root,
        vec![source, compatible],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let optimized = repo.search(&request(20)).unwrap();
    let reference = repo.search_reference(&request(20)).unwrap();
    assert_eq!(hit_ids(&optimized), ["S2", "S10"]);
    assert_eq!(hit_ids(&reference), ["S2", "S10"]);
    assert_eq!(
        optimized.counters.sort_target_reads, 0,
        "compatible related stored keys must not read target records"
    );
    assert!(reference.counters.sort_target_reads > 0);

    std::fs::remove_dir_all(root).unwrap();
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
            let repo = crate::lifecycle::Index::build_repository_for_test(
                &root,
                vec![source_definition.clone(), target_definition.clone()],
                RepositoryOptions {
                    query_workers: workers,
                    ..Default::default()
                },
                crate::source::registry::SourceRegistry::builtin(),
            )
            .map_err(|error| error.to_string())
            .unwrap();
            let actual = repo.search(&query).unwrap();
            assert_eq!(actual.count, Some(20));
            assert_eq!(actual.hits, repo.search_reference(&query).unwrap().hits);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn reference_forward_sort_reuses_sources_and_accounts_actual_reads() {
    fn source_definition(path: &str, relation: Option<&str>) -> CollectionDefinition {
        let mut definition = CollectionDefinition::folder(path, path, SourceFormat::Json);
        definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        q4_field(&mut definition.schema, "id", LogicalType::Id);
        if let Some(target) = relation {
            q4_field(&mut definition.schema, "entry", LogicalType::Id);
            definition
                .schema
                .register_relation(RelationDefinition {
                    path: vec!["entry".into()],
                    target_collection: target.into(),
                    cardinality: RelationCardinality::One,
                    reverse_alias: None,
                })
                .unwrap();
        }
        definition
    }

    fn target_definition(path: &str) -> CollectionDefinition {
        let mut definition = CollectionDefinition::folder(path, path, SourceFormat::Json);
        definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        q4_field(&mut definition.schema, "id", LogicalType::Id);
        q4_field(&mut definition.schema, "title", LogicalType::String);
        definition
    }

    fn sort_query(path: &str) -> Query {
        Query {
            collections: vec!["roots".into()],
            filter: None,
            fts: None,
            options: QueryOptions {
                sort: vec![SortField::computed(
                    SortExpression {
                        path: path.into(),
                        binding: None,
                        reducer: None,
                    },
                    false,
                )],
                limit: 20,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        }
    }

    let one_hop_root =
        std::env::temp_dir().join(format!("objekte-forward-source-{}", std::process::id()));
    for directory in ["roots", "targets"] {
        std::fs::create_dir_all(one_hop_root.join(directory)).unwrap();
    }
    for (relative, document) in [
        ("roots/r1.json", r#"{"id":"R1","entry":"T1"}"#),
        ("roots/r2.json", r#"{"id":"R2","entry":"T2"}"#),
        ("targets/t1.json", r#"{"id":"T1","title":"Alpha"}"#),
        ("targets/t2.json", r#"{"id":"T2","title":"Zulu"}"#),
    ] {
        std::fs::write(one_hop_root.join(relative), document).unwrap();
    }
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &one_hop_root,
        vec![
            source_definition("roots", Some("targets")),
            target_definition("targets"),
        ],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let observations = crate::source::access::arm_search_read_observations();
    let result = repo.search_reference(&sort_query("entry.title")).unwrap();
    assert_eq!(hit_ids(&result), ["R1", "R2"], "{:?}", result.counters);
    assert_eq!(result.count, Some(2));
    assert_eq!(result.counters.relation_source_reads, 0);
    assert_eq!(result.counters.sort_target_reads, 2);
    assert_eq!(result.counters.files_opened, 4);
    assert_eq!(result.counters.record_parses, 4);
    let observations = observations.finish();
    assert_eq!(observations.len(), 2);
    assert!(observations.iter().all(|(path, _, _)| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with('t'))
    }));
    let one_hop_bytes = [
        "roots/r1.json",
        "roots/r2.json",
        "targets/t1.json",
        "targets/t2.json",
    ]
    .into_iter()
    .map(|path| std::fs::metadata(one_hop_root.join(path)).unwrap().len())
    .sum::<u64>();
    assert_eq!(result.counters.bytes_read, one_hop_bytes);
    drop(repo);
    std::fs::remove_dir_all(one_hop_root).unwrap();

    let two_hop_root =
        std::env::temp_dir().join(format!("objekte-forward-source-two-{}", std::process::id()));
    for directory in ["roots", "intermediates", "targets"] {
        std::fs::create_dir_all(two_hop_root.join(directory)).unwrap();
    }
    for (relative, document) in [
        ("roots/r1.json", r#"{"id":"R1","entry":"I"}"#),
        ("roots/r2.json", r#"{"id":"R2","entry":"I"}"#),
        ("intermediates/i.json", r#"{"id":"I","entry":"T"}"#),
        ("targets/t.json", r#"{"id":"T","title":"Tie"}"#),
    ] {
        std::fs::write(two_hop_root.join(relative), document).unwrap();
    }
    let intermediate = source_definition("intermediates", Some("targets"));
    let repo = crate::lifecycle::Index::build_repository_for_test(
        &two_hop_root,
        vec![
            source_definition("roots", Some("intermediates")),
            intermediate,
            target_definition("targets"),
        ],
        RepositoryOptions::default(),
        crate::source::registry::SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())
    .unwrap();
    let observations = crate::source::access::arm_search_read_observations();
    let result = repo
        .search_reference(&sort_query("entry.entry.title"))
        .unwrap();
    assert_eq!(hit_ids(&result), ["R1", "R2"]);
    assert_eq!(result.count, Some(2));
    assert_eq!(result.counters.relation_source_reads, 1);
    assert_eq!(result.counters.sort_target_reads, 1);
    assert_eq!(result.counters.sort_target_reuse, 1);
    assert_eq!(result.counters.files_opened, 4);
    assert_eq!(result.counters.record_parses, 4);
    let observations = observations.finish();
    let observed_names = observations
        .iter()
        .map(|(path, _, _)| path.file_name().unwrap().to_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(observed_names, ["i.json", "t.json"]);
    let two_hop_bytes = [
        "roots/r1.json",
        "roots/r2.json",
        "intermediates/i.json",
        "targets/t.json",
    ]
    .into_iter()
    .map(|path| std::fs::metadata(two_hop_root.join(path)).unwrap().len())
    .sum::<u64>();
    assert_eq!(result.counters.bytes_read, two_hop_bytes);

    let intermediate_path = two_hop_root.join("intermediates/i.json");
    let target_path = two_hop_root.join("targets/t.json");
    let intermediate_bytes = std::fs::read(&intermediate_path).unwrap();
    let target_bytes = std::fs::read(&target_path).unwrap();
    std::fs::remove_file(&target_path).unwrap();
    let terminal_missing = repo
        .search_reference(&sort_query("entry.entry.title"))
        .unwrap();
    assert!(terminal_missing.hits.is_empty());
    assert_eq!(terminal_missing.count, None);
    assert!(!terminal_missing.counters.match_count_complete);
    assert!(
        terminal_missing
            .counters
            .source_diagnostics
            .iter()
            .any(|diagnostic| { diagnostic.file_path == std::path::Path::new("targets/t.json") })
    );
    std::fs::write(&target_path, &target_bytes).unwrap();

    std::fs::remove_file(&intermediate_path).unwrap();
    let intermediate_missing = repo
        .search_reference(&sort_query("entry.entry.title"))
        .unwrap();
    assert!(intermediate_missing.hits.is_empty());
    assert_eq!(intermediate_missing.count, None);
    assert!(!intermediate_missing.counters.match_count_complete);
    assert!(
        intermediate_missing
            .counters
            .source_diagnostics
            .iter()
            .any(|diagnostic| {
                diagnostic.file_path == std::path::Path::new("intermediates/i.json")
            })
    );
    std::fs::write(&intermediate_path, intermediate_bytes).unwrap();

    drop(repo);
    std::fs::remove_dir_all(two_hop_root).unwrap();
}
