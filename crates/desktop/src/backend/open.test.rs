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
}
