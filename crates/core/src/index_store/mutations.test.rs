//! Atomic file admission, update and deletion tests.

use std::sync::Arc;

use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SourceRevision,
};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::posting::ScalarKey;

use crate::index_store::construction::discover_file;
use crate::index_store::value_codec::{
    CollectionMetadata, ConstructionState, GramKind, GramProjection, RecordMetadata, StateKind,
    StoredContributions, StoreMetadata,
};
use crate::index_store::{IndexStore, StoreConfig, read, write};

fn store() -> Arc<IndexStore> {
    Arc::new(IndexStore::new(StoreConfig::default()).expect("store"))
}

fn collection() -> CollectionMetadata {
    CollectionMetadata {
        id: CollectionId(0),
        name: "mutations".into(),
        generation: 1,
        source_fingerprint: [0; 32],
        slot_high_water: 0,
        live_records: 0,
        discovered_files: 0,
        pending_files: 0,
        indexed_files: 0,
        unavailable_files: 0,
        processed_files: 0,
        next_binding_id: 0,
    }
}

fn file() -> FileEntry {
    FileEntry {
        id: FileId(0),
        relative_path: std::path::PathBuf::from("records.jsonl").into(),
        revision: SourceRevision {
            modified_ns: 5,
            size: 100,
        },
        created_ns: None,
        collections: vec![CollectionId(0)],
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    }
}

fn record(slot: u32, id: &str, start: u64, end: u64) -> RecordEntry {
    RecordEntry {
        slot: RecordSlot(slot),
        id: RecordId(id.into()),
        file: FileId(0),
        range: ByteRange { start, end },
        revision: SourceRevision {
            modified_ns: 5,
            size: 100,
        },
        class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
    }
}

fn contribution(id: &str, start: u64, end: u64, value: &str, gram: u64) -> StoredContributions {
    let entry = record(0, id, start, end);
    StoredContributions {
        record: RecordMetadata::from_entry(&entry, 1),
        values: vec![(0, ScalarKey::String(value.into()))],
        lengths: vec![(0, crate::model::Number::Integer(value.len() as i128))],
        states: vec![(0, StateKind::PathPresence)],
        coverage: Vec::new(),
        domains: Vec::new(),
        sorts: Vec::new(),
        relations: Vec::new(),
        grams: vec![GramProjection {
            binding: 0,
            kind: GramKind::Normal,
            grams: vec![gram],
        }],
    }
}

fn prepared(values: &[(&str, u64, u64, &str, u64)]) -> crate::index_store::mutations::PreparedFile {
    crate::index_store::mutations::PreparedFile {
        file: file(),
        file_version: 1,
        fingerprint: Some([9; 32]),
        records: values
            .iter()
            .map(|(id, start, end, value, gram)| contribution(id, *start, *end, value, *gram))
            .collect(),
    }
}

fn prepared_catalog(store: &IndexStore, meta: &mut StoreMetadata) {
    let tx = store.begin_write().expect("write");
    meta.construction = ConstructionState::Building;
    meta.catalog_complete = true;
    write::put_metadata(&tx, meta).expect("metadata");
    let mut collection = collection();
    write::put_collection(&tx, &collection).expect("collection");
    let entry = file();
    discover_file(
        &tx,
        &mut collection,
        &entry,
        1,
        meta.build_epoch,
        meta.revision,
    )
    .expect("discover");
    write::put_collection(&tx, &collection).expect("collection");
    tx.commit().expect("commit");
}

fn initial_metadata(store: &Arc<IndexStore>) -> StoreMetadata {
    store.view().expect("view").metadata().clone()
}

