use super::*;
use crate::tui::state::Hit;
use objekte_core::Repository;
use std::fs;

#[test]
fn detail_preserves_nested_fields_and_long_values() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        r#"{"outer":{"items":[{"name":"A very long value with unicode 🐈 and more"}]}}"#,
    )
    .unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let record = repo.get_record("docs", "a").unwrap().unwrap();
    let DetailColumns {
        names,
        values,
        matched,
    } = columns(&Hit::Filter(record), 16, 12);
    assert_eq!(names.len(), values.len());
    assert!(matched.iter().all(|matched| !matched));
    assert!(names.iter().any(|name| name == "outer"));
    assert!(names.iter().any(|name| name == "  items"));
    assert!(names.iter().any(|name| name == "    [0] name"));
    assert!(
        values
            .join("")
            .contains("A very long value with unicode 🐈 and more")
    );
}

#[test]
fn detail_compacts_arrays_hides_empty_fields_and_separates_metadata_once() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        r#"{"blank":{"child":""},"empty":"","false":false,"mixed":[{"name":"Ada","role":"Writer"},{"name":"Bob","role":"Editor"}],"nil":null,"nothing":[],"tags":["one","","two"],"zero":0}"#,
    )
    .unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let record = repo.get_record("docs", "a").unwrap().unwrap();
    let rows = rows(&Hit::Filter(record));
    let data_start = rows
        .iter()
        .position(|row| row.name.is_empty() && row.value.is_empty())
        .unwrap();
    assert_eq!(rows.iter().filter(|row| row.name.is_empty()).count(), 1);
    assert!(
        rows[..data_start]
            .iter()
            .any(|row| row.name == "size" && row.value.ends_with(" KB"))
    );
    assert!(!rows.iter().any(|row| row.name == "size_bytes"));
    assert_eq!(rows[data_start + 1].name, "false");
    assert_eq!(rows[data_start + 1].value, "false");
    assert!(
        rows.iter()
            .any(|row| row.name == "zero" && row.value == "0")
    );
    assert!(
        rows.iter()
            .any(|row| row.name == "tags" && row.value == "one, two")
    );
    assert!(
        !rows
            .iter()
            .any(|row| ["blank", "empty", "nil", "nothing"].contains(&row.name.as_str()))
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.value.contains("items") || row.value.contains("fields"))
    );
    assert!(
        rows.iter()
            .any(|row| row.name == "  [0] name" && row.value == "Ada")
    );
    assert!(
        rows.iter()
            .any(|row| row.name == "      role" && row.value == "Writer")
    );
    assert!(
        rows.iter()
            .any(|row| row.name == "  [1] name" && row.value == "Bob")
    );
    assert_eq!(size_kb(1_234), "1.23 KB");
}

#[test]
fn wrapping_preserves_newlines_and_unicode() {
    assert_eq!(wrap("ab\n🐈cd", 2), ["ab", "🐈", "cd"]);
    assert_eq!(wrap("a\tb", 8), [r"a\tb"]);
    assert_eq!(wrap_value("word word word", 9), ["word ", "word word"]);
    assert_eq!(wrap_value("abcdefghijk", 5), ["abcde", "fghij", "k"]);
    assert_eq!(wrap_value("one\n🐈 a", 4), ["one", "🐈 a"]);
}

#[test]
fn matched_text_paths_mark_leaf_labels_and_values_stop_at_eighty_cells() {
    let temp = tempfile::tempdir().unwrap();
    let collection = temp.path().join("data/docs");
    fs::create_dir_all(&collection).unwrap();
    fs::write(
        collection.join("a.json"),
        format!(
            r#"{{"notes":[{{"body":"Moon {}","other":"Earth"}},{{"body":"Sun"}}],"tags":["Moon","Star"]}}"#,
            "long ".repeat(25)
        ),
    )
    .unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let hit = repo
        .search_text(&objekte_core::TextQuery {
            pattern: "Moon".into(),
            mode: objekte_core::TextMode::Word,
            ..Default::default()
        })
        .unwrap()
        .hits
        .remove(0);
    assert_eq!(hit.fields, ["notes[].body", "tags[]"]);
    assert_eq!(hit.matched_paths, ["notes[0].body", "tags[0]"]);
    let hit = Hit::Text(hit);
    let rows = rows(&hit);
    assert!(
        rows.iter()
            .any(|row| row.name == "  [0] body" && row.matched)
    );
    assert!(
        rows.iter()
            .any(|row| row.name == "  [1] body" && !row.matched)
    );
    assert!(rows.iter().any(|row| row.name == "tags" && row.matched));
    assert!(
        rows.iter()
            .any(|row| { row.name == "matched paths" && row.value == "notes[0].body, tags[0]" })
    );
    assert!(
        !rows
            .iter()
            .any(|row| row.name == "      other" && row.matched)
    );
    let columns = columns(&hit, 30, 200);
    assert_eq!(columns.names.len(), columns.matched.len());
    assert!(
        columns
            .values
            .iter()
            .all(|line| UnicodeWidthStr::width(line.as_str()) <= 80)
    );
    let body_index = columns
        .names
        .iter()
        .position(|name| name == "  [0] body")
        .unwrap();
    assert!(columns.matched[body_index]);
    assert!(columns.values[body_index].ends_with(' '));
}
