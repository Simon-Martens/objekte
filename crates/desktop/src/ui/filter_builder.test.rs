use super::*;

#[test]
fn supported_manual_conditions_populate_without_rewriting_text() {
    let text = "year >= 1900 && title = 'Book'";
    let state = BuilderState::from_text(text);
    assert_eq!(state.status, BuilderStatus::Synchronized);
    assert_eq!(state.rows.len(), 2);
    assert_eq!(state.rows[0].field, "year");
    assert_eq!(state.rows[0].comparison, Comparison::AtLeast);
    assert_eq!(state.rows[0].value.as_deref(), Some("1900"));
    assert_eq!(state.rows[0].value_type, Some(ValueType::Number));
    assert_eq!(state.rows[1].value.as_deref(), Some("Book"));
    assert_eq!(state.rows[1].value_type, Some(ValueType::String));
    assert_eq!(text, "year >= 1900 && title = 'Book'");
}

#[test]
fn typed_serialization_escapes_strings_and_keeps_numbers_unquoted() {
    let rows = vec![
        ConditionDraft {
            field: "['a.b']".into(),
            predicate: None,
            comparison: Comparison::Equal,
            value: Some("A \"quote\"\nline".into()),
            value_type: Some(ValueType::String),
        },
        ConditionDraft {
            field: "year".into(),
            predicate: None,
            comparison: Comparison::AtLeast,
            value: Some("1900".into()),
            value_type: Some(ValueType::Number),
        },
    ];
    let text = serialize_conditions(&rows).unwrap().unwrap();
    assert_eq!(
        text,
        "[\"a.b\"] = \"A \\\"quote\\\"\\nline\" && year >= 1900"
    );
    assert_eq!(BuilderState::from_text(&text).rows.len(), 2);
    let number_as_string = serialize_conditions(&[ConditionDraft {
        field: "code".into(),
        predicate: None,
        comparison: Comparison::Equal,
        value: Some("1900".into()),
        value_type: Some(ValueType::String),
    }])
    .unwrap()
    .unwrap();
    assert_eq!(number_as_string, "code = \"1900\"");
}

#[test]
fn unsupported_valid_expressions_never_show_partial_rows() {
    for text in [
        "year >= 1900 || type = 'book'",
        "name:lower = 'book'",
        "tags[*] = 'x'",
        "SORT BY year DESC",
        "year >= 1900 SORT BY year",
        "\"literal full text\"",
    ] {
        let mut state = BuilderState::from_text(text);
        assert_eq!(state.status, BuilderStatus::Unsupported, "{text}");
        assert!(state.rows.is_empty(), "{text}");
        assert!(state.complete_change().is_err(), "{text}");
    }
}

#[test]
fn incomplete_builder_row_does_not_erase_current_filter() {
    let mut state = BuilderState::from_text("year >= 1900");
    state.add_condition();
    assert_eq!(state.complete_change().unwrap(), None);
    assert_eq!(state.status, BuilderStatus::Incomplete);
    state.rows[1] = ConditionDraft {
        field: "type".into(),
        predicate: None,
        comparison: Comparison::Equal,
        value: Some("book".into()),
        value_type: Some(ValueType::String),
    };
    assert_eq!(
        state.complete_change().unwrap().as_deref(),
        Some("year >= 1900 && type = \"book\"")
    );
}

#[test]
fn string_type_without_entered_value_remains_incomplete() {
    let mut state = BuilderState::from_text("");
    state.rows[0].field = "title".into();
    state.rows[0].value_type = Some(ValueType::String);
    assert_eq!(state.complete_change().unwrap(), None);
    state.rows[0].value = Some(String::new());
    assert_eq!(
        state.complete_change().unwrap().as_deref(),
        Some("title = \"\"")
    );
}

#[test]
fn text_edits_and_undo_rebuild_builder_from_authoritative_text() {
    let mut state = BuilderState::from_text("year >= 1900");
    state.add_condition();
    state = BuilderState::from_text("year >= 1900 && type = 'book'");
    assert_eq!(state.status, BuilderStatus::Synchronized);
    assert_eq!(state.rows.len(), 2);
    state = BuilderState::from_text("year >= 1900");
    assert_eq!(state.rows.len(), 1);
    state = BuilderState::from_text("year >=");
    assert!(matches!(state.status, BuilderStatus::Invalid(_)));
    assert!(state.rows.is_empty());
}

