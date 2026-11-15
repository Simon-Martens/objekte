//! Store ownership, snapshot and reset tests.

use std::sync::Arc;

use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SourceRevision,
};
use crate::model::{CollectionId, FieldPath, FileId, RecordId, RecordSlot};
use crate::posting::ScalarKey;

use super::construction::{ConstructionCounters, mark_indexed};
use super::key_codec::encode_scalar;
use super::regeneration::reset_tables;
use super::value_codec::{
    BindingDescriptor, BindingIdentity, BindingRole, BuildFileState, CollectionMetadata,
    ConstructionState, FieldDescriptor, FileBuildState, GramKind, RecordMetadata,
    RelationOccurrence, SortDefinition, SortDirection, SortProjection, StateKind, StoredBinding,
    StoredContributions, ValueDirectoryEntry,
};
use super::{IndexStore, StoreConfig, StoreError, StoreView, read, slot_allocator, tables, write};

fn config() -> StoreConfig {
    StoreConfig {
        cache_bytes: 8 * 1024 * 1024,
        configuration_digest: [7; 32],
        source_root: b"/tmp/source".to_vec(),
    }
}

fn collection() -> CollectionMetadata {
    CollectionMetadata {
        id: CollectionId(0),
        name: "test".into(),
        generation: 1,
        source_fingerprint: [3; 32],
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

fn file_entry(id: u32, path: &str) -> FileEntry {
    FileEntry {
        id: FileId(id),
        relative_path: std::path::PathBuf::from(path).into(),
        revision: SourceRevision {
            modified_ns: 10,
            size: 100,
        },
        created_ns: None,
        collections: vec![CollectionId(0)],
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    }
}

fn record_entry(slot: u32, id: &str) -> RecordEntry {
    RecordEntry {
        slot: RecordSlot(slot),
        id: RecordId(id.into()),
        file: FileId(0),
        range: ByteRange { start: 0, end: 10 },
        revision: SourceRevision {
            modified_ns: 10,
            size: 100,
        },
        class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
    }
}

fn contributions(record: &RecordEntry, incarnation: u64) -> StoredContributions {
    StoredContributions {
        record: RecordMetadata::from_entry(record, incarnation),
        values: vec![
            (0, ScalarKey::String("alpha".into())),
            (0, ScalarKey::Number(crate::model::Number::Integer(4))),
        ],
        lengths: vec![(0, crate::model::Number::Integer(5))],
        state_masks: vec![(0, (1 << StateKind::PathPresence.tag()) | (1 << StateKind::NonEmpty.tag()))],
        coverage: Vec::new(),
        domains: Vec::new(),
        sorts: Vec::new(),
        relations: Vec::new(),
        grams: Vec::new(),
    }
}

fn empty_store() -> Arc<IndexStore> {
    Arc::new(IndexStore::new(config()).expect("store"))
}

#[test]
fn empty_store_has_every_registered_table_and_metadata() {
    let store = empty_store();
    let view = store.view().expect("view");
    assert_eq!(view.revision(), 1);
    assert_eq!(view.metadata().construction, ConstructionState::Empty);
    assert!(!view.metadata().catalog_complete);
    let names = store.table_names().expect("tables");
    assert_eq!(names.len(), tables::ALL_TABLES.len());
    for (name, _) in tables::ALL_TABLES {
        assert!(names.iter().any(|observed| observed == name), "{name}");
    }
}

#[test]
fn write_and_read_one_record_through_a_view() {
    let store = empty_store();
    let mut metadata = store.view().expect("view").metadata().clone();
    {
        let tx = store.begin_write().expect("write");
        let mut collection = collection();
        write::put_collection(&tx, &collection).expect("collection");
        write::put_file(&tx, &file_entry(0, "a.json"), 1, None).expect("file");
        let (slot, incarnation) =
            slot_allocator::allocate(&tx, &mut collection).expect("slot");
        assert_eq!(slot, RecordSlot(0));
        assert_eq!(incarnation, 1);
        let entry = record_entry(slot.0, "rec");
        write::put_record(
            &tx,
            collection.id,
            slot,
            &RecordMetadata::from_entry(&entry, incarnation),
        )
        .expect("record");
        write::put_logical_id(&tx, collection.id, &entry.id, slot, incarnation).expect("id");
        write::put_file_record(&tx, entry.file, collection.id, slot).expect("file record");
        let contribution = contributions(&entry, incarnation);
        write::insert_structured(&tx, collection.id, slot, &contribution).expect("members");
        write::put_contributions(&tx, collection.id, slot, &contribution).expect("contributions");
        write::add_pending_file(&tx, collection.id, entry.file).expect("pending");
        write::put_build_state(
            &tx,
            entry.file,
            &FileBuildState {
                state: BuildFileState::Pending,
                expected_file_version: 1,
                build_epoch: metadata.build_epoch,
                last_admission_revision: metadata.revision,
                error: None,
            },
        )
        .expect("state");
        let mut counters = ConstructionCounters::from_collection(&collection);
        counters.observe_discovered(1);
        counters.apply_to(&mut collection);
        write::put_collection(&tx, &collection).expect("collection update");
        mark_indexed(
            &tx,
            &mut collection,
            entry.file,
            1,
            metadata.build_epoch,
            metadata.revision + 1,
        )
        .expect("indexed");
        write::put_collection(&tx, &collection).expect("collection final");
        metadata.revision += 1;
        write::put_metadata(&tx, &metadata).expect("metadata");
        tx.commit().expect("commit");
    }
    let view = store.view().expect("view");
    assert_eq!(view.revision(), 2);
    assert_eq!(
        read::record(&view, CollectionId(0), RecordSlot(0))
            .expect("record")
            .expect("present")
            .id,
        RecordId("rec".into())
    );
    assert_eq!(
        read::record_by_logical_id(&view, CollectionId(0), "rec").expect("logical"),
        Some((RecordSlot(0), 1))
    );
    assert!(read::file(&view, FileId(0)).expect("file").is_some());
    assert_eq!(
        read::file_by_native_path(&view, b"a.json").expect("path"),
        Some(FileId(0))
    );
    assert_eq!(
        read::value_members(
            &view,
            CollectionId(0),
            0,
            &ScalarKey::String("alpha".into())
        )
        .expect("members"),
        vec![RecordSlot(0)]
    );
    let directory = read::value_directory_entry(
        &view,
        CollectionId(0),
        0,
        &ScalarKey::String("alpha".into()),
    )
    .expect("directory")
    .expect("entry");
    assert_eq!(directory.cardinality, 1);
    assert_eq!(
        read::state_members(&view, CollectionId(0), 0, StateKind::NonEmpty).expect("state"),
        vec![RecordSlot(0)]
    );
    assert_eq!(
        read::build_state_kind(&view, FileId(0)).expect("state"),
        Some(BuildFileState::Indexed)
    );
    assert!(read::pending_files(&view, CollectionId(0)).expect("pending").is_empty());
}

#[test]
fn aborted_transaction_leaves_previous_snapshot_intact() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    let collection = collection();
    write::put_collection(&tx, &collection).expect("collection");
    tx.abort().expect("abort");
    let view = store.view().expect("view");
    assert!(read::all_collections(&view).expect("collections").is_empty());
}

#[test]
fn retained_view_survives_commit_and_owner_drop() {
    let store = empty_store();
    let old = store.view().expect("old view");
    let tx = store.begin_write().expect("write");
    write::put_collection(&tx, &collection()).expect("collection");
    tx.commit().expect("commit");
    drop(store);
    assert_eq!(read::all_collections(&old).expect("old").len(), 0);
}

#[test]
fn old_view_keeps_old_revision_after_reset() {
    let store = empty_store();
    {
        let tx = store.begin_write().expect("write");
        write::put_collection(&tx, &collection()).expect("collection");
        tx.commit().expect("commit");
    }
    let old = store.view().expect("old");
    assert_eq!(read::all_collections(&old).expect("old collections").len(), 1);
    let report = store.clear_index().expect("clear");
    assert!(report.new_revision > report.old_revision);
    let new = store.view().expect("new");
    assert_eq!(new.revision(), report.new_revision);
    let collections = read::all_collections(&new).expect("new collections");
    assert_eq!(collections.len(), 1);
    assert_eq!(collections[0].generation, 2);
    assert_eq!(collections[0].live_records, 0);
    assert_eq!(read::all_collections(&old).expect("old collections")[0].generation, 1);
}

#[test]
fn reset_deletes_and_recreates_every_table() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    write::put_collection(&tx, &collection()).expect("collection");
    let mut metadata = read::metadata_from_write(&tx).expect("metadata");
    let report = reset_tables(&tx, &mut metadata, ConstructionState::Empty).expect("reset");
    tx.commit().expect("commit");
    assert_eq!(report.deleted_tables, tables::ALL_TABLES.len());
    assert_eq!(report.recreated_tables, tables::ALL_TABLES.len());
    let view = store.view().expect("view");
    assert_eq!(view.revision(), report.new_revision);
    assert!(read::all_collections(&view).expect("collections")[0]
        .live_records
        == 0);
}

