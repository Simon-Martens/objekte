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
            configured_cache_policy: None,
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
            configured_cache_policy: None,
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
        configured_cache_policy: None,
    };
    assert!(open(&spec, &CancellationToken::new()).is_err());
    args.load_index = None;
    args.write_index = Some(root.path().join(".objekte").display().to_string());
    let first = open(
        &OpenSpec {
            args: args.clone(),
            cwd: root.path().to_path_buf(),
            configured_cache_policy: None,
        },
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(first.diagnostics.snapshot_save.is_some());
    args.write_index = None;
    args.load_index = Some(root.path().join(".objekte").display().to_string());
    let second = open(
        &OpenSpec {
            args,
            cwd: root.path().to_path_buf(),
            configured_cache_policy: None,
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
        write_index: Some(root.path().join(".objekte").display().to_string()),
        ..Default::default()
    };
    let spec = |args: Arguments| OpenSpec {
        args,
        cwd: root.path().to_path_buf(),
        configured_cache_policy: None,
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
    args.load_index = Some(root.path().join(".objekte").display().to_string());
    let second = open(&spec(args), &CancellationToken::new()).unwrap();
    assert!(second.diagnostics.startup.restored);
    assert!(second.diagnostics.snapshot_save.is_none());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let result = loop {
        if let Some(result) = second.take_background_snapshot_save() {
            break result;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "background cache write timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(result.unwrap().is_some());
    assert!(!second.repository.index().status().unwrap().dirty());
    assert_ne!(std::fs::read(cache).unwrap(), saved);
}

#[test]
fn configured_repository_reuses_cache_unless_build_in_memory_is_selected() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".objekte/collections")).unwrap();
    std::fs::create_dir_all(root.path().join("data")).unwrap();
    std::fs::write(
        root.path().join(".objekte/collections/data.schema.json"),
        br#"{"type":"object","x-objekte":{"name":"data","path":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    let spec = |configured_cache_policy| OpenSpec {
        args: Arguments {
            repo: Some(root.path().to_path_buf()),
            ..Default::default()
        },
        cwd: root.path().to_path_buf(),
        configured_cache_policy,
    };
    let first = open(&spec(None), &CancellationToken::new()).unwrap();
    assert!(!first.diagnostics.startup.restored);
    let cache = first.configuration.cache_save.clone().unwrap();
    first.repository.index().flush_cache(&cache).unwrap();
    let restored = open(&spec(None), &CancellationToken::new()).unwrap();
    assert!(restored.diagnostics.startup.restored);
    let memory = open(
        &spec(Some(CacheOpenPolicy::Ignore)),
        &CancellationToken::new(),
    )
    .unwrap();
    assert!(!memory.diagnostics.startup.restored);
    assert!(memory.configuration.cache_load.is_none());
    assert!(memory.configuration.cache_save.is_none());
    let separate_cache = root.path().join("manual.snapshot");
    memory
        .repository
        .index()
        .flush_cache(&separate_cache)
        .unwrap();
    assert!(!memory.repository.index().status().unwrap().dirty());
    restored.repository.index().shutdown().unwrap();
}

#[test]
fn configured_desktop_open_replaces_invalid_cache() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".objekte/collections")).unwrap();
    std::fs::create_dir_all(root.path().join("data")).unwrap();
    std::fs::write(
        root.path().join(".objekte/collections/data.schema.json"),
        br#"{"type":"object","x-objekte":{"name":"data","path":"data","format":"json","id":"filename"}}"#,
    ).unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    let cache = objekte_search_core::storage::snapshot_path(
        root.path(),
        objekte_search_core::workload::BigramAlphabet::Ascii,
    );
    std::fs::write(&cache, b"invalid cache").unwrap();
    let spec = OpenSpec {
        args: Arguments {
            repo: Some(root.path().to_owned()),
            ..Default::default()
        },
        cwd: root.path().to_owned(),
        configured_cache_policy: None,
    };
    let rebuilt = open(&spec, &CancellationToken::new()).unwrap();
    assert!(!rebuilt.diagnostics.startup.restored);
    assert_ne!(std::fs::read(&cache).unwrap(), b"invalid cache");
    let restored = open(&spec, &CancellationToken::new()).unwrap();
    assert!(restored.diagnostics.startup.restored);
    restored
        .repository
        .index()
        .wait_for_reconciliation()
        .unwrap();
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
        configured_cache_policy: None,
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
fn incremental_desktop_open_rebuilds_invalid_selected_cache_after_construction() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".objekte/collections")).unwrap();
    std::fs::create_dir_all(root.path().join("data")).unwrap();
    std::fs::write(
        root.path().join(".objekte/collections/data.schema.json"),
        br#"{"type":"object","x-objekte":{"name":"data","path":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    let cache = objekte_search_core::storage::snapshot_path(
        root.path(),
        objekte_search_core::workload::BigramAlphabet::Ascii,
    );
    std::fs::write(&cache, b"invalid cache").unwrap();
    let spec = OpenSpec {
        args: Arguments {
            repo: Some(root.path().to_owned()),
            ..Default::default()
        },
        cwd: root.path().to_owned(),
        configured_cache_policy: None,
    };
    let (session, task) = open_incremental(&spec, &CancellationToken::new()).unwrap();
    task.expect("invalid cache must rebuild")
        .wait_complete()
        .unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let saved = loop {
        if let Some(result) = session.take_background_snapshot_save() {
            break result.unwrap();
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cache replacement timed out"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    };
    assert!(saved.is_some());
    assert_ne!(std::fs::read(&cache).unwrap(), b"invalid cache");
    let direct = Index::load_cache(
        root.path(),
        session.configuration.definitions.as_ref().clone(),
        session.configuration.options,
        &cache,
    );
    assert!(
        direct.is_ok(),
        "saved cache failed validation: {:?}",
        direct.err()
    );
    let (restored, task) = open_incremental(&spec, &CancellationToken::new()).unwrap();
    assert!(task.is_none());
    assert!(restored.diagnostics.startup.restored);
}

#[cfg(unix)]
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
        configured_cache_policy: None,
    };
    let (session, task) = open_incremental(&spec, &CancellationToken::new()).unwrap();
    let view = session.repository.index().view().unwrap();
    let row = view.catalogue_rows().next().unwrap();
    assert_eq!(row.relative_path.as_os_str().as_bytes(), b"raw-\xff.json");
    task.unwrap().wait_complete().unwrap();
}
