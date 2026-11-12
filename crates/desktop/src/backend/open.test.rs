use super::*;
use crate::backend::test_support::write_records;

#[test]
fn staged_plain_open_exposes_catalogue_before_returning_searchable_session() {
    let root = tempfile::tempdir().unwrap();
    write_records(root.path());
    let spec = OpenSpec {
        args: Arguments {
            path: vec![root.path().to_path_buf()],
            ..Default::default()
        },
        cwd: root.path().to_path_buf(),
        configured_cache_policy: None,
    };
    let catalogue = Arc::new(Mutex::new(None));
    let observed = Arc::clone(&catalogue);
    let session = open_staged(&spec, &CancellationToken::new(), move |task, root, collections| {
        assert!(root.is_absolute());
        assert_eq!(collections, ["files"]);
        *observed.lock().unwrap() = Some(task);
    }).unwrap();
    let task = catalogue.lock().unwrap().take().unwrap();
    let files = task.catalogue().unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0].relative_path, PathBuf::from("a.json"));
    assert_eq!(session.repository.index().status().unwrap().live_records, 3);
    assert!(!session.diagnostics.startup.restored);
    assert!(!root.path().join(".objekte").exists());
}

#[test]
fn staged_explicit_write_then_cached_open_preserves_search_ready_path() {
    let root = tempfile::tempdir().unwrap();
    write_records(root.path());
    let cache_dir = root.path().join(".objekte");
    let mut args = Arguments {
        path: vec![root.path().to_path_buf()],
        write_index: Some(cache_dir.display().to_string()),
        ..Default::default()
    };
    let spec = |args: Arguments| OpenSpec {
        args,
        cwd: root.path().to_path_buf(),
        configured_cache_policy: None,
    };
    let first = open_staged(&spec(args.clone()), &CancellationToken::new(), |_, _, _| {})
        .unwrap();
    assert!(!first.diagnostics.startup.restored);
    let deadline = Instant::now() + std::time::Duration::from_secs(5);
    loop {
        if let Some(result) = first.take_background_snapshot_save() {
            assert!(result.unwrap().is_some());
            break;
        }
        assert!(Instant::now() < deadline, "background snapshot did not finish");
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    args.write_index = None;
    args.load_index = Some(cache_dir.display().to_string());
    let second = open_staged(&spec(args), &CancellationToken::new(), |task, _, _| {
        assert_eq!(task.status().unwrap().phase, objekte_search_core::lifecycle::StagedBuildPhase::Complete);
    }).unwrap();
    assert!(second.diagnostics.startup.restored);
    assert_eq!(second.repository.index().status().unwrap().live_records, 3);
}

#[test]
fn staged_configured_open_restores_cache_unless_memory_build_is_selected() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join(".objekte/collections")).unwrap();
    std::fs::create_dir_all(root.path().join("data")).unwrap();
    std::fs::write(
        root.path().join(".objekte/collections/data.schema.json"),
        br#"{"type":"object","x-objekte":{"name":"data","path":"data","format":"json","id":"filename"}}"#,
    ).unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    let spec = |configured_cache_policy| OpenSpec {
        args: Arguments { repo: Some(root.path().to_path_buf()), ..Default::default() },
        cwd: root.path().to_path_buf(),
        configured_cache_policy,
    };
    let first = open_staged(&spec(None), &CancellationToken::new(), |task, _, collections| {
        assert_eq!(collections, ["data"]);
        assert!(matches!(task.status().unwrap().phase, objekte_search_core::lifecycle::StagedBuildPhase::Discovering
            | objekte_search_core::lifecycle::StagedBuildPhase::BuildingBase
            | objekte_search_core::lifecycle::StagedBuildPhase::SearchReady
            | objekte_search_core::lifecycle::StagedBuildPhase::Complete));
    }).unwrap();
    assert!(!first.diagnostics.startup.restored);
    let cache = first.configuration.cache_save.clone().unwrap();
    first.repository.index().flush_cache(&cache).unwrap();
    let restored = open_staged(&spec(None), &CancellationToken::new(), |_, _, _| {}).unwrap();
    assert!(restored.diagnostics.startup.restored);
    let memory = open_staged(&spec(Some(CacheOpenPolicy::Ignore)), &CancellationToken::new(), |_, _, _| {}).unwrap();
    assert!(!memory.diagnostics.startup.restored);
    assert!(memory.configuration.cache_load.is_none());
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
        assert!(std::time::Instant::now() < deadline, "background cache write timed out");
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
    memory.repository.index().flush_cache(&separate_cache).unwrap();
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
        args: Arguments { repo: Some(root.path().to_owned()), ..Default::default() },
        cwd: root.path().to_owned(),
        configured_cache_policy: None,
    };
    let rebuilt = open(&spec, &CancellationToken::new()).unwrap();
    assert!(!rebuilt.diagnostics.startup.restored);
    assert_ne!(std::fs::read(&cache).unwrap(), b"invalid cache");
    let restored = open(&spec, &CancellationToken::new()).unwrap();
    assert!(restored.diagnostics.startup.restored);
    restored.repository.index().wait_for_reconciliation().unwrap();
}
