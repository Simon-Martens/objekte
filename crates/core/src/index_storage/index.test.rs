use crate::lifecycle::Index;
use crate::model::{FileId, LogicalType};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::FieldDefinition;
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::{SourceFormat, SourceRegistry};
use crate::workload::ReturnMode;

use super::status::{FreshnessState, StorageOrigin};
use super::store::{SaveHooks, SaveOptions};

fn fixture() -> (tempfile::TempDir, CollectionDefinition) {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"text":"Alpha"}"#).unwrap();
    std::fs::write(root.path().join("data/b.json"), br#"{"text":"Beta"}"#).unwrap();
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::Json);
    definition
        .schema
        .register_field(FieldDefinition {
            path: vec!["text".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
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
    let report = index.save_to(store.path(), SaveOptions::default()).unwrap();
    assert!(!report.unchanged);
    assert!(report.objects_written >= 2);
    let opened = super::store::open_store(store.path()).unwrap();
    assert!(opened.state.is_none());
    assert!(opened.manifest.state.is_none());
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
    let view = loaded.view().unwrap();
    let collection = &view.repository().collections()[0];
    if let Some(segment) = view.0.segments[0].first() {
        let segment_catalog =
            crate::index_catalog::IndexCatalog::segment(&segment.index, collection);
        let segment_access = segment_catalog
            .field_postings(&["text".to_owned()], "default", false)
            .into_iter()
            .flatten()
            .next()
            .expect("loaded segment access");
        assert!(matches!(
            segment_access.access,
            crate::index_storage::access::PostingAccess::Packed(_)
        ));
        let schema = &collection.definition.schema;
        let packed_text = segment_catalog
            .field_text_access(schema, &["text".to_owned()], "default", false, true, true)
            .expect("packed text access");
        assert!(matches!(
            packed_text,
            crate::index_storage::access::BigramAccess::Packed { .. }
        ));
        let request = crate::bigram::BigramRequest::new("Alpha");
        let candidates = packed_text
            .candidates_prepared(&request)
            .expect("packed candidates");
        assert!(candidates.contains(0));
        assert!(segment.index.raw.normal.is_empty());
        assert!(segment.index.raw.skip.is_empty());
    }
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

fn hit_identity(index: &Index, query: &Query) -> Vec<(String, String)> {
    let mut hits = index
        .view()
        .unwrap()
        .search(query)
        .unwrap()
        .hits
        .into_iter()
        .map(|hit| (hit.collection, hit.record_id))
        .collect::<Vec<_>>();
    hits.sort();
    hits
}

#[test]
fn built_and_loaded_packed_queries_return_identical_results() {
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
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let filters = [
        "text = \"Alpha\"",
        "text != \"Alpha\"",
        "text = \"Missing\"",
    ];
    for filter in filters {
        let query = Query {
            collections: vec!["items".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: Vec::new(),
                limit: 50,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        assert_eq!(
            hit_identity(&index, &query),
            hit_identity(&loaded, &query),
            "{filter}"
        );
    }
    let fts_query = |text: &str| Query {
        collections: vec!["items".into()],
        fts: Some(crate::query::FtsQuery {
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            mode: crate::query::FtsMode::Literal,
            literal: text.into(),
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 50,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    assert_eq!(
        hit_identity(&index, &fts_query("Alpha")),
        hit_identity(&loaded, &fts_query("Alpha")),
        "FTS packed candidate execution"
    );
}

#[test]
fn loaded_packed_readers_answer_after_native_postings_are_removed() {
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
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let view = loaded.view().unwrap();
    let mut published = (*view.0).clone();
    let mut segments = Vec::new();
    for segment in published.segments[0].iter() {
        let mut native = (*segment.index).clone();
        native.postings.fields.clear();
        native.raw.normal.clear();
        native.raw.skip.clear();
        native.fallback_text.normal.clear();
        native.fallback_text.skip.clear();
        native.custom_text.clear();
        let mut replacement = (**segment).clone();
        replacement.index = std::sync::Arc::new(native);
        segments.push(std::sync::Arc::new(replacement));
    }
    published.segments[0] = std::sync::Arc::new(segments);
    let packed_only = Index::from_published(published);
    std::fs::remove_dir_all(root.path().join("data")).unwrap();
    assert_eq!(count(&packed_only, "Alpha"), 1);
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
    loaded
        .save_to(changed.path(), SaveOptions::default())
        .unwrap();
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


struct MutateDuringSave {
    index: Index,
    path: std::path::PathBuf,
}


impl SaveHooks for MutateDuringSave {
    fn before_current_replace(&mut self) -> Result<(), super::error::StorageError> {
        self.index
            .add_file(&self.path)
            .map(|_| ())
            .map_err(|error| super::error::StorageError::Conflict(error.to_string()))
    }
}


#[test]
fn a_publication_during_save_is_reported_as_newer() {
    let (root, definition) = fixture();
    let index = Index::build(
        root.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let new_file = root.path().join("data/c.json");
    std::fs::write(&new_file, br#"{"text":"Gamma"}"#).unwrap();
    let store = tempfile::tempdir().unwrap();
    let mut hooks = MutateDuringSave {
        index: index.clone(),
        path: new_file,
    };
    let report = index
        .save_to_inner(
            store.path(),
            SaveOptions::default(),
            || false,
            |_| {},
            &mut hooks,
        )
        .unwrap();
    assert!(report.newer_publication_exists);
    assert!(index.storage_status().unwrap().unsaved_changes);
    assert_eq!(count(&index, "Gamma"), 1);
    let loaded = Index::load_from(
        store.path(),
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&loaded, "Alpha"), 1);
    assert_eq!(count(&loaded, "Gamma"), 0);
}

#[test]
fn a_rebuilt_loaded_index_keeps_its_store_lineage() {
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
    loaded.start_rebuild().unwrap().wait().unwrap();
    loaded
        .save_to(store.path(), SaveOptions::default())
        .unwrap();
    let reloaded = Index::load_from(
        store.path(),
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&reloaded, "Alpha"), 1);
    assert_eq!(count(&reloaded, "Beta"), 1);
}

#[test]
fn a_compacted_index_can_be_saved_and_reopened() {
    let (root, definition) = fixture();
    let index = Index::build(
        root.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    index.start_compaction().unwrap().wait().unwrap();
    let store = tempfile::tempdir().unwrap();
    index.save_to(store.path(), SaveOptions::default()).unwrap();
    let loaded = Index::load_from(
        store.path(),
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&loaded, "Alpha"), 1);
    assert_eq!(count(&loaded, "Beta"), 1);
    let view = loaded.view().unwrap();
    let collection = &view.repository().collections()[0];
    let segment = view.0.segments[0].first().expect("compacted segment");
    let access = crate::index_catalog::IndexCatalog::segment(&segment.index, collection)
        .field_postings(&["text".to_owned()], "default", false)
        .into_iter()
        .flatten()
        .next()
        .expect("compacted segment access");
    assert!(matches!(
        access.access,
        crate::index_storage::access::PostingAccess::Packed(_)
    ));
}

#[test]
fn loaded_packed_index_compacts_after_add_replace_move_and_delete() {
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
    let pinned = loaded.view().unwrap();
    assert_eq!(pinned.search(&query("Alpha")).unwrap().count, Some(1));
    assert_eq!(pinned.search(&query("Beta")).unwrap().count, Some(1));

    let a = root.path().join("data/a.json");
    std::fs::write(&a, br#"{"text":"Alpha replaced"}"#).unwrap();
    let handle = loaded.view().unwrap().file_handle(FileId(0)).unwrap();
    loaded.update_file(handle, &a).unwrap();

    let b = root.path().join("data/b.json");
    let c = root.path().join("data/c.json");
    std::fs::rename(&b, &c).unwrap();
    let handle = loaded.view().unwrap().file_handle(FileId(1)).unwrap();
    loaded.move_file(handle, &c).unwrap();

    let d = root.path().join("data/d.json");
    std::fs::write(&d, br#"{"text":"Gamma"}"#).unwrap();
    loaded.add_file(&d).unwrap();
    let e = root.path().join("data/e.json");
    std::fs::write(&e, br#"{"text":"Delta"}"#).unwrap();
    let added = loaded.add_file(&e).unwrap();
    loaded.delete_file(added.file).unwrap();

    // Maintenance must not need any source file after mutations were staged.
    std::fs::remove_dir_all(root.path().join("data")).unwrap();
    let report = loaded.compact().unwrap();
    assert!(report.slots_before >= report.slots_after);
    assert_eq!(count(&loaded, "Alpha replaced"), 1);
    assert_eq!(count(&loaded, "Beta"), 1);
    assert_eq!(count(&loaded, "Gamma"), 1);
    assert_eq!(count(&loaded, "Delta"), 0);
    // A pinned pre-mutation view remains valid and unchanged.
    assert_eq!(pinned.search(&query("Alpha")).unwrap().count, Some(1));
    assert_eq!(pinned.search(&query("Beta")).unwrap().count, Some(1));
    assert_eq!(pinned.search(&query("Gamma")).unwrap().count, Some(0));

    let compacted_store = tempfile::tempdir().unwrap();
    loaded
        .save_to(compacted_store.path(), SaveOptions::default())
        .unwrap();
    let reloaded = Index::load_from(
        compacted_store.path(),
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&reloaded, "Alpha replaced"), 1);
    assert_eq!(count(&reloaded, "Beta"), 1);
    assert_eq!(count(&reloaded, "Gamma"), 1);
    assert_eq!(count(&reloaded, "Delta"), 0);
    let view = reloaded.view().unwrap();
    let segment = view.0.segments[0]
        .first()
        .expect("compacted packed segment");
    assert!(segment.packed.is_some());
    assert!(segment.index.records.as_slice().is_none());
}

#[test]
fn packed_reuse_preserves_deleted_and_replaced_visibility() {
    let (root, definition) = fixture();
    let index = Index::build(
        root.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let first = tempfile::tempdir().unwrap();
    index.save_to(first.path(), SaveOptions::default()).unwrap();
    let loaded = Index::load_from(
        first.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    let handle = loaded
        .view()
        .unwrap()
        .file_handle_for_path(&root.path().join("data/a.json"))
        .expect("deleted file handle");
    loaded.delete_file(handle).unwrap();
    // No compaction: the immutable run bytes are reused and visibility must
    // still carry the retirement.
    let second = tempfile::tempdir().unwrap();
    loaded
        .save_to(second.path(), SaveOptions::default())
        .unwrap();
    let reloaded = Index::load_from(
        second.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&reloaded, "Alpha"), 0);
    assert_eq!(count(&reloaded, "Beta"), 1);

    std::fs::write(root.path().join("data/b.json"), br#"{"text":"Gamma"}"#).unwrap();
    let handle = reloaded
        .view()
        .unwrap()
        .file_handle_for_path(&root.path().join("data/b.json"))
        .expect("replaced file handle");
    reloaded
        .update_file(handle, &root.path().join("data/b.json"))
        .unwrap();
    let third = tempfile::tempdir().unwrap();
    reloaded
        .save_to(third.path(), SaveOptions::default())
        .unwrap();
    let replaced = Index::load_from(
        third.path(),
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&replaced, "Beta"), 0);
    assert_eq!(count(&replaced, "Gamma"), 1);
}

#[test]
fn same_store_save_reuses_mapped_run_and_visibility_objects() {
    let (root, definition) = fixture();
    let index = Index::build(
        root.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let extra = root.path().join("data/extra.json");
    std::fs::write(&extra, br#"{"text":"Extra"}"#).unwrap();
    index.add_file(&extra).unwrap();
    let store = tempfile::tempdir().unwrap();
    index.save_to(store.path(), SaveOptions::default()).unwrap();
    let loaded = Index::load_from(
        store.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    std::fs::write(
        root.path().join("data/a.json"),
        br#"{"text":"Alpha changed"}"#,
    )
    .unwrap();
    let handle = loaded
        .view()
        .unwrap()
        .file_handle_for_path(&root.path().join("data/a.json"))
        .expect("updated file handle");
    loaded
        .update_file(handle, &root.path().join("data/a.json"))
        .unwrap();
    let report = loaded
        .save_to(store.path(), SaveOptions::default())
        .unwrap();
    assert!(!report.unchanged);
    assert!(report.objects_reused >= 2);
    assert!(report.bytes_reused > 0);
    let reloaded = Index::load_from(
        store.path(),
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&reloaded, "Alpha changed"), 1);
    assert_eq!(count(&reloaded, "Beta"), 1);
    assert_eq!(count(&reloaded, "Extra"), 1);
}

#[test]
fn background_save_reports_progress_and_completion() {
    let (root, definition) = fixture();
    let index = Index::build(
        root.path(),
        vec![definition.clone()],
        RepositoryOptions::default(),
    )
    .unwrap();
    let store = tempfile::tempdir().unwrap();
    let task = index
        .start_save(store.path().to_owned(), SaveOptions::default())
        .unwrap();
    assert!(index.storage_status().unwrap().active_save_task);
    let report = task.wait().unwrap();
    assert!(!report.unchanged);
    assert!(task.is_complete().unwrap());
    assert!(!index.storage_status().unwrap().active_save_task);
    let loaded = Index::load_from(
        store.path(),
        vec![definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap();
    assert_eq!(count(&loaded, "Alpha"), 1);
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
            vec![CollectionDefinition::folder(
                "items",
                "data",
                SourceFormat::Json
            )],
            RepositoryOptions::default(),
            SourceRegistry::builtin(),
        )
        .is_err()
    );
}
