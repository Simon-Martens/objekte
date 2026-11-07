use super::*;

fn query(search: &str) -> Query {
    Query {
        search: search.into(),
        ..Default::default()
    }
}

#[test]
fn settings_round_trip_and_a_missing_file_gives_defaults() {
    let temp = tempfile::tempdir().unwrap();
    let path = Settings::path(temp.path());
    assert_eq!(Settings::load(&path).unwrap(), Settings::default());
    let mut settings = Settings {
        theme: Some("light".into()),
        ..Default::default()
    };
    settings.remember(query("Goethe"));
    settings.saved.push(SavedQuery {
        name: "Early".into(),
        query: Query {
            filter: "year < 1800".into(),
            collection: Some("contents".into()),
            ..Default::default()
        },
    });
    settings.toggle_column(Some("contents"), "annotation");
    settings.save(&path).unwrap();
    assert_eq!(Settings::load(&path).unwrap(), settings);
}

#[test]
fn history_moves_repeats_to_the_end_and_skips_empty_queries() {
    let mut settings = Settings::default();
    settings.remember(query("a"));
    settings.remember(query("b"));
    settings.remember(query("a"));
    settings.remember(query("  "));
    assert_eq!(settings.history, [query("b"), query("a")]);
    for index in 0..(HISTORY + 5) {
        settings.remember(query(&index.to_string()));
    }
    assert_eq!(settings.history.len(), HISTORY);
}

#[test]
fn columns_toggle_per_collection() {
    let mut settings = Settings::default();
    settings.toggle_column(None, "title");
    assert_eq!(settings.hidden(None), ["title"]);
    assert!(settings.hidden(Some("docs")).is_empty());
    settings.toggle_column(None, "title");
    assert!(settings.columns.is_empty());
}

#[test]
fn a_broken_file_reports_its_path() {
    let temp = tempfile::tempdir().unwrap();
    let path = Settings::path(temp.path());
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "{").unwrap();
    assert!(Settings::load(&path).unwrap_err().contains("tui.json"));
}

#[test]
fn old_settings_keep_the_existing_sensitive_literal_default() {
    let query: Query = serde_json::from_str(r#"{"search":"Needle","filter":""}"#).unwrap();
    assert!(!query.case_insensitive);
    assert!(!query.regex);
    let changed = Query {
        case_insensitive: true,
        regex: true,
        ..query
    };
    let encoded = serde_json::to_string(&changed).unwrap();
    assert!(encoded.contains("\"case_insensitive\":true"));
    assert!(encoded.contains("\"regex\":true"));
}
