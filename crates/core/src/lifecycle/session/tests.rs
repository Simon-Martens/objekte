use super::*;
use crate::model::Value;
use crate::query::{QueryOptions, parse_filter};
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};

#[test]
fn adapter_routes_search_to_latest_publication_while_retaining_metadata() {
    let root = std::env::temp_dir().join(format!(
        "objekte-indexed-adapter-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data")).unwrap();
    let source = root.join("data/a.row");
    std::fs::write(&source, br#"("name"="A")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
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
    let handle = adapter
        .index()
        .view()
        .unwrap()
        .file_handle_for_path(&source)
        .unwrap()
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
    assert_eq!(adapter.files().root(), Some(root.as_path()));
    assert_eq!(adapter.files().files.len(), 1);
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
    std::fs::write(root.join("data/a.row"), br#"("name"="A")"#).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
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
    let second = root.join("data/b.row");
    std::fs::write(&second, br#"("name"="B")"#).unwrap();
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
    let other = crate::lifecycle::Index::start_build(
        &root,
        vec![crate::source::test_support::document_folder("data", "data")],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
    .unwrap();
    assert!(adapter.with_pinned_view(other.view().unwrap()).is_err());
    other.shutdown().unwrap();
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn pinned_view_uses_published_range_after_old_path_is_replaced() {
    use crate::query_execution::runner::DirectLookupResult;
    use crate::query_execution::streaming::{
        CancellationToken, ResultScope, StreamControl, StreamEvent, StreamOptions, StreamOutcome,
    };
    use crate::query_results::Payload;

    let root = std::env::temp_dir().join(format!(
        "objekte-indexed-pin-incarnation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let original = root.join("data/original.rows");
    let destination = root.join("data/moved.rows");
    std::fs::write(&original, b"(\"name\"=\"old\")\n").unwrap();
    let original_metadata = std::fs::metadata(&original).unwrap();
    let index = crate::lifecycle::Index::start_build(
        &root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            crate::source::test_support::line_format(),
        )],
        RepositoryOptions::default(),
        crate::source::test_support::fixture_registry(),
    )
    .and_then(|task| task.wait_complete())
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
    let handle = pinned.file_handle_for_path(&original).unwrap().unwrap();
    std::fs::rename(&original, &destination).unwrap();
    index.move_file(handle, &destination).unwrap();
    std::fs::write(&original, b"(\"name\"=\"new\")\n").unwrap();
    std::fs::File::open(&original)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(original_metadata.modified().unwrap()))
        .unwrap();
    let replaced_revision =
        crate::inventory::SourceRevision::from_metadata(&std::fs::metadata(&original).unwrap())
            .unwrap();
    let original_revision =
        crate::inventory::SourceRevision::from_metadata(&original_metadata).unwrap();
    assert_eq!(replaced_revision.size, original_revision.size);
    assert_eq!(replaced_revision.modified_ns, original_revision.modified_ns);
    assert_ne!(replaced_revision.object_id, original_revision.object_id);

    assert!(!matches!(
        pinned.get_record_available("data", &id, ReturnMode::Bytes),
        Ok(DirectLookupResult::Found(hit))
            if hit.payload == Some(Payload::Bytes(b"(\"name\"=\"new\")".to_vec())),
    ));
    query.options.return_mode = ReturnMode::Bytes;
    assert!(!matches!(pinned.search(&query), Ok(result)
        if result.hits.iter().any(|hit| hit.payload == Some(Payload::Bytes(b"(\"name\"=\"new\")".to_vec())))));

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
    assert!(
        !rows
            .iter()
            .any(|hit| hit.payload == Some(Payload::Bytes(b"(\"name\"=\"new\")".to_vec())))
    );
    assert!(
        !matches!(outcome, Ok(StreamOutcome::Complete(summary)) if summary.counters.match_count_complete && rows.len() == 1)
    );
    index.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
