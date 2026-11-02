use super::*;

#[test]
fn unmodified_fields_and_literals_borrow_large_values() {
    let document = serde_json::json!({"payload": {"text": "x".repeat(256 * 1024)}});
    let field = FieldOperand {
        path: vec![JsonPathStep::Key("payload".into())],
        modifier: None,
    };
    let selected = field.select(&document);
    assert_eq!(selected.len(), 1);
    assert!(std::ptr::eq(selected[0].value(), &document["payload"]));

    let literal = Operand::Literal(serde_json::json!({"text": "y".repeat(256 * 1024)}));
    let selected = literal.select(&document);
    let Operand::Literal(value) = &literal else {
        unreachable!()
    };
    assert!(std::ptr::eq(selected[0].value(), value));
}

#[test]
fn cardinality_counts_only_values_eligible_for_a_modifier() {
    let document = serde_json::json!({"items": [1, "A", false, "B"]});
    let lower = FieldOperand {
        path: vec![JsonPathStep::Key("items".into())],
        modifier: Some(Modifier::Lower),
    };
    assert!(lower.more_than_one(&document));
    let one = serde_json::json!({"items": [1, "A", false]});
    assert!(!lower.more_than_one(&one));

    let length = FieldOperand {
        path: vec![JsonPathStep::Key("items".into())],
        modifier: Some(Modifier::Length),
    };
    assert!(!length.more_than_one(&document));
    assert_eq!(length.select(&document)[0].value(), &Value::from(4));
}

#[test]
fn later_branch_cardinality_is_checked_before_short_circuit() {
    let filter = JsonFilter::compile("name = 'match' || items = 'A'", &BTreeMap::new()).unwrap();
    let document = serde_json::json!({"name": "match", "items": ["A", "B"]});
    assert!(matches!(filter.matches(&document), Err(Error::Request(_))));
}