#[test]
fn admission_writes_every_family_and_publishes_one_revision() {
    let store = store();
    let mut meta = initial_metadata(&store);
    prepared_catalog(&store, &mut meta);
    let report = crate::index_store::mutations::admit_file(
        &store,
        CollectionId(0),
        &prepared(&[("a", 0, 4, "alpha", 1), ("b", 4, 8, "beta", 2)]),
        meta.build_epoch,
    )
    .expect("admit");
    assert_eq!(report.slots, vec![RecordSlot(0), RecordSlot(1)]);
    let view = store.view().expect("view");
    assert_eq!(view.revision(), report.revision);
    assert!(read::pending_files(&view, CollectionId(0))
        .expect("pending")
        .is_empty());
    assert_eq!(
        read::build_state_kind(&view, FileId(0)).expect("state"),
        Some(crate::index_store::value_codec::BuildFileState::Indexed)
    );
    assert_eq!(
        read::record_by_logical_id(&view, CollectionId(0), "b").expect("logical"),
        Some((RecordSlot(1), 1))
    );
    assert_eq!(
        read::value_members(
            &view,
            CollectionId(0),
            0,
            &ScalarKey::String("beta".into())
        )
        .expect("members"),
        vec![RecordSlot(1)]
    );
    assert_eq!(
        read::gram_statistics(&view, CollectionId(0), 0, GramKind::Normal, 2)
            .expect("stats")
            .expect("present"),
        (1, 1)
    );
    let validation = crate::index_store::validation::validate(&view).expect("validate");
    assert!(validation.is_valid(), "{:?}", validation.errors);
}

#[test]
fn failed_admission_aborts_the_whole_file() {
    let store = store();
    let mut meta = initial_metadata(&store);
    prepared_catalog(&store, &mut meta);
    // End beyond the file size: the checked range write fails after slots
    // have been allocated inside the transaction.
    let bad = prepared(&[("a", 0, 4, "alpha", 1), ("b", 4, 200, "beta", 2)]);
    assert!(crate::index_store::mutations::admit_file(
        &store,
        CollectionId(0),
        &bad,
        meta.build_epoch
    )
    .is_err());
    let view = store.view().expect("view");
    assert_eq!(
        read::build_state_kind(&view, FileId(0)).expect("state"),
        Some(crate::index_store::value_codec::BuildFileState::Pending)
    );
    assert_eq!(
        read::pending_files(&view, CollectionId(0))
            .expect("pending")
            .len(),
        1
    );
    assert_eq!(
        read::record(&view, CollectionId(0), RecordSlot(0)).expect("record"),
        None
    );
    assert_eq!(read::collection(&view, CollectionId(0)).expect("collection").live_records, 0);
}

#[test]
fn update_retains_slots_removes_disappeared_and_reuses_free_slots() {
    let store = store();
    let mut meta = initial_metadata(&store);
    prepared_catalog(&store, &mut meta);
    crate::index_store::mutations::admit_file(
        &store,
        CollectionId(0),
        &prepared(&[("a", 0, 4, "alpha", 1), ("b", 4, 8, "beta", 2)]),
        meta.build_epoch,
    )
    .expect("admit");
    let epoch = store.view().expect("view").build_epoch();
    let mut replacement = prepared(&[("a", 0, 6, "gamma", 3), ("c", 6, 10, "delta", 4)]);
    replacement.file_version = 2;
    let mut replacement_file = file();
    replacement_file.revision.modified_ns = 6;
    replacement.file = replacement_file;
    let report = crate::index_store::mutations::update_file(
        &store,
        CollectionId(0),
        &replacement,
        1,
        epoch,
    )
    .expect("update");
    assert_eq!(report.retained, 1);
    assert_eq!(report.removed, 1);
    assert_eq!(report.added, 1);
    let view = store.view().expect("view");
    assert_eq!(
        read::record_by_logical_id(&view, CollectionId(0), "a").expect("a"),
        Some((RecordSlot(0), 1)),
        "retained record keeps slot and incarnation"
    );
    assert_eq!(
        read::record_by_logical_id(&view, CollectionId(0), "b").expect("b"),
        None
    );
    assert_eq!(
        read::record_by_logical_id(&view, CollectionId(0), "c").expect("c"),
        Some((RecordSlot(1), 2)),
        "new record reuses the smallest free slot with a new incarnation"
    );
    assert!(read::value_members(
        &view,
        CollectionId(0),
        0,
        &ScalarKey::String("alpha".into())
    )
    .expect("old value")
    .is_empty());
    assert_eq!(
        read::value_members(
            &view,
            CollectionId(0),
            0,
            &ScalarKey::String("gamma".into())
        )
        .expect("new value"),
        vec![RecordSlot(0)]
    );
    assert_eq!(
        read::gram_statistics(&view, CollectionId(0), 0, GramKind::Normal, 2)
            .expect("old gram"),
        None
    );
    assert_eq!(
        read::gram_statistics(&view, CollectionId(0), 0, GramKind::Normal, 3)
            .expect("new gram")
            .expect("present"),
        (1, 1)
    );
    let validation = crate::index_store::validation::validate(&view).expect("validate");
    assert!(validation.is_valid(), "{:?}", validation.errors);
}

