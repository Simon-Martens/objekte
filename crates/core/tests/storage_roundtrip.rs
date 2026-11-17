use objekte_search_core::lifecycle::Index;
use objekte_search_core::query::{Query, QueryOptions};
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::storage::{StorageOpenOptions, StorageWriteOptions};
use objekte_search_core::workload::ReturnMode;

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
    vec![CollectionDefinition::folder(
        "items",
        "data",
        SourceFormat::Json,
    )]
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
