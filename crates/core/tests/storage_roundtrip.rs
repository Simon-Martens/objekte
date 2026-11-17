use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::storage::{StorageOpenOptions, StorageWriteOptions};
use objekte_search_core::workload::{ReturnMode, SortField};

fn fixture_root(label: &str) -> std::path::PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-storage-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(root.join("storage")).unwrap();
    std::fs::write(
        root.join("data/a.json"),
        br#"{"id":"a","title":"Alpha token"}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("data/b.json"),
        br#"{"id":"b","title":"Beta token"}"#,
    )
    .unwrap();
    root
}

fn definitions() -> Vec<CollectionDefinition> {
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    for (name, kind) in [("id", LogicalType::Id), ("title", LogicalType::String)] {
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: false,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    definition
        .sort_fields
        .push(objekte_search_core::model::FieldPath::parse("title").unwrap());
    vec![definition]
}

fn all_records() -> Query {
    Query {
        collections: vec!["items".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 20,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

#[test]
fn explicit_save_reopens_a_complete_searchable_snapshot() {
    let root = fixture_root("roundtrip");
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
    let before = index.view().unwrap().search(&all_records()).unwrap();
    let report = index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    assert!(!report.unchanged);
    let repeated = index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    assert!(repeated.unchanged);
    assert_eq!(repeated.objects_new, 0);
    assert_eq!(repeated.bytes_serialized, 0);
    assert_eq!(repeated.bytes_written, 0);
    assert_eq!(repeated.sync_calls, 0);
    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let after = reopened.view().unwrap().search(&all_records()).unwrap();
    assert_eq!(after.count, before.count);
    assert_eq!(
        after
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>(),
        before
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>()
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn open_stays_payload_lazy_until_an_indexed_filter_selects_a_posting() {
    let root = fixture_root("lazy");
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    let mut native_sorted = all_records();
    native_sorted.options.sort = vec![SortField::simple("title", false)];
    assert_eq!(
        index.view().unwrap().search(&native_sorted).unwrap().count,
        Some(2)
    );

    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    assert_eq!(cold.index_reads, 1);

    let mut request = all_records();
    request.filter = Some(parse_filter("title = \"Alpha token\"").unwrap());
    let result = reopened.view().unwrap().search(&request).unwrap();
    assert_eq!(result.count, Some(1));
    let warm = reopened.storage_status().unwrap();
    assert!(warm.index_reads > cold.index_reads);

    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn configured_sort_frames_load_only_when_sort_execution_needs_them() {
    let root = fixture_root("sort-lazy");
    let index = Index::build(
        &root,
        definitions(),
        RepositoryOptions::default(),
    )
    .unwrap();
    index
        .archive_to(root.join("storage"), StorageWriteOptions::default())
        .unwrap();
    drop(index);

    let reopened = Index::open_storage(
        &root,
        definitions(),
        RepositoryOptions::default(),
        root.join("storage"),
        StorageOpenOptions::default(),
    )
    .unwrap();
    let cold = reopened.storage_status().unwrap();
    let mut request = all_records();
    request.options.sort = vec![SortField::simple("title", false)];
    request.options.count = false;
    let result = reopened.view().unwrap().search(&request).unwrap();
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["path:a.json", "path:b.json"]
    );
    let warm = reopened.storage_status().unwrap();
    assert!(warm.index_reads > cold.index_reads);
    reopened.view().unwrap().search(&request).unwrap();
    assert_eq!(
        reopened.storage_status().unwrap().index_reads,
        warm.index_reads
    );
    reopened.shutdown().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
