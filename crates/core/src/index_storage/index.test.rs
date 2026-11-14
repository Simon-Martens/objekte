use crate::lifecycle::Index;
use crate::query::{parse_filter, Query, QueryOptions};
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::{SourceFormat, SourceRegistry};
use crate::workload::ReturnMode;

use super::status::{FreshnessState, StorageOrigin};
use super::store::SaveOptions;

fn fixture() -> (tempfile::TempDir, CollectionDefinition) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(
        root.path().join("data/a.json"),
        br#"{"text":"Alpha"}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("data/b.json"),
        br#"{"text":"Beta"}"#,
    )
    .unwrap();
    let definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    (root, definition)
}

fn query(text: &str) -> Query {
    Query {
        collections: vec!["items".into()],
        fts: None,
        filter: Some(parse_filter(&format!("text = \"{text}\"")).unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 50,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    }
}

fn count(index: &Index, text: &str) -> usize {
    index
        .view()
        .unwrap()
        .search(&query(text))
        .unwrap()
        .count
        .unwrap()
}

#[test]
fn core_index_saves_loads_and_remains_updateable() {
    let (root, definition) = fixture();
    let index = Index::build(
        root.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    assert_eq!(count(&index, "Alpha"), 1);
    assert_eq!(count(&index, "Beta"), 1);

    let store = tempfile::tempdir().unwrap();
    let report = index
        .save_to(store.path(), SaveOptions::default())
        .unwrap();
    assert!(!report.unchanged);
    assert!(report.objects_written >= 3);
    let opened = super::store::open_store(store.path()).unwrap();
    opened.verify_full().unwrap();
    super::catalogue::CatalogueView::open(opened.catalogue_backing())
        .unwrap()
        .verify_full()
        .unwrap();
    for run in &opened.runs {
        let frozen = super::segment::FrozenSegment::open(opened.run_backing(run)).unwrap();
        frozen.verify_full().unwrap();
    }

    let loaded = Index::load_from(
        store.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&loaded, "Alpha"), 1);
    assert_eq!(count(&loaded, "Beta"), 1);
    let status = loaded.storage_status().unwrap();
    assert_eq!(status.origin, StorageOrigin::Loaded);
    assert_eq!(status.freshness, FreshnessState::Unchecked);
    assert!(!status.unsaved_changes);

    // The loaded index still owns its metadata without re-reading sources.
    assert_eq!(loaded.status().unwrap().indexed_files, 2);

    let second = tempfile::tempdir().unwrap();
    let loaded_report = loaded
        .save_to(second.path(), SaveOptions::default())
        .unwrap();
    assert!(!loaded_report.unchanged);
    let loaded_again = Index::load_from(
        second.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&loaded_again, "Beta"), 1);

    let unchanged = loaded_again
        .save_to(second.path(), SaveOptions::default())
        .unwrap();
    assert!(unchanged.unchanged);
    assert_eq!(unchanged.objects_written, 0);
}

#[test]
fn a_loaded_index_accepts_new_files_and_saves_them() {
    let (root, definition) = fixture();
    let index = Index::build(
        root.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let store = tempfile::tempdir().unwrap();
    index.save_to(store.path(), SaveOptions::default()).unwrap();
    let loaded = Index::load_from(
        store.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let new_file = root.path().join("data/c.json");
    std::fs::write(&new_file, br#"{"text":"Gamma"}"#).unwrap();
    loaded.add_file(&new_file).unwrap();
    assert_eq!(count(&loaded, "Gamma"), 1);

    let changed = tempfile::tempdir().unwrap();
    loaded.save_to(changed.path(), SaveOptions::default()).unwrap();
    let reloaded = Index::load_from(
        changed.path(),
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&reloaded, "Gamma"), 1);
    assert_eq!(count(&reloaded, "Alpha"), 1);
}

#[test]
fn load_rejects_incompatible_definitions_and_missing_state() {
    let (root, definition) = fixture();
    let index = Index::build(
        root.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let store = tempfile::tempdir().unwrap();
    index.save_to(store.path(), SaveOptions::default()).unwrap();

    let mut incompatible = definition;
    incompatible.name = "other".into();
    assert!(
        Index::load_from(
            store.path(),
            vec![incompatible],
            RepositoryOptions::default(),
            SourceRegistry::builtin(),
        )
        .is_err()
    );

    let empty = tempfile::tempdir().unwrap();
    assert!(
        Index::load_from(
            empty.path(),
            vec![CollectionDefinition::folder("items", "data", SourceFormat::Json)],
            RepositoryOptions::default(),
            SourceRegistry::builtin(),
        )
        .is_err()
    );
}
