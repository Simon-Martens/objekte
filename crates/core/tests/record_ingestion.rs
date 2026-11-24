use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use objekte_search_core::files::FileCatalog;
use objekte_search_core::ingestion::{IngestionOptions, SubmitError};
use objekte_search_core::inventory::{ByteRange, SourceRevision};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{LogicalType, Number, Value};
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::query_results::Payload;
use objekte_search_core::records::reader::{
    RecordProjection, RecordRead, RecordReadError, RecordReader,
};
use objekte_search_core::records::{RecordKey, RecordVersion};
use objekte_search_core::schema::{FieldDefinition, Schema};

fn record(year: i128, note: &str) -> Value {
    Value::Object(BTreeMap::from([
        ("year".into(), Value::Number(Number::integer(year))),
        ("note".into(), Value::String(note.into())),
    ]))
}

#[test]
fn public_batches_publish_typed_records_and_preserve_old_views() {
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["year".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let mut builder = Index::builder();
    let collection = builder.collection("entries", schema).unwrap();
    let provider = builder
        .register_records(collection, "application.entries", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let empty = index.view().unwrap();
    let writer = index.ingestion().writer(provider).unwrap();

    let mut batch = writer.batch();
    batch.insert("entry-47", record(1808, "Leipzig")).unwrap();
    let first = writer.submit(batch).unwrap().wait_published().unwrap();
    let view = index.view().unwrap();
    assert!(view.revision() >= first);
    assert!(
        empty
            .get_record("entries", "entry-47", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    let hit = view
        .get_record("entries", "entry-47", ReturnMode::Logical)
        .unwrap()
        .unwrap();
    assert_eq!(hit.payload, Some(Payload::Logical(record(1808, "Leipzig"))));
    assert!(hit.file.is_none());
    assert_eq!(
        view.get_record("entries", "entry-47", ReturnMode::Bytes)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::OriginalBytesUnavailable)
    );

    let query = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("year = 1808 && note ~ 'Leipzig'").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let result = view.search(&query).unwrap();
    assert_eq!(result.count, Some(1));
    assert_eq!(result.hits[0].record_id, "entry-47");

    let mut replacement = writer.batch();
    replacement
        .replace("entry-47", record(1809, "Wien"))
        .unwrap();
    writer
        .submit(replacement)
        .unwrap()
        .wait_published()
        .unwrap();
    assert_eq!(
        view.get_record("entries", "entry-47", ReturnMode::Logical)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::Logical(record(1808, "Leipzig")))
    );
    assert_eq!(index.view().unwrap().search(&query).unwrap().count, Some(0));

    let mut deletion = writer.batch();
    deletion.delete("entry-47").unwrap();
    writer.submit(deletion).unwrap().wait_published().unwrap();
    assert!(
        index
            .view()
            .unwrap()
            .get_record("entries", "entry-47", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
}

#[test]
fn overflow_and_rejected_batch_keep_inputs_and_publication_atomic() {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let provider = builder
        .register_records(collection, "application.entries", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let session = index
        .ingestion_with_options(IngestionOptions {
            pending_ram_bytes: 1,
            spool_bytes: 1024 * 1024,
            max_batch_bytes: 1024 * 1024,
            spool_directory: None,
        })
        .unwrap();
    let writer = session.writer(provider).unwrap();
    let mut first = writer.batch();
    first.insert("entry-47", record(1808, "Leipzig")).unwrap();
    let first = match writer.try_submit(first) {
        Err(SubmitError::WouldBlock(batch)) => batch,
        _ => panic!("memory-only offer should retain the batch"),
    };
    writer.submit(first).unwrap().wait_published().unwrap();
    assert!(session.metrics().high_water_spool_bytes > 0);

    let mut rejected = writer.batch();
    rejected.insert("new", record(1809, "Wien")).unwrap();
    rejected
        .insert("entry-47", record(1808, "duplicate"))
        .unwrap();
    assert!(writer.submit(rejected).unwrap().wait_published().is_err());
    assert!(
        index
            .view()
            .unwrap()
            .get_record("entries", "new", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(session.finish().is_err());

    let fresh = index.ingestion();
    let writer = fresh.writer(provider).unwrap();
    let mut retry = writer.batch();
    retry.insert("new", record(1809, "Wien")).unwrap();
    writer.submit(retry).unwrap().wait_published().unwrap();
    assert!(
        index
            .view()
            .unwrap()
            .get_record("entries", "new", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
}

#[test]
fn public_catalog_links_and_reverse_lookup_are_pinned_with_records() {
    let root = std::env::temp_dir().join(format!(
        "objekte-file-links-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("source.txt"), b"alpha beta").unwrap();
    std::fs::write(root.join("empty.txt"), b"").unwrap();
    let revision =
        SourceRevision::from_metadata(&std::fs::metadata(root.join("source.txt")).unwrap())
            .unwrap();
    let empty_revision =
        SourceRevision::from_metadata(&std::fs::metadata(root.join("empty.txt")).unwrap()).unwrap();
    let mut builder = Index::builder();
    let first = builder.collection("first", Schema::default()).unwrap();
    let second = builder.collection("second", Schema::default()).unwrap();
    let first_registration = builder.register_records(first, "test.first", 1).unwrap();
    let second_registration = builder.register_records(second, "test.second", 1).unwrap();
    let catalog = builder
        .register_file_catalog(FileCatalog::new(&root).unwrap())
        .unwrap();
    builder
        .enable_file_links(first_registration, catalog)
        .unwrap();
    builder
        .enable_file_links(second_registration, catalog)
        .unwrap();
    let index = builder.build().unwrap();
    let before = index.view().unwrap();
    let session = index.ingestion();
    let first_writer = session.writer(first_registration).unwrap();
    let mut batch = first_writer.batch();
    let file = batch.declare_file("source.txt", revision).unwrap();
    batch
        .insert_with_file(
            "same",
            record(1808, "alpha"),
            file,
            Some(ByteRange::checked(0, 5, revision.size).unwrap()),
        )
        .unwrap();
    batch
        .insert_with_file("path-only", record(1809, "beta"), file, None)
        .unwrap();
    batch.insert("unlinked", record(1810, "none")).unwrap();
    batch.declare_file("empty.txt", empty_revision).unwrap();
    first_writer
        .submit(batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let second_writer = session.writer(second_registration).unwrap();
    let mut batch = second_writer.batch();
    let shared = batch.declare_file("source.txt", revision).unwrap();
    batch
        .insert_with_file(
            "same",
            record(1811, "alpha"),
            shared,
            Some(ByteRange::checked(6, 10, revision.size).unwrap()),
        )
        .unwrap();
    second_writer
        .submit(batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let view = index.view().unwrap();
    assert!(
        before
            .file_catalog()
            .unwrap()
            .file_by_path(std::path::Path::new("source.txt"))
            .is_none()
    );
    let files = view.file_catalog().unwrap();
    let row = files
        .file_by_path(std::path::Path::new("source.txt"))
        .unwrap();
    assert_eq!(row.collections.len(), 2);
    assert_eq!(files.records_for_file(row.id).unwrap().len(), 3);
    assert_eq!(
        files
            .file_for_record("first", "path-only")
            .unwrap()
            .unwrap()
            .range,
        None
    );
    assert_eq!(files.file_for_record("first", "unlinked").unwrap(), None);
    assert_eq!(
        view.get_record("first", "same", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .unwrap()
            .range
            .unwrap()
            .end,
        5
    );
    assert_eq!(
        files
            .records_for_file(
                files
                    .file_by_path(std::path::Path::new("empty.txt"))
                    .unwrap()
                    .id
            )
            .unwrap()
            .len(),
        0
    );
    let handle = view.file_handle(row.id).unwrap();
    std::fs::rename(root.join("source.txt"), root.join("moved.txt")).unwrap();
    let moved = index.move_file(handle, &root.join("moved.txt")).unwrap();
    let after_move = index.view().unwrap();
    assert_eq!(
        after_move
            .get_record("first", "same", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .unwrap()
            .path,
        std::path::PathBuf::from("moved.txt")
    );
    assert_eq!(
        view.get_record("first", "same", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .unwrap()
            .path,
        std::path::PathBuf::from("source.txt")
    );
    std::fs::write(root.join("moved.txt"), b"alpha gamma expanded").unwrap();
    let updated = index
        .update_file(moved.file, &root.join("moved.txt"))
        .unwrap();
    let after_update = index.view().unwrap();
    assert!(
        after_update
            .get_record("first", "same", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .is_none()
    );
    assert!(
        after_update
            .get_record("first", "path-only", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .is_some()
    );
    assert_eq!(
        after_update
            .file_catalog()
            .unwrap()
            .records_for_file(row.id)
            .unwrap()
            .len(),
        1
    );
    index.delete_file(updated.file).unwrap();
    let after_delete = index.view().unwrap();
    assert!(
        after_delete
            .file_catalog()
            .unwrap()
            .file_by_path(std::path::Path::new("moved.txt"))
            .is_none()
    );
    assert!(
        after_delete
            .get_record("first", "same", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .is_none()
    );
    assert!(
        after_delete
            .get_record("second", "same", ReturnMode::Logical)
            .unwrap()
            .is_some()
    );
    assert_eq!(files.records_for_file(row.id).unwrap().len(), 3);
    session.finish().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

struct VersionedFixture(Mutex<(RecordVersion, Value)>);

impl RecordReader for VersionedFixture {
    fn read_projected(
        &self,
        _: &RecordKey,
        expected_version: RecordVersion,
        _: &RecordProjection,
    ) -> Result<RecordRead, RecordReadError> {
        let current = self.0.lock().unwrap();
        if current.0 != expected_version {
            return Err(RecordReadError::ChangedVersion {
                expected: expected_version,
                actual: Some(current.0),
            });
        }
        Ok(RecordRead {
            version: current.0,
            value: current.1.clone(),
            text: Vec::new(),
            original_bytes: None,
        })
    }
}

#[test]
fn versioned_provider_disposes_input_and_checks_rereads() {
    let root = std::env::temp_dir().join(format!("objekte-provider-links-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    std::fs::write(root.join("source.txt"), b"alpha beta").unwrap();
    let file_revision =
        SourceRevision::from_metadata(&std::fs::metadata(root.join("source.txt")).unwrap())
            .unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["year".into()],
            kind: LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let mut builder = Index::builder();
    let collection = builder.collection("entries", schema).unwrap();
    let registration = builder
        .register_records(collection, "fixture.reader", 1)
        .unwrap();
    let reader = Arc::new(VersionedFixture(Mutex::new((
        RecordVersion::new(7),
        record(1808, "Leipzig"),
    ))));
    builder
        .register_reader(registration, reader.clone())
        .unwrap();
    let catalog = builder
        .register_file_catalog(FileCatalog::new(&root).unwrap())
        .unwrap();
    builder.enable_file_links(registration, catalog).unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    let mut batch = writer.batch();
    let file = batch.declare_file("source.txt", file_revision).unwrap();
    batch
        .insert_from_reader_with_file(
            "entry-47",
            record(1808, "Leipzig"),
            RecordVersion::new(7),
            file,
            Some(ByteRange::checked(0, 5, file_revision.size).unwrap()),
        )
        .unwrap();
    writer.submit(batch).unwrap().wait_published().unwrap();
    let view = index.view().unwrap();
    assert_eq!(
        view.file_catalog()
            .unwrap()
            .file_for_record("entries", "entry-47")
            .unwrap()
            .unwrap()
            .range
            .unwrap()
            .end,
        5
    );
    assert_eq!(
        view.get_record("entries", "entry-47", ReturnMode::Logical)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::Logical(record(1808, "Leipzig")))
    );
    let query = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("year = 1808").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    };
    assert_eq!(view.search(&query).unwrap().count, Some(1));
    let by_file = Query {
        filter: Some(parse_filter("@file.path = 'source.txt'").unwrap()),
        options: QueryOptions {
            return_mode: ReturnMode::Locator,
            ..query.options.clone()
        },
        ..query.clone()
    };
    assert_eq!(view.search(&by_file).unwrap().count, Some(1));
    let residual = Query {
        filter: Some(parse_filter("note ~ 'Leipzig'").unwrap()),
        options: QueryOptions {
            return_mode: ReturnMode::Locator,
            ..query.options.clone()
        },
        ..query.clone()
    };
    assert_eq!(view.search(&residual).unwrap().count, Some(1));
    *reader.0.lock().unwrap() = (RecordVersion::new(8), record(1809, "Wien"));
    assert!(
        view.get_record("entries", "entry-47", ReturnMode::Logical)
            .is_err()
    );
    assert!(view.search(&residual).is_err());
    let handle = view
        .file_handle_for_path(std::path::Path::new("source.txt"))
        .unwrap();
    index.delete_file(handle).unwrap();
    assert!(
        index
            .view()
            .unwrap()
            .get_record("entries", "entry-47", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .is_none()
    );
    session.finish().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
