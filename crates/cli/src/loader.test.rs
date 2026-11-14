use super::*;
use objekte_search_core::index_storage::IndexStorageLocation;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::lifecycle_caller::IndexedRepository;
use objekte_search_core::source::SourceRegistry;

fn open_with_policy(
    args: &Arguments,
    paths: &ResolvedPaths,
    configured: &ConfiguredDefinitions,
) -> Result<IndexedRepository, String> {
    let storage = args.resolved_index_storage_policy(paths)?;
    let index = Index::open_with_storage(
        &paths.root,
        configured.definitions.clone(),
        repository_options(args),
        storage,
        SourceRegistry::builtin(),
    )
    .map_err(|error| error.to_string())?;
    IndexedRepository::new(index).map_err(|error| error.to_string())
}

#[test]
fn shared_cli_and_tui_loader_enables_bounded_verified_cache() {
    assert_eq!(
        repository_options(&Arguments::default()).verified_cache_bytes,
        64 * 1024 * 1024,
    );
}

#[test]
fn configured_repository_reopens_its_managed_generation() {
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
        index_storage: IndexStorageChoice::Repository,
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    let built = open_with_policy(&args, &paths, &configured).unwrap();
    assert!(!built.index().restored_from_storage());
    assert!(root.path().join(".objekte/indexes").is_dir());
    let restored = open_with_policy(&args, &paths, &configured).unwrap();
    assert!(restored.index().restored_from_storage());
    restored.index().wait_for_reconciliation().unwrap();
}

#[test]
fn selected_storage_removes_recognized_old_snapshot_without_loading_it() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("a.json"), br#"{"title":"A"}"#).unwrap();
    std::fs::create_dir(root.path().join(".objekte")).unwrap();
    let args = Arguments {
        path: vec![root.path().to_owned()],
        index_storage: IndexStorageChoice::Repository,
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    let cache = root.path().join(".objekte/search-core-ascii.snapshot");
    std::fs::write(&cache, b"OBJSNAP1\x1b\0\0\0").unwrap();
    let rebuilt = open_with_policy(&args, &paths, &configured).unwrap();
    assert!(!rebuilt.index().restored_from_storage());
    assert!(!cache.exists());
    let restored = open_with_policy(&args, &paths, &configured).unwrap();
    assert!(restored.index().restored_from_storage());
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
    let opened = open_with_policy(&args, &paths, &configured).unwrap();
    assert_eq!(opened.collections()[0].definition.name, "data");
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
    assert_eq!(cli.schema_fields, core.schema_fields);
    assert_eq!(cli.definitions.len(), core.definitions.len());
    assert_eq!(cli.definitions[0].name, core.definitions[0].name);
    assert_eq!(
        cli.definitions[0].source_path,
        core.definitions[0].source_path
    );
    assert_eq!(cli.definitions[0].format, core.definitions[0].format);
    assert_eq!(cli.definitions[0].identity, core.definitions[0].identity);
}

#[test]
fn configured_custom_base_keeps_unrelated_source_siblings() {
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
        index_storage: IndexStorageChoice::Custom,
        index_directory: Some(cache.clone()),
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
    let repo = open_with_policy(&args, &paths, &configured).unwrap();
    assert!(cache.join("objekte-indexes").is_dir());
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
fn custom_index_base_inside_source_excludes_only_objekte_namespace() {
    let root = tempfile::tempdir().unwrap();
    let custom = root.path().join("chosen-base");
    std::fs::create_dir(&custom).unwrap();
    let args = Arguments {
        path: vec![root.path().to_owned()],
        index_storage: IndexStorageChoice::Custom,
        index_directory: Some(custom.clone()),
        ..Arguments::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    assert_eq!(
        configured.definitions[0].discovery.exclude_directories,
        vec![
            PathBuf::from(".objekte"),
            PathBuf::from("chosen-base/objekte-indexes")
        ]
    );
    assert!(
        !configured.definitions[0]
            .discovery
            .exclude_directories
            .contains(&PathBuf::from("chosen-base"))
    );
}

#[test]
fn default_memory_policy_excludes_configuration_directory_once() {
    let root = tempfile::tempdir().unwrap();
    let args = Arguments::default();
    let paths = args.resolve_paths(root.path()).unwrap();
    let configured = configure_definitions(&args, &paths).unwrap();
    assert_eq!(
        configured.definitions[0].discovery.exclude_directories,
        vec![PathBuf::from(".objekte")]
    );
    assert!(configured.definitions[0].discovery.exclude_files.is_empty());
    assert_eq!(
        args.index_storage_policy().unwrap().location,
        IndexStorageLocation::MemoryOnly
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
}
