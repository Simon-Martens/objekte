use super::*;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceFormat;

#[test]
fn configured_memory_open_ignores_existing_deprecated_snapshot() {
    let root = tempfile::tempdir().unwrap();
    let schemas = root.path().join(".objekte/collections");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(root.path().join("data/a.json"), r#"{"title":"A"}"#).unwrap();
    std::fs::write(
        schemas.join("files.schema.json"),
        br#"{"type":"object","properties":{"title":{"type":"string"}},"x-objekte":{"name":"files","path":"data","format":"json","id":"filename"}}"#,
    )
    .unwrap();
    let cache_path = root.path().join(".objekte/search-core-ascii.snapshot");
    std::fs::write(&cache_path, b"invalid snapshot").unwrap();
    let args = Arguments {
        repo: Some(root.path().to_owned()),
        fts: Some("A".into()),
        ..Arguments::default()
    };
    let mut output = Vec::new();
    assert_eq!(
        run(&args, &mut output, &CancellationToken::new()),
        Exit::Success
    );
    assert_eq!(std::fs::read(cache_path).unwrap(), b"invalid snapshot");
    let events: Vec<serde_json::Value> = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert!(events.iter().any(|event| event["type"] == "query_complete"));
    assert_eq!(events.last().unwrap()["index_action"], "built_in_memory");
}

#[test]
fn unavailable_source_is_a_partial_query_error_not_a_success() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    let first = data.join("a.json");
    std::fs::write(&first, r#"{"year":2024}"#).unwrap();
    let index = Index::build(
        root.path(),
        vec![CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let repository = IndexedRepository::new(index).unwrap();
    std::fs::remove_file(first).unwrap();
    let args = Arguments {
        path: vec![data],
        query: Some("year >= 0".into()),
        ..Default::default()
    };
    let paths = args.resolve_paths(root.path()).unwrap();
    let collections = vec!["files".to_owned()];
    let mut output = Vec::new();
    let exit = run_on_repository(
        ExistingRun {
            args: &args,
            paths: &paths,
            repository: &repository,
            collections: &collections,
            schema_fields: 0,
            schema_files: &[],
            ignored_repository_metadata: false,
            startup_ns: 0,
                    },
        QuerySource::open(&args),
        &mut output,
        &CancellationToken::new(),
    );
    assert_eq!(exit, Exit::Operation);
    let events: Vec<serde_json::Value> = output
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
        .map(|line| serde_json::from_slice(line).unwrap())
        .collect();
    assert!(events.iter().any(|event| event["type"] == "query_error"
        && event["stage"] == "source"
        && event["partial"] == true));
    assert!(!events.iter().any(|event| event["type"] == "query_complete"));
    assert_eq!(events.last().unwrap()["type"], "run_error");
}
