use objekte_core::bigram::{
    BigramFilter, BigramIndexBuilder, BigramOverlay, BigramSnapshot,
    build_bigram_index_from_content, extract_bigrams,
};
use objekte_core::file_picker::FilePicker;
use objekte_core::{JsonQuery, Repository};
use std::{fs, path::Path, process::Command};

#[test]
fn public_bigram_api_indexes_caller_content_and_tracks_changes() {
    let contents: &[Option<&[u8]>] = &[Some(b"alpha"), None, Some(b"beta")];
    let index = build_bigram_index_from_content(contents);
    let candidates = index.query(b"alpha").unwrap();
    assert!(BigramFilter::is_candidate(&candidates, 0));
    assert!(!BigramFilter::is_candidate(&candidates, 1));
    assert!(!BigramFilter::is_candidate(&candidates, 2));

    let mut overlay = BigramOverlay::new(contents.len());
    overlay.modify_file(0, b"omega");
    overlay.delete_file(2);
    assert_eq!(overlay.query_modified(&extract_bigrams(b"om")), vec![0]);
    assert_eq!(overlay.tombstones()[0] & (1 << 2), 1 << 2);

    let builder = BigramIndexBuilder::new(2);
    let skip = BigramIndexBuilder::new(2);
    builder.add_file_content(&skip, 0, b"xy");
    builder.add_file_content(&skip, 1, b"zz");
    assert!(builder.compress(None).has_key(u16::from_be_bytes(*b"xy")));
}

#[test]
fn compressed_filter_roundtrips_dense_sparse_and_skip_columns() {
    let contents = (0..4096)
        .map(|slot| {
            let mut text = b"ordinary text ".to_vec();
            if slot % 2 == 0 {
                text.extend_from_slice(b"ab");
            }
            if slot % 20 == 0 {
                text.extend_from_slice(b"zq");
            }
            text
        })
        .collect::<Vec<_>>();
    let refs = contents
        .iter()
        .map(|content| Some(content.as_slice()))
        .collect::<Vec<_>>();
    let index = build_bigram_index_from_content(&refs);
    assert!(index.dense_count() > 0);
    assert!(index.sparse_count() > 0);
    assert!(index.skip_index().is_some());
    let encoded = index.to_bytes().unwrap();
    let restored = BigramFilter::from_bytes(&encoded).unwrap();
    for pattern in [b"ab".as_slice(), b"zq", b"zq ab", b"ordinary", b"missing"] {
        assert_eq!(restored.query(pattern), index.query(pattern));
    }
    assert!(BigramFilter::from_bytes(&encoded[..encoded.len() - 1]).is_err());
}