#[test]
fn view_is_send_and_sync_so_parallel_queries_can_share_it() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<StoreView>();
}

#[test]
fn slot_reuse_uses_smallest_free_slot_and_advances_incarnation() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    let mut collection = collection();
    write::put_collection(&tx, &collection).expect("collection");
    let (first, first_incarnation) = slot_allocator::allocate(&tx, &mut collection).expect("first");
    let (second, second_incarnation) =
        slot_allocator::allocate(&tx, &mut collection).expect("second");
    assert_eq!(first, RecordSlot(0));
    assert_eq!(second, RecordSlot(1));
    assert_eq!(first_incarnation, 1);
    assert_eq!(second_incarnation, 1);
    slot_allocator::release(&tx, collection.id, first).expect("release");
    let (reused, reused_incarnation) =
        slot_allocator::allocate(&tx, &mut collection).expect("reuse");
    assert_eq!(reused, first);
    assert_eq!(reused_incarnation, 2);
    assert_eq!(collection.slot_high_water, 2);
    tx.abort().expect("abort");
}

#[test]
fn stale_slot_handle_is_rejected_after_reuse() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    let mut collection = collection();
    write::put_collection(&tx, &collection).expect("collection");
    let (slot, incarnation) = slot_allocator::allocate(&tx, &mut collection).expect("slot");
    let handle = slot_allocator::SlotHandle {
        collection: collection.id,
        generation: collection.generation,
        slot,
        incarnation,
    };
    slot_allocator::validate_handle(&tx, handle, collection.generation).expect("fresh");
    slot_allocator::release(&tx, collection.id, slot).expect("release");
    let (_, next) = slot_allocator::allocate(&tx, &mut collection).expect("reuse");
    let error = slot_allocator::validate_handle(&tx, handle, collection.generation)
        .expect_err("stale");
    assert!(matches!(error, StoreError::StaleHandle(_)));
    assert_eq!(next, 2);
}

