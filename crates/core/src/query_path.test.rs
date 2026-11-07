use super::*;

#[test]
fn explicit_root_paths_keep_zero_or_more_steps() {
    let root = QueryPath::parse("@root").unwrap();
    assert_eq!(root.field.root, PathRoot::Data);
    assert!(root.field.segments.is_empty());
    assert!(root.steps.is_empty());

    let position = QueryPath::parse("@root[0]").unwrap();
    assert_eq!(position.steps, [PathStep::ArrayIndex(0)]);
    assert!(position.has_position);

    let nested = QueryPath::parse("@root[*].title").unwrap();
    assert_eq!(
        nested.steps,
        [PathStep::EachElement, PathStep::Field("title".into())]
    );
    assert_eq!(nested.field.segments, ["title"]);
    assert_eq!(
        QueryPath::parse("@root.title").unwrap().field,
        QueryPath::parse("title").unwrap().field
    );
    assert_eq!(
        QueryPath::parse("@root[\"\"]").unwrap().field.segments,
        [""]
    );
}

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

#[test]
fn backlink_root_is_reserved_and_keeps_explicit_positions() {
    let path = QueryPath::parse("@backlinks.mentions[0].title").unwrap();
    assert_eq!(path.field.root, PathRoot::Backlinks);
    assert_eq!(path.field.segments, ["mentions", "title"]);
    assert_eq!(path.steps[0], PathStep::Field("mentions".into()));
    assert_eq!(path.steps[1], PathStep::ArrayIndex(0));
    assert!(path.has_position);
    assert_eq!(path.display, "@backlinks.mentions[0].title");
    let quoted = QueryPath::parse("['@backlinks'].mentions.title").unwrap();
    assert_eq!(quoted.field.root, PathRoot::Data);
    assert!(QueryPath::parse("@backlinks").is_err());
    assert!(QueryPath::parse("@backlinks.").is_err());
}

#[test]
fn binding_split_skips_quoted_colons_and_array_selectors() {
    for (input, path, binding) in [
        ("x:lower", "x", Some("lower")),
        ("[\"a:b\"]", "[\"a:b\"]", None),
        ("['a:b']", "['a:b']", None),
        (
            "items[0][\"a:b\"]:lower",
            "items[0][\"a:b\"]",
            Some("lower"),
        ),
        ("[\"a\\\"b:c\"]:lower", "[\"a\\\"b:c\"]", Some("lower")),
        ("[\"a]b:c\"]", "[\"a]b:c\"]", None),
        ("items[*].name", "items[*].name", None),
    ] {
        assert_eq!(split_path_binding(input), (path, binding), "{input}");
    }
}

#[test]
fn query_steps_and_schema_addresses_round_trip_through_their_own_rendering() {
    for input in [
        "@root",
        "@root[0]",
        "@root[*].title",
        "@file.status",
        "@backlinks.mentions[0].title",
        "['@file'].size",
        "['']",
        "items['0'].name",
        "items[0][\"a:b\"]",
        "items[*]['a b']",
        "['a>=b']",
        "['a\\'b']",
        "[\"a]b\"]",
    ] {
        let parsed = QueryPath::parse(input).unwrap();
        let again = QueryPath::parse(&parsed.display).unwrap();
        assert_eq!(again.field, parsed.field, "{input}");
        assert_eq!(again.steps, parsed.steps, "{input}");
        assert_eq!(
            FieldPath::parse(&parsed.field.display()).unwrap(),
            parsed.field,
            "{input}"
        );
    }
}
