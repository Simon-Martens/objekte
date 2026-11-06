use super::*;

#[test]
fn explicit_array_steps_are_distinct_from_quoted_object_keys() {
    let path = QueryPath::parse("items[0].name").unwrap();
    assert_eq!(path.field.segments, ["items", "name"]);
    assert_eq!(
        path.steps,
        [
            PathStep::Field("items".into()),
            PathStep::ArrayIndex(0),
            PathStep::Field("name".into()),
        ]
    );
    assert!(path.has_position);
    let key = QueryPath::parse("items['0'].name").unwrap();
    assert_eq!(key.field.segments, ["items", "0", "name"]);
    assert_eq!(key.steps[1], PathStep::Field("0".into()));
    assert!(!key.has_position);
    let nested = QueryPath::parse("matrix[1][2]").unwrap();
    assert_eq!(
        nested.steps,
        [
            PathStep::Field("matrix".into()),
            PathStep::ArrayIndex(1),
            PathStep::ArrayIndex(2),
        ]
    );
    let each = QueryPath::parse("items[*].price").unwrap();
    assert_eq!(each.steps[1], PathStep::EachElement);
    assert!(each.has_explicit_array_step);
    assert!(!each.has_position);
    assert_eq!(
        QueryPath::parse("['literal.key'][0]")
            .unwrap()
            .field
            .segments,
        ["literal.key"]
    );
}

#[test]
fn malformed_or_overflowing_selectors_fail_cleanly() {
    for path in [
        "items[-1]",
        "items[1.5]",
        "items[4294967296]",
        "items[",
        "items[]",
        "items[1]tail",
        "items.",
        ".items",
        "[0]",
        "items[abc]",
    ] {
        assert!(QueryPath::parse(path).is_err(), "{path}");
    }
}
