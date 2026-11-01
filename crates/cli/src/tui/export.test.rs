use super::*;
use objekte_core::Repository;
use std::{
    fs::{self, File, FileTimes},
    time::{Duration, UNIX_EPOCH},
};

#[test]
fn export_all_and_selected_hit_include_source_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for (id, modified) in [("a", 2000), ("b", 1000)] {
        let path = collection.join(format!("{id}.json"));
        fs::write(&path, r#"{"note":"Moon"}"#).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(modified)))
            .unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.kind = crate::tui::state::SearchKind::Filter;
    app.input.text = "note ~ 'Moon'".into();
    app.search();
    let request = app.last_request.clone().unwrap();

    let all = temp.path().join("all.json");
    assert_eq!(write_all(&mut app, &all, &request).unwrap(), 2);
    let data: serde_json::Value = serde_json::from_slice(&fs::read(&all).unwrap()).unwrap();
    assert_eq!(data["total"], 2);
    assert_eq!(data["hits"].as_array().unwrap().len(), 2);
    assert!(data["hits"][0]["record"]["size_bytes"].as_u64().unwrap() > 0);
    assert!(
        data["hits"][0]["record"]["modified_unix_nanos"]
            .as_str()
            .is_some()
    );

    let one = temp.path().join("one.json");
    write_one(&one, Some(&request), &app.hits[0]).unwrap();
    let data: serde_json::Value = serde_json::from_slice(&fs::read(&one).unwrap()).unwrap();
    assert_eq!(data["hit"]["record"]["id"], "a");
    assert!(write_one(&one, Some(&request), &app.hits[0]).is_err());
}

#[test]
fn export_all_ignores_the_visible_page_and_preserves_text_hit_fields() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    for index in 0..103 {
        let path = collection.join(format!("r{index:03}.json"));
        fs::write(&path, r#"{"note":"Moon"}"#).unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_times(
                FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(1000 - index)),
            )
            .unwrap();
    }
    let mut repo = Repository::open(temp.path()).unwrap();
    let mut app = App::new(&mut repo);
    app.kind = crate::tui::state::SearchKind::Text;
    app.input.text = "Moon".into();
    app.search();
    assert_eq!(app.hits.len(), 100);
    app.next_page();
    assert_eq!(app.hits.len(), 3);
    let request = app.last_request.clone().unwrap();
    let all = temp.path().join("all-text.json");
    assert_eq!(write_all(&mut app, &all, &request).unwrap(), 103);
    let data: serde_json::Value = serde_json::from_slice(&fs::read(&all).unwrap()).unwrap();
    assert_eq!(data["total"], 103);
    assert_eq!(data["hits"].as_array().unwrap().len(), 103);
    assert_eq!(data["hits"][0]["fields"], json!(["note"]));
    assert_eq!(data["hits"][0]["matched_paths"], json!(["note"]));
    assert_eq!(data["hits"][102]["record"]["id"], "r102");
}
