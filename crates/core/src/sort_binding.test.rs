use super::*;
use crate::model::{ArrayValue, Number};


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
