use super::*;
use crate::backend::{OpenSpec, open};
use crate::backend::{query_page, test_support::fixture};
use objekte_cli::arguments::Arguments;
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

#[test]
fn malformed_source_keeps_raw_fallback_payload_in_export() {
    let source = tempfile::tempdir().unwrap();
    fs::write(source.path().join("broken.json"), b"{not valid json").unwrap();
    fs::write(source.path().join("null.json"), b"null").unwrap();
    let session = open(
        &OpenSpec {
            args: Arguments {
                path: vec![source.path().to_path_buf()],
                ..Default::default()
            },
            cwd: source.path().to_path_buf(),
            configured_cache_policy: None,
        },
        &CancellationToken::new(),
    )
    .unwrap();
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("all.json");
    export_all_json(
        &session,
        &QuerySpec::default(),
        &path,
        &CancellationToken::new(),
    )
    .unwrap();
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let hits = value["hits"].as_array().unwrap();
    assert!(hits.iter().any(|hit| hit["record"] == "{not valid json"));
    assert!(hits.iter().any(|hit| hit["record"].is_null()));
}

#[test]
fn all_export_includes_rows_beyond_loaded_viewport() {
    let fixture = fixture();
    let token = CancellationToken::new();
    let page = query_page(&fixture.session, &QuerySpec::default(), 0, 1, &token).unwrap();
    assert_eq!(page.hits.len(), 1);
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("all.json");
    let count = export_all_json(&fixture.session, &QuerySpec::default(), &path, &token).unwrap();
    assert_eq!(count, 3);
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["hits"].as_array().unwrap().len(), 3);
}

#[test]
fn json_export_can_retain_the_selected_publication() {
    let fixture = fixture();
    let index = fixture.session.repository.index();
    let old_view = index.view().unwrap();
    let added = fixture._root.path().join("d.json");
    fs::write(&added, r#"{"title":"Delta","year":2025}"#).unwrap();
    index.add_file(&added).unwrap();
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("old.json");
    let count = export_all_json_on_view(
        &fixture.session,
        &old_view,
        &QuerySpec::default(),
        &path,
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(count, 3);
    let value: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    assert_eq!(value["hits"].as_array().unwrap().len(), 3);
}

#[test]
fn unavailable_source_does_not_leave_a_shortened_successful_export() {
    let fixture = fixture();
    let view = fixture.session.repository.index().view().unwrap();
    fs::remove_file(fixture._root.path().join("a.json")).unwrap();
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("incomplete.json");
    let result = export_all_json_on_view(
        &fixture.session,
        &view,
        &QuerySpec::default(),
        &path,
        &CancellationToken::new(),
    );
    assert_eq!(result.unwrap_err().kind, BackendErrorKind::Query);
    assert!(!path.exists());
}

#[test]
fn removed_record_cannot_be_exported_from_an_old_row_identity() {
    let fixture = fixture();
    let index = fixture.session.repository.index();
    let old_view = index.view().unwrap();
    let source = fixture._root.path().join("a.json");
    let handle = old_view.file_handle_for_path(&source).unwrap();
    index.delete_file(handle).unwrap();
    let current_view = index.view().unwrap();
    let output = tempfile::tempdir().unwrap();
    let path = output.path().join("selected.json");
    let result = export_selected_json_on_view(
        &fixture.session,
        &current_view,
        &fixture.session.collections[0],
        "path:a.json",
        &path,
        &CancellationToken::new(),
    );
    assert_eq!(result.unwrap_err().kind, BackendErrorKind::Query);
    assert!(!path.exists());
}
