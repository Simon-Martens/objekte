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
    let task = task.expect("fresh open must retain construction task");
    let catalogue = session.repository.index().view().unwrap();
    assert_eq!(catalogue.catalogue_rows().count(), 64);
    assert_eq!(task.status().unwrap().discovered_files, 64);
    task.wait_complete().unwrap();
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
    let row = view.catalogue_rows().next().unwrap();
    assert_eq!(row.relative_path.as_os_str().as_bytes(), b"raw-\xff.json");
    if let Some(task) = task {
        task.wait_complete().unwrap();
    }
}

#[test]
fn desktop_backend_saves_and_reopens_an_explicit_index() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let store = root.path().join("store");
    std::fs::create_dir(&source).unwrap();
    write_records(&source);
    let save_args = Arguments {
        path: vec![source.clone()],
        save_index: Some(store.clone()),
        ..Default::default()
    };
    let saved = open(
        &OpenSpec {
            args: save_args,
            cwd: root.path().to_path_buf(),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(store.join("CURRENT").exists());
    assert_eq!(
        saved.repository.index().storage_status().unwrap().origin,
        objekte_search_core::index_storage::status::StorageOrigin::Built
    );
    let load_args = Arguments {
        path: vec![source],
        load_index: Some(store),
        ..Default::default()
    };
    let loaded = open(
        &OpenSpec {
            args: load_args,
            cwd: root.path().to_path_buf(),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        loaded.repository.index().storage_status().unwrap().origin,
        objekte_search_core::index_storage::status::StorageOrigin::Loaded
    );
    assert_eq!(loaded.collections, ["files"]);
}

#[test]
fn desktop_backend_opens_a_custom_index_without_source_paths() {
    let root = tempfile::tempdir().unwrap();
    let store = root.path().join("store");
    write_records(root.path());
    let save_args = Arguments {
        path: vec![root.path().to_path_buf()],
        save_index: Some(store.clone()),
        ..Default::default()
    };
    open(
        &OpenSpec {
            args: save_args,
            cwd: root.path().to_path_buf(),
        },
        &CancellationToken::new(),
    )
    .unwrap();

    let load_args = Arguments {
        load_index: Some(store),
        ..Default::default()
    };
    let loaded = open(
        &OpenSpec {
            args: load_args,
            cwd: root.path().to_path_buf(),
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert_eq!(
        loaded.repository.index().storage_status().unwrap().origin,
        objekte_search_core::index_storage::status::StorageOrigin::Loaded
    );
    assert_eq!(loaded.collections, ["files"]);
    assert_eq!(loaded.paths.root, root.path().canonicalize().unwrap());
}
