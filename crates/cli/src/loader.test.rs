use super::*;

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
fn legacy_explicit_cache_directory_excludes_siblings_and_owned_temps() {
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
        vec![PathBuf::from(".objekte"), PathBuf::from("cache[odd]")]
    );

    configured.definitions[0].discovery.recursive = true;
    let repo = Repository::open_with_snapshot_policy(
        root.path(),
        configured.definitions,
        RepositoryOptions::default(),
        SnapshotOpenPolicy::new(SnapshotMode::Disabled, None),
    )
    .unwrap();
    let files = repo
        .files()
        .files
        .iter()
        .map(|file| file.relative_path.clone())
        .collect::<Vec<_>>();
    assert_eq!(files, vec![PathBuf::from("data/a.json")]);
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
