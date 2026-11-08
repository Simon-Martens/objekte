use super::*;

#[test]
fn settings_use_distinct_path_and_restore_tui_query_fields() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = DesktopPreferences::load(temp.path()).unwrap();
    assert_ne!(
        DesktopPreferences::path(temp.path()),
        Settings::path(temp.path())
    );
    let query = SearchDraft {
        full_text: "Goethe".into(),
        structured: "year < 1800".into(),
        filter_enabled: true,
        collection: Some("contents".into()),
        case_sensitive: false,
        regex: true,
        ..SearchDraft::default()
    };
    settings.remember(&query).unwrap();
    settings.save_query("Early", &query).unwrap();
    settings
        .toggle_column(Some("contents"), "annotation")
        .unwrap();
    settings.set_dark(true).unwrap();
    let loaded = DesktopPreferences::load(temp.path()).unwrap();
    assert_eq!(loaded.restore_history(0), Some(query.clone()));
    assert_eq!(loaded.restore_saved("Early"), Some(query));
    assert_eq!(loaded.hidden(Some("contents")), ["annotation"]);
    assert_eq!(loaded.dark(), Some(true));
}

#[test]
fn named_query_upsert_and_empty_validation_match_tui() {
    let temp = tempfile::tempdir().unwrap();
    let mut settings = DesktopPreferences::load(temp.path()).unwrap();
    assert!(settings.save_query("", &SearchDraft::default()).is_err());
    assert!(
        settings
            .save_query("empty", &SearchDraft::default())
            .is_err()
    );
    for text in ["first", "replaced"] {
        settings
            .save_query(
                "Name",
                &SearchDraft {
                    full_text: text.into(),
                    ..SearchDraft::default()
                },
            )
            .unwrap();
    }
    assert_eq!(settings.saved().len(), 1);
    assert_eq!(settings.saved()[0].draft.full_text, "replaced");
}
