use super::*;
use crate::ui::model::{ResultRow, RowKey};


#[test]
fn new_repository_discards_old_results_and_detail() {
    let mut state = UiState::default();
    state.root = Some("old".into());
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
    assert!(state.root.is_none());
    assert!(state.page.rows.is_empty());
    assert!(state.detail.is_none());
    assert_eq!(state.busy, Busy::Opening);
}

#[test]
fn query_restore_keeps_exact_inputs() {
    let mut state = UiState::default();
    let draft = SearchDraft {
        full_text: "ÄÖÜ".into(),
        structured: "score >= 10 SORT BY score DESC".into(),
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