#[test]
fn delete_removes_rows_without_reading_the_source_and_frees_slots() {
    let store = store();
    let mut meta = initial_metadata(&store);
    prepared_catalog(&store, &mut meta);
    crate::index_store::mutations::admit_file(
        &store,
        CollectionId(0),
        &prepared(&[("a", 0, 4, "alpha", 1), ("b", 4, 8, "beta", 2)]),
        meta.build_epoch,
    )
    .expect("admit");
    let report = crate::index_store::mutations::delete_file(&store, FileId(0)).expect("delete");
    assert_eq!(report.removed, 2);
    let view = store.view().expect("view");
    assert!(read::file(&view, FileId(0)).expect("file").is_none());
    assert!(read::record(&view, CollectionId(0), RecordSlot(0))
        .expect("record")
        .is_none());
    assert_eq!(
        read::record_by_logical_id(&view, CollectionId(0), "a").expect("logical"),
        None
    );
    assert_eq!(
        read::value_members(
            &view,
            CollectionId(0),
            0,
            &ScalarKey::String("alpha".into())
        )
        .expect("members")
        .len(),
        0
    );
    assert_eq!(
        read::gram_statistics(&view, CollectionId(0), 0, GramKind::Normal, 1)
            .expect("stats"),
        None
    );
    assert_eq!(
        crate::index_store::validation::collection_record_count(&view, CollectionId(0)).expect("count"),
        0
    );
    assert!(read::pending_files(&view, CollectionId(0))
        .expect("pending")
        .is_empty());
}

#[test]
fn unavailable_files_keep_explicit_state_and_no_records() {
    let store = store();
    let mut meta = initial_metadata(&store);
    prepared_catalog(&store, &mut meta);
    let revision = crate::index_store::mutations::mark_file_unavailable(
        &store,
        CollectionId(0),
        FileId(0),
        1,
        meta.build_epoch,
        "permission denied".into(),
    )
    .expect("unavailable");
    let view = store.view().expect("view");
    assert_eq!(view.revision(), revision);
    let state = read::build_state(&view, FileId(0))
        .expect("state")
        .expect("present");
    assert_eq!(state.state, crate::index_store::value_codec::BuildFileState::Unavailable);
    assert_eq!(state.error.as_deref(), Some("permission denied"));
    assert!(read::pending_files(&view, CollectionId(0))
        .expect("pending")
        .is_empty());
    assert_eq!(read::collection(&view, CollectionId(0)).expect("collection").live_records, 0);
}

#[test]
fn move_keeps_file_id_and_slots_and_updates_paths() {
    let store = store();
    let mut meta = initial_metadata(&store);
    prepared_catalog(&store, &mut meta);
    crate::index_store::mutations::admit_file(
        &store,
        CollectionId(0),
        &prepared(&[("a", 0, 4, "alpha", 1)]),
        meta.build_epoch,
    )
    .expect("admit");
    let mut destination = file();
    destination.relative_path = std::path::PathBuf::from("moved.jsonl").into();
    crate::index_store::mutations::move_file(&store, FileId(0), destination, 1).expect("move");
    let view = store.view().expect("view");
    assert_eq!(
        read::file_by_native_path(&view, b"records.jsonl").expect("old path"),
        None
    );
    assert_eq!(
        read::file_by_native_path(&view, b"moved.jsonl").expect("new path"),
        Some(FileId(0))
    );
    assert_eq!(
        read::record_by_logical_id(&view, CollectionId(0), "a").expect("logical"),
        Some((RecordSlot(0), 1))
    );
}

