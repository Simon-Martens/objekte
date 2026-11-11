use super::*;
use crate::backend::test_support::fixture;

fn draft(filter: &str) -> SearchDraft {
    SearchDraft {
        structured: filter.into(),
        filter_enabled: true,
        ..SearchDraft::default()
    }
}

#[test]
fn incomplete_filter_reports_position_and_keeps_it_invalid() {
    let fixture = fixture();
    let result = validate_request(&fixture.session, &draft("year >="));
    assert_eq!(
        result,
        FilterValidationState::Invalid {
            message: "Expected a value after >=".into(),
            byte_offset: Some(7),
        }
    );
}

#[test]
fn unquoted_wildcard_gets_a_useful_filter_error() {
    let fixture = fixture();
    assert!(matches!(
        validate_request(&fixture.session, &draft("title ~ A%")),
        FilterValidationState::Invalid { message, .. }
            if message == "Quote wildcard values, for example: name ~ 'A%'"
    ));
    assert_eq!(
        validate_request(&fixture.session, &draft("title ~ 'A%'")),
        FilterValidationState::Valid
    );
}

#[test]
fn disabled_filter_is_ignored_without_changing_its_draft() {
    let fixture = fixture();
    let mut query = draft("year >=");
    query.filter_enabled = false;
    assert_eq!(
        validate_request(&fixture.session, &query),
        FilterValidationState::Valid
    );
    assert_eq!(query.structured, "year >=");
    query.filter_enabled = true;
    assert!(matches!(
        validate_request(&fixture.session, &query),
        FilterValidationState::Invalid { .. }
    ));
}

#[test]
fn inline_sort_and_quoted_literal_conflicts_are_checked() {
    let fixture = fixture();
    let mut query = draft("year >= 2022 SORT BY year DESC");
    assert_eq!(
        validate_request(&fixture.session, &query),
        FilterValidationState::Valid
    );
    query.structured = "\"Alpha\"".into();
    assert_eq!(
        validate_request(&fixture.session, &query),
        FilterValidationState::Valid
    );
    query.full_text = "Beta".into();
    assert!(
        matches!(validate_request(&fixture.session, &query), FilterValidationState::Invalid { message, .. } if message.contains("cannot be combined"))
    );
}

#[test]
fn field_choices_include_metadata_schema_and_loaded_columns() {
    let fixture = fixture();
    let choices = field_choices(
        &fixture.session,
        None,
        &["title".into(), "year".into(), "[\"a.b\"]".into()],
    );
    assert_eq!(choices.len(), 16);
    assert!(
        choices.iter().filter(|choice| !choice.path.starts_with('@'))
            .all(|choice| choice.value_type.is_none())
    );
    assert!(choices.iter().any(|choice| choice.path == "year"));
}

#[test]
fn metadata_choices_are_typed_and_available_in_every_scope() {
    let fixture = fixture();
    for scope in [None, Some("books"), Some("missing")] {
        let choices = field_choices(&fixture.session, scope, &["@file.size".into()]);
        for (path, kind) in [
            ("@record.id", ValueType::String),
            ("@record.byte_start", ValueType::Number),
            ("@record.byte_end", ValueType::Number),
            ("@record.byte_length", ValueType::Number),
            ("@record.class", ValueType::Number),
            ("@file.id", ValueType::Number),
            ("@file.path", ValueType::String),
            ("@file.size", ValueType::Number),
            ("@file.created", ValueType::String),
            ("@file.modified", ValueType::String),
            ("@file.status", ValueType::Number),
            ("@collection.name", ValueType::String),
            ("@repository.root", ValueType::String),
        ] {
            let found: Vec<_> = choices.iter().filter(|choice| choice.path == path).collect();
            assert_eq!(found.len(), 1, "{scope:?}: {path}");
            assert_eq!(found[0].value_type, Some(kind), "{path}");
            assert!(QueryPath::parse(path).is_ok(), "{path}");
        }
    }
}

#[test]
fn desktop_validates_metadata_filters_with_size_units() {
    let fixture = fixture();
    for filter in [
        "@file.size >= 23MB",
        "@file.size < 2KB",
        "@file.size <= 02GiB",
        "@record.byte_length >= 1KiB",
        "@collection.name = 'books' && @record.byte_start = 0",
    ] {
        assert_eq!(
            validate_request(&fixture.session, &draft(filter)),
            FilterValidationState::Valid,
            "{filter}"
        );
    }
}
