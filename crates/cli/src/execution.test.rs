use super::*;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceFormat;

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
