use super::*;
use crate::ui::model::{ResultRow, RowKey};

#[test]
fn replacement_open_retains_old_workspace_until_success() {
    let mut state = UiState {
        root: Some("old".into()),
        ..UiState::default()
    };
    state.page.rows.push(ResultRow {
        key: RowKey {
            token: 1,
            collection: "old".into(),
            record_id: "1".into(),
            file: "old/1.json".into(),
            byte_start: 0,
        },
        cells: vec!["old".into()],
        matched_fields: Vec::new(),
        preview: None,
    });
    state.detail = Some(DetailView::default());
    state.apply(UiUpdate::Opening("Opening".into()));
    assert_eq!(state.root.as_deref(), Some("old"));
    assert_eq!(state.page.rows.len(), 1);
    assert!(state.detail.is_some());
    assert_eq!(state.busy, Busy::Opening);
    state.apply(UiUpdate::Opened {
        root: "new".into(),
        collections: vec!["records".into()],
        single_collection: true,
    });
    assert_eq!(state.root.as_deref(), Some("new"));
    assert!(state.page.rows.is_empty());
    assert!(state.detail.is_none());
}

#[test]
fn opening_a_repository_returns_to_the_results_workspace() {
    let mut state = UiState {
        show_open_options: true,
        ..UiState::default()
    };
    state.apply(UiUpdate::Opened {
        root: "example".into(),
        collections: vec!["records".into()],
        single_collection: true,
    });
    assert!(!state.show_open_options);
    assert_eq!(state.search.collection.as_deref(), Some("records"));
}

#[test]
fn query_restore_keeps_exact_inputs() {
    let mut state = UiState::default();
    let draft = SearchDraft {
        full_text: "ÄÖÜ".into(),
        structured: "score >= 10 SORT BY score DESC".into(),
        filter_enabled: true,
        collection: Some("docs".into()),
        case_sensitive: false,
        regex: true,
        sort: String::new(),
        offset: "10".into(),
        limit: "0".into(),
        count: true,
    };
    state.apply(UiUpdate::RestoreQuery(draft.clone()));
    assert_eq!(state.search, draft);
}

#[test]
fn page_refresh_keeps_detail_for_same_selected_token() {
    let mut state = UiState::default();
    let page = PageView {
        rows: vec![ResultRow {
            key: RowKey {
                token: 42,
                collection: "docs".into(),
                record_id: "1".into(),
                file: "a.json".into(),
                byte_start: 0,
            },
            cells: vec!["first".into()],
            matched_fields: vec![],
            preview: Some("preview".into()),
        }],
        selected: Some(0),
        ..PageView::default()
    };
    state.apply(UiUpdate::Page(page.clone()));
    state.apply(UiUpdate::Detail(Some(DetailView {
        title: "record".into(),
        ..DetailView::default()
    })));
    state.show_raw = true;
    state.apply(UiUpdate::Page(page));
    assert_eq!(state.detail.as_ref().unwrap().title, "record");
    assert!(state.show_raw);
    state.apply(UiUpdate::QueryStarted("Searching".into()));
    assert!(state.detail.is_none());
}

#[test]
fn columns_update_preserves_hidden_choices() {
    let mut state = UiState::default();
    state.apply(UiUpdate::Columns {
        available: vec!["id".into(), "title".into()],
        hidden: vec!["title".into()],
    });
    assert_eq!(state.available_columns, ["id", "title"]);
    assert_eq!(state.hidden_columns, ["title"]);
}

#[test]
fn successful_query_start_clears_previous_input_errors() {
    let mut state = UiState::default();
    for field in [
        ErrorField::FullText,
        ErrorField::Structured,
        ErrorField::Sort,
        ErrorField::Range,
    ] {
        state.apply(UiUpdate::Error {
            field,
            message: "old error".into(),
        });
    }
    state.apply(UiUpdate::QueryStarted("Searching".into()));
    assert!(state.errors.is_empty());
}
