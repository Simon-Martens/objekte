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
            comparison: Comparison::Equal,
            value: Some("A \"quote\"\nline".into()),
            value_type: Some(ValueType::String),
        },
        ConditionDraft {
            field: "year".into(),
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