#[test]
fn key_prefix_scans_do_not_leak_between_collections() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    write::insert_value_member(
        &tx,
        CollectionId(1),
        0,
        &ScalarKey::String("x".into()),
        RecordSlot(1),
    )
    .expect("member");
    write::insert_value_member(
        &tx,
        CollectionId(2),
        0,
        &ScalarKey::String("x".into()),
        RecordSlot(2),
    )
    .expect("member");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        read::value_members(&view, CollectionId(1), 0, &ScalarKey::String("x".into()))
            .expect("members"),
        vec![RecordSlot(1)]
    );
    assert_eq!(
        read::value_members(&view, CollectionId(2), 0, &ScalarKey::String("x".into()))
            .expect("members"),
        vec![RecordSlot(2)]
    );
}

#[test]
fn binding_identity_and_descriptor_round_trip() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    let identity = BindingIdentity::Declared {
        path: vec!["title".into()],
    };
    let encoded = bincode::serialize(&identity).expect("identity");
    write::put_binding(
        &tx,
        CollectionId(0),
        &encoded,
        &StoredBinding {
            id: 9,
            descriptor: BindingDescriptor {
                identity,
                role: BindingRole::Structured,
                kind: crate::model::LogicalType::String,
                multi: false,
                text_alphabet: None,
                text_segment: None,
                fingerprint: [1; 32],
            },
        },
    )
    .expect("binding");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    let record = read::binding_by_identity(&view, CollectionId(0), &encoded)
        .expect("lookup")
        .expect("present");
    assert_eq!(record.id, 9);
    assert_eq!(
        read::binding(&view, CollectionId(0), 9)
            .expect("by id")
            .expect("present")
            .kind,
        crate::model::LogicalType::String
    );
}

