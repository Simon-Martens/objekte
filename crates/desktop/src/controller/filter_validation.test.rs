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
fn field_choices_are_bounded_to_schema_and_loaded_columns() {
    let fixture = fixture();
    let choices = field_choices(
        &fixture.session,
        None,
        &["title".into(), "year".into(), "[\"a.b\"]".into()],
    );
    assert_eq!(choices.len(), 3);
    assert!(choices.iter().all(|choice| choice.value_type.is_none()));
    assert!(choices.iter().any(|choice| choice.path == "year"));
}
