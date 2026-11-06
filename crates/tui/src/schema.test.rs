use super::*;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::repository::{Repository, RepositoryOptions, SnapshotMode};
use objekte_search_core::workload::ReturnMode;

#[test]
fn schema_files_remain_an_addon_to_in_memory_definitions() {
    let temp = tempfile::tempdir().unwrap();
    let schemas = temp.path().join(".objekte/collections");
    std::fs::create_dir_all(&schemas).unwrap();
    std::fs::create_dir_all(temp.path().join("data/docs")).unwrap();
    std::fs::write(
        schemas.join("docs.schema.json"),
        r#"{"type":"object","properties":{"title":{"type":"string"},"year":{"type":"number"}},"x-index-paths":["year"],"x-sort-indexes":[[{"field":"year","direction":"asc"}]],"x-objekte":{"name":"docs","path":"data/docs","format":"json","id":"filename"}}"#,
    ).unwrap();
    std::fs::write(
        temp.path().join("data/docs/one.json"),
        r#"{"title":"needle","year":1810}"#,
    )
    .unwrap();
    let definitions = load_definitions(temp.path()).unwrap();
    assert_eq!(definitions.len(), 1);
    assert!(definitions[0].schema.fields()[&vec!["year".into()]].indexed);
    assert_eq!(definitions[0].sort_fields.len(), 1);
    let repository = Repository::open_with_options(
        temp.path(),
        definitions,
        RepositoryOptions {
            snapshot_mode: SnapshotMode::Disabled,
            ..RepositoryOptions::default()
        },
    )
    .unwrap();
    let result = repository
        .search(&Query {
            collections: vec!["docs".into()],
            fts: None,
            filter: Some(parse_filter("year >= 1800").unwrap()),
            options: QueryOptions {
                sort: Vec::new(),
                limit: 10,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Bytes,
                evidence: true,
            },
        })
        .unwrap();
    assert_eq!(result.count, 1);
    assert_eq!(result.hits[0].record_id, "one");
}

#[test]
fn musenalm_schemas_load_without_core_specific_vocabulary() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../examples/musenalm");
    let definitions = load_definitions(&root).unwrap();
    assert_eq!(definitions.len(), 7);
    assert!(
        definitions
            .iter()
            .any(|definition| definition.name == "contents")
    );
}
