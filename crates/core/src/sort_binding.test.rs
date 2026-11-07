use super::*;
use crate::model::{ArrayValue, LogicalType, Number};
use crate::query::QueryOptions;
use crate::reference::{FieldDefinition, RegisteredBinding};
use crate::repository::{CollectionDefinition, RepositoryOptions, SnapshotMode};
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
            snapshot_mode: SnapshotMode::Disabled,
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
