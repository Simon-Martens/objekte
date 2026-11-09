use super::*;

#[test]
fn shared_cli_and_tui_loader_enables_bounded_verified_cache() {
    assert_eq!(
        repository_options(&Arguments::default()).verified_cache_bytes,
        64 * 1024 * 1024,
    );
}

#[test]
fn configured_repository_uses_lifecycle_cache_for_load_and_write_flags() {
    let root = tempfile::tempdir().unwrap();
    let schemas = root.path().join(".objekte/collections");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    std::fs::write(
        schemas.join("data.schema.json"),
        br#"{
            "type":"object",
            "properties":{"title":{"type":"string"}},
            "x-objekte":{"name":"data","path":"data","format":"json","id":"filename"}
        }"#,
    )
    .unwrap();
    let args = Arguments {
        repo: Some(root.path().to_owned()),
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    let options = RepositoryOptions::default();
    let write = SnapshotOpenPolicy::new(SnapshotMode::Disabled, None);
    let (built, _, save) =
        open_repository(root.path(), &configured, options, write, true, true, false).unwrap();
    assert!(!built.startup().restored);
    assert!(save.is_some());
    let cache = objekte_search_core::storage::snapshot_path(root.path(), options.bigram_alphabet);
    assert!(cache.is_file());
    let load = SnapshotOpenPolicy::new(SnapshotMode::Required, None);
    let (restored, _, save) =
        open_repository(root.path(), &configured, options, load, false, true, false).unwrap();
    assert!(restored.startup().restored);
    assert!(save.is_none());
    restored.index().wait_for_reconciliation().unwrap();
}

#[test]
fn explicit_folder_load_replaces_invalid_cache_and_restores_next_time() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.json"), br#"{"title":"A"}"#).unwrap();
    std::fs::create_dir(root.path().join(".objekte")).unwrap();
    let args = Arguments {
        path: vec![root.path().to_owned()],
        load_index: Some(root.path().join(".objekte").display().to_string()),
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    let options = repository_options(&args);
    let cache = objekte_search_core::storage::snapshot_path(root.path(), options.bigram_alphabet);
    std::fs::write(&cache, b"invalid cache").unwrap();
    let policy = snapshot_policy(&args, &paths);
    let (rebuilt, _, _) = open_repository(
        &paths.root, &configured, options, policy.clone(), false, false, false,
    ).unwrap();
    assert!(!rebuilt.startup().restored);
    assert_ne!(std::fs::read(&cache).unwrap(), b"invalid cache");
    let (restored, _, _) = open_repository(
        &paths.root, &configured, options, policy, false, false, false,
    ).unwrap();
    assert!(restored.startup().restored);
    restored.index().wait_for_reconciliation().unwrap();
}

#[test]
fn configured_open_uses_the_already_loaded_schema() {
    let root = tempfile::tempdir().unwrap();
    let schemas = root.path().join(".objekte/collections");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    let schema = schemas.join("data.schema.json");
    std::fs::write(
        &schema,
        br#"{
            "type":"object",
            "properties":{"title":{"type":"string"}},
            "x-objekte":{"name":"data","path":"data","format":"json","id":"filename"}
        }"#,
    )
    .unwrap();
    let args = Arguments {
        repo: Some(root.path().to_owned()),
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    std::fs::write(&schema, b"invalid JSON after configuration was captured").unwrap();
    let (opened, _, _) = open_repository(
        root.path(),
        &configured,
        RepositoryOptions::default(),
        SnapshotOpenPolicy::new(SnapshotMode::Disabled, None),
        false,
        true,
        false,
    )
    .unwrap();
    assert_eq!(opened.collections()[0].definition.name, "data");
}

#[test]
fn explicit_path_cache_write_cannot_replace_a_schema_hardlink() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.json"), br#"{"title":"A"}"#).unwrap();
    let schema = root.path().join("schema.json");
    let bytes = br#"{"type":"object","properties":{"title":{"type":"string"}}}"#;
    std::fs::write(&schema, bytes).unwrap();
    let cache_dir = root.path().join("cache");
    std::fs::create_dir(&cache_dir).unwrap();
    let cache = objekte_search_core::storage::snapshot_path_in(
        &cache_dir,
        RepositoryOptions::default().bigram_alphabet,
    );
    std::fs::hard_link(&schema, &cache).unwrap();
    let args = Arguments {
        path: vec![root.path().to_owned()],
        schema: Some(schema.clone()),
        write_index: Some(cache_dir.to_string_lossy().into_owned()),
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    let error = open_repository(
        &paths.root,
        &configured,
        repository_options(&args),
        snapshot_policy(&args, &paths),
        true,
        false,
        false,
    )
    .err()
    .unwrap();
    assert!(error.contains("aliases configuration file"), "{error}");
    assert_eq!(std::fs::read(schema).unwrap(), bytes);
    assert_eq!(std::fs::read(cache).unwrap(), bytes);
}

