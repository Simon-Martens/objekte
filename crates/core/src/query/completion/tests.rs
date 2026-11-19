use super::*;

#[test]
fn detects_partial_field_and_value_without_valid_whole_expression() {
    assert_eq!(
        context("na >=", 2).unwrap().target,
        CompletionTarget::Field { sort: false }
    );
    assert_eq!(
        context("year >= 19", 10).unwrap().target,
        CompletionTarget::Value {
            field: "year".into()
        }
    );
    assert_eq!(context("year >= \"Hei", 12).unwrap().replace, 8..12);
}

#[test]
fn completes_boolean_field_tests_without_treating_quoted_colons_as_modifiers() {
    let modifier = context("annotation:st", "annotation:st".len()).unwrap();
    assert_eq!(modifier.target, CompletionTarget::Modifier);
    assert_eq!(modifier.replace, 11..13);
    assert_eq!(modifier.prefix, "st");
    assert_eq!(
        insert("annotation:st", modifier.replace, "string")
            .unwrap()
            .0,
        "annotation:string"
    );
    assert_eq!(
        context("annotation:? = tr", "annotation:? = tr".len())
            .unwrap()
            .target,
        CompletionTarget::BooleanValue
    );
    assert_eq!(
        context("[\"a:b\"]:nu", "[\"a:b\"]:nu".len())
            .unwrap()
            .target,
        CompletionTarget::Modifier
    );
    assert_eq!(
        context("[\"a:b\"] = tr", "[\"a:b\"] = tr".len())
            .unwrap()
            .target,
        CompletionTarget::Value {
            field: "[\"a:b\"]".into()
        }
    );
}

#[test]
fn insertion_keeps_surrounding_expression_and_unicode_offsets() {
    let text = "name = \"Häi\" && age > 1";
    let position = text.find("Häi").unwrap() + "Hä".len();
    let completion = context(text, position).unwrap();
    assert_eq!(
        completion.target,
        CompletionTarget::Value {
            field: "name".into()
        }
    );
    let (result, _) = insert(text, completion.replace, "\"Heidelberg\"").unwrap();
    assert_eq!(result, "name = \"Heidelberg\" && age > 1");
}

#[test]
fn quoted_nested_path_replaces_one_complete_token() {
    let text = "foo[\"ba\"] = 1";
    let at = text.find("ba").unwrap() + 2;
    let context = context(text, at).unwrap();
    assert_eq!(context.replace, 0..9);
    assert_eq!(context.prefix, "foo[\"ba");
    assert_eq!(context.target, CompletionTarget::Field { sort: false });
    assert_eq!(
        insert(text, context.replace, "foo[\"bar\"]").unwrap().0,
        "foo[\"bar\"] = 1"
    );
}

#[test]
fn offers_fields_only_where_a_field_can_begin() {
    for text in [
        "name ",
        "name:lower ",
        "name = 'Ada' ",
        "name = 'Ada' SORT ",
        "!",
        "name = 'Ada' !",
        "name = 'Ada' (",
        "name = 'Ada' && !",
    ] {
        assert!(context(text, text.len()).is_none(), "{text}");
    }
    for text in [
        "",
        "(",
        "! (",
        "name = 'Ada' && ",
        "name = 'Ada' || (",
        "name = 'Ada' && !( ",
    ] {
        assert_eq!(
            context(text, text.len()).unwrap().target,
            CompletionTarget::Field { sort: false },
            "{text}"
        );
    }
}

#[test]
fn sort_completion_stops_after_a_field_or_direction() {
    for text in [
        "SORT BY ",
        "SORT  BY ",
        "name = 'Ada' SORT BY title, ",
        "SORT BY MIN(",
        "SORT BY MIN (",
        "SORT BY title, MAX (",
    ] {
        assert_eq!(
            context(text, text.len()).unwrap().target,
            CompletionTarget::Field { sort: true },
            "{text}"
        );
    }
    for text in ["SORT BY title ", "SORT BY title ASC", "SORT BY MIN(title) "] {
        assert!(context(text, text.len()).is_none(), "{text}");
    }
    let quoted = "name = 'SORT BY' && title";
    assert_eq!(
        context(quoted, quoted.len()).unwrap().target,
        CompletionTarget::Field { sort: false }
    );
}

#[test]
fn single_quoted_values_and_spaced_nested_paths_keep_their_token() {
    let value = "name ~ 'A%'";
    let caret = value.find('%').unwrap() + 1;
    let found = context(value, caret).unwrap();
    assert_eq!(found.replace, 7..11);
    assert_eq!(found.prefix, "A%");
    assert_eq!(
        found.target,
        CompletionTarget::Value {
            field: "name".into()
        }
    );

    let field = "foo['a b'] = 1";
    let caret = field.find('b').unwrap() + 1;
    let found = context(field, caret).unwrap();
    assert_eq!(found.replace, 0..10);
    assert_eq!(found.target, CompletionTarget::Field { sort: false });

    let nested_value = "name = 'x' && foo['a b'] ~ 'Al";
    let found = context(nested_value, nested_value.len()).unwrap();
    assert_eq!(
        found.target,
        CompletionTarget::Value {
            field: "foo['a b']".into()
        }
    );
}

#[test]
fn standalone_and_finished_strings_do_not_offer_fields() {
    assert!(context("\"Alpha\"", 1).is_none());
    assert!(context("name = 'Ada'", "name = 'Ada'".len()).is_none());
    assert!(context("SORT BY 'x", "SORT BY 'x".len()).is_none());
}

#[test]
fn metadata_paths_complete_in_filters_and_sorts() {
    for prefix in ["@record.", "@file.", "@collection."] {
        for before in ["", "year > 0 && ", "SORT BY "] {
            let text = format!("{before}{prefix}");
            let found = context(&text, text.len()).unwrap();
            assert_eq!(found.prefix, prefix);
            assert_eq!(
                found.target,
                CompletionTarget::Field {
                    sort: before == "SORT BY "
                }
            );
            assert_eq!(found.replace, before.len()..text.len());
        }
    }
    let text = "@file.size >= 02GiB";
    let found = context(text, text.len()).unwrap();
    assert_eq!(
        found.target,
        CompletionTarget::Value {
            field: "@file.size".into()
        }
    );
    assert_eq!(found.prefix, "02GiB");
}
