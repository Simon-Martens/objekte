use super::*;
use crate::backend::{query_page, test_support::fixture};
use serde_json::Value;

#[test]
fn selected_and_all_exports_match_tui_json_shape() {
    let fixture = fixture();
    let token = CancellationToken::new();
    let output = tempfile::tempdir().unwrap();
    let spec = QuerySpec {
        filter: "year >= 2023".into(),
        limit: Some(2),
        ..Default::default()
    };
    let page = query_page(&fixture.session, &spec, 0, 2, &token).unwrap();
    let selected_path = output.path().join("selected.json");
    export_selected_json(&fixture.session, &page.hits[0], &selected_path, &token).unwrap();
    let selected: Value = serde_json::from_slice(&fs::read(selected_path).unwrap()).unwrap();
    assert_eq!(selected, hit_value(&page.hits[0]).unwrap());
    let all_path = output.path().join("all.json");
    let count = export_all_json(&fixture.session, &spec, &all_path, &token).unwrap();
    let all: Value = serde_json::from_slice(&fs::read(all_path).unwrap()).unwrap();
    assert_eq!(count, 2);
    assert_eq!(all["kind"], "search");
    assert_eq!(all["query"]["filter"], "year >= 2023");
    assert_eq!(all["total"], 2);
    assert_eq!(all["hits"].as_array().unwrap().len(), 2);
    assert_eq!(all["hits"][0], selected);
}

#[test]
fn export_rejects_source_and_overwrite_and_cancellation() {
    let fixture = fixture();
    let output = tempfile::tempdir().unwrap();
    let spec = QuerySpec::default();
    let token = CancellationToken::new();
    let inside = fixture._root.path().join("export.json");
    let error = export_all_json(&fixture.session, &spec, &inside, &token).unwrap_err();
    assert_eq!(error.kind, BackendErrorKind::Options);
    let outside = output.path().join("export.json");
    fs::write(&outside, "old").unwrap();
    let error = export_all_json(&fixture.session, &spec, &outside, &token).unwrap_err();
    assert_eq!(error.kind, BackendErrorKind::Output);
    assert_eq!(fs::read_to_string(&outside).unwrap(), "old");
    token.cancel();
    let error = export_all_json(
        &fixture.session,
        &spec,
        &output.path().join("new.json"),
        &token,
    )
    .unwrap_err();
    assert_eq!(error.kind, BackendErrorKind::Cancelled);
}

#[test]
fn zero_limit_exports_empty_valid_document() {
    let fixture = fixture();
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("empty.json");
    let count = export_all_json(
        &fixture.session,
        &QuerySpec {
            limit: Some(0),
            ..Default::default()
        },
        &path,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(count, 0);
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["total"], 0);
    assert!(value["hits"].as_array().unwrap().is_empty());
}
