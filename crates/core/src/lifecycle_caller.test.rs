use super::*;
use crate::query::QueryOptions;
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::SourceFormat;

#[test]
fn adapter_routes_search_to_latest_publication_while_retaining_metadata() {
    let root = std::env::temp_dir().join(format!(
        "objekte-indexed-adapter-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let source = root.join("data/a.json");
    std::fs::write(&source, br#"{"name":"A"}"#).unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let adapter = IndexedRepository::new(index).unwrap();
    let query = Query {
        collections: vec!["data".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(adapter.collections()[0].definition.name, "data");
    assert_eq!(adapter.search(&query).unwrap().count, Some(1));
    let cache = root.join("index.snapshot");
    adapter.index().flush_cache(&cache).unwrap();
    assert!(
        !IndexedRepository::new(adapter.index().clone())
            .unwrap()
            .startup()
            .restored
    );
    let loaded = Index::load_cache(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
        &cache,
    )
    .unwrap();
    assert!(
        IndexedRepository::new(loaded.clone())
            .unwrap()
            .startup()
            .restored
    );
    loaded.shutdown().unwrap();
    let handle = adapter
        .index()
        .view()
        .unwrap()
        .file_handle_for_path(&source)
        .unwrap();
    adapter.index().delete_file(handle).unwrap();
    assert_eq!(adapter.search(&query).unwrap().count, Some(0));
    // The pinned base still describes the original indexed generation.
    assert_eq!(adapter.files().files.len(), 1);
    std::fs::remove_dir_all(root).unwrap();
}