#[test]
fn repository_loader_matches_core_definitions_and_protects_schema_paths() {
    let root = tempfile::tempdir().unwrap();
    let schemas = root.path().join(".objekte/collections");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    let schema = schemas.join("files.schema.json");
    std::fs::write(
        &schema,
        br#"{
            "type":"object",
            "properties":{"title":{"type":"string","x-index":true}},
            "x-objekte":{"name":"files","path":"data","format":"json","id":"filename"}
        }"#,
    )
    .unwrap();
    let args = Arguments {
        repo: Some(root.path().to_owned()),
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let cli = configure_definitions(&args, &paths).unwrap();
    let core = load_repository_configuration(root.path()).unwrap();
    assert_eq!(cli.schema_files, core.schema_files);
    assert_eq!(cli.protected_files, core.protected_files);
    assert_eq!(cli.schema_fields, core.schema_fields);
    assert_eq!(cli.definitions.len(), core.definitions.len());
    assert_eq!(cli.definitions[0].name, core.definitions[0].name);
    assert_eq!(
        cli.definitions[0].source_path,
        core.definitions[0].source_path
    );
    assert_eq!(cli.definitions[0].format, core.definitions[0].format);
    assert_eq!(cli.definitions[0].identity, core.definitions[0].identity);
    assert_eq!(cli.protected_files, vec![schema.canonicalize().unwrap()]);
}

#[test]
fn configured_cache_directory_keeps_unrelated_source_siblings() {
    let root = tempfile::tempdir().unwrap();
    let schemas = root.path().join(".objekte/collections");
    let data = root.path().join("data");
    let cache = data.join("cache[odd]");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(data.join("a.json"), br#"{"title":"A"}"#).unwrap();
    std::fs::write(cache.join("keep.json"), br#"{"title":"keep"}"#).unwrap();
    std::fs::write(
        schemas.join("files.schema.json"),
        br#"{
            "type":"object",
            "properties":{"title":{"type":"string"}},
            "x-objekte":{"name":"files","path":"data","format":"json","id":"filename"}
        }"#,
    )
    .unwrap();
    let args = Arguments {
        repo: Some(root.path().to_owned()),
        write_index: Some(cache.to_string_lossy().into_owned()),
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let mut configured = configure_definitions(&args, &paths).unwrap();
    let definition = &configured.definitions[0];
    assert!(definition.discovery.exclude_files.is_empty());
    assert_eq!(
        definition.discovery.exclude_directories,
        vec![PathBuf::from(".objekte")]
    );

    configured.definitions[0].discovery.recursive = true;
    let (repo, _, saved) = open_repository(
        root.path(),
        &configured,
        RepositoryOptions::default(),
        SnapshotOpenPolicy::new(SnapshotMode::Disabled, Some(cache)),
        true,
        true,
        false,
    )
    .unwrap();
    assert!(saved.is_some());
    let view = repo.index().view().unwrap();
    assert!(
        view.file_handle_for_path(std::path::Path::new("data/a.json"))
            .is_some()
    );
    assert!(
        view.file_handle_for_path(std::path::Path::new("data/cache[odd]/keep.json"))
            .is_some()
    );
    assert_eq!(repo.index().status().unwrap().indexed_files, 2);
}

#[test]
fn default_index_directory_retains_legacy_duplicate_exclusion() {
    let root = tempfile::tempdir().unwrap();
    let args = Arguments {
        write_index: Some(String::new()),
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    assert_eq!(
        configured.definitions[0].discovery.exclude_directories,
        vec![PathBuf::from(".objekte"), PathBuf::from(".objekte")]
    );
    assert!(configured.definitions[0].discovery.exclude_files.is_empty());
    assert!(
        configured.definitions[0]
            .discovery
            .exclude
            .iter()
            .all(|pattern| !pattern.contains("objekte-snapshot"))
    );
}

#[test]
fn manual_schema_still_ignores_repository_metadata() {
    let root = tempfile::tempdir().unwrap();
    let schema = root.path().join("selected.json");
    std::fs::write(
        &schema,
        br#"{
            "type":"object",
            "properties":{"title":{"type":"string"}},
            "x-objekte":{"name":"not-the-files-collection"}
        }"#,
    )
    .unwrap();
    let args = Arguments {
        schema: Some(schema.clone()),
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    assert_eq!(configured.definitions[0].name, "files");
    assert!(configured.ignored_repository_metadata);
    assert_eq!(
        configured.schema_files,
        vec![schema.canonicalize().unwrap()]
    );
    assert_eq!(configured.protected_files, configured.schema_files);
}
