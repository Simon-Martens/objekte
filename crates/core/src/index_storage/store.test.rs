use std::sync::Arc;

use roaring::RoaringBitmap;

use uuid::Uuid;

use super::catalogue::{CatalogueView, FileRowInput, build_catalogue};
use super::error::StorageError;
use super::format::{ObjectBuilder, ObjectKind, SectionKind};
use super::manifest::SourceRegistrationIdentity;
use super::records::{FilePartInput, RecordInput};
use super::segment::FrozenSegment;
use super::segment_builder::{SegmentInput, build_segment};
use super::store::{
    ObjectInput, RunInput, STORE_FILE, SaveHooks, SaveOptions, SavePlan, inspect_store, open_store,
    save, save_with_control,
};
use super::visibility::build_visibility;

fn catalogue_object(row: FileRowInput) -> Arc<[u8]> {
    let sections = build_catalogue(&[row]).unwrap();
    let mut builder = ObjectBuilder::new(ObjectKind::Catalogue);
    builder
        .add_section(SectionKind::FileRows, sections.file_rows)
        .unwrap();
    builder
        .add_section(SectionKind::PathDictionary, sections.path_dictionary)
        .unwrap();
    builder
        .add_section(SectionKind::Diagnostics, sections.diagnostics)
        .unwrap();
    builder
        .add_section(SectionKind::Statistics, sections.statistics)
        .unwrap();
    Arc::from(builder.finish().unwrap().into_bytes())
}

fn row(path: &str) -> FileRowInput {
    FileRowInput {
        file_id: 0,
        status: 0,
        tombstone: false,
        file_version: 1,
        content_generation: 1,
        size: 10,
        modified_ns: 1_700_000_000_000_000_000,
        created_ns: None,
        native_path: path.as_bytes().to_vec(),
        query_path: path.as_bytes().to_vec(),
        collections: vec![0],
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
        fingerprint: None,
    }
}

fn segment_object(record_id: &str) -> Arc<[u8]> {
    let built = build_segment(SegmentInput {
        collection: 0,
        source_id: "fixture".into(),
        source_version: 1,
        file_parts: vec![FilePartInput {
            file_id: 0,
            content_generation: 1,
            local_record_start: 0,
            record_count: 1,
            segment_slot_start: 0,
        }],
        records: vec![RecordInput {
            part_ordinal: 0,
            local_record_id: 0,
            byte_start: 0,
            byte_end: 10,
            logical_id: record_id.into(),
            diagnostic_ref: 0,
            record_class: 0,
        }],
        identity_order: vec![0],
        fields: Vec::new(),
        bigrams: Vec::new(),
        sorts: Vec::new(),
        relations: Vec::new(),
        logical_records: vec![0],
        empty_arrays: Vec::new(),
        active_records: vec![0],
        metadata: None,
        estimated_index_bytes: 0,
    })
    .unwrap();
    Arc::from(built.object.into_bytes())
}

fn plan(lineage: Uuid, publication: Uuid, path: &str, record: &str) -> SavePlan {
    SavePlan {
        publication_id: publication,
        lineage_id: lineage,
        source_root: std::env::temp_dir().join("objekte-store-source"),
        model_version: 1,
        configuration_identity: [7u8; 32],
        source_registrations: vec![SourceRegistrationIdentity {
            id: "fixture".into(),
            version: 1,
        }],
        collation_version: "test".into(),
        bigram_representation_version: "test".into(),
        file_id_high_water: 1,
        catalogue: ObjectInput::resident(ObjectKind::Catalogue, catalogue_object(row(path))),
        state: None,
        runs: vec![RunInput {
            collection: 0,
            ordinal: 1,
            level: 0,
            slot_count: 1,
            active_count: 1,
            object: ObjectInput::resident(ObjectKind::Segment, segment_object(record)),
            visibility: build_visibility(&RoaringBitmap::from_iter([0]), 1).unwrap(),
        }],
        collections: Vec::new(),
        frontend_metadata: None,
    }
}


struct FailBeforeCurrent;


impl SaveHooks for FailBeforeCurrent {
    fn before_current_replace(&mut self) -> Result<(), StorageError> {
        Err(StorageError::Conflict("injected before CURRENT".into()))
    }
}


struct FailManifest;


impl SaveHooks for FailManifest {
    fn before_manifest_write(&mut self) -> Result<(), StorageError> {
        Err(StorageError::Conflict("injected manifest write".into()))
    }
}


struct FailAfterObject;


impl SaveHooks for FailAfterObject {
    fn after_object_staged(&mut self, _sha256: &str) -> Result<(), StorageError> {
        Err(StorageError::Conflict(
            "injected after object staging".into(),
        ))
    }
}


struct FailAfterCurrent;


