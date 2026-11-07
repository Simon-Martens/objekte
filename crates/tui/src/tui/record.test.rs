use super::*;
use crate::tui::test_support::fixture;
use objekte_search_core::repository::Payload;

fn sample() -> SearchHit {
    let mut fixture = fixture(1);
    for _ in 0..1_000 {
        fixture.app.tick(std::time::Instant::now());
        if !fixture.app.searching {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    fixture.app.hits[0].clone()
}

#[test]
fn nested_json_columns_and_field_folding_keep_occurrence_labels() {
    let mut hit = sample();
    hit.payload = Some(Payload::Bytes(
        br#"{"meta":{"title":"Nested title"},"items":[{"name":"First"},{"name":"Second"}],"empty":null}"#.to_vec(),
    ));
    hit.matched_fields = vec!["items[0].name".into()];
    assert_eq!(column_value(&hit, "meta.title"), "Nested title");
    assert_eq!(column_value(&hit, "items.name"), "First, Second");
    assert_eq!(column_value(&hit, "empty"), "");

    let mut state = RecordState::default();
    let rows = state.rows(&hit, None);
    assert!(
        rows.iter()
            .any(|row| row.label == "items" && row.branch.is_some())
    );
    assert!(!rows.iter().any(|row| row.label == "[0]"));
    state.toggle("items", true);
    let rows = state.rows(&hit, None);
    assert!(
        rows.iter()
            .any(|row| row.label == "name" && row.value == "First" && row.matched)
    );
    state.toggle("meta", false);
    assert!(
        !state
            .rows(&hit, None)
            .iter()
            .any(|row| row.value == "Nested title")
    );
}

#[test]
fn logical_payload_is_structured_and_non_json_bytes_have_raw_view() {
    let mut hit = sample();
    hit.payload = Some(Payload::Logical(Value::Object(
        std::collections::BTreeMap::from([
            ("title".into(), Value::String("Logical title".into())),
            ("count".into(), Value::Number(Number::integer(3))),
        ]),
    )));
    assert_eq!(column_value(&hit, "title"), "Logical title");
    assert!(
        RecordState::default()
            .rows(&hit, None)
            .iter()
            .any(|row| row.label == "count" && row.value == "3")
    );
    assert!(raw_text(&hit).contains("Logical title"));

    hit.payload = Some(Payload::Bytes(b"pipe|record|without|json".to_vec()));
    assert!(
        RecordState::default()
            .rows(&hit, None)
            .iter()
            .any(|row| row.label == "Source")
    );
    assert_eq!(raw_text(&hit), "pipe|record|without|json");
}

#[test]
fn cli_projection_uses_decoded_paths_and_verified_text() {
    let mut hit = sample();
    hit.payload = Some(Payload::Bytes(r#"{"title":"Stra\u00dfe","a.b":"literal dot","items":[{"name":"Müller"},null],"empty":[],"nil":null,"escape":"\u001b[31m"}"#.as_bytes().to_vec()));
    hit.matched_fields = vec!["title".into()];
    let columns = vec![
        "title".into(),
        "[\"a.b\"]".into(),
        "items[*].name".into(),
        "empty".into(),
        "nil".into(),
        "missing".into(),
    ];
    let row = display_row(&hit, &columns, "STRASSE", false, false);
    assert_eq!(
        row.fields,
        ["Straße", "literal dot", "Müller", "[]", "null", "∅"]
    );
    assert!(row.matched.contains("Straße"));
    hit.matched_fields.clear();
    assert!(display_row(&hit, &[], "", true, false).matched.is_empty());
    assert!(!raw_text(&hit).contains('\u{1b}'));
}
