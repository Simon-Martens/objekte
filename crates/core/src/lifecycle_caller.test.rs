use super::*;
use crate::model::Value;
use crate::query::{QueryOptions, parse_filter};
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
    let record_id = adapter.search(&query).unwrap().hits[0].record_id.clone();
    let mut bound_query = query.clone();
    bound_query.filter = Some(parse_filter("@record.id = $id").unwrap());
    let parameters = std::collections::HashMap::from([("id".into(), Value::Id(record_id.clone()))]);
    let bound = adapter
        .search_with_parameters(&bound_query, &parameters)
        .unwrap();
    assert_eq!(bound.count, Some(1));
    assert_eq!(bound.hits[0].record_id, record_id);
    assert_eq!(
        adapter
            .index()
            .view()
            .unwrap()
            .search_with_parameters(&bound_query, &parameters)
            .unwrap()
            .count,
        Some(1)
    );
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
    assert_eq!(
        adapter
            .search_with_parameters(&bound_query, &parameters)
            .unwrap()
            .count,
        Some(0)
    );
    // The compatibility facade retains configuration without pinning the old
    // indexed generation after a publication.
    assert_eq!(adapter.files().root, root);
    assert!(adapter.files().files.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_adapter_keeps_batch_queries_on_one_publication() {
    let root = std::env::temp_dir().join(format!(
        "objekte-indexed-adapter-pin-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::write(root.join("data/a.json"), br#"{"name":"A"}"#).unwrap();
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
    let adapter = IndexedRepository::new(index.clone()).unwrap();
    let pinned = adapter.with_pinned_view(index.view().unwrap()).unwrap();
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
    assert_eq!(pinned.search(&query).unwrap().count, Some(1));
    let second = root.join("data/b.json");
    std::fs::write(&second, br#"{"name":"B"}"#).unwrap();
    index.add_file(&second).unwrap();
    assert_eq!(adapter.search(&query).unwrap().count, Some(2));
    assert_eq!(pinned.search(&query).unwrap().count, Some(1));
    let mut bound_query = query.clone();
    bound_query.filter = Some(parse_filter("name = $name").unwrap());
    let parameters = std::collections::HashMap::from([("name".into(), Value::String("B".into()))]);
    assert_eq!(
        adapter
            .search_with_parameters(&bound_query, &parameters)
            .unwrap()
            .count,
        Some(1)
    );
    assert_eq!(
        pinned
            .search_with_parameters(&bound_query, &parameters)
            .unwrap()
            .count,
        Some(0)
    );
    let other = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    assert!(adapter.with_pinned_view(other.view().unwrap()).is_err());
    other.shutdown().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