impl SaveHooks for FailAfterCurrent {
    fn after_current_replace(&mut self) -> Result<(), StorageError> {
        Err(StorageError::Conflict("injected after CURRENT".into()))
    }
}


struct FailCleanup;


impl SaveHooks for FailCleanup {
    fn during_cleanup(&mut self) -> Result<(), StorageError> {
        Err(StorageError::Conflict("injected cleanup".into()))
    }
}


#[test]
fn save_load_reuse_and_inspect_round_trip() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let publication = Uuid::new_v4();
    let report = save(
        directory.path(),
        plan(lineage, publication, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    assert!(!report.unchanged);
    assert_eq!(report.objects_written, 3);
    assert!(report.bytes_written > 0);
    assert!(directory.path().join(STORE_FILE).exists());

    let opened = open_store(directory.path()).unwrap();
    assert_eq!(opened.manifest.publication_id, publication.to_string());
    assert_eq!(opened.runs.len(), 1);
    opened.verify_full().unwrap();
    let catalogue = CatalogueView::open(opened.catalogue_backing()).unwrap();
    assert_eq!(catalogue.path(b"a.txt").unwrap(), Some(0));
    let frozen = FrozenSegment::open(opened.run_backing(&opened.runs[0])).unwrap();
    assert_eq!(
        frozen.records().logical_id(0).unwrap().as_deref(),
        Some("record-a")
    );

    let info = inspect_store(directory.path()).unwrap();
    assert_eq!(info.manifest.publication_id, publication.to_string());
    assert_eq!(info.marker.store_id, report.store_id);

    let repeated = save(
        directory.path(),
        plan(lineage, publication, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    assert!(repeated.unchanged);
    assert_eq!(repeated.objects_written, 0);
    assert_eq!(repeated.bytes_written, 0);
}

#[test]
fn changed_save_reuses_segment_objects_and_keeps_a_valid_generation() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let first = Uuid::new_v4();
    save(
        directory.path(),
        plan(lineage, first, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    let second = Uuid::new_v4();
    let report = save(
        directory.path(),
        plan(lineage, second, "b.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    assert!(!report.unchanged);
    assert_eq!(report.objects_written, 1);
    assert_eq!(report.objects_reused, 2);
    let opened = open_store(directory.path()).unwrap();
    assert_eq!(opened.manifest.publication_id, second.to_string());
    assert_eq!(
        CatalogueView::open(opened.catalogue_backing())
            .unwrap()
            .path(b"b.txt")
            .unwrap(),
        Some(0)
    );
    assert_eq!(
        FrozenSegment::open(opened.run_backing(&opened.runs[0]))
            .unwrap()
            .records()
            .logical_id(0)
            .unwrap()
            .as_deref(),
        Some("record-a")
    );
}

#[test]
fn save_to_a_second_directory_copies_the_same_objects() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let publication = Uuid::new_v4();
    let report = save(
        first.path(),
        plan(lineage, publication, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    let copied = save(
        second.path(),
        plan(lineage, publication, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    assert_eq!(copied.objects_written, report.objects_written);
    let opened = open_store(second.path()).unwrap();
    assert_eq!(opened.manifest.publication_id, publication.to_string());
    opened.verify_full().unwrap();
}

#[test]
fn cleanup_is_skipped_while_a_reader_lease_is_open() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let first = Uuid::new_v4();
    save(
        directory.path(),
        plan(lineage, first, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    let opened = open_store(directory.path()).unwrap();
    let second = Uuid::new_v4();
    let report = save(
        directory.path(),
        plan(lineage, second, "b.txt", "record-b"),
        SaveOptions::default(),
    )
    .unwrap();
    assert!(report.cleanup.readers_present);
    drop(opened);
    let third = Uuid::new_v4();
    let report = save(
        directory.path(),
        plan(lineage, third, "c.txt", "record-c"),
        SaveOptions::default(),
    )
    .unwrap();
    assert!(!report.cleanup.readers_present);
    assert_eq!(
        open_store(directory.path())
            .unwrap()
            .manifest
            .publication_id,
        third.to_string()
    );
}

#[test]
fn failure_before_commit_keeps_the_previous_generation() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let first = Uuid::new_v4();
    save(
        directory.path(),
        plan(lineage, first, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    let second = Uuid::new_v4();
    let error = save_with_control(
        directory.path(),
        plan(lineage, second, "b.txt", "record-b"),
        SaveOptions::default(),
        || false,
        |_| {},
        &mut FailBeforeCurrent,
    )
    .unwrap_err();
    assert!(matches!(error, StorageError::Conflict(_)));
    let opened = open_store(directory.path()).unwrap();
    assert_eq!(opened.manifest.publication_id, first.to_string());
    assert_eq!(
        CatalogueView::open(opened.catalogue_backing())
            .unwrap()
            .path(b"a.txt")
            .unwrap(),
        Some(0)
    );
}

#[test]
fn manifest_write_failure_keeps_the_previous_generation() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let first = Uuid::new_v4();
    save(
        directory.path(),
        plan(lineage, first, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    let second = Uuid::new_v4();
    assert!(
        save_with_control(
            directory.path(),
            plan(lineage, second, "b.txt", "record-b"),
            SaveOptions::default(),
            || false,
            |_| {},
            &mut FailManifest,
        )
        .is_err()
    );
    assert_eq!(
        open_store(directory.path())
            .unwrap()
            .manifest
            .publication_id,
        first.to_string()
    );
}

#[test]
fn failure_after_staging_keeps_the_previous_generation() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let first = Uuid::new_v4();
    save(
        directory.path(),
        plan(lineage, first, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    let second = Uuid::new_v4();
    assert!(
        save_with_control(
            directory.path(),
            plan(lineage, second, "b.txt", "record-b"),
            SaveOptions::default(),
            || false,
            |_| {},
            &mut FailAfterObject,
        )
        .is_err()
    );
    assert_eq!(
        open_store(directory.path())
            .unwrap()
            .manifest
            .publication_id,
        first.to_string()
    );
}

#[test]
fn failure_after_current_replacement_is_reported_as_cleanup_only() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let publication = Uuid::new_v4();
    let report = save_with_control(
        directory.path(),
        plan(lineage, publication, "a.txt", "record-a"),
        SaveOptions::default(),
        || false,
        |_| {},
        &mut FailAfterCurrent,
    )
    .unwrap();
    assert!(!report.cleanup.errors.is_empty());
    assert_eq!(
        open_store(directory.path())
            .unwrap()
            .manifest
            .publication_id,
        publication.to_string()
    );
}

#[test]
fn cleanup_failure_does_not_invalidate_commitment() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let publication = Uuid::new_v4();
    let report = save_with_control(
        directory.path(),
        plan(lineage, publication, "a.txt", "record-a"),
        SaveOptions::default(),
        || false,
        |_| {},
        &mut FailCleanup,
    )
    .unwrap();
    assert!(!report.cleanup.errors.is_empty());
    assert_eq!(
        open_store(directory.path())
            .unwrap()
            .manifest
            .publication_id,
        publication.to_string()
    );
}

#[test]
fn cancellation_before_commit_leaves_no_visible_generation() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let publication = Uuid::new_v4();
    let error = save_with_control(
        directory.path(),
        plan(lineage, publication, "a.txt", "record-a"),
        SaveOptions::default(),
        || true,
        |_| {},
        &mut super::store::NoHooks,
    )
    .unwrap_err();
    assert!(matches!(error, StorageError::Cancelled));
    assert!(open_store(directory.path()).is_err());
}

#[test]
fn corrupt_manifest_and_missing_objects_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let publication = Uuid::new_v4();
    save(
        directory.path(),
        plan(lineage, publication, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    let opened = open_store(directory.path()).unwrap();
    let manifest_path = directory
        .path()
        .join("generations")
        .join(opened.current.generation.to_string())
        .join("manifest.json");
    drop(opened);
    let mut bytes = std::fs::read(&manifest_path).unwrap();
    bytes[0] ^= 0xff;
    std::fs::write(&manifest_path, &bytes).unwrap();
    assert!(open_store(directory.path()).is_err());
}

#[test]
fn missing_object_is_reported() {
    let directory = tempfile::tempdir().unwrap();
    let lineage = Uuid::new_v4();
    let publication = Uuid::new_v4();
    save(
        directory.path(),
        plan(lineage, publication, "a.txt", "record-a"),
        SaveOptions::default(),
    )
    .unwrap();
    let opened = open_store(directory.path()).unwrap();
    let object = opened.runs[0].object.path().to_owned();
    drop(opened);
    std::fs::remove_file(&object).unwrap();
    assert!(open_store(directory.path()).is_err());
}

#[test]
fn undedicated_directory_and_source_root_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(directory.path().join("unrelated.txt"), b"keep").unwrap();
    let lineage = Uuid::new_v4();
    let publication = Uuid::new_v4();
    assert!(
        save(
            directory.path(),
            plan(lineage, publication, "a.txt", "record-a"),
            SaveOptions::default(),
        )
        .is_err()
    );
    assert_eq!(
        std::fs::read(directory.path().join("unrelated.txt")).unwrap(),
        b"keep"
    );

    let store = tempfile::tempdir().unwrap();
    let mut plan = plan(lineage, publication, "a.txt", "record-a");
    plan.source_root = store.path().to_owned();
    assert!(save(store.path(), plan, SaveOptions::default()).is_err());
}
