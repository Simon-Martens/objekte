use super::*;
use crate::ui::model::{ResultRow, RowKey};

#[test]
fn visible_query_preview_survives_new_tab_and_cache_stays_bounded() {
    let mut state = UiState::default();
    let first = state.workspace.opened(None);
    state.page.columns = vec!["@id".into()];
    state.page.exact_total = Some(120);
    for index in 0..50 {
        state.projected_rows.insert(index, ResultRow {
            key: RowKey {
                token: index as u64,
                collection: "files".into(),
                record_id: format!("record-{index}"),
                file: format!("{index}.json"),
                byte_start: 0,
            },
            cells: vec![format!("record-{index}")],
            matched_fields: Vec::new(),
            match_highlights: Vec::new(),
            preview: None,
        });
    }
    state.snapshot_current_query();
    state.workspace.open_query(SearchDraft::default());
    state.prune_query_snapshots();
    let super::super::workspace::TabId::Search(first_id) = first else { unreachable!() };
    assert_eq!(state.query_snapshots.get(&first_id).and_then(|snapshot| snapshot.exact_total), Some(120));
    assert_eq!(state.query_snapshots.get(&first_id).unwrap().rows.len(), 40);
    for _ in 0..4 {
        state.workspace.open_query(SearchDraft::default());
        state.snapshot_current_query();
        state.prune_query_snapshots();
    }
    assert!(state.query_snapshots.len() <= 3);
    assert!(state.query_snapshots.values().all(|snapshot| snapshot.rows.len() <= 40));
    assert!(state.workspace.tab(first).is_some());
}

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
        match_highlights: Vec::new(),
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
fn late_validation_cannot_replace_newer_invalid_state() {
    let mut state = UiState {
        root: Some("records".into()),
        ..UiState::default()
    };
    state.loaded_count = 1;
    state.apply(UiUpdate::FilterValidation {
        generation: 2,
        state: FilterValidationState::Invalid {
            message: "Expected a value".into(),
            byte_offset: Some(7),
        },
    });
    state.apply(UiUpdate::FilterValidation {
        generation: 1,
        state: FilterValidationState::Valid,
    });
    assert_eq!(state.filter_validation_generation, 2);
    assert!(matches!(
        state.filter_validation,
        Some(FilterValidationState::Invalid { .. })
    ));
    assert!(state.showing_last_valid_results);
}

#[test]
fn opening_a_repository_returns_to_the_results_workspace() {
    let mut state = UiState {
        open_dialog: super::super::open_dialog::OpenDialogState {
            visible: true,
            ..super::super::open_dialog::OpenDialogState::default()
        },
        ..UiState::default()
    };
    state.apply(UiUpdate::Opened {
        root: "example".into(),
        collections: vec!["records".into()],
        single_collection: true,
    });
    assert!(!state.open_dialog.visible);
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
    };
    state.apply(UiUpdate::RestoreQuery(draft.clone()));
    assert_eq!(state.search, draft);
}

#[test]
fn query_restore_does_not_activate_retained_filter() {
    let mut state = UiState::default();
    let draft = SearchDraft {
        structured: "year >= 1900 SORT BY year DESC".into(),
        filter_enabled: false,
        ..SearchDraft::default()
    };
    state.apply(UiUpdate::RestoreQuery(draft.clone()));
    assert_eq!(state.search, draft);
}

#[test]
fn projection_refresh_keeps_detail_and_cache_is_bounded() {
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
            match_highlights: vec![],
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
    state.apply(UiUpdate::Projection(page));
    assert_eq!(state.detail.as_ref().unwrap().title, "record");
    assert!(state.show_raw);
    for start in [100, 200, 300] {
        state.apply(UiUpdate::Projection(PageView {
            rows: vec![ResultRow {
                key: RowKey {
                    token: start as u64,
                    collection: "docs".into(),
                    record_id: start.to_string(),
                    file: "a.json".into(),
                    byte_start: 0,
                },
                cells: vec!["value".into()],
                matched_fields: vec![],
                match_highlights: vec![],
                preview: None,
            }],
            offset: start,
            ..PageView::default()
        }));
    }
    assert!(!state.projected_rows.contains_key(&0));
    assert_eq!(state.projected_rows.len(), 3);
    state.apply(UiUpdate::QueryStarted("Searching".into()));
    assert_eq!(state.detail.as_ref().unwrap().title, "record");
}