#[test]
fn sort_entries_are_ordered_by_encoded_key() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    let definition = SortDefinition {
        id: 0,
        constituents: vec![super::value_codec::SortConstituent {
            key: FieldPath {
                root: crate::model::PathRoot::Data,
                segments: vec!["n".into()],
            },
            direction: SortDirection::Ascending,
            options: Default::default(),
        }],
        locale: "und".into(),
        collation_version: "test".into(),
    };
    write::put_sort_definition(&tx, CollectionId(0), &definition).expect("definition");
    let projections = [
        (2u32, crate::model::Number::Integer(20)),
        (1u32, crate::model::Number::Integer(10)),
        (0u32, crate::model::Number::Integer(30)),
    ];
    for (slot, number) in projections {
        super::sorting::insert_sort_projection(
            &tx,
            CollectionId(0),
            RecordSlot(slot),
            &format!("id{slot}"),
            &[SortProjection {
                definition: 0,
                key: vec![crate::sorting::SortKey::Number(number)],
            }],
            std::slice::from_ref(&definition),
        )
        .expect("sort");
    }
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    let entries = read::sort_entries(&view, CollectionId(0), 0, 10).expect("entries");
    assert_eq!(
        entries
            .iter()
            .map(|(id, slot, _)| (id.as_str(), slot.0))
            .collect::<Vec<_>>(),
        vec![("id1", 1), ("id2", 2), ("id0", 0)]
    );
}

#[test]
fn fts_batch_updates_statistics_and_candidates() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    let mut batch = super::fts::FtsBatch::new();
    for slot in [0u32, 1, 4095, 4096, 4097] {
        batch.set(0, GramKind::Normal, 11, slot);
    }
    super::fts::apply_batch(&tx, CollectionId(0), &batch).expect("batch");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    let stats = read::gram_statistics(&view, CollectionId(0), 0, GramKind::Normal, 11)
        .expect("stats")
        .expect("present");
    assert_eq!(stats, (5, 2));
    let candidates = super::fts::candidates(
        &view,
        CollectionId(0),
        0,
        GramKind::Normal,
        &[11],
        None,
    )
    .expect("candidates")
    .expect("restriction");
    assert_eq!(
        candidates.iter().collect::<Vec<_>>(),
        vec![0, 1, 4095, 4096, 4097]
    );
}

#[test]
fn gram_lists_are_versioned_and_reject_trailing_bytes() {
    let grams = vec![1u64, 2, 130, 1_000_000];
    let encoded = super::value_codec::encode_gram_list(&grams);
    assert_eq!(
        super::value_codec::decode_gram_list(&encoded).expect("decode"),
        grams
    );
    let mut trailing = encoded;
    trailing.push(0);
    assert!(matches!(
        super::value_codec::decode_gram_list(&trailing),
        Err(StoreError::CorruptIndex(_))
    ));
}

