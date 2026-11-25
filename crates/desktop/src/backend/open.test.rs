use super::*;
use crate::backend::test_support::write_records;

#[test]
fn opens_folder_without_index_artifacts() {
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
fn incremental_desktop_open_exposes_catalogue_and_finishes_without_implicit_cache() {
    let root = tempfile::tempdir().unwrap();
    for number in 0..64 {
        std::fs::write(
            root.path().join(format!("{number:04}.json")),
            format!(r#"{{"title":"Record {number}"}}"#),
        )
        .unwrap();
    }
    let spec = OpenSpec {
        args: Arguments {
            path: vec![root.path().to_owned()],
            ..Default::default()
        },
        cwd: root.path().to_owned(),
    };
    let (session, task) = open_incremental(&spec, &CancellationToken::new()).unwrap();
    let catalogue = session.repository.index().view().unwrap();
    assert_eq!(catalogue.catalogue_rows().count(), 64);
    if let Some(task) = task {
        assert_eq!(task.status().unwrap().discovered_files, 64);
        task.wait_complete().unwrap();
    }
    assert!(
        session
            .repository
            .index()
            .view()
            .unwrap()
            .initial_construction_complete()
    );
    assert!(!root.path().join(".objekte").exists());
}

#[test]
fn incremental_catalogue_preserves_non_utf8_file_paths() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};

    let root = tempfile::tempdir().unwrap();
    let filename = std::ffi::OsString::from_vec(b"raw-\xff.json".to_vec());
    std::fs::write(root.path().join(filename), br#"{"title":"A"}"#).unwrap();
    let spec = OpenSpec {
        args: Arguments {
            path: vec![root.path().to_owned()],
            ..Default::default()
        },
        cwd: root.path().to_owned(),
    };
    let (session, task) = open_incremental(&spec, &CancellationToken::new()).unwrap();
    let view = session.repository.index().view().unwrap();
    let rows = view.catalogue_rows().collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        1,
        "status={:?} paths={:?}",
        task.as_ref().map(|task| task.status().unwrap()),
        view.file_paths().collect::<Vec<_>>()
    );
    let row = &rows[0];
    assert_eq!(row.relative_path.as_os_str().as_bytes(), b"raw-\xff.json");
    if let Some(task) = task {
        task.wait_complete().unwrap();
    }
}