#[test]
fn continuation_appends_once_and_exhaustion_establishes_total() {
    let row = |index: usize| ResultRow {
        key: RowKey {
            token: index as u64,
            collection: "docs".into(),
            record_id: index.to_string(),
            file: "a.json".into(),
            byte_start: 0,
        },
        cells: vec![index.to_string()],
        matched_fields: Vec::new(),
        match_highlights: Vec::new(),
        preview: None,
    };
    let mut state = UiState::default();
    state.apply(UiUpdate::Page(PageView {
        columns: vec!["@id".into()],
        rows: (0..100).map(row).collect(),
        has_next: true,
        ..PageView::default()
    }));
    assert_eq!(state.loaded_count, 100);
    assert_eq!(state.page.exact_total, None);
    state.apply(UiUpdate::ContinuationStarted);
    state.apply(UiUpdate::AppendPage(PageView {
        columns: vec!["@id".into()],
        rows: vec![row(100)],
        offset: 100,
        ..PageView::default()
    }));
    assert_eq!(state.loaded_count, 101);
    assert_eq!(state.page.exact_total, Some(101));
    assert!(!state.loading_more);
    state.apply(UiUpdate::AppendPage(PageView {
        rows: vec![row(100)],
        offset: 100,
        ..PageView::default()
    }));
    assert_eq!(state.loaded_count, 101);
    assert_eq!(state.projected_rows.get(&100).unwrap().key.token, 100);
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

#[test]
fn index_progress_does_not_erase_query_validation_or_displayed_page() {
    let mut state = UiState { root: Some("repo".into()), ..UiState::default() };
    state.apply(UiUpdate::Error {
        field: ErrorField::Structured,
        message: "invalid draft".into(),
    });
    state.loaded_count = 100;
    let mut status = IndexUiStatus::default();
    status.index_instance = 1;
    status.source_location = "repo".into();
    status.check = super::super::model::IndexTaskState::Running {
        operation_id: 7,
        processed: Some(3),
        total: None,
    };
    state.apply(UiUpdate::IndexStatus(status));
    assert_eq!(state.errors.get(&ErrorField::Structured).map(String::as_str), Some("invalid draft"));
    assert_eq!(state.loaded_count, 100);
    assert_eq!(state.status, "invalid draft");
    assert!(matches!(state.index_status.check, super::super::model::IndexTaskState::Running { operation_id: 7, .. }));
}

#[test]
fn same_repository_index_replacement_keeps_tabs_and_drafts() {
    let mut state = UiState {
        root: Some("repo".into()),
        search: SearchDraft { full_text: "retained".into(), ..SearchDraft::default() },
        ..UiState::default()
    };
    state.tabs.push(RecordTabSummary {
        id: 3,
        title: "record".into(),
        identity: "docs:1".into(),
        position: 0,
        can_back: false,
        can_previous: false,
        can_next: false,
        loading: false,
        links_loading: false,
        freshness: super::super::model::TabFreshness::Outdated,
    });
    let status = IndexUiStatus { index_instance: 2, source_location: "repo".into(), ..IndexUiStatus::default() };
    state.apply(UiUpdate::IndexReplaced(status));
    assert_eq!(state.tabs.len(), 1);
    assert_eq!(state.search.full_text, "retained");
    assert!(state.result_outdated);
}

#[test]
fn exhausted_incomplete_page_never_infers_an_exact_total() {
    let mut state = UiState::default();
    state.apply(UiUpdate::Page(PageView {
        incomplete: true,
        rows: vec![ResultRow {
            key: RowKey { token: 1, collection: "docs".into(), record_id: "1".into(), file: "a.json".into(), byte_start: 0 },
            cells: vec!["one".into()], matched_fields: Vec::new(), match_highlights: Vec::new(), preview: None,
        }],
        ..PageView::default()
    }));
    assert_eq!(state.page.exact_total, None);
    assert!(state.page.incomplete);
}

#[test]
fn overwrite_prompt_names_exact_path_and_clears_after_success() {
    let mut state = UiState::default();
    state.apply(UiUpdate::SaveIndexOverwriteRequired("/tmp/other-index.bin".into()));
    assert_eq!(state.index_overwrite_pending.as_deref(), Some("/tmp/other-index.bin"));
    state.apply(UiUpdate::IndexStatus(IndexUiStatus {
        save: super::super::model::IndexTaskState::Succeeded("Index saved".into()),
        ..IndexUiStatus::default()
    }));
    assert!(state.index_overwrite_pending.is_none());
}
