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

#[test]
fn pinned_view_rejects_replacement_at_old_path_with_spoofed_revision() {
    use crate::run::DirectLookupResult;
    use crate::streaming::{
        CancellationToken, ResultScope, StreamControl, StreamEvent, StreamOptions, StreamOutcome,
    };

    let root = std::env::temp_dir().join(format!(
        "objekte-indexed-pin-incarnation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let original = root.join("data/original.jsonl");
    let destination = root.join("data/moved.jsonl");
    std::fs::write(&original, b"{\"name\":\"old\"}\n").unwrap();
    let original_metadata = std::fs::metadata(&original).unwrap();
    let index = Index::build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::JsonLines,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let pinned = index.view().unwrap();
    let mut query = Query {
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
    let id = pinned.search(&query).unwrap().hits[0].record_id.clone();
    let handle = pinned.file_handle_for_path(&original).unwrap();
    std::fs::rename(&original, &destination).unwrap();
    index.move_file(handle, &destination).unwrap();
    std::fs::write(&original, b"{\"name\":\"new\"}\n").unwrap();
    std::fs::File::open(&original)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(original_metadata.modified().unwrap()))
        .unwrap();
    assert_eq!(
        crate::inventory::SourceRevision::from_metadata(&std::fs::metadata(&original).unwrap())
            .unwrap(),
        crate::inventory::SourceRevision::from_metadata(&original_metadata).unwrap(),
    );

    assert!(matches!(
        pinned
            .get_record_available("data", &id, ReturnMode::Bytes)
            .unwrap(),
        DirectLookupResult::SourceUnavailable(_),
    ));
    query.options.return_mode = ReturnMode::Bytes;
    let result = pinned.search(&query).unwrap();
    assert_eq!(result.count, None);
    assert!(!result.counters.match_count_complete);
    assert!(result.counters.source_unavailable_events > 0);
    assert!(result.hits.is_empty());

    let mut rows = Vec::new();
    let outcome = pinned.search_stream_scoped(
        &query,
        ResultScope::All { offset: 0 },
        StreamOptions::default(),
        &CancellationToken::new(),
        &mut |event| {
            if let StreamEvent::OrderedRows { rows: batch, .. } = event {
                rows.extend(batch.iter().cloned());
            }
            Ok(StreamControl::Continue)
        },
    );
    assert!(rows.is_empty());
    assert!(matches!(
        outcome,
        Ok(StreamOutcome::Complete(summary)) if !summary.counters.match_count_complete && summary.count.is_none()
    ));
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
