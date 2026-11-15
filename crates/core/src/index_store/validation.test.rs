//! Store invariant validation tests.

use std::sync::Arc;

use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SourceRevision,
};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::posting::ScalarKey;

use super::super::value_codec::{
    CollectionMetadata, GramKind, RecordMetadata, StateKind, StoredContributions,
};
use super::super::{IndexStore, StoreConfig, read, write};
use super::*;

fn store() -> Arc<IndexStore> {
    Arc::new(IndexStore::new(StoreConfig::default()).expect("store"))
}

fn collection() -> CollectionMetadata {
    CollectionMetadata {
        id: CollectionId(0),
        name: "validation".into(),
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
        relative_path: std::path::PathBuf::from("a.json").into(),
        revision: SourceRevision {
            modified_ns: 1,
            size: 32,
        },
        created_ns: None,
        collections: vec![CollectionId(0)],
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    }
}

fn record(slot: RecordSlot) -> RecordEntry {
    RecordEntry {
        slot,
        id: RecordId(format!("r{}", slot.0).into()),
        file: FileId(0),
        range: ByteRange { start: 0, end: 4 },
        revision: SourceRevision {
            modified_ns: 1,
            size: 32,
        },
        class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
    }
}

#[test]
fn consistent_record_is_valid_and_corruption_is_reported() {
    let store = store();
    {
        let tx = store.begin_write().expect("write");
        let mut collection = collection();
        write::put_collection(&tx, &collection).expect("collection");
        write::put_file(&tx, &file(), 1, None).expect("file");
        let entry = record(RecordSlot(0));
        let (slot, incarnation) =
            super::super::slot_allocator::allocate(&tx, &mut collection).expect("slot");
        write::put_record(
            &tx,
            collection.id,
            slot,
            &RecordMetadata::from_entry(&entry, incarnation),
        )
        .expect("record");
        write::put_logical_id(&tx, collection.id, &entry.id, slot, incarnation).expect("id");
        write::put_file_record(&tx, entry.file, collection.id, slot).expect("file record");
        let contribution = StoredContributions {
            record: RecordMetadata::from_entry(&entry, incarnation),
            values: vec![(0, ScalarKey::String("x".into()))],
            lengths: vec![(0, crate::model::Number::Integer(4))],
            state_masks: vec![(0, 1 << StateKind::PathPresence.tag())],
            coverage: Vec::new(),
            domains: Vec::new(),
            sorts: Vec::new(),
            relations: Vec::new(),
            grams: vec![super::super::value_codec::GramProjection {
                binding: 0,
                kind: GramKind::Normal,
                grams: vec![9],
            }],
        };
        write::insert_structured(&tx, collection.id, slot, &contribution).expect("structured");
        super::super::fts::insert_record_grams(&tx, collection.id, slot, &contribution.grams)
            .expect("grams");
        write::put_contributions(&tx, collection.id, slot, &contribution).expect("contributions");
        write::put_build_state(
            &tx,
            entry.file,
            &super::super::value_codec::FileBuildState {
                state: super::super::value_codec::BuildFileState::Indexed,
                expected_file_version: 1,
                build_epoch: 1,
                last_admission_revision: 1,
                error: None,
            },
        )
        .expect("build state");
        write::put_collection(&tx, &collection).expect("collection");
        tx.commit().expect("commit");
    }
    let view = store.view().expect("view");
    let report = validate(&view).expect("validate");
    assert!(report.is_valid(), "{:?}", report.errors);
    assert_eq!(report.live_records, 1);
    assert_eq!(report.value_members, 1);
    assert_eq!(report.fts_blocks, 1);

    // Corrupt the value directory without changing the membership.
    {
        let tx = store.begin_write().expect("write");
        write::set_value_directory(
            &tx,
            CollectionId(0),
            0,
            &ScalarKey::String("x".into()),
            &super::super::value_codec::ValueDirectoryEntry {
                cardinality: 7,
                representative: ScalarKey::String("x".into()),
            },
        )
        .expect("directory");
        tx.commit().expect("commit");
    }
    let view = store.view().expect("view");
    let report = validate(&view).expect("validate");
    assert!(!report.is_valid());
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains("cardinality")),
        "{:?}",
        report.errors
    );
}

#[test]
fn validation_reports_free_list_and_incarnation_violations() {
    let store = store();
    {
        let tx = store.begin_write().expect("write");
        write::put_collection(&tx, &collection()).expect("collection");
        // A free slot at or above high water is an invariant violation.
        super::super::slot_allocator::release(&tx, CollectionId(0), RecordSlot(5))
            .expect("release");
        tx.commit().expect("commit");
    }
    let view = store.view().expect("view");
    let report = validate(&view).expect("validate");
    assert!(!report.is_valid());
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains("above high water")),
        "{:?}",
        report.errors
    );
}

#[test]
fn reset_abort_preserves_the_previous_schema() {
    let store = store();
    {
        let tx = store.begin_write().expect("write");
        write::put_collection(&tx, &collection()).expect("collection");
        tx.commit().expect("commit");
    }
    let tx = store.begin_write().expect("write");
    let mut metadata = read::metadata_from_write(&tx).expect("metadata");
    let report =
        super::super::regeneration::reset_tables(&tx, &mut metadata, super::super::value_codec::ConstructionState::Empty)
            .expect("reset");
    tx.abort().expect("abort");
    let view = store.view().expect("view");
    assert_eq!(view.revision(), 1);
    assert_eq!(
        read::all_collections(&view).expect("collections")[0].live_records,
        0
    );
    assert_eq!(report.old_revision, 1);
}

#[test]
fn reset_recreates_an_empty_consistent_store() {
    let store = store();
    {
        let tx = store.begin_write().expect("write");
        write::put_collection(&tx, &collection()).expect("collection");
        write::put_file(&tx, &file(), 1, None).expect("file");
        tx.commit().expect("commit");
    }
    store.clear_index().expect("clear");
    let view = store.view().expect("view");
    let report = validate(&view).expect("validate");
    assert!(report.is_valid(), "{:?}", report.errors);
    assert_eq!(report.live_records, 0);
    assert_eq!(report.value_members, 0);
    assert_eq!(report.fts_blocks, 0);
    assert!(read::file(&view, FileId(0)).expect("file").is_none());
}
