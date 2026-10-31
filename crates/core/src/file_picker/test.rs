use super::*;
use crate::{JsonFilter, json};
use std::{collections::BTreeMap, fs, process::Command};

fn put(root: &Path, id: &str, json: &str) {
    fs::write(root.join(format!("{id}.json")), json).unwrap();
}

fn ids(picker: &FilePicker, terms: Option<&[String]>) -> Vec<String> {
    picker
        .candidates(terms)
        .into_iter()
        .map(|slot| picker.id(slot).unwrap().to_owned())
        .collect()
}

#[test]
fn indexes_decoded_values_with_stable_slots_and_targeted_updates() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a", r#"{"needle":"absent","title":"Moon"}"#);
    put(dir.path(), "b", r#"{"title":"\u004doonlit"}"#);
    put(dir.path(), "c", r#"{"title":"Earth"}"#);
    put(dir.path(), "d", r#"{"title":"needle"}"#);
    let mut picker = FilePicker::open(dir.path().canonicalize().unwrap().as_path()).unwrap();
    assert_eq!(picker.len(), 4);
    assert!(picker.index.skip_index().is_some());
    assert!(picker.index_bytes() > 0);
    assert_eq!(ids(&picker, Some(&["Moon".into()])), vec!["a", "b"]);
    assert_eq!(ids(&picker, Some(&["needle".into()])), vec!["d"]);
    let a_slot = picker.position("a").map(|p| picker.sorted[p]).unwrap();
    let a_revision = picker.revision("a").unwrap();

    put(dir.path(), "a", r#"{"needle":"absent","title":"Sunrise"}"#);
    picker.refresh_id("a").unwrap();
    assert_eq!(picker.revision("a"), Some(a_revision + 1));
    assert_eq!(picker.position("a").map(|p| picker.sorted[p]), Ok(a_slot));
    // The old posting remains in the immutable index; the changed slot is
    // included conservatively until rebuild and exact matching checks it.
    assert!(ids(&picker, Some(&["Moon".into()])).contains(&"a".into()));
    assert!(ids(&picker, Some(&["Sunrise".into()])).contains(&"a".into()));

    let source = Source::open(picker.root()).unwrap();
    picker.rebuild_index_from_source(&source).unwrap();
    assert_eq!(ids(&picker, Some(&["Moon".into()])), vec!["b"]);
    assert_eq!(ids(&picker, Some(&["Sunrise".into()])), vec!["a"]);

    fs::remove_file(dir.path().join("b.json")).unwrap();
    picker.refresh_id("b").unwrap();
    assert_eq!(picker.revision("b"), Some(2));
    put(dir.path(), "b", r#"{"title":"Returned"}"#);
    picker.refresh_id("b").unwrap();
    assert_eq!(picker.revision("b"), Some(3));
    fs::remove_file(dir.path().join("b.json")).unwrap();
    picker.refresh_id("b").unwrap();
    put(dir.path(), "z", r#"{"title":"Moonlight"}"#);
    picker.refresh_id("z").unwrap();
    assert_eq!(ids(&picker, None), vec!["a", "c", "d", "z"]);
    assert_eq!(ids(&picker, Some(&["Moon".into()])), vec!["z"]);
}

#[test]
fn record_candidates_follow_id_order_across_changed_and_deleted_slots() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a", r#"{"title":"Match"}"#);
    put(dir.path(), "b", r#"{"title":"Match"}"#);
    put(dir.path(), "c", r#"{"title":"Match"}"#);
    for number in 0..100 {
        put(dir.path(), &format!("z{number:03}"), r#"{"title":"Other"}"#);
    }
    let mut picker = FilePicker::open(dir.path()).unwrap();

    put(dir.path(), "a", r#"{"title":"Changed"}"#);
    picker.refresh_id("a").unwrap();
    fs::remove_file(dir.path().join("c.json")).unwrap();
    picker.refresh_id("c").unwrap();
    put(dir.path(), "aa", r#"{"title":"Match"}"#);
    picker.refresh_id("aa").unwrap();

    let ids = picker
        .candidate_records(Some("Match"), None, &[])
        .into_iter()
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    // The changed "a" bypasses its stale posting exactly once. The appended
    // "aa" slot sorts before the older "b" slot and deleted "c" stays out.
    assert_eq!(ids, ["a", "aa", "b"]);
}

#[test]
fn folded_index_candidates_do_not_change_exact_value_regex_results() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a", r#"{"title":"Straße"}"#);
    let picker = FilePicker::open(dir.path()).unwrap();
    assert_eq!(picker.regex_candidate_ids("STRASSE"), ["a"]);
    assert!(picker.search_values_regex("STRASSE").unwrap().is_empty());
    assert_eq!(picker.search_values_regex("Straße").unwrap(), ["a"]);
}

#[test]
fn inventory_refresh_tracks_additions_removals_and_changes() {
    let dir = tempfile::tempdir().unwrap();
    put(dir.path(), "a", r#"{"title":"Alpha"}"#);
    put(dir.path(), "b", r#"{"title":"Beta"}"#);
    let mut picker = FilePicker::open(dir.path().canonicalize().unwrap().as_path()).unwrap();
    let revision = picker.revision("a").unwrap();
    put(dir.path(), "a", r#"{"title":"Alpha revised"}"#);
    fs::remove_file(dir.path().join("b.json")).unwrap();
    put(dir.path(), "c", "invalid json");
    assert_eq!(picker.refresh().unwrap(), 3);
    assert_eq!(picker.revision("a"), Some(revision + 1));
    assert_eq!(ids(&picker, None), vec!["a", "c"]);
    assert!(
        picker
            .candidates(Some(&["Alpha".into()]))
            .iter()
            .any(|&slot| picker.id(slot) == Some("a"))
    );
    let source = Source::open(picker.root()).unwrap();
    picker.rebuild_index_from_source(&source).unwrap();
    assert!(ids(&picker, Some(&["Alpha".into()])).contains(&"a".into()));
    assert_eq!(picker.files.len(), picker.len());
    assert_eq!(picker.revision("a"), Some(revision + 1));
}

#[test]
fn git_status_is_available_on_demand() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("data/records");
    fs::create_dir_all(&root).unwrap();
    put(&root, "a", r#"{"title":"Alpha"}"#);
    if Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(dir.path())
        .status()
        .is_err()
    {
        return;
    }
    let picker = FilePicker::open(root.canonicalize().unwrap().as_path()).unwrap();
    assert_eq!(
        picker.git_status().unwrap().unwrap().get("a"),
        Some(&GitState {
            index: '?',
            worktree: '?'
        })
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(dir.path())
            .args(["add", "data/records/a.json"])
            .status()
            .unwrap()
            .success()
    );
    put(&root, "a", r#"{"title":"Changed"}"#);
    assert_eq!(
        picker.git_status().unwrap().unwrap().get("a"),
        Some(&GitState {
            index: 'A',
            worktree: 'M'
        })
    );
}

#[test]
fn regex_planner_and_full_values_search_match_a_brute_force_scan() {
    let dir = tempfile::tempdir().unwrap();
    for (id, json) in [
        ("a", r#"{"title":"Moonlight","year":1800}"#),
        ("b", r#"{"title":"Sunshine","year":1900}"#),
        ("c", r#"{"annotation":"foo distant bar"}"#),
        ("d", r#"{"title":"m00n"}"#),
        ("e", r#"{"title":"Über"}"#),
        ("f", r#"{"title":"\u004doon"}"#),
        ("g", "invalid json"),
        ("h", r#"{"Moon":"field name only","title":"Earth"}"#),
    ] {
        put(dir.path(), id, json);
    }
    let mut picker = FilePicker::open(dir.path()).unwrap();
    for pattern in [
        "Moon",
        "(?i:moon)",
        "Moon|Sun",
        "(Moon|Sun)light",
        "Mo.n",
        "[Mm]oon",
        "foo.*bar",
        "[0-9]+",
        "Über",
        "^",
        "Moon.*missing",
    ] {
        let matcher = regex::bytes::RegexBuilder::new(pattern)
            .unicode(false)
            .build()
            .unwrap();
        let source = Source::open(picker.root()).unwrap();
        let mut expected = Vec::new();
        for &slot in &picker.sorted {
            let id = picker.id(slot).unwrap();
            let stamp = &picker.files[slot].as_ref().unwrap().stamp;
            let projection = projected_values_with_text(
                &source,
                id,
                stamp,
                &[],
                picker.presence.fields(),
                &picker.view,
                ProjectionOptions {
                    folded_index: false,
                    text_rules: None,
                },
            )
            .unwrap();
            let matches = match projection {
                ProjectionOutcome::Valid(text) => matcher.is_match(&text.bytes),
                ProjectionOutcome::Malformed(text) => matcher.is_match(text.as_bytes()),
            };
            if matches {
                expected.push(id.to_owned());
            }
        }
        let candidates = picker.regex_candidate_ids(pattern);
        for id in &expected {
            assert!(candidates.contains(&id.as_str()), "{pattern} lost {id}");
        }
        assert_eq!(
            picker.search_values_regex(pattern).unwrap(),
            expected,
            "{pattern}"
        );
    }

    put(dir.path(), "h", r#"{"title":"Moonrise"}"#);
    picker.refresh_id("h").unwrap();
    assert!(picker.regex_candidate_ids("Moon").contains(&"h"));
    assert!(
        picker
            .search_values_regex("Moon")
            .unwrap()
            .contains(&"h".into())
    );
    picker.rebuild_index().unwrap();
    assert!(
        picker
            .search_values_regex("Moon")
            .unwrap()
            .contains(&"h".into())
    );
}

#[test]
fn filter_regex_prefilter_never_loses_a_structural_match() {
    let dir = tempfile::tempdir().unwrap();
    for (id, raw) in [
        ("a", r#"{"title":"Moonlight","year":1800,"flag":true}"#),
        ("b", r#"{"title":"Sunshine","year":1900,"flag":false}"#),
        ("c", r#"{"title":"\u004doon","year":1700,"flag":true}"#),
        ("d", r#"{"annotation":"Moon","year":1800}"#),
        ("e", "not json"),
    ] {
        put(dir.path(), id, raw);
    }
    let picker = FilePicker::open(dir.path()).unwrap();
    for expression in [
        "title = 'Moon'",
        "title ~ :ignorecase('moon')",
        "title ~ 'Moon%'",
        "title ~ 'Moon' && year >= 1800",
        "title ~ 'Moon' || title ~ 'Sun'",
        "flag = true",
    ] {
        let filter = JsonFilter::compile(expression, &BTreeMap::new()).unwrap();
        let candidates = filter
            .values_prefilter()
            .map(|(_, pattern)| picker.regex_candidate_ids(&pattern))
            .unwrap_or_else(|| picker.candidate_ids(None));
        for &slot in &picker.sorted {
            let id = picker.id(slot).unwrap();
            let raw = fs::read_to_string(picker.root().join(format!("{id}.json"))).unwrap();
            if let Ok(value) = json::parse(&raw)
                && filter.matches(&value).unwrap()
            {
                assert!(candidates.contains(&id), "{expression} lost {id}");
            }
        }
    }
}
