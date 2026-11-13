use super::*;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceFormat;

#[cfg(feature = "tui")]
#[test]
fn tui_invalid_selected_cache_opens_incremental_catalogue() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(root.path().join("data/a.json"), r#"{"title":"A"}"#).unwrap();
    let cache_path = root.path().join(".objekte/search-core-ascii.snapshot");
    std::fs::create_dir(cache_path.parent().unwrap()).unwrap();
    std::fs::write(&cache_path, b"invalid snapshot").unwrap();
    let configured = ConfiguredDefinitions {
        definitions: vec![CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )],
        schema_files: Vec::new(),
        protected_files: Vec::new(),
        schema_fields: 0,
        ignored_repository_metadata: false,
    };
    let opening = selected_cache_or_incremental(
        root.path().to_owned(),
        configured,
        RepositoryOptions::default(),
        cache_path.clone(),
    )
    .unwrap();
    let crate::tui::TuiOpening::Initial(task, _) = opening else {
        panic!("invalid selected cache did not start incremental construction");
    };
    let index = task.wait_catalogue().unwrap();
    assert_eq!(index.view().unwrap().catalogue_rows().count(), 1);
    let complete = task.wait_complete().unwrap();
    complete.flush_cache(&cache_path).unwrap();
    let configured = ConfiguredDefinitions {
        definitions: vec![CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )],
        schema_files: Vec::new(),
        protected_files: Vec::new(),
        schema_fields: 0,
        ignored_repository_metadata: false,
    };
    let opening = selected_cache_or_incremental(
        root.path().to_owned(),
        configured,
        RepositoryOptions::default(),
        cache_path,
    )
    .unwrap();
    let crate::tui::TuiOpening::Ready(repository) = opening else {
        panic!("valid completed snapshot did not restore");
    };
    assert!(repository.startup().restored);
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
            snapshot_save: None,
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