#[test]
fn numeric_builder_values_accept_exact_size_units_and_round_trip() {
    for (value, bytes) in [
        ("23MB", "23000000"),
        ("2KB", "2000"),
        ("02GiB", "2147483648"),
        ("1.5KiB", "1536"),
        ("0B", "0"),
        ("4096", "4096"),
    ] {
        let row = ConditionDraft {
            field: "@file.size".into(),
            predicate: None,
            comparison: Comparison::AtLeast,
            value: Some(value.into()),
            value_type: Some(ValueType::Number),
        };
        let text = serialize_conditions(&[row]).unwrap().unwrap();
        assert_eq!(text, format!("@file.size >= {value}"));
        let state = BuilderState::from_text(&text);
        assert_eq!(state.status, BuilderStatus::Synchronized);
        assert_eq!(state.rows[0].value.as_deref(), Some(bytes));
        assert_eq!(state.rows[0].value_type, Some(ValueType::Number));
    }
}

#[test]
fn numeric_builder_rejects_invalid_sizes_and_nonliteral_expressions() {
    for value in [
        "2XB",
        "2kb",
        "-2KB",
        "0.1B",
        "170141183460469231731687303715884105728B",
        "'23MB'",
        "$size",
        "@file.size",
        "true",
        "2KB || year = 0",
        "2KB SORT BY @file.size",
    ] {
        assert!(
            serialize_conditions(&[ConditionDraft {
                field: "@file.size".into(),
                predicate: None,
                comparison: Comparison::AtLeast,
                value: Some(value.into()),
                value_type: Some(ValueType::Number),
            }])
            .is_err(),
            "{value}"
        );
    }
}

#[test]
fn boolean_field_tests_round_trip_through_the_authoritative_filter_text() {
    for test in [
        BooleanTest::NonEmpty,
        BooleanTest::String,
        BooleanTest::Number,
        BooleanTest::Array,
        BooleanTest::Null,
        BooleanTest::Object,
        BooleanTest::Exists,
    ] {
        for (comparison, value, symbol) in [
            (Comparison::Equal, "true", "="),
            (Comparison::NotEqual, "false", "!="),
        ] {
            let text = serialize_conditions(&[ConditionDraft {
                field: "annotation".into(),
                predicate: Some(test),
                comparison,
                value: Some(value.into()),
                value_type: Some(ValueType::Boolean),
            }])
            .unwrap()
            .unwrap();
            assert_eq!(
                text,
                format!("annotation{} {symbol} {value}", test.suffix())
            );
            let state = BuilderState::from_text(&text);
            assert_eq!(state.status, BuilderStatus::Synchronized, "{text}");
            assert_eq!(state.rows[0].predicate, Some(test), "{text}");
            assert_eq!(state.rows[0].value.as_deref(), Some(value), "{text}");
        }
    }
}

#[test]
fn boolean_tests_reject_scalar_orders_and_invalid_values() {
    for (comparison, value, kind) in [
        (Comparison::Greater, "true", ValueType::Boolean),
        (Comparison::Equal, "yes", ValueType::Boolean),
        (Comparison::Equal, "true", ValueType::String),
    ] {
        assert!(
            serialize_conditions(&[ConditionDraft {
                field: "annotation".into(),
                predicate: Some(BooleanTest::NonEmpty),
                comparison,
                value: Some(value.into()),
                value_type: Some(kind),
            }])
            .is_err()
        );
    }
    let mut state = BuilderState::from_text("annotation:? = true && title = 'Book'");
    assert_eq!(state.status, BuilderStatus::Synchronized);
    assert_eq!(state.rows.len(), 2);
    state.rows[0].value = Some("false".into());
    assert_eq!(
        state.complete_change().unwrap().as_deref(),
        Some("annotation:? = false && title = \"Book\"")
    );
}