#[test]
fn value_directory_keeps_one_representative() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    let value = ScalarKey::Number(crate::model::Number::Integer(1));
    write::insert_value_member(&tx, CollectionId(0), 0, &value, RecordSlot(0)).expect("one");
    write::insert_value_member(&tx, CollectionId(0), 0, &value, RecordSlot(1)).expect("two");
    assert_eq!(
        write::get_contributions(&tx, CollectionId(0), RecordSlot(0)).expect("get"),
        None
    );
    write::remove_value_member(&tx, CollectionId(0), 0, &value, RecordSlot(1)).expect("remove");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    let entry = read::value_directory_entry(&view, CollectionId(0), 0, &value)
        .expect("directory")
        .expect("entry");
    assert_eq!(entry.cardinality, 1);
    assert_eq!(entry.representative, value);
}

#[test]
fn relation_rows_keep_occurrence_positions_and_incoming_identity() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    let occurrence = RelationOccurrence {
        declaration_ordinal: 2,
        occurrence_ordinal: 1,
        target_collection: CollectionId(3),
        target_id: "target".into(),
        positions: vec![0, 4, 9],
    };
    super::relations::insert_relation_occurrences(
        &tx,
        CollectionId(1),
        RecordSlot(5),
        std::slice::from_ref(&occurrence),
    )
    .expect("relations");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    let outgoing = read::outgoing_relations(&view, CollectionId(1), 2).expect("outgoing");
    assert_eq!(outgoing.len(), 1);
    let (collection, target, positions) =
        super::relations::decode_outgoing(&outgoing[0].1).expect("decode");
    assert_eq!(collection, CollectionId(3));
    assert_eq!(target, "target");
    assert_eq!(positions, vec![0, 4, 9]);
    let incoming = read::incoming_relations(&view, CollectionId(3), "target").expect("incoming");
    assert_eq!(incoming.len(), 1);
    assert_eq!(
        super::relations::decode_incoming(&incoming[0]).expect("decode"),
        vec![0, 4, 9]
    );
}

#[test]
fn validation_reports_a_consistent_store() {
    let store = empty_store();
    let view = store.view().expect("view");
    let report = super::validation::validate(&view).expect("validate");
    assert!(report.is_valid(), "{:?}", report.errors);
    assert_eq!(report.collections, 0);
}

#[test]
fn field_descriptor_round_trip() {
    let store = empty_store();
    let tx = store.begin_write().expect("write");
    let path = vec!["author".to_owned()];
    write::put_field(
        &tx,
        CollectionId(0),
        &path,
        &FieldDescriptor {
            path: FieldPath {
                root: crate::model::PathRoot::Data,
                segments: path.clone(),
            },
            kind: crate::model::LogicalType::String,
            multi: true,
            indexed: true,
            values_indexed: true,
            value_complete: false,
            length_complete: true,
            empty_string_complete: true,
            covered_records: 3,
        },
    )
    .expect("field");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    let field = read::field(&view, CollectionId(0), &path)
        .expect("field")
        .expect("present");
    assert_eq!(field.covered_records, 3);
    assert!(field.multi);
}

#[test]
fn value_directory_encoding_is_stable() {
    let entry = ValueDirectoryEntry {
        cardinality: 42,
        representative: ScalarKey::String("value".into()),
    };
    let encoded = super::value_codec::encode(&entry).expect("encode");
    assert_eq!(encoded[0], super::value_codec::VALUE_FORMAT_TAG);
    let decoded: ValueDirectoryEntry =
        super::value_codec::decode(&encoded, "test").expect("decode");
    assert_eq!(decoded, entry);
    assert!(super::value_codec::decode::<ValueDirectoryEntry>(&[], "test").is_err());
}

#[test]
fn scalar_encoding_is_not_dependent_on_rust_enum_layout() {
    assert_eq!(
        encode_scalar(&ScalarKey::String("a".into())).expect("scalar"),
        vec![0, b'a', 0, 0]
    );
    assert_eq!(
        encode_scalar(&ScalarKey::Id("a".into())).expect("scalar"),
        vec![1, b'a', 0, 0]
    );
    assert_eq!(
        encode_scalar(&ScalarKey::Bool(true)).expect("scalar"),
        vec![3, 1]
    );
}
