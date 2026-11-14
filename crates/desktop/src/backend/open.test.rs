use super::*;
use crate::backend::test_support::write_records;
use objekte_search_core::index_storage::{IndexResidency, IndexStorageLocation};

#[test]
fn shared_storage_opens_all_desktop_locations_and_restores_disk_generations() {
    let root = tempfile::tempdir().unwrap();
    let custom = tempfile::tempdir().unwrap();
    write_records(root.path());
    for location in [
        IndexStorageLocation::MemoryOnly,
        IndexStorageLocation::SystemTemporary,
        IndexStorageLocation::RepositoryDirectory,
        IndexStorageLocation::CustomDirectory(custom.path().to_path_buf()),
    ] {
        let disk = location != IndexStorageLocation::MemoryOnly;
        let spec = OpenSpec {
            args: Arguments {
                path: vec![root.path().to_path_buf()],
                ..Default::default()
            },
            cwd: root.path().to_path_buf(),
            storage_policy: Some(IndexStoragePolicy {
                location,
                residency: if disk {
                    IndexResidency::OnDemand
                } else {
                    IndexResidency::FullyResident
                },
            }),
        };
        let first = open(&spec, &CancellationToken::new()).unwrap();
        let status = first.repository.index().storage_status().unwrap();
        assert_eq!(status.managed_directory.is_some(), disk);
        assert_eq!(status.durable_generation.is_some(), disk);
        assert!(
            !root
                .path()
                .join(".objekte")
                .join("search-index.json")
                .exists()
        );
        first.repository.index().shutdown().unwrap();
        if disk {
            let reopened = open(&spec, &CancellationToken::new()).unwrap();
            assert!(reopened.diagnostics.startup.restored);
            assert!(
                reopened
                    .repository
                    .index()
                    .storage_status()
                    .unwrap()
                    .durable_generation
                    .is_some()
            );
            reopened.repository.index().shutdown().unwrap();
        }
    }
}

#[test]
fn managed_incremental_open_exposes_catalogue_before_durable_completion() {
    let root = tempfile::tempdir().unwrap();
    write_records(root.path());
    let spec = OpenSpec {
        args: Arguments {
            path: vec![root.path().to_path_buf()],
            ..Default::default()
        },
        cwd: root.path().to_path_buf(),
        storage_policy: Some(IndexStoragePolicy {
            location: IndexStorageLocation::RepositoryDirectory,
            residency: IndexResidency::OnDemand,
        }),
    };
    let (session, task) = open_incremental(&spec, &CancellationToken::new()).unwrap();
    if let Some(task) = &task {
        assert!(task.status().unwrap().catalogue_complete);
    }
    if let Some(task) = task {
        task.wait_complete().unwrap();
    }
    assert!(session.repository.index().storage_status().unwrap().durable_generation.is_some());
    session.repository.index().shutdown().unwrap();
}

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
            storage_policy: None,
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
            storage_policy: None,
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
        storage_policy: None,
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
        storage_policy: None,
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
    task.unwrap().wait_complete().unwrap();
}

#[test]
fn corrupt_managed_open_reports_classified_recovery_guidance() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.json"), r#"{"title":"Alpha"}"#).unwrap();
    let spec = OpenSpec {
        storage_policy: Some(IndexStoragePolicy {
            location: IndexStorageLocation::RepositoryDirectory,
            residency: IndexResidency::OnDemand,
        }),
        args: Arguments { path: vec![root.path().to_path_buf()], ..Default::default() },
        cwd: root.path().to_path_buf(),
    };
    let first = open(&spec, &CancellationToken::new()).unwrap();
    let directory = first.repository.index().storage_status().unwrap().managed_directory.unwrap();
    first.repository.index().shutdown().unwrap();
    std::fs::write(directory.join("manifest.bin"), b"invalid manifest").unwrap();
    let error = match open(&spec, &CancellationToken::new()) {
        Ok(_) => panic!("corrupt store unexpectedly opened"),
        Err(error) => error,
    };
    assert!(error.message.contains("Index data is damaged"), "{}", error.message);
    assert!(error.message.contains("Rebuild the index"));
}
