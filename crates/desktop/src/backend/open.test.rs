use super::*;
use crate::backend::test_support::write_records;

#[test]
fn opens_folder_without_implicit_snapshot_write() {
    let root = tempfile::tempdir().unwrap();
    write_records(root.path());
    let args = Arguments {
        path: vec![root.path().to_path_buf()],
        ..Default::default()
    };
    let session = open(
        &OpenSpec {
            args,
            cwd: root.path().to_path_buf(),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(session.single_collection);
    assert_eq!(session.collections, ["files"]);
    assert!(!session.diagnostics.startup.restored);
    assert!(!root.path().join(".objekte").exists());
}

#[test]
fn repeated_paths_keep_cli_collection_names() {
    let root = tempfile::tempdir().unwrap();
    let left = root.path().join("left");
    let right = root.path().join("right");
    std::fs::create_dir_all(&left).unwrap();
    std::fs::create_dir_all(&right).unwrap();
    write_records(&left);
    write_records(&right);
    let args = Arguments {
        path: vec![left, right],
        ..Default::default()
    };
    let session = open(
        &OpenSpec {
            args,
            cwd: root.path().to_path_buf(),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(!session.single_collection);
    assert_eq!(session.collections, ["left", "right"]);
}

#[test]
fn write_then_load_uses_explicit_snapshot_policy() {
    let root = tempfile::tempdir().unwrap();
    write_records(root.path());
    let mut args = Arguments {
        path: vec![root.path().to_path_buf()],
        load_index: Some(String::new()),
        ..Default::default()
    };
    let spec = OpenSpec {
        args: args.clone(),
        cwd: root.path().to_path_buf(),
    };
    assert!(open(&spec, &CancellationToken::new()).is_err());
    args.load_index = None;
    args.write_index = Some(String::new());
    let first = open(
        &OpenSpec {
            args: args.clone(),
            cwd: root.path().to_path_buf(),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(first.diagnostics.snapshot_save.is_some());
    args.write_index = None;
    args.load_index = Some(String::new());
    let second = open(
        &OpenSpec {
            args,
            cwd: root.path().to_path_buf(),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(second.diagnostics.startup.restored);
    assert!(second.diagnostics.snapshot_save.is_none());
    assert!(second.take_background_snapshot_save().is_none());
}

#[test]
fn desktop_combined_load_and_write_returns_before_source_reconciliation() {
    let root = tempfile::tempdir().unwrap();
    write_records(root.path());
    let mut args = Arguments {
        path: vec![root.path().to_path_buf()],
        write_index: Some(String::new()),
        ..Default::default()
    };
    let spec = |args: Arguments| OpenSpec {
        args,
        cwd: root.path().to_path_buf(),
    };
    let first = open(&spec(args.clone()), &CancellationToken::new()).unwrap();
    assert!(first.diagnostics.snapshot_save.is_some());
    let cache = objekte_search_core::storage::snapshot_path(
        root.path(),
        objekte_search_core::workload::BigramAlphabet::Ascii,
    );
    let saved = std::fs::read(&cache).unwrap();
    std::fs::write(
        root.path().join("a.json"),
        r#"{"title":"Changed","year":2024}"#,
    )
    .unwrap();
    args.load_index = Some(String::new());
    let second = open(&spec(args), &CancellationToken::new()).unwrap();
    assert!(second.diagnostics.startup.restored);
    assert!(second.diagnostics.snapshot_save.is_none());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let result = loop {
        if let Some(result) = second.take_background_snapshot_save() {
            break result;
        }
        assert!(std::time::Instant::now() < deadline, "background cache write timed out");
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(result.unwrap().is_some());
    assert!(!second.repository.index().status().unwrap().dirty());
    assert_ne!(std::fs::read(cache).unwrap(), saved);
}
