use super::*;

#[test]
fn migration_preserves_legacy_names_order_columns_and_theme() {
    let temp = tempfile::tempdir().unwrap();
    let path = DesktopPreferences::path(temp.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    let legacy = serde_json::json!({
        "theme": "light",
        "history": [
            {"search": "first"},
            {"search": "last", "filter": "year >= 1900", "regex": true}
        ],
        "saved": [
            {"name": "My old title", "search": "first"},
            {"name": "Another title", "search": "last", "filter": "year >= 1900"}
        ],
        "columns": {"letters": {"hidden": ["summary"]}}
    });
    fs::write(&path, serde_json::to_vec(&legacy).unwrap()).unwrap();
    let mut preferences = DesktopPreferences::load(temp.path()).unwrap();
    assert_eq!(preferences.dark(), Some(false));
    assert_eq!(preferences.hidden(Some("letters")), ["summary"]);
    assert_eq!(preferences.restore_history(0).unwrap().full_text, "last");
    assert_eq!(preferences.saved()[0].name.as_deref(), Some("My old title"));
    assert_ne!(preferences.saved()[0].id, preferences.saved()[1].id);
    assert!(preferences.saved()[1].draft.filter_enabled);
    // Reading legacy settings alone does not rewrite the file.
    assert_eq!(
        fs::read(&path).unwrap(),
        serde_json::to_vec(&legacy).unwrap()
    );
    preferences.set_dark_local(true);
    preferences.persist().unwrap();
    let reloaded = DesktopPreferences::load(temp.path()).unwrap();
    assert_eq!(reloaded.saved()[0].name.as_deref(), Some("My old title"));
    assert_eq!(reloaded.dark(), Some(true));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&fs::read(&path).unwrap()).unwrap()["version"],
        VERSION
    );
    assert_ne!(path, LegacySettings::path(temp.path()));
}

#[test]
fn bookmarks_use_definition_identity_and_keep_disabled_filter() {
    let temp = tempfile::tempdir().unwrap();
    let mut preferences = DesktopPreferences::load(temp.path()).unwrap();
    let first = SearchDraft {
        collection: Some("letters".into()),
        structured: "year >= 1900".into(),
        filter_enabled: false,
        case_sensitive: false,
        regex: true,
        ..SearchDraft::default()
    };
    let mut second = first.clone();
    second.structured = "year > 1900".into();
    let first_id = preferences.save_query(&first).unwrap();
    let second_id = preferences.save_query(&second).unwrap();
    assert_ne!(first_id, second_id);
    assert_eq!(preferences.saved().len(), 2);
    assert_eq!(preferences.saved()[0].name, preferences.saved()[1].name);
    preferences.rename_local(first_id, "My collection").unwrap();
    assert_eq!(preferences.save_query(&first).unwrap(), first_id);
    preferences.remember(&first).unwrap();
    preferences.remember(&second).unwrap();
    preferences.remember(&first).unwrap();
    let reloaded = DesktopPreferences::load(temp.path()).unwrap();
    assert_eq!(reloaded.saved()[0].name.as_deref(), Some("My collection"));
    assert_eq!(reloaded.restore_saved(first_id), Some(first.clone()));
    assert_eq!(reloaded.restore_saved(second_id), Some(second));
    assert_eq!(reloaded.history().len(), 2);
    assert_eq!(reloaded.restore_history(0), Some(first));
}

#[test]
fn generated_titles_are_display_only() {
    let draft = SearchDraft {
        full_text: "  Heidel  berg ".into(),
        structured: "year >= 1900 SORT BY year DESC".into(),
        filter_enabled: true,
        collection: Some("letters".into()),
        ..SearchDraft::default()
    };
    assert_eq!(
        generated_title(&draft),
        "letters · Heidel berg · year >= 1900 SORT BY year DESC"
    );
    assert_eq!(draft.full_text, "  Heidel  berg ");
    assert_eq!(draft.structured, "year >= 1900 SORT BY year DESC");
    assert_eq!(
        generated_title(&SearchDraft::default()),
        "All collections · All records"
    );
}

#[test]
fn column_widths_and_wrapping_persist_per_collection() {
    let temp = tempfile::tempdir().unwrap();
    let mut preferences = DesktopPreferences::load(temp.path()).unwrap();
    preferences.set_column_width_local(Some("letters"), "title", 237.4);
    preferences.toggle_wrap_local(Some("letters"), "title");
    preferences.persist().unwrap();
    let reloaded = DesktopPreferences::load(temp.path()).unwrap();
    let (widths, wrapped) = reloaded.table_presentation(Some("letters"));
    assert_eq!(widths["title"], 237);
    assert_eq!(wrapped, ["title"]);
    assert!(reloaded.table_presentation(Some("other")).0.is_empty());
}

#[test]
fn nested_column_visibility_is_explicit_and_scoped() {
    let temp = tempfile::tempdir().unwrap();
    let mut preferences = DesktopPreferences::load(temp.path()).unwrap();
    assert!(nested_column("nested.title"));
    assert!(!nested_column("[\"literal.key\"]"));
    assert!(preferences.shown_nested(Some("letters")).is_empty());
    preferences.toggle_column_local(Some("letters"), "nested.title");
    preferences.persist().unwrap();
    let mut reloaded = DesktopPreferences::load(temp.path()).unwrap();
    assert_eq!(reloaded.shown_nested(Some("letters")), ["nested.title"]);
    assert!(reloaded.shown_nested(Some("other")).is_empty());
    reloaded.toggle_column_local(Some("letters"), "nested.title");
    assert!(reloaded.shown_nested(Some("letters")).is_empty());
}
