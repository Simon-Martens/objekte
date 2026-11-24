use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use objekte_search_core::files::FileCatalog;
use objekte_search_core::ingestion::{IngestionOptions, SubmitError};
use objekte_search_core::inventory::{ByteRange, SourceRevision};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{LogicalType, Number, Value};
use objekte_search_core::query::options::{ReturnMode, SortField};
use objekte_search_core::query::{
    CaseSensitivity, FtsField, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use objekte_search_core::query_execution::streaming::{
    CancellationToken, ResultScope, StreamControl, StreamEvent, StreamOptions, StreamOutcome,
};
use objekte_search_core::query_results::Payload;
use objekte_search_core::records::reader::{
    RecordProjection, RecordRead, RecordReadError, RecordReadRequest, RecordReader,
};
use objekte_search_core::records::{RecordKey, RecordVersion};
use objekte_search_core::repository::configuration::{
    CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::schema::relations::{RelationCardinality, RelationDefinition};
use objekte_search_core::schema::{FieldDefinition, Schema};
use objekte_search_core::source::{SourceFormat, registry::SourceRegistry};

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
    batch
        .insert("unlinked-survivor", record(1812, "kept"))
        .unwrap();
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
    for (predicate, expected) in [
        ("@record.byte_start = 0", vec!["same"]),
        ("@record.byte_start != 0", vec![]),
    ] {
        let query = Query {
            collections: vec!["first".into()],
            filter: Some(parse_filter(predicate).unwrap()),
            fts: None,
            options: QueryOptions {
                sort: Vec::new(),
                offset: 0,
                count: true,
                limit: 10,
                return_mode: ReturnMode::Locator,
                evidence: false,
            },
        };
        let result = view.search(&query).unwrap();
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{predicate}"
        );
        assert_eq!(result.hits, view.search_reference(&query).unwrap().hits);
    }
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
            .records_for_file_in_collection(row.id, "first")
            .unwrap()
            .len(),
        2
    );
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
    let mut deletion = first_writer.batch();
    deletion.delete("unlinked").unwrap();
    first_writer
        .submit(deletion)
        .unwrap()
        .wait_published()
        .unwrap();
    index.compact().unwrap();
    let compacted = index.view().unwrap();
    for id in ["same", "path-only", "unlinked-survivor"] {
        assert!(
            compacted
                .get_record("first", id, ReturnMode::Logical)
                .unwrap()
                .is_some()
        );
    }
    assert!(
        compacted
            .get_record("first", "unlinked", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(
        view.get_record("first", "unlinked", ReturnMode::Logical)
            .unwrap()
            .is_some()
    );
    assert_eq!(
        compacted
            .file_catalog()
            .unwrap()
            .records_for_file(row.id)
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        compacted
            .get_record("first", "same", ReturnMode::Bytes)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::OriginalBytesUnavailable)
    );
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

struct VersionedFixture(Mutex<(RecordVersion, Value)>, Mutex<Vec<RecordProjection>>);

impl RecordReader for VersionedFixture {
    fn read_projected(
        &self,
        _: &RecordKey,
        expected_version: RecordVersion,
        projection: &RecordProjection,
    ) -> Result<RecordRead, RecordReadError> {
        self.1.lock().unwrap().push(projection.clone());
        let current = self.0.lock().unwrap();
        if current.0 != expected_version {
            return Err(RecordReadError::ChangedVersion {
                expected: expected_version,
                actual: Some(current.0),
            });
        }
        let mut value = current.1.clone();
        if !projection.fields.is_empty()
            && let Value::Object(fields) = &mut value
        {
            fields.retain(|name, _| {
                projection
                    .fields
                    .iter()
                    .any(|path| path.first() == Some(name))
            });
        }
        Ok(RecordRead {
            version: current.0,
            value,
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
    let reader = Arc::new(VersionedFixture(
        Mutex::new((RecordVersion::new(7), record(1808, "Leipzig"))),
        Mutex::new(Vec::new()),
    ));
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
    batch
        .insert_from_reader("retired", record(1808, "Leipzig"), RecordVersion::new(7))
        .unwrap();
    writer.submit(batch).unwrap().wait_published().unwrap();
    let pinned = index.view().unwrap();
    let mut deletion = writer.batch();
    deletion.delete("retired").unwrap();
    writer.submit(deletion).unwrap().wait_published().unwrap();
    index.compact().unwrap();
    let view = index.view().unwrap();
    assert!(
        pinned
            .get_record("entries", "retired", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert!(
        view.get_record("entries", "retired", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
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
    assert!(
        reader.1.lock().unwrap().iter().any(|projection| {
            projection.fields == [vec!["note".to_owned()]] && !projection.text
        })
    );
    let mut streamed = Vec::new();
    view.search_stream_scoped(
        &residual,
        ResultScope::All { offset: 0 },
        StreamOptions::default(),
        &CancellationToken::new(),
        &mut |event| {
            if let StreamEvent::OrderedRows { rows, .. } = event {
                streamed.extend(rows.iter().map(|hit| hit.record_id.clone()));
            }
            Ok(StreamControl::Continue)
        },
    )
    .unwrap();
    assert_eq!(streamed, ["entry-47"]);
    let text_query = Query {
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "Leipzig".into(),
            pattern_parameter: None,
            fields: vec![FtsField {
                path: objekte_search_core::model::FieldPath::parse("note").unwrap(),
                binding: None,
                query_path: None,
            }],
        }),
        filter: None,
        ..query.clone()
    };
    assert_eq!(view.search(&text_query).unwrap().count, Some(1));
    assert!(
        reader
            .1
            .lock()
            .unwrap()
            .iter()
            .any(|projection| projection.text)
    );
    assert_eq!(
        view.get_record("entries", "entry-47", ReturnMode::Bytes)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::OriginalBytesUnavailable)
    );
    assert!(
        reader
            .1
            .lock()
            .unwrap()
            .iter()
            .any(|projection| projection.original_bytes)
    );
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

#[test]
fn routed_direct_lookup_survives_many_batches_and_compaction() {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder
        .register_records(collection, "fixture.routes", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    for number in 0..96 {
        let mut batch = writer.batch();
        batch
            .insert(
                format!("entry-{number}"),
                Value::String(format!("value-{number}")),
            )
            .unwrap();
        writer.submit(batch).unwrap().wait_published().unwrap();
    }
    let before = index.view().unwrap();
    for number in [0, 12, 48, 95] {
        assert_eq!(
            before
                .get_record("entries", &format!("entry-{number}"), ReturnMode::Logical)
                .unwrap()
                .unwrap()
                .payload,
            Some(Payload::Logical(Value::String(format!("value-{number}"))))
        );
    }
    let mut change = writer.batch();
    change
        .replace("entry-12", Value::String("new".into()))
        .unwrap();
    writer.submit(change).unwrap().wait_published().unwrap();
    index.compact().unwrap();
    let after = index.view().unwrap();
    assert_eq!(
        after
            .get_record("entries", "entry-12", ReturnMode::Logical)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::Logical(Value::String("new".into())))
    );
    assert_eq!(
        before
            .get_record("entries", "entry-12", ReturnMode::Logical)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::Logical(Value::String("value-12".into())))
    );
    session.finish().unwrap();
}

#[test]
fn compaction_preserves_untouched_retained_survivor_in_partially_retired_segment() {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder
        .register_records(collection, "compaction.survivor", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let writer = index.ingestion().writer(registration).unwrap();

    let mut first = writer.batch();
    first
        .insert("a", Value::String("original-a".into()))
        .unwrap();
    first
        .insert("b", Value::String("original-b".into()))
        .unwrap();
    writer.submit(first).unwrap().wait_published().unwrap();
    let pinned = index.view().unwrap();

    let mut deletion = writer.batch();
    deletion.delete("b").unwrap();
    writer.submit(deletion).unwrap().wait_published().unwrap();
    index.compact().unwrap();

    let current = index.view().unwrap();
    assert_eq!(
        current
            .get_record("entries", "a", ReturnMode::Logical)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::Logical(Value::String("original-a".into())))
    );
    assert_eq!(
        current
            .get_record("entries", "a", ReturnMode::Bytes)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::OriginalBytesUnavailable)
    );
    assert!(
        current
            .get_record("entries", "b", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    for id in ["a", "b"] {
        assert!(
            pinned
                .get_record("entries", id, ReturnMode::Logical)
                .unwrap()
                .is_some()
        );
    }
}

#[test]
fn admission_retirement_compaction_keeps_unmodified_native_survivors() {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder
        .register_records(collection, "compaction.automatic", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let writer = index.ingestion().writer(registration).unwrap();
    let mut batch = writer.batch();
    for number in 0..256 {
        batch
            .insert(
                format!("id-{number:03}"),
                Value::String(format!("body-{number:03}")),
            )
            .unwrap();
    }
    writer.submit(batch).unwrap().wait_published().unwrap();
    let pinned = index.view().unwrap();
    let mut deletion = writer.batch();
    for number in 0..64 {
        deletion.delete(format!("id-{number:03}")).unwrap();
    }
    writer.submit(deletion).unwrap().wait_published().unwrap();
    let mut next = writer.batch();
    next.insert("trigger", Value::String("next".into()))
        .unwrap();
    writer.submit(next).unwrap().wait_published().unwrap();
    assert_eq!(index.compact().unwrap().retired_slots_removed, 0);

    let current = index.view().unwrap();
    for number in 64..256 {
        let id = format!("id-{number:03}");
        assert_eq!(
            current
                .get_record("entries", &id, ReturnMode::Logical)
                .unwrap()
                .unwrap()
                .payload,
            Some(Payload::Logical(Value::String(format!("body-{number:03}"))))
        );
    }
    assert!(
        current
            .get_record("entries", "id-000", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(
        current
            .get_record("entries", "trigger", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert!(
        pinned
            .get_record("entries", "id-000", ReturnMode::Logical)
            .unwrap()
            .is_some()
    );
}

#[test]
fn fileless_records_stream_in_page_and_all_scopes() {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder
        .register_records(collection, "fixture.stream", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    let mut batch = writer.batch();
    for number in 0..7 {
        batch
            .insert(format!("entry-{number}"), Value::String("Leipzig".into()))
            .unwrap();
    }
    writer.submit(batch).unwrap().wait_published().unwrap();
    let view = index.view().unwrap();
    let query = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            offset: 0,
            limit: 3,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    };
    for (scope, expected) in [
        (
            ResultScope::Page {
                offset: 0,
                limit: 3,
            },
            3,
        ),
        (ResultScope::All { offset: 0 }, 7),
    ] {
        let mut ids = Vec::new();
        let outcome = view
            .search_stream_scoped(
                &query,
                scope,
                StreamOptions::default(),
                &CancellationToken::new(),
                &mut |event| {
                    if let StreamEvent::OrderedRows { rows, .. } = event {
                        ids.extend(rows.iter().map(|hit| hit.record_id.clone()));
                    }
                    Ok(StreamControl::Continue)
                },
            )
            .unwrap();
        assert_eq!(ids.len(), expected);
        assert!(matches!(outcome, StreamOutcome::Complete(_)));
    }
    let residual = Query {
        filter: Some(parse_filter("@record.id = 'entry-3'").unwrap()),
        ..query
    };
    let mut streamed = Vec::new();
    view.search_stream_scoped(
        &residual,
        ResultScope::All { offset: 0 },
        StreamOptions::default(),
        &CancellationToken::new(),
        &mut |event| {
            if let StreamEvent::OrderedRows { rows, .. } = event {
                streamed.extend(rows.iter().map(|hit| hit.record_id.clone()));
            }
            Ok(StreamControl::Continue)
        },
    )
    .unwrap();
    assert_eq!(streamed, ["entry-3"]);
    session.finish().unwrap();
}

#[test]
fn fileless_rebuild_keeps_retained_records_and_rejects_stale_provider() {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let retained = builder.register_records(collection, "owned", 1).unwrap();
    let provider = builder.register_records(collection, "reader", 1).unwrap();
    let reader = Arc::new(VersionedFixture(
        Mutex::new((RecordVersion::new(4), Value::String("original".into()))),
        Mutex::new(Vec::new()),
    ));
    builder.register_reader(provider, reader.clone()).unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let owned_writer = session.writer(retained).unwrap();
    let mut owned = owned_writer.batch();
    owned
        .insert("owned-1", Value::String("kept".into()))
        .unwrap();
    owned_writer
        .submit(owned)
        .unwrap()
        .wait_published()
        .unwrap();
    let provider_writer = session.writer(provider).unwrap();
    let mut from_reader = provider_writer.batch();
    from_reader
        .insert_from_reader(
            "reader-1",
            Value::String("original".into()),
            RecordVersion::new(4),
        )
        .unwrap();
    provider_writer
        .submit(from_reader)
        .unwrap()
        .wait_published()
        .unwrap();
    let before = index.view().unwrap();
    let rebuilt = index.rebuild().unwrap();
    assert!(rebuilt.revision > before.revision());
    assert_eq!(rebuilt.live_records, 2);
    assert_eq!(
        index
            .view()
            .unwrap()
            .get_record("entries", "owned-1", ReturnMode::Logical)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::Logical(Value::String("kept".into())))
    );
    *reader.0.lock().unwrap() = (RecordVersion::new(5), Value::String("changed".into()));
    assert!(index.rebuild().is_err());
    assert_eq!(index.view().unwrap().revision(), rebuilt.revision);
    assert_eq!(
        before
            .get_record("entries", "owned-1", ReturnMode::Logical)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::Logical(Value::String("kept".into())))
    );
    session.finish().unwrap();
}

#[test]
fn fileless_residual_text_sort_and_stream_agree() {
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
    let registration = builder.register_records(collection, "residual", 1).unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    let mut batch = writer.batch();
    batch.insert("alpha", record(1808, "abc Leipzig")).unwrap();
    batch.insert("beta", record(1808, "abXbc Leipzig")).unwrap();
    batch.insert("gamma", record(1809, "Wien")).unwrap();
    writer.submit(batch).unwrap().wait_published().unwrap();
    let view = index.view().unwrap();
    let base = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("year = 1808 && note ~ 'Leipzig'").unwrap()),
        options: QueryOptions {
            sort: vec![SortField::simple("year", false)],
            offset: 0,
            limit: 10,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    let result = view.search(&base).unwrap();
    assert_eq!(result.count, Some(2));
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["alpha", "beta"]
    );
    let text = Query {
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "abc".into(),
            pattern_parameter: None,
            fields: vec![FtsField {
                path: objekte_search_core::model::FieldPath::parse("note").unwrap(),
                binding: None,
                query_path: None,
            }],
        }),
        filter: None,
        ..base
    };
    let result = view.search(&text).unwrap();
    assert_eq!(result.count, Some(1));
    assert_eq!(result.hits[0].record_id, "alpha");
    assert!(
        result.hits[0]
            .matched_fields
            .iter()
            .any(|field| field.to_string().contains("note"))
    );
    let mut streamed = Vec::new();
    view.search_stream_scoped(
        &text,
        ResultScope::All { offset: 0 },
        StreamOptions::default(),
        &CancellationToken::new(),
        &mut |event| {
            if let StreamEvent::OrderedRows { rows, .. } = event {
                streamed.extend(rows.iter().map(|hit| hit.record_id.clone()));
            }
            Ok(StreamControl::Continue)
        },
    )
    .unwrap();
    assert_eq!(streamed, ["alpha"]);
    session.finish().unwrap();
}

struct CorpusReader(Mutex<HashMap<String, Value>>, Mutex<Vec<usize>>);

impl RecordReader for CorpusReader {
    fn read_projected(
        &self,
        key: &RecordKey,
        expected_version: RecordVersion,
        _: &RecordProjection,
    ) -> Result<RecordRead, RecordReadError> {
        if expected_version != RecordVersion::new(1) {
            return Err(RecordReadError::ChangedVersion {
                expected: expected_version,
                actual: Some(RecordVersion::new(1)),
            });
        }
        let value = self
            .0
            .lock()
            .unwrap()
            .get(key.record.0.as_ref())
            .cloned()
            .ok_or_else(|| RecordReadError::Unavailable("record missing".into()))?;
        Ok(RecordRead {
            version: expected_version,
            value,
            text: Vec::new(),
            original_bytes: None,
        })
    }

    fn read_batch(
        &self,
        requests: &[RecordReadRequest],
        projection: &RecordProjection,
    ) -> Vec<Result<RecordRead, RecordReadError>> {
        self.1.lock().unwrap().push(requests.len());
        requests
            .iter()
            .map(|request| self.read_projected(&request.key, request.expected_version, projection))
            .collect()
    }
}

#[test]
fn logical_corpus_agrees_across_json_retained_and_provider() {
    let root = std::env::temp_dir().join(format!(
        "objekte-cross-backing-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(root.join("entries")).unwrap();
    let documents = [
        serde_json::json!({"id":"r1","details":{"city":"Leipzig"},"tags":["alpha","beta"],"issued":"2024-02-29","amount":1.25,"optional":null}),
        serde_json::json!({"id":"r2","details":{"city":"Wien"},"tags":["beta"],"issued":"2024-03-01","amount":2.5}),
    ];
    for (number, document) in documents.iter().enumerate() {
        std::fs::write(
            root.join("entries").join(format!("{number}.json")),
            serde_json::to_vec(document).unwrap(),
        )
        .unwrap();
    }
    let mut schema = Schema::default();
    for (path, kind) in [
        (vec!["id"], LogicalType::Id),
        (vec!["details", "city"], LogicalType::String),
        (vec!["issued"], LogicalType::Date),
        (vec!["amount"], LogicalType::Number),
    ] {
        schema
            .register_field(FieldDefinition {
                path: path.into_iter().map(str::to_owned).collect(),
                kind,
                required: false,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    let mut file_definition =
        CollectionDefinition::folder("entries", "entries", SourceFormat::Json);
    file_definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    file_definition.schema = schema.clone();
    let files = Index::start_build(
        &root,
        vec![file_definition],
        RepositoryOptions::default(),
        SourceRegistry::builtin(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();

    let mut owned_builder = Index::builder();
    let collection = owned_builder.collection("entries", schema.clone()).unwrap();
    let registration = owned_builder
        .register_records(collection, "owned", 1)
        .unwrap();
    let owned = owned_builder.build().unwrap();
    let owned_session = owned.ingestion();
    let writer = owned_session.writer(registration).unwrap();
    let mut batch = writer.batch();
    for (number, document) in documents.iter().enumerate() {
        batch
            .insert(
                format!("r{}", number + 1),
                Value::from_json(document.clone()).unwrap(),
            )
            .unwrap();
    }
    writer.submit(batch).unwrap().wait_published().unwrap();

    let values = ["r1", "r2"]
        .into_iter()
        .map(|id| {
            let hit = owned
                .view()
                .unwrap()
                .get_record("entries", id, ReturnMode::Logical)
                .unwrap()
                .unwrap();
            let Some(Payload::Logical(value)) = hit.payload else {
                panic!("logical payload missing")
            };
            (id.to_owned(), value)
        })
        .collect();
    let reader = Arc::new(CorpusReader(Mutex::new(values), Mutex::new(Vec::new())));
    let mut provider_builder = Index::builder();
    let collection = provider_builder.collection("entries", schema).unwrap();
    let registration = provider_builder
        .register_records(collection, "reader", 1)
        .unwrap();
    provider_builder
        .register_reader(registration, reader.clone())
        .unwrap();
    let provider = provider_builder.build().unwrap();
    let provider_session = provider.ingestion();
    let writer = provider_session.writer(registration).unwrap();
    let mut batch = writer.batch();
    for (number, document) in documents.iter().enumerate() {
        batch
            .insert_from_reader(
                format!("r{}", number + 1),
                Value::from_json(document.clone()).unwrap(),
                RecordVersion::new(1),
            )
            .unwrap();
    }
    writer.submit(batch).unwrap().wait_published().unwrap();

    let query = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("amount >= 1.25 && details.city ~ 'i'").unwrap()),
        options: QueryOptions {
            sort: vec![SortField::simple("amount", false)],
            offset: 0,
            limit: 10,
            count: true,
            return_mode: ReturnMode::Logical,
            evidence: true,
        },
    };
    let answers = [&files, &owned, &provider]
        .into_iter()
        .enumerate()
        .map(|(index_number, index)| {
            let view = index.view().unwrap();
            let result = view.search(&query).unwrap();
            let reference = view
                .search_reference(&query)
                .unwrap_or_else(|error| panic!("index {index_number}: {error}"));
            assert_eq!(result.count, reference.count);
            assert_eq!(result.hits, reference.hits);
            assert_eq!(result.count, Some(2));
            result
                .hits
                .into_iter()
                .map(|hit| (hit.record_id, hit.payload, hit.matched_fields))
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    assert_eq!(answers[0], answers[1]);
    assert_eq!(answers[1], answers[2]);
    assert!(reader.1.lock().unwrap().iter().any(|&size| size >= 2));
    assert_eq!(
        answers[0]
            .iter()
            .map(|row| row.0.as_str())
            .collect::<Vec<_>>(),
        ["r1", "r2"]
    );
    for (filter, expected) in [
        ("optional:null = true", "r1"),
        ("optional:exists = false", "r2"),
        ("tags[0] = 'alpha'", "r1"),
        ("issued = 2024-02-29", "r1"),
        ("amount = 2.5", "r2"),
    ] {
        let selected = Query {
            filter: Some(parse_filter(filter).unwrap()),
            ..query.clone()
        };
        for index in [&files, &owned, &provider] {
            let result = index.view().unwrap().search(&selected).unwrap();
            assert_eq!(result.count, Some(1), "{filter}");
            assert_eq!(result.hits[0].record_id, expected, "{filter}");
        }
    }
    let scoped_text = Query {
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "Leipzig".into(),
            pattern_parameter: None,
            fields: vec![FtsField {
                path: objekte_search_core::model::FieldPath::parse("details.city").unwrap(),
                binding: None,
                query_path: None,
            }],
        }),
        filter: None,
        ..query.clone()
    };
    for index in [&files, &owned, &provider] {
        let view = index.view().unwrap();
        let result = view.search(&scoped_text).unwrap();
        assert_eq!(
            result.hits,
            view.search_reference(&scoped_text).unwrap().hits
        );
        assert_eq!(result.count, Some(1));
        assert_eq!(result.hits[0].record_id, "r1");
    }
    for id in ["r1", "r2"] {
        assert!(
            files
                .view()
                .unwrap()
                .get_record("entries", id, ReturnMode::Locator)
                .unwrap()
                .unwrap()
                .file
                .is_some()
        );
        assert!(
            owned
                .view()
                .unwrap()
                .get_record("entries", id, ReturnMode::Locator)
                .unwrap()
                .unwrap()
                .file
                .is_none()
        );
        assert!(
            provider
                .view()
                .unwrap()
                .get_record("entries", id, ReturnMode::Locator)
                .unwrap()
                .unwrap()
                .file
                .is_none()
        );
    }
    owned_session.finish().unwrap();
    provider_session.finish().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn fileless_relations_resolve_target_fields_and_backlinks() {
    let mut entries_schema = Schema::default();
    let mut mentions_schema = Schema::default();
    let add = |schema: &mut Schema, name: &str, kind| {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    };
    add(&mut entries_schema, "id", LogicalType::Id);
    add(&mut entries_schema, "title", LogicalType::String);
    add(&mut mentions_schema, "id", LogicalType::Id);
    add(&mut mentions_schema, "entry", LogicalType::Id);
    mentions_schema
        .register_relation(RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "entries".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: Some("mentions".into()),
        })
        .unwrap();
    let mut builder = Index::builder();
    let entries = builder.collection("entries", entries_schema).unwrap();
    let mentions = builder.collection("mentions", mentions_schema).unwrap();
    let entries_registration = builder.register_records(entries, "entries", 1).unwrap();
    let mentions_registration = builder.register_records(mentions, "mentions", 1).unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let entries_writer = session.writer(entries_registration).unwrap();
    let mut batch = entries_writer.batch();
    batch
        .insert(
            "a",
            Value::from_json(serde_json::json!({"id":"a","title":"Leipzig"})).unwrap(),
        )
        .unwrap();
    batch
        .insert(
            "b",
            Value::from_json(serde_json::json!({"id":"b","title":"Wien"})).unwrap(),
        )
        .unwrap();
    entries_writer
        .submit(batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let mentions_writer = session.writer(mentions_registration).unwrap();
    let mut batch = mentions_writer.batch();
    batch
        .insert(
            "m",
            Value::from_json(serde_json::json!({"id":"m","entry":"a"})).unwrap(),
        )
        .unwrap();
    batch
        .insert(
            "n",
            Value::from_json(serde_json::json!({"id":"n","entry":"b"})).unwrap(),
        )
        .unwrap();
    mentions_writer
        .submit(batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let query = Query {
        collections: vec!["mentions".into()],
        fts: None,
        filter: Some(parse_filter("entry.title = 'Leipzig'").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            offset: 0,
            limit: 10,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let view = index.view().unwrap();
    let result = view.search(&query).unwrap();
    assert_eq!(result.hits, view.search_reference(&query).unwrap().hits);
    assert_eq!(result.count, Some(1));
    assert_eq!(result.hits[0].record_id, "m");
    let logical = Query {
        options: QueryOptions {
            return_mode: ReturnMode::Logical,
            ..query.options.clone()
        },
        ..query
    };
    let result = index.view().unwrap().search(&logical).unwrap();
    assert_eq!(result.count, Some(1));
    assert!(matches!(result.hits[0].payload, Some(Payload::Logical(_))));
    let residual = Query {
        filter: Some(parse_filter("entry.title ~ 'Leipzig'").unwrap()),
        ..logical
    };
    assert_eq!(
        index.view().unwrap().search(&residual).unwrap().count,
        Some(1)
    );
    let backlink = Query {
        collections: vec!["entries".into()],
        filter: Some(parse_filter("@backlinks.mentions.id ?= 'm'").unwrap()),
        ..residual
    };
    let result = index.view().unwrap().search(&backlink).unwrap();
    assert_eq!(
        result.hits,
        index
            .view()
            .unwrap()
            .search_reference(&backlink)
            .unwrap()
            .hits
    );
    assert_eq!(result.count, Some(1));
    assert_eq!(result.hits[0].record_id, "a");
    for (expression, expected) in [
        ("entry.title = 'Leipzig' || id = 'n'", vec!["m", "n"]),
        ("!(entry.title = 'Wien')", vec!["m"]),
    ] {
        let boolean = Query {
            collections: vec!["mentions".into()],
            filter: Some(parse_filter(expression).unwrap()),
            ..backlink.clone()
        };
        let result = index.view().unwrap().search(&boolean).unwrap();
        assert_eq!(result.count, Some(expected.len()), "{expression}");
        assert_eq!(
            result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect::<Vec<_>>(),
            expected,
            "{expression}"
        );
    }
    let related_fts = Query {
        collections: vec!["mentions".into()],
        filter: Some(parse_filter("id = 'm'").unwrap()),
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "Leipzig".into(),
            pattern_parameter: None,
            fields: vec![FtsField {
                path: objekte_search_core::model::FieldPath::parse("entry.title").unwrap(),
                binding: None,
                query_path: None,
            }],
        }),
        ..backlink
    };
    let result = index.view().unwrap().search(&related_fts).unwrap();
    assert_eq!(result.count, Some(1));
    assert_eq!(result.hits[0].record_id, "m");
    session.finish().unwrap();
}

#[test]
fn fileless_two_hop_relation_joins_indexed_targets() {
    let mut places_schema = Schema::default();
    let mut entries_schema = Schema::default();
    let mut mentions_schema = Schema::default();
    for (schema, name, kind) in [
        (&mut places_schema, "name", LogicalType::String),
        (&mut entries_schema, "place", LogicalType::Id),
        (&mut mentions_schema, "entry", LogicalType::Id),
    ] {
        schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    mentions_schema
        .register_field(FieldDefinition {
            path: vec!["note".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    mentions_schema
        .register_field(FieldDefinition {
            path: vec!["expected_city".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    entries_schema
        .register_relation(RelationDefinition {
            path: vec!["place".into()],
            target_collection: "places".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    mentions_schema
        .register_relation(RelationDefinition {
            path: vec!["entry".into()],
            target_collection: "entries".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    let mut options = RepositoryOptions::default();
    options.segment_policy =
        objekte_search_core::index::segment::SegmentPolicy::with_target_records(
            std::num::NonZeroUsize::new(1).unwrap(),
        )
        .unwrap();
    let mut builder = Index::builder().with_options(options);
    let places = builder.collection("places", places_schema).unwrap();
    let entries = builder.collection("entries", entries_schema).unwrap();
    let mentions = builder.collection("mentions", mentions_schema).unwrap();
    let places_registration = builder.register_records(places, "places", 1).unwrap();
    let entries_registration = builder.register_records(entries, "entries", 1).unwrap();
    let mentions_registration = builder.register_records(mentions, "mentions", 1).unwrap();
    let reader = Arc::new(CorpusReader(
        Mutex::new(HashMap::from([
            (
                "p".to_string(),
                Value::from_json(serde_json::json!({"name":"Leipzig"})).unwrap(),
            ),
            (
                "q".to_string(),
                Value::from_json(serde_json::json!({"name":"Wien"})).unwrap(),
            ),
        ])),
        Mutex::new(Vec::new()),
    ));
    builder
        .register_reader(places_registration, reader)
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    for (registration, id, value) in [
        (
            places_registration,
            "p",
            serde_json::json!({"name":"Leipzig"}),
        ),
        (places_registration, "q", serde_json::json!({"name":"Wien"})),
        (entries_registration, "e", serde_json::json!({"place":"p"})),
        (entries_registration, "f", serde_json::json!({"place":"q"})),
        (
            mentions_registration,
            "m",
            serde_json::json!({"entry":"e","note":"plain","expected_city":"Leipzig"}),
        ),
        (
            mentions_registration,
            "n",
            serde_json::json!({"entry":"f","note":"Leipzig","expected_city":"Berlin"}),
        ),
    ] {
        let writer = session.writer(registration).unwrap();
        let mut batch = writer.batch();
        if registration == places_registration {
            batch
                .insert_from_reader(id, Value::from_json(value).unwrap(), RecordVersion::new(1))
                .unwrap();
        } else {
            batch.insert(id, Value::from_json(value).unwrap()).unwrap();
        }
        writer.submit(batch).unwrap().wait_published().unwrap();
    }
    let query = Query {
        collections: vec!["mentions".into()],
        fts: None,
        filter: Some(parse_filter("entry.place.name = 'Leipzig'").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            offset: 0,
            limit: 10,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    let result = index.view().unwrap().search(&query).unwrap();
    assert_eq!(result.count, Some(1));
    assert_eq!(result.hits[0].record_id, "m");
    assert_eq!(index.view().unwrap().segment_summaries().len(), 6);
    let related_sort = Query {
        filter: None,
        options: QueryOptions {
            sort: vec![SortField::simple("entry.place.name", false)],
            ..query.options.clone()
        },
        ..query.clone()
    };
    let sorted = index.view().unwrap().search(&related_sort).unwrap();
    let sorted_reference = index
        .view()
        .unwrap()
        .search_reference(&related_sort)
        .unwrap();
    assert_eq!(sorted.count, Some(2));
    assert_eq!(sorted.hits, sorted_reference.hits);
    assert_eq!(
        sorted
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m", "n"]
    );
    for expression in [
        "entry.place.name ~ 'Leipzig'",
        "!(entry.place.name = 'Wien')",
        "entry.place.name = expected_city",
    ] {
        let residual = Query {
            filter: Some(parse_filter(expression).unwrap()),
            ..query.clone()
        };
        let result = index.view().unwrap().search(&residual).unwrap();
        let reference = index.view().unwrap().search_reference(&residual).unwrap();
        assert_eq!(result.count, reference.count, "{expression}");
        assert_eq!(result.hits, reference.hits, "{expression}");
        assert_eq!(result.count, Some(1), "{expression}");
        assert_eq!(result.hits[0].record_id, "m", "{expression}");
    }
    let related_text = Query {
        filter: None,
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "Leipzig".into(),
            pattern_parameter: None,
            fields: vec![
                FtsField {
                    path: objekte_search_core::model::FieldPath::parse("entry.place.name").unwrap(),
                    binding: None,
                    query_path: None,
                },
                FtsField {
                    path: objekte_search_core::model::FieldPath::parse("note").unwrap(),
                    binding: None,
                    query_path: None,
                },
            ],
        }),
        ..query
    };
    let result = index.view().unwrap().search(&related_text).unwrap();
    let reference = index
        .view()
        .unwrap()
        .search_reference(&related_text)
        .unwrap();
    assert_eq!(result.count, reference.count);
    assert_eq!(result.hits, reference.hits);
    assert_eq!(result.count, Some(2));
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["m", "n"]
    );
    session.finish().unwrap();
}

#[test]
fn public_builder_mixes_file_ranges_and_typed_records_in_one_collection() {
    let root = std::env::temp_dir().join(format!(
        "objekte-mixed-source-builder-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/file.json");
    std::fs::write(&path, br#"{"id":"file","note":"file","place":"p"}"#).unwrap();
    let mut entries_schema = Schema::default();
    for (field, kind) in [
        ("id", LogicalType::Id),
        ("note", LogicalType::String),
        ("place", LogicalType::Id),
    ] {
        entries_schema
            .register_field(FieldDefinition {
                path: vec![field.into()],
                kind,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
    }
    entries_schema
        .register_relation(RelationDefinition {
            path: vec!["place".into()],
            target_collection: "places".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    let mut places_schema = Schema::default();
    places_schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let mut builder = Index::builder();
    let entries = builder.collection("entries", entries_schema).unwrap();
    let places = builder.collection("places", places_schema).unwrap();
    let entries_registration = builder
        .register_records(entries, "typed.entries", 1)
        .unwrap();
    let places_registration = builder.register_records(places, "typed.places", 1).unwrap();
    builder
        .register_file_catalog(FileCatalog::new(&root).unwrap())
        .unwrap();
    let mut importer = CollectionDefinition::folder("entries", "data", SourceFormat::Json);
    importer.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    builder
        .register_file_source(entries, importer.file_source())
        .unwrap();
    let index = builder.build().unwrap();
    index.add_file(&path).unwrap();
    let session = index.ingestion();
    let place_writer = session.writer(places_registration).unwrap();
    let mut place_batch = place_writer.batch();
    place_batch
        .insert(
            "p",
            Value::from_json(serde_json::json!({"name":"Leipzig"})).unwrap(),
        )
        .unwrap();
    place_writer
        .submit(place_batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let entry_writer = session.writer(entries_registration).unwrap();
    let mut entry_batch = entry_writer.batch();
    entry_batch
        .insert(
            "native",
            Value::from_json(serde_json::json!({"id":"native","note":"native","place":"p"}))
                .unwrap(),
        )
        .unwrap();
    entry_writer
        .submit(entry_batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let view = index.view().unwrap();
    assert!(
        view.get_record("entries", "file", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .is_some()
    );
    assert!(
        view.get_record("entries", "native", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .is_none()
    );
    let query = Query {
        collections: vec!["entries".into()],
        filter: Some(parse_filter("place.name = 'Leipzig'").unwrap()),
        fts: None,
        options: QueryOptions {
            sort: vec![SortField::simple("note", false)],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let result = view.search(&query).unwrap();
    assert_eq!(result.count, Some(2));
    assert_eq!(result.hits, view.search_reference(&query).unwrap().hits);
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["file", "native"]
    );
    for (literal, expected) in [("file", "file"), ("native", "native")] {
        let text_query = Query {
            filter: None,
            fts: Some(FtsQuery {
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: FtsMode::Literal,
                literal: literal.into(),
                pattern_parameter: None,
                fields: vec![FtsField {
                    path: objekte_search_core::model::FieldPath::parse("note").unwrap(),
                    binding: None,
                    query_path: None,
                }],
            }),
            ..query.clone()
        };
        let found = view.search(&text_query).unwrap();
        assert_eq!(found.count, Some(1));
        assert_eq!(found.hits[0].record_id, expected);
        assert_eq!(found.hits, view.search_reference(&text_query).unwrap().hits);
    }
    session.finish().unwrap();
    index.rebuild().unwrap();
    let rebuilt = index.view().unwrap().search(&query).unwrap();
    assert_eq!(rebuilt.count, Some(2));
    assert_eq!(rebuilt.hits[0].record_id, "file");
    assert_eq!(rebuilt.hits[1].record_id, "native");
    std::fs::write(&path, br#"{"id":"file","note":"changed","place":"p"}"#).unwrap();
    assert!(index.rebuild().is_err());
    std::fs::remove_dir_all(root).unwrap();
}