#[test]
fn snapshot_restores_slot_files_and_rejects_source_changes() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a.json"), r#"{"text":"alpha"}"#).unwrap();
    fs::write(temp.path().join("b.json"), r#"{"text":"beta"}"#).unwrap();
    let picker = FilePicker::open(temp.path()).unwrap();
    let path = temp.path().join("index.bigram");
    picker.save_bigram_snapshot(&path).unwrap();
    let mut repository = Repository::open(temp.path()).unwrap();
    repository
        .save_bigram_snapshot("@root", &temp.path().join("repository.bigram"))
        .unwrap();
    let restored = BigramSnapshot::load_checked(&path, temp.path(), false).unwrap();
    assert_eq!(restored.files.len(), 2);
    assert_eq!(restored.files[0].id, "a");
    assert_eq!(restored.files[0].filename, "a.json");
    assert_eq!(restored.files[0].size_bytes, 16);
    assert!(
        restored.files[0]
            .modified_unix_nanos
            .parse::<i128>()
            .is_ok()
    );
    assert_eq!(restored.candidate_files(b"alpha")[0].id, "a");
    fs::write(temp.path().join("a.json"), r#"{"text":"changed"}"#).unwrap();
    assert!(BigramSnapshot::load_checked(&path, temp.path(), false).is_err());
    assert!(
        picker
            .save_bigram_snapshot(&temp.path().join("stale.bigram"))
            .is_err()
    );

    fs::write(temp.path().join("c.json"), r#"{"text":"gamma"}"#).unwrap();
    assert!(BigramSnapshot::load_checked(&path, temp.path(), false).is_err());

    let mut bytes = fs::read(&path).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&path, bytes).unwrap();
    assert!(BigramSnapshot::load(&path).is_err());
}

fn git(root: &Path, args: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn snapshot_records_git_commit_and_rejects_a_new_commit() {
    if Command::new("git").arg("--version").output().is_err() {
        return;
    }
    let temp = tempfile::tempdir().unwrap();
    git(temp.path(), &["init", "-q"]);
    fs::write(temp.path().join("a.json"), r#"{"text":"alpha"}"#).unwrap();
    git(temp.path(), &["add", "a.json"]);
    git(
        temp.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.org",
            "commit",
            "-qm",
            "first",
        ],
    );
    let head = git(temp.path(), &["rev-parse", "HEAD"]);
    let picker = FilePicker::open(temp.path()).unwrap();
    let path = temp.path().join("index.bigram");
    picker.save_bigram_snapshot(&path).unwrap();
    let restored = BigramSnapshot::load_checked(&path, temp.path(), false).unwrap();
    assert_eq!(restored.git_commit.as_deref(), Some(head.as_str()));
    let cache = Repository::open(temp.path())
        .unwrap()
        .save_bigram_cache("@root")
        .unwrap();
    let cached_bytes = fs::read(&cache).unwrap();

    fs::write(temp.path().join("README"), "another commit").unwrap();
    git(temp.path(), &["add", "README"]);
    git(
        temp.path(),
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.org",
            "commit",
            "-qm",
            "second",
        ],
    );
    assert!(BigramSnapshot::load_checked(&path, temp.path(), false).is_err());
    let mut reopened = Repository::open(temp.path()).unwrap();
    reopened.warm_bigram_index("@root").unwrap();
    assert_eq!(reopened.metrics().secondary_snapshot_loads, 1);
    assert_eq!(fs::read(&cache).unwrap(), cached_bytes);
}

#[test]
fn repository_loads_updates_and_resaves_its_bigram_cache() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a.json"), r#"{"text":"alpha"}"#).unwrap();
    let mut repo = Repository::open(temp.path()).unwrap();
    let cache = repo.save_bigram_cache("@root").unwrap();
    assert!(cache.starts_with(temp.path().join(".objekte/.cache")));
    assert!(cache.with_extension("secondary").is_file());
    assert_eq!(
        BigramSnapshot::load_checked(&cache, temp.path(), false)
            .unwrap()
            .changed_slots
            .len(),
        0
    );

    fs::write(temp.path().join("a.json"), r#"{"text":"omega"}"#).unwrap();
    let mut reopened = Repository::open(temp.path()).unwrap();
    reopened.warm_bigram_index("@root").unwrap();
    assert_eq!(reopened.metrics().bigram_snapshot_loads, 1);
    assert_eq!(reopened.metrics().secondary_snapshot_loads, 1);
    let query = |term: &str| JsonQuery {
        filter: format!("text ~ '{term}'"),
        collections: Some(vec!["@root".into()]),
        count: true,
        ..Default::default()
    };
    assert_eq!(reopened.query_json(&query("omega")).unwrap().total, Some(1));
    assert_eq!(reopened.query_json(&query("alpha")).unwrap().total, Some(0));
    let updated = BigramSnapshot::load_checked(&cache, temp.path(), false).unwrap();
    assert_eq!(updated.changed_slots, vec![0]);
    assert!(updated.changed_bigrams[&0].contains(&u16::from_be_bytes(*b"om")));
    let bytes = fs::read(&cache).unwrap();

    let mut reopened_again = Repository::open(temp.path()).unwrap();
    reopened_again.warm_bigram_index("@root").unwrap();
    assert_eq!(reopened_again.metrics().bigram_snapshot_loads, 1);
    assert_eq!(reopened_again.metrics().secondary_snapshot_loads, 1);
    assert_eq!(fs::read(&cache).unwrap(), bytes);

    fs::write(temp.path().join("a.json"), r#"{"text":"delta"}"#).unwrap();
    reopened_again.refresh("@root").unwrap();
    let refreshed = BigramSnapshot::load_checked(&cache, temp.path(), false).unwrap();
    assert!(refreshed.changed_bigrams[&0].contains(&u16::from_be_bytes(*b"de")));
    assert_eq!(
        reopened_again.query_json(&query("delta")).unwrap().total,
        Some(1)
    );

    fs::write(temp.path().join("a.json"), r#"{"text":"epsilon"}"#).unwrap();
    let mut changed_again = Repository::open(temp.path()).unwrap();
    changed_again.warm_bigram_index("@root").unwrap();
    assert_eq!(changed_again.metrics().secondary_snapshot_loads, 1);
    assert_eq!(
        changed_again.query_json(&query("epsilon")).unwrap().total,
        Some(1)
    );
    assert!(
        BigramSnapshot::load_checked(&cache, temp.path(), false)
            .unwrap()
            .changed_bigrams[&0]
            .contains(&u16::from_be_bytes(*b"ep"))
    );

    fs::write(temp.path().join("b.json"), r#"{"text":"beta"}"#).unwrap();
    let mut with_added_file = Repository::open(temp.path()).unwrap();
    with_added_file.warm_bigram_index("@root").unwrap();
    assert_eq!(with_added_file.metrics().bigram_snapshot_loads, 0);
    let rebuilt = BigramSnapshot::load_checked(&cache, temp.path(), false).unwrap();
    assert_eq!(rebuilt.files.len(), 2);
    assert!(rebuilt.changed_slots.is_empty());

    with_added_file.reindex_bigram_cache("@root").unwrap();
    assert!(
        BigramSnapshot::load_checked(&cache, temp.path(), false)
            .unwrap()
            .changed_slots
            .is_empty()
    );

    let mut corrupt = fs::read(&cache).unwrap();
    *corrupt.last_mut().unwrap() ^= 1;
    fs::write(&cache, corrupt).unwrap();
    let mut after_corruption = Repository::open(temp.path()).unwrap();
    after_corruption.warm_bigram_index("@root").unwrap();
    assert_eq!(after_corruption.metrics().bigram_snapshot_loads, 0);
    assert_eq!(after_corruption.metrics().secondary_snapshot_loads, 0);
    assert!(BigramSnapshot::load_checked(&cache, temp.path(), false).is_ok());
}

#[test]
fn damaged_secondary_cache_is_rebuilt_once() {
    let temp = tempfile::tempdir().unwrap();
    fs::write(temp.path().join("a.json"), r#"{"text":"alpha"}"#).unwrap();
    let cache = Repository::open(temp.path())
        .unwrap()
        .save_bigram_cache("@root")
        .unwrap();
    let sidecar = cache.with_extension("secondary");
    let mut bytes = fs::read(&sidecar).unwrap();
    *bytes.last_mut().unwrap() ^= 1;
    fs::write(&sidecar, bytes).unwrap();

    let mut rebuilt = Repository::open(temp.path()).unwrap();
    rebuilt.warm_bigram_index("@root").unwrap();
    assert_eq!(rebuilt.metrics().bigram_snapshot_loads, 1);
    assert_eq!(rebuilt.metrics().secondary_snapshot_loads, 0);

    let mut restored = Repository::open(temp.path()).unwrap();
    restored.warm_bigram_index("@root").unwrap();
    assert_eq!(restored.metrics().secondary_snapshot_loads, 1);
}