#[test]
fn epochs_fence_stale_admission() {
    let store = store();
    let mut meta = initial_metadata(&store);
    prepared_catalog(&store, &mut meta);
    let stale_epoch = meta.build_epoch;
    let tx = store.begin_write().expect("write");
    let mut metadata = read::metadata_from_write(&tx).expect("metadata");
    metadata.build_epoch += 1;
    write::put_metadata(&tx, &metadata).expect("epoch");
    tx.commit().expect("commit");
    let error = crate::index_store::mutations::admit_file(
        &store,
        CollectionId(0),
        &prepared(&[("a", 0, 4, "alpha", 1)]),
        stale_epoch,
    )
    .expect_err("stale");
    assert!(matches!(error, crate::index_store::StoreError::StaleHandle(_)));
    let view = store.view().expect("view");
    assert!(read::pending_files(&view, CollectionId(0))
        .expect("pending")
        .len()
        == 1);
}

#[test]
fn deleting_a_target_does_not_delete_incoming_logical_references() {
    let store = store();
    let mut meta = initial_metadata(&store);
    // Add a second collection/file so a relation can target it.
    let tx = store.begin_write().expect("write");
    let mut first = collection();
    let mut second = collection();
    second.id = CollectionId(1);
    second.name = "target".into();
    write::put_collection(&tx, &first).expect("first");
    write::put_collection(&tx, &second).expect("second");
    let mut first_file = file();
    first_file.collections = vec![CollectionId(0)];
    discover_file(&tx, &mut first, &first_file, 1, meta.build_epoch, meta.revision)
        .expect("discover first");
    let mut second_file = file();
    second_file.id = FileId(1);
    second_file.relative_path = std::path::PathBuf::from("target.jsonl").into();
    second_file.collections = vec![CollectionId(1)];
    discover_file(&tx, &mut second, &second_file, 1, meta.build_epoch, meta.revision)
        .expect("discover second");
    meta.construction = ConstructionState::Building;
    meta.catalog_complete = true;
    write::put_metadata(&tx, &meta).expect("metadata");
    write::put_collection(&tx, &first).expect("first");
    write::put_collection(&tx, &second).expect("second");
    tx.commit().expect("commit");

    let mut source = contribution("source", 0, 4, "source", 1);
    source.relations.push(crate::index_store::value_codec::RelationOccurrence {
        declaration_ordinal: 0,
        occurrence_ordinal: 0,
        target_collection: CollectionId(1),
        target_id: "target".into(),
        positions: vec![0],
    });
    let source_file = crate::index_store::mutations::PreparedFile {
        file: first_file,
        file_version: 1,
        fingerprint: None,
        records: vec![source],
    };
    crate::index_store::mutations::admit_file(&store, CollectionId(0), &source_file, meta.build_epoch)
        .expect("admit source");
    let mut target = contribution("target", 0, 4, "target", 2);
    target.record.file = FileId(1);
    let target_file = crate::index_store::mutations::PreparedFile {
        file: second_file,
        file_version: 1,
        fingerprint: None,
        records: vec![target],
    };
    crate::index_store::mutations::admit_file(&store, CollectionId(1), &target_file, meta.build_epoch)
        .expect("admit target");

    let view = store.view().expect("view");
    assert_eq!(
        read::incoming_relations(&view, CollectionId(1), "target")
            .expect("incoming")
            .len(),
        1
    );
    crate::index_store::mutations::delete_file(&store, FileId(1)).expect("delete target");
    let view = store.view().expect("view");
    assert_eq!(
        read::incoming_relations(&view, CollectionId(1), "target")
            .expect("incoming")
            .len(),
        1,
        "source-owned reference remains after target deletion"
    );
    assert_eq!(
        read::record_by_logical_id(&view, CollectionId(1), "target").expect("target"),
        None
    );
}
