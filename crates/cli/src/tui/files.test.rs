use super::*;
use crate::tui::app::{App, Hit, View};
use crate::tui::input::Input;
use objekte_core::Repository;
use std::fs;


#[test]
fn base64_matches_the_standard_alphabet_and_padding() {
    assert_eq!(base64(b""), "");
    assert_eq!(base64(b"f"), "Zg==");
    assert_eq!(base64(b"fo"), "Zm8=");
    assert_eq!(base64(b"foo"), "Zm9v");
    assert_eq!(base64("Grüße".as_bytes()), "R3LDvMOfZQ==");
    assert_eq!(osc52("hi"), "\x1b]52;c;aGk=\x07");
}

#[test]
fn an_edited_record_is_reindexed_and_reshown() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"title":"Moon"}"#).unwrap();
    fs::write(collection.join("b.json"), r#"{"title":"Moon too"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    // A collection only takes reported changes once it is indexed.
    repo.diff("docs").unwrap();
    let mut app = App::new(&mut repo);
    app.search = Input::with_text("Moon");
    app.search();
    let index = app
        .hits
        .iter()
        .position(|hit| hit.record().id == "a")
        .unwrap();
    app.selected = index;
    app.open_detail();
    app.edit_record();
    let Some(crate::tui::lists::Deferred::Edit(path)) = app.deferred.pop() else {
        panic!("an edit is queued");
    };
    assert_eq!(path, collection.join("a.json"));
    fs::write(&path, r#"{"title":"Moon edited"}"#).unwrap();
    app.apply_edit(&path);
    assert!(app.notice.as_ref().unwrap().text.contains("Record updated"));
    assert_eq!(app.view, View::Detail);
    let shown = app.current_detail_hit().unwrap().record();
    assert_eq!(shown.id, "a");
    assert_eq!(shown.data["title"], "Moon edited");
    let Hit::Text(_) = &app.hits[app.selected] else {
        panic!("still a text hit");
    };
}

#[test]
fn refresh_reports_files_changed_by_other_programs() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(collection.join("a.json"), r#"{"title":"Moon"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    repo.diff("docs").unwrap();
    let mut app = App::new(&mut repo);
    app.refresh_files();
    assert_eq!(app.notice.as_ref().unwrap().text, "No files changed");
    fs::write(collection.join("b.json"), r#"{"title":"Moon"}"#).unwrap();
    fs::remove_file(collection.join("a.json")).unwrap();
    app.search = Input::with_text("Moon");
    app.refresh_files();
    assert_eq!(
        app.notice.as_ref().unwrap().text,
        "Files updated: 1 new, 1 deleted"
    );
}
