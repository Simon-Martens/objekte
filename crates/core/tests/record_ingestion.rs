use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Barrier, Mutex};

use objekte_search_core::files::FileCatalog;
use objekte_search_core::ingestion::{IngestionOptions, RecordInput, SubmitError};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{LogicalType, Number, Value};
use objekte_search_core::query::clock::ClockContext;
use objekte_search_core::query::options::{ReturnMode, SortField};
use objekte_search_core::query::{
    CaseSensitivity, FtsField, FtsMode, FtsQuery, Query, QueryOptions, parse_filter,
};
use objekte_search_core::query_execution::streaming::{
    CancellationToken, ExecutionContext, ExecutionErrorKind, ExecutionLimits, ResultScope,
    StreamControl, StreamEvent, StreamOptions, StreamOutcome,
};
use objekte_search_core::query_results::Payload;
use objekte_search_core::records::reader::{
    RecordProjection, RecordRead, RecordReadError, RecordReadRequest, RecordReader,
};
use objekte_search_core::records::{RecordKey, RecordVersion};
use objekte_search_core::repository::configuration::{
    CollectionDeclaration, CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::schema::relations::{RelationCardinality, RelationDefinition};
use objekte_search_core::schema::{FieldDefinition, Schema};
use objekte_search_core::source::{ExtractedRecord, SourceReader, TextSegment};
use objekte_search_core::source::{SourceFormat, registry::SourceRegistry};

struct PausingLinesReader {
    pause_next: AtomicBool,
    parses: AtomicUsize,
    entered: Arc<Barrier>,
    release: Arc<Barrier>,
}

impl SourceReader for PausingLinesReader {
    fn name(&self) -> &'static str {
        "pausing-lines"
    }

    fn logical_text_coverage(&self) -> objekte_search_core::source::LogicalTextCoverage {
        objekte_search_core::source::LogicalTextCoverage::AllStringFields
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<objekte_search_core::inventory::ByteRange>, String> {
        let mut ranges = Vec::new();
        let mut start = 0usize;
        for (index, byte) in file.iter().enumerate() {
            if *byte == b'\n' {
                if start < index {
                    ranges.push(objekte_search_core::inventory::ByteRange::checked(
                        start as u64,
                        index as u64,
                        file.len() as u64,
                    )?);
                }
                start = index + 1;
            }
        }
        Ok(ranges)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        self.parses.fetch_add(1, Ordering::SeqCst);
        if self.pause_next.swap(false, Ordering::SeqCst) {
            self.entered.wait();
            self.release.wait();
        }
        let json = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        Ok(ExtractedRecord {
            value: Value::from_json(json)?,
            text: Vec::new(),
        })
    }
}

fn record(year: i128, note: &str) -> Value {
    Value::Object(BTreeMap::from([
        ("year".into(), Value::Number(Number::integer(year))),
        ("note".into(), Value::String(note.into())),
    ]))
}

fn no_source_reads() -> ExecutionContext {
    ExecutionContext::new(
        ClockContext::new(time::OffsetDateTime::now_utc()),
        ExecutionLimits {
            relation_reads: 0,
            relation_bytes: 0,
            source_record_bytes: 0,
            ..ExecutionLimits::default()
        },
        None,
    )
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
    assert_eq!(
        batch
            .insert_from_reader("invalid", record(1807, "invalid"), RecordVersion::new(1))
            .unwrap_err(),
        "provider input needs a registered reader"
    );
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
    let logical_query = Query {
        options: QueryOptions {
            return_mode: ReturnMode::Logical,
            ..query.options.clone()
        },
        ..query.clone()
    };
    let logical = view
        .search_with_context(&logical_query, &no_source_reads())
        .unwrap();
    assert_eq!(
        logical.hits[0].payload,
        Some(Payload::Logical(record(1808, "Leipzig")))
    );
    assert_eq!(logical.counters.selected_payload_reads, 0);
    assert_eq!(logical.counters.files_opened, 0);
    assert_eq!(logical.counters.record_parses, 0);
    index.configure_verified_cache(1024 * 1024).unwrap();
    let cold = view.search(&logical_query).unwrap();
    let warm = view.search(&logical_query).unwrap();
    assert!(!cold.counters.verified_result_hit);
    assert!(warm.counters.verified_result_hit);
    assert_eq!(warm.hits, cold.hits);

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
fn empty_strings_with_reserved_capacity_count_toward_batch_limit() {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder
        .register_records(collection, "test.entries", 1)
        .unwrap();
    let index = builder.build().unwrap();
    let writer = index
        .ingestion_with_options(IngestionOptions {
            max_batch_bytes: 1024,
            ..IngestionOptions::default()
        })
        .unwrap()
        .writer(registration)
        .unwrap();
    let mut value = String::with_capacity(8 * 1024);
    value.push_str("");
    let mut batch = writer.batch();
    batch.insert("large-empty", Value::String(value)).unwrap();
    let batch = match writer.submit(batch) {
        Err(SubmitError::BatchTooLarge(batch)) => batch,
        _ => panic!("reserved capacity must be included in the batch limit"),
    };
    assert_eq!(batch.len(), 1);
    let mut valid = writer.batch();
    valid.insert("small", Value::String("ok".into())).unwrap();
    writer.submit(valid).unwrap().wait_published().unwrap();
}

#[test]
fn importer_only_move_outside_selection_retires_empty_catalog_membership() {
    let root = std::env::temp_dir().join(format!(
        "objekte-importer-only-transition-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    std::fs::create_dir_all(root.join("attachments")).unwrap();
    let inside = root.join("data/one.json");
    let outside = root.join("attachments/one.json");
    std::fs::write(&inside, br#"{"title":"imported"}"#).unwrap();
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    builder
        .register_file_catalog(FileCatalog::new(&root).unwrap())
        .unwrap();
    let importer = CollectionDefinition::folder("entries", "data", SourceFormat::Json);
    builder
        .register_file_source(collection, importer.file_source())
        .unwrap();
    let index = builder.build().unwrap();
    let added = index.add_file(&inside).unwrap();
    let pinned = index.view().unwrap();
    assert_eq!(index.status().unwrap().live_records, 1);
    std::fs::rename(&inside, &outside).unwrap();
    let moved = index.move_file(added.file, &outside).unwrap();
    assert_eq!(moved.retired_records, 1);
    assert_eq!(moved.indexed_records, 0);
    assert_eq!(index.status().unwrap().live_records, 0);
    let current = index.view().unwrap();
    assert!(current.file_handle_for_path(&outside).is_none());
    assert!(
        current
            .file_catalog()
            .unwrap()
            .file_by_path(std::path::Path::new("attachments/one.json"))
            .is_none()
    );
    assert_eq!(
        current
            .file_catalog()
            .unwrap()
            .records_for_file(added.file.file_id),
        Err(objekte_search_core::files::LookupError::FileNotFound)
    );
    assert!(
        pinned
            .file_catalog()
            .unwrap()
            .file(added.file.file_id)
            .is_ok()
    );
    index.rebuild().unwrap();
    assert_eq!(index.status().unwrap().live_records, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn rootless_file_operations_and_reconciliation_return_without_panicking() {
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder.register_records(collection, "rootless", 1).unwrap();
    let index = builder.build().unwrap();
    let writer = index.ingestion().writer(registration).unwrap();
    let mut batch = writer.batch();
    batch
        .insert("direct", Value::String("kept".into()))
        .unwrap();
    writer.submit(batch).unwrap().wait_published().unwrap();
    assert!(
        index
            .add_file(std::path::Path::new("missing.json"))
            .is_err()
    );
    assert!(
        index
            .view()
            .unwrap()
            .file_handle_for_path(std::path::Path::new("/tmp/missing.json"))
            .is_none()
    );
    let report = index.start_reconcile().unwrap().wait().unwrap();
    assert!(report.issues.is_empty());
    assert_eq!(report.examined, 0);
    assert!(
        index
            .view()
            .unwrap()
            .get_record("entries", "direct", ReturnMode::Logical)
            .unwrap()
            .is_some()
    );

    let root = std::env::temp_dir().join(format!(
        "objekte-empty-catalog-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let mut builder = Index::builder();
    builder.collection("entries", Schema::default()).unwrap();
    builder
        .register_file_catalog(FileCatalog::new(&root).unwrap())
        .unwrap();
    let empty_catalog = builder.build().unwrap();
    assert!(empty_catalog.view().unwrap().file_catalog().is_some());
    let report = empty_catalog.start_reconcile().unwrap().wait().unwrap();
    assert!(report.issues.is_empty());
    assert_eq!(report.examined, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_native_text_does_not_claim_logical_field_coverage() {
    let root = std::env::temp_dir().join(format!(
        "objekte-text-coverage-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder
        .register_records(collection, "native-text", 1)
        .unwrap();
    builder
        .register_file_catalog(FileCatalog::new(&root).unwrap())
        .unwrap();
    builder
        .register_file_source(
            collection,
            CollectionDefinition::folder("entries", "data", SourceFormat::Json).file_source(),
        )
        .unwrap();
    let index = builder.build().unwrap();
    let writer = index.ingestion().writer(registration).unwrap();
    let mut batch = writer.batch();
    batch
        .insert_with_text(
            "target",
            Value::from_json(serde_json::json!({"note":"Leipzig"})).unwrap(),
            vec![TextSegment {
                path: vec!["note".into()],
                text: "Dresden".into(),
            }],
        )
        .unwrap();
    for number in 0..300 {
        batch
            .insert(
                format!("decoy-{number}"),
                Value::from_json(serde_json::json!({"note":"Hamburg"})).unwrap(),
            )
            .unwrap();
    }
    writer.submit(batch).unwrap().wait_published().unwrap();
    let query = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("note ~ 'Leipzig'").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let result = index.view().unwrap().search(&query).unwrap();
    assert_eq!(result.count, Some(1));
    assert_eq!(result.hits[0].record_id, "target");
    let fts = Query {
        filter: None,
        fts: Some(FtsQuery {
            case_sensitivity: CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "Dresden".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        ..query
    };
    let result = index.view().unwrap().search(&fts).unwrap();
    assert_eq!(result.count, Some(1));
    assert_eq!(result.hits[0].record_id, "target");
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
fn relation_cache_rechecks_provider_target_outside_result_members() {
    let mut places_schema = Schema::default();
    places_schema
        .register_field(FieldDefinition {
            path: vec!["name".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: false,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let mut entries_schema = Schema::default();
    entries_schema
        .register_field(FieldDefinition {
            path: vec!["place".into()],
            kind: LogicalType::Id,
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
    let mut builder = Index::builder();
    let places = builder.collection("places", places_schema).unwrap();
    let entries = builder.collection("entries", entries_schema).unwrap();
    let place_registration = builder.register_records(places, "places", 1).unwrap();
    let entry_registration = builder.register_records(entries, "entries", 1).unwrap();
    let provider = Arc::new(VersionedFixture(
        Mutex::new((
            RecordVersion::new(1),
            Value::from_json(serde_json::json!({"name":"Leipzig"})).unwrap(),
        )),
        Mutex::new(Vec::new()),
    ));
    builder
        .register_reader(place_registration, provider.clone())
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let place_writer = session.writer(place_registration).unwrap();
    let mut place_batch = place_writer.batch();
    place_batch
        .insert_from_reader(
            "p",
            Value::from_json(serde_json::json!({"name":"Leipzig"})).unwrap(),
            RecordVersion::new(1),
        )
        .unwrap();
    place_writer
        .submit(place_batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let entry_writer = session.writer(entry_registration).unwrap();
    let mut entry_batch = entry_writer.batch();
    entry_batch
        .insert(
            "e",
            Value::from_json(serde_json::json!({"place":"p"})).unwrap(),
        )
        .unwrap();
    entry_writer
        .submit(entry_batch)
        .unwrap()
        .wait_published()
        .unwrap();
    index.configure_verified_cache(1 << 20).unwrap();
    let query = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("place.name = 'Leipzig'").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            offset: 0,
            limit: 10,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let view = index.view().unwrap();
    assert_eq!(view.search(&query).unwrap().count, Some(1));
    assert!(!view.search(&query).unwrap().counters.verified_result_hit);
    *provider.0.lock().unwrap() = (
        RecordVersion::new(2),
        Value::from_json(serde_json::json!({"name":"Wien"})).unwrap(),
    );
    assert!(view.search(&query).is_err());
    session.finish().unwrap();
}

#[test]
fn versioned_provider_disposes_input_and_checks_rereads() {
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
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    let mut batch = writer.batch();
    batch
        .insert_input(
            "entry-47",
            RecordInput::new(record(1808, "Leipzig"))
                .with_text(vec![TextSegment {
                    path: vec!["note".into()],
                    text: "Leipzig".into(),
                }])
                .from_reader(RecordVersion::new(7)),
        )
        .unwrap();
    batch
        .insert_from_reader("retired", record(1808, "Leipzig"), RecordVersion::new(7))
        .unwrap();
    writer.submit(batch).unwrap().wait_published().unwrap();
    let mut replacement = writer.batch();
    replacement
        .replace_input(
            "entry-47",
            RecordInput::new(record(1808, "Leipzig"))
                .with_text(vec![TextSegment {
                    path: vec!["note".into()],
                    text: "Leipzig".into(),
                }])
                .from_reader(RecordVersion::new(7)),
        )
        .unwrap();
    writer
        .submit(replacement)
        .unwrap()
        .wait_published()
        .unwrap();
    let pinned = index.view().unwrap();
    let mut deletion = writer.batch();
    deletion.delete("retired").unwrap();
    writer.submit(deletion).unwrap().wait_published().unwrap();
    index.compact().unwrap();
    let view = index.view().unwrap();
    let provider_query = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("year = 1808").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            limit: 1,
            offset: 0,
            count: false,
            return_mode: ReturnMode::Logical,
            evidence: false,
        },
    };
    reader.1.lock().unwrap().clear();
    let denied = view
        .search_with_context(&provider_query, &no_source_reads())
        .unwrap_err();
    assert_eq!(denied.kind, ExecutionErrorKind::Resource);
    assert!(reader.1.lock().unwrap().is_empty());
    let provided = view.search(&provider_query).unwrap();
    assert_eq!(provided.hits[0].record_id, "entry-47");
    assert_eq!(provided.counters.selected_payload_reads, 1);
    assert_eq!(provided.counters.files_opened, 0);
    assert_eq!(provided.counters.record_parses, 0);
    assert_eq!(reader.1.lock().unwrap().len(), 1);
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
    assert!(
        view.get_record("entries", "entry-47", ReturnMode::Locator)
            .unwrap()
            .unwrap()
            .file
            .is_none()
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
    index.configure_verified_cache(1024 * 1024).unwrap();
    reader.1.lock().unwrap().clear();
    let cold = view.search(&query).unwrap();
    let cold_reader_calls = reader.1.lock().unwrap().len();
    reader.1.lock().unwrap().clear();
    let warm = view.search(&query).unwrap();
    assert!(!cold.counters.verified_result_hit);
    assert!(warm.counters.verified_result_hit);
    assert_eq!(warm.hits, cold.hits);
    assert!(cold_reader_calls >= 1);
    assert_eq!(reader.1.lock().unwrap().len(), 1);
    index.configure_verified_cache(0).unwrap();
    assert_eq!(view.search(&query).unwrap().hits, warm.hits);
    index.configure_verified_cache(1024 * 1024).unwrap();
    assert!(!view.search(&query).unwrap().counters.verified_result_hit);
    assert!(view.search(&query).unwrap().counters.verified_result_hit);
    let by_file = Query {
        filter: Some(parse_filter("@file.path = 'source.txt'").unwrap()),
        options: QueryOptions {
            return_mode: ReturnMode::Locator,
            ..query.options.clone()
        },
        ..query.clone()
    };
    assert_eq!(view.search(&by_file).unwrap().count, Some(0));
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
    session.finish().unwrap();
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
            .search_stream_scoped_with_context(
                &query,
                scope,
                StreamOptions::default(),
                &no_source_reads(),
                &mut |event| {
                    if let StreamEvent::OrderedRows { rows, .. } = event {
                        ids.extend(rows.iter().map(|hit| hit.record_id.clone()));
                    }
                    Ok(StreamControl::Continue)
                },
            )
            .unwrap();
        assert_eq!(ids.len(), expected);
        let StreamOutcome::Complete(summary) = outcome else {
            panic!("retained stream did not complete")
        };
        assert_eq!(summary.counters.files_opened, 0);
        assert_eq!(summary.counters.bytes_read, 0);
        assert_eq!(summary.counters.record_parses, 0);
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

struct CorpusReader(
    Mutex<HashMap<String, Value>>,
    Mutex<Vec<usize>>,
    Mutex<Vec<RecordProjection>>,
);

impl RecordReader for CorpusReader {
    fn read_projected(
        &self,
        key: &RecordKey,
        expected_version: RecordVersion,
        projection: &RecordProjection,
    ) -> Result<RecordRead, RecordReadError> {
        self.2.lock().unwrap().push(projection.clone());
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
fn queued_provider_singletons_share_one_initial_segment_and_publication() {
    let values = (0..32)
        .map(|number| (format!("r{number}"), Value::String("Leipzig".into())))
        .collect();
    let reader = Arc::new(CorpusReader(
        Mutex::new(values),
        Mutex::new(Vec::new()),
        Mutex::new(Vec::new()),
    ));
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder
        .register_records(collection, "grouped-provider", 1)
        .unwrap();
    builder.register_reader(registration, reader).unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    session.pause_worker_for_measurement().unwrap();
    let writer = session.writer(registration).unwrap();
    let mut receipts = Vec::new();
    for number in 0..32 {
        let mut batch = writer.batch();
        batch
            .insert_from_reader(
                format!("r{number}"),
                Value::String("Leipzig".into()),
                RecordVersion::new(1),
            )
            .unwrap();
        receipts.push(writer.submit(batch).unwrap());
    }
    session.resume_worker_for_measurement();
    let revision = receipts[0].wait_published().unwrap();
    assert!(
        receipts
            .into_iter()
            .skip(1)
            .all(|receipt| receipt.wait_published() == Ok(revision))
    );
    session.finish().unwrap();
    assert_eq!(session.metrics().root_installations, 1);
    let view = index.view().unwrap();
    assert_eq!(view.segment_summaries().len(), 1);
    assert_eq!(index.status().unwrap().live_records, 32);
}

#[test]
fn relation_scan_batches_projected_provider_sources_for_deep_page() {
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
    let mut entries_schema = Schema::default();
    for (name, kind, indexed) in [
        ("place", LogicalType::Id, true),
        ("note", LogicalType::String, false),
    ] {
        entries_schema
            .register_field(FieldDefinition {
                path: vec![name.into()],
                kind,
                required: true,
                nullable: false,
                indexed,
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
    let mut builder = Index::builder();
    let places = builder.collection("places", places_schema).unwrap();
    let entries = builder.collection("entries", entries_schema).unwrap();
    let places_registration = builder.register_records(places, "places", 1).unwrap();
    let entries_registration = builder.register_records(entries, "entries", 1).unwrap();
    let values = (0..131)
        .map(|number| {
            let place = if number == 130 { "missing" } else { "p" };
            let note = if number % 2 == 0 { "Leipzig" } else { "Wien" };
            (
                format!("e{number:03}"),
                Value::from_json(serde_json::json!({"place":place,"note":note})).unwrap(),
            )
        })
        .collect();
    let reader = Arc::new(CorpusReader(
        Mutex::new(values),
        Mutex::new(Vec::new()),
        Mutex::new(Vec::new()),
    ));
    builder
        .register_reader(entries_registration, reader.clone())
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let place_writer = session.writer(places_registration).unwrap();
    let mut batch = place_writer.batch();
    batch
        .insert(
            "p",
            Value::from_json(serde_json::json!({"name":"Leipzig"})).unwrap(),
        )
        .unwrap();
    place_writer
        .submit(batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let writer = session.writer(entries_registration).unwrap();
    let mut batch = writer.batch();
    for number in 0..131 {
        let place = if number == 130 { "missing" } else { "p" };
        let note = if number % 2 == 0 { "Leipzig" } else { "Wien" };
        batch
            .insert_from_reader(
                format!("e{number:03}"),
                Value::from_json(serde_json::json!({"place":place,"note":note})).unwrap(),
                RecordVersion::new(1),
            )
            .unwrap();
    }
    writer.submit(batch).unwrap().wait_published().unwrap();
    let query = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("note ~ 'Leipzig' && place.name = 'Leipzig'").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            offset: 60,
            limit: 5,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let result = index.view().unwrap().search(&query).unwrap();
    assert_eq!(result.count, Some(65));
    assert_eq!(
        result
            .hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["e120", "e122", "e124", "e126", "e128"]
    );
    assert!(reader.1.lock().unwrap().iter().any(|size| *size > 1));
    assert!(reader.2.lock().unwrap().iter().any(|projection| {
        projection
            .fields
            .iter()
            .any(|field| field == &["note".to_string()])
    }));
    session.finish().unwrap();
}

#[test]
fn selective_native_relation_count_and_deep_page_use_exact_bitmaps() {
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
    let mut entries_schema = Schema::default();
    for (name, kind) in [("place", LogicalType::Id), ("year", LogicalType::Number)] {
        entries_schema
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
    entries_schema
        .register_relation(RelationDefinition {
            path: vec!["place".into()],
            target_collection: "places".into(),
            cardinality: RelationCardinality::One,
            reverse_alias: None,
        })
        .unwrap();
    let mut builder = Index::builder();
    let places = builder.collection("places", places_schema).unwrap();
    let entries = builder.collection("entries", entries_schema).unwrap();
    let places_registration = builder.register_records(places, "places", 1).unwrap();
    let entries_registration = builder.register_records(entries, "entries", 1).unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let places_writer = session.writer(places_registration).unwrap();
    let mut places_batch = places_writer.batch();
    for (id, name) in [("p", "Leipzig"), ("q", "Wien")] {
        places_batch
            .insert(
                id,
                Value::from_json(serde_json::json!({"name": name})).unwrap(),
            )
            .unwrap();
    }
    places_writer
        .submit(places_batch)
        .unwrap()
        .wait_published()
        .unwrap();
    let writer = session.writer(entries_registration).unwrap();
    let mut batch = writer.batch();
    for number in 0..8192 {
        let selected = number % 16 == 8;
        batch
            .insert(
                format!("e{number:05}"),
                Value::from_json(serde_json::json!({
                    "place": if selected { "p" } else { "q" },
                    "year": if selected { 1808 } else { 1807 }
                }))
                .unwrap(),
            )
            .unwrap();
    }
    writer.submit(batch).unwrap().wait_published().unwrap();
    let query = Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("year = 1808 && place.name = 'Leipzig'").unwrap()),
        options: QueryOptions {
            sort: Vec::new(),
            offset: 0,
            limit: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let view = index.view().unwrap();
    let count = view.search(&query).unwrap();
    assert_eq!(count.count, Some(512));
    assert_eq!(count.counters.exact_posting_matches, 512);
    assert_eq!(count.counters.files_opened, 0);
    assert_eq!(count.counters.record_parses, 0);
    let page = view
        .search(&Query {
            options: QueryOptions {
                offset: 500,
                limit: 5,
                ..query.options.clone()
            },
            ..query
        })
        .unwrap();
    assert_eq!(page.count, Some(512));
    assert_eq!(page.counters.exact_posting_matches, 512);
    assert_eq!(
        page.hits
            .iter()
            .map(|hit| hit.record_id.as_str())
            .collect::<Vec<_>>(),
        ["e08008", "e08024", "e08040", "e08056", "e08072"]
    );
    session.finish().unwrap();
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
    let reader = Arc::new(CorpusReader(
        Mutex::new(values),
        Mutex::new(Vec::new()),
        Mutex::new(Vec::new()),
    ));
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
        Mutex::new(Vec::new()),
    ));
    builder
        .register_reader(places_registration, reader.clone())
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
    assert!(
        reader
            .2
            .lock()
            .unwrap()
            .iter()
            .any(|projection| projection.fields == [vec!["name".to_string()]])
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
    index.configure_verified_cache(1024 * 1024).unwrap();
    let file_only = Query {
        collections: vec!["entries".into()],
        filter: Some(parse_filter("id = 'file'").unwrap()),
        fts: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let cold_file = view.search(&file_only).unwrap();
    let warm_file = view.search(&file_only).unwrap();
    assert!(!cold_file.counters.verified_result_hit);
    assert!(warm_file.counters.verified_result_hit);
    assert_eq!(cold_file.hits, warm_file.hits);
    session.finish().unwrap();
    index.rebuild().unwrap();
    assert!(
        !index
            .view()
            .unwrap()
            .search(&file_only)
            .unwrap()
            .counters
            .verified_result_hit
    );
    let rebuilt = index.view().unwrap().search(&query).unwrap();
    assert_eq!(rebuilt.count, Some(2));
    assert_eq!(rebuilt.hits[0].record_id, "file");
    assert_eq!(rebuilt.hits[1].record_id, "native");
    std::fs::write(&path, br#"{"id":"file","note":"changed","place":"p"}"#).unwrap();
    assert!(index.rebuild().is_err());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn one_collection_interleaves_managed_files_retained_and_versioned_records() {
    for file_first in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "objekte-core-managed-mixed-{}-{file_first}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let path = root.join("data/source.jsonl");
        std::fs::write(
            &path,
            b"{\"id\":\"file-1\",\"title\":\"shared\"}\n{\"id\":\"file-2\",\"title\":\"shared\"}\n",
        )
        .unwrap();
        let mut schema = Schema::default();
        for (name, kind) in [("id", LogicalType::Id), ("title", LogicalType::String)] {
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
        let mut builder = Index::builder();
        let collection = builder.collection("entries", schema).unwrap();
        let registration = builder
            .register_records(collection, "application", 1)
            .unwrap();
        let provider = Arc::new(VersionedFixture(
            Mutex::new((
                RecordVersion::new(7),
                Value::from_json(serde_json::json!({"id":"manual-2","title":"shared"})).unwrap(),
            )),
            Mutex::new(Vec::new()),
        ));
        builder
            .register_reader(registration, provider.clone())
            .unwrap();
        builder
            .register_file_catalog(FileCatalog::new(&root).unwrap())
            .unwrap();
        let mut definition =
            CollectionDefinition::folder("entries", "data", SourceFormat::JsonLines);
        definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        builder
            .register_file_source(collection, definition.file_source())
            .unwrap();
        let index = builder.build().unwrap();
        let session = index.ingestion();
        let writer = session.writer(registration).unwrap();
        let insert_direct = |id: &str, provider_backed: bool| {
            let mut batch = writer.batch();
            let value = if provider_backed {
                Value::from_json(serde_json::json!({"id":id,"title":"shared"})).unwrap()
            } else {
                Value::from_json(serde_json::json!({
                    "id":id,
                    "title":"shared",
                    "path":"data/source.jsonl",
                    "@file":{"path":"data/source.jsonl"}
                }))
                .unwrap()
            };
            if provider_backed {
                batch
                    .insert_from_reader(id, value, RecordVersion::new(7))
                    .unwrap();
            } else {
                batch.insert(id, value).unwrap();
            }
            writer.submit(batch).unwrap().wait_published().unwrap();
        };
        let file = if file_first {
            let file = index.add_file(&path).unwrap();
            insert_direct("manual-1", false);
            insert_direct("manual-2", true);
            file
        } else {
            insert_direct("manual-1", false);
            let file = index.add_file(&path).unwrap();
            insert_direct("manual-2", true);
            file
        };
        let view = index.view().unwrap();
        let catalog = view.file_catalog().unwrap();
        let mut members = catalog.records_for_file(file.file.file_id).unwrap();
        members.sort();
        assert_eq!(
            members
                .iter()
                .map(|key| key.record.0.as_ref())
                .collect::<Vec<_>>(),
            ["file-1", "file-2"]
        );
        for id in ["manual-1", "manual-2"] {
            assert!(catalog.file_for_record("entries", id).unwrap().is_none());
            assert!(
                view.get_record("entries", id, ReturnMode::Locator)
                    .unwrap()
                    .unwrap()
                    .file
                    .is_none()
            );
        }
        for id in ["file-1", "file-2"] {
            assert_eq!(
                catalog
                    .file_for_record("entries", id)
                    .unwrap()
                    .unwrap()
                    .file_id,
                file.file.file_id
            );
        }
        let query = |filter: &str| Query {
            collections: vec!["entries".into()],
            fts: None,
            filter: Some(parse_filter(filter).unwrap()),
            options: QueryOptions {
                sort: vec![SortField::simple("title", false)],
                offset: 0,
                limit: 10,
                count: true,
                return_mode: ReturnMode::Logical,
                evidence: true,
            },
        };
        let all = view.search(&query("title = 'shared'")).unwrap();
        assert_eq!(all.count, Some(4));
        assert_eq!(
            all.hits,
            view.search_reference(&query("title = 'shared'"))
                .unwrap()
                .hits
        );
        let fts = Query {
            fts: Some(FtsQuery {
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: FtsMode::Literal,
                literal: "shared".into(),
                pattern_parameter: None,
                fields: vec![FtsField {
                    path: objekte_search_core::model::FieldPath::parse("title").unwrap(),
                    binding: None,
                    query_path: None,
                }],
            }),
            filter: None,
            ..query("title = 'shared'")
        };
        assert_eq!(view.search(&fts).unwrap().count, Some(4));
        let composed = view
            .search(&query("@file.size > 0 || @record.id = 'manual-1'"))
            .unwrap();
        assert_eq!(composed.count, Some(3));
        assert_eq!(
            composed.hits,
            view.search_reference(&query("@file.size > 0 || @record.id = 'manual-1'"))
                .unwrap()
                .hits
        );
        let file_only = view
            .search(&query("@file.path = 'data/source.jsonl'"))
            .unwrap();
        assert_eq!(file_only.count, Some(2));
        assert_eq!(
            view.search(&query("path = 'data/source.jsonl'"))
                .unwrap()
                .count,
            Some(1)
        );
        assert!(!provider.1.lock().unwrap().is_empty());
        std::fs::write(&path, b"{\"id\":\"file-2\",\"title\":\"shared\"}\n").unwrap();
        let replaced = index.update_file(file.file, &path).unwrap();
        let changed = index.view().unwrap();
        assert!(
            changed
                .get_record("entries", "file-1", ReturnMode::Locator)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            changed
                .file_catalog()
                .unwrap()
                .records_for_file(replaced.file.file_id)
                .unwrap()
                .len(),
            1
        );
        for id in ["manual-1", "manual-2"] {
            assert!(
                changed
                    .get_record("entries", id, ReturnMode::Locator)
                    .unwrap()
                    .is_some()
            );
        }
        assert!(
            view.get_record("entries", "file-1", ReturnMode::Locator)
                .unwrap()
                .is_some()
        );
        index.compact().unwrap();
        index.rebuild().unwrap();
        let current_file = index
            .view()
            .unwrap()
            .file_handle_for_path(std::path::Path::new("data/source.jsonl"))
            .unwrap();
        let moved_path = root.join("data/moved.jsonl");
        std::fs::rename(&path, &moved_path).unwrap();
        let moved = index.move_file(current_file, &moved_path).unwrap();
        let moved_view = index.view().unwrap();
        assert_eq!(
            moved_view
                .file_catalog()
                .unwrap()
                .records_for_file(moved.file.file_id)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            moved_view
                .search(&query("@file.path = 'data/moved.jsonl'"))
                .unwrap()
                .count,
            Some(1)
        );
        index.delete_file(moved.file).unwrap();
        std::fs::remove_file(&moved_path).unwrap();
        let report = index.start_reconcile().unwrap().wait().unwrap();
        assert!(report.issues.is_empty());
        let final_view = index.view().unwrap();
        for id in ["manual-1", "manual-2"] {
            assert!(
                final_view
                    .get_record("entries", id, ReturnMode::Locator)
                    .unwrap()
                    .is_some()
            );
        }
        assert!(
            final_view
                .get_record("entries", "file-2", ReturnMode::Locator)
                .unwrap()
                .is_none()
        );
        session.finish().unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(unix)]
#[test]
fn pinned_file_bytes_reject_same_stamp_replacement() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-pinned-swap-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/source.jsonl");
    let replacement = root.join("data/replacement.jsonl");
    let a = b"{\"id\":\"file-1\",\"title\":\"aaaa\"}\n";
    let b = b"{\"id\":\"file-1\",\"title\":\"bbbb\"}\n";
    assert_eq!(a.len(), b.len());
    std::fs::write(&path, a).unwrap();
    let mut builder = Index::builder();
    let collection = builder.collection("entries", Schema::default()).unwrap();
    builder
        .register_file_catalog(FileCatalog::new(&root).unwrap())
        .unwrap();
    let mut definition = CollectionDefinition::folder("entries", "data", SourceFormat::JsonLines);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    builder
        .register_file_source(collection, definition.file_source())
        .unwrap();
    let index = builder.build().unwrap();
    let first = index.add_file(&path).unwrap();
    let pinned = index.view().unwrap();
    let old = pinned
        .get_record("entries", "file-1", ReturnMode::Bytes)
        .unwrap()
        .unwrap();
    assert_eq!(old.payload, Some(Payload::Bytes(a[..a.len() - 1].to_vec())));

    let old_metadata = std::fs::metadata(&path).unwrap();
    std::fs::write(&replacement, b).unwrap();
    std::fs::File::open(&replacement)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(old_metadata.modified().unwrap()))
        .unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    let new_metadata = std::fs::metadata(&path).unwrap();
    assert_eq!(new_metadata.len(), old_metadata.len());
    assert_eq!(
        new_metadata.modified().unwrap(),
        old_metadata.modified().unwrap()
    );
    let old_bytes = pinned.get_record("entries", "file-1", ReturnMode::Bytes);
    assert!(
        !matches!(old_bytes, Ok(Some(ref hit)) if hit.payload == Some(Payload::Bytes(b[..b.len() - 1].to_vec())))
    );
    let updated = index.update_file(first.file, &path).unwrap();
    assert_eq!(updated.indexed_records, 1);
    let current = index.view().unwrap();
    assert_eq!(
        current
            .get_record("entries", "file-1", ReturnMode::Bytes)
            .unwrap()
            .unwrap()
            .payload,
        Some(Payload::Bytes(b[..b.len() - 1].to_vec()))
    );
    assert_eq!(
        current
            .file_catalog()
            .unwrap()
            .records_for_file(first.file.file_id)
            .unwrap()
            .len(),
        1
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn file_and_direct_routes_reject_cross_owner_identity_changes() {
    for file_first in [false, true] {
        let root = std::env::temp_dir().join(format!(
            "objekte-managed-owner-{}-{file_first}",
            std::process::id()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        let path = root.join("data/source.jsonl");
        std::fs::write(&path, b"{\"id\":\"same\",\"title\":\"file\"}\n").unwrap();
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder.register_records(collection, "direct", 1).unwrap();
        builder
            .register_file_catalog(FileCatalog::new(&root).unwrap())
            .unwrap();
        let mut definition =
            CollectionDefinition::folder("entries", "data", SourceFormat::JsonLines);
        definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
        builder
            .register_file_source(collection, definition.file_source())
            .unwrap();
        let index = builder.build().unwrap();
        let direct_value =
            Value::from_json(serde_json::json!({"id":"same","title":"direct"})).unwrap();
        let submit = |mutation: &str| {
            let session = index.ingestion();
            let writer = session.writer(registration).unwrap();
            let mut batch = writer.batch();
            match mutation {
                "insert" => batch.insert("same", direct_value.clone()).unwrap(),
                "replace" => batch.replace("same", direct_value.clone()).unwrap(),
                "delete" => batch.delete("same").unwrap(),
                _ => unreachable!(),
            }
            let result = writer.submit(batch).unwrap().wait_published();
            let _ = session.finish();
            result
        };
        if file_first {
            let file = index.add_file(&path).unwrap();
            assert!(submit("insert").is_err());
            for mutation in ["replace", "delete"] {
                assert!(submit(mutation).is_err());
            }
            let view = index.view().unwrap();
            assert_eq!(
                view.file_catalog()
                    .unwrap()
                    .records_for_file(file.file.file_id)
                    .unwrap()
                    .len(),
                1
            );
            assert!(
                view.get_record("entries", "same", ReturnMode::Locator)
                    .unwrap()
                    .unwrap()
                    .file
                    .is_some()
            );
        } else {
            submit("insert").unwrap();
            assert!(index.add_file(&path).is_err());
            let view = index.view().unwrap();
            assert!(
                view.get_record("entries", "same", ReturnMode::Locator)
                    .unwrap()
                    .unwrap()
                    .file
                    .is_none()
            );
            submit("delete").unwrap();
            let file = index.add_file(&path).unwrap();
            assert_eq!(
                index
                    .view()
                    .unwrap()
                    .file_catalog()
                    .unwrap()
                    .records_for_file(file.file.file_id)
                    .unwrap()
                    .len(),
                1
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn direct_batch_publishes_while_multisegment_file_update_is_paused() {
    let root = std::env::temp_dir().join(format!(
        "objekte-managed-paused-update-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let path = root.join("data/source.bar");
    let lines = |count: usize| {
        (0..count)
            .map(|number| format!("{{\"id\":\"f{number}\",\"title\":\"shared\"}}\n"))
            .collect::<String>()
    };
    std::fs::write(&path, lines(4)).unwrap();
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let reader = Arc::new(PausingLinesReader {
        pause_next: AtomicBool::new(false),
        parses: AtomicUsize::new(0),
        entered: entered.clone(),
        release: release.clone(),
    });
    let mut sources = SourceRegistry::default();
    sources.register("bar", 1, "bar", reader.clone()).unwrap();
    let mut builder =
        Index::builder()
            .with_source_registry(sources)
            .with_options(RepositoryOptions {
                segment_policy:
                    objekte_search_core::index::segment::SegmentPolicy::with_target_records(
                        std::num::NonZeroUsize::new(2).unwrap(),
                    )
                    .unwrap(),
                ..RepositoryOptions::default()
            });
    let collection = builder.collection("entries", Schema::default()).unwrap();
    let registration = builder.register_records(collection, "direct", 1).unwrap();
    builder
        .register_file_catalog(FileCatalog::new(&root).unwrap())
        .unwrap();
    let mut definition =
        CollectionDefinition::folder("entries", "data", SourceFormat::registered("bar"));
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    builder
        .register_file_source(collection, definition.file_source())
        .unwrap();
    let index = builder.build().unwrap();
    let initial = index.add_file(&path).unwrap();
    let before = index.view().unwrap();
    assert_eq!(
        before
            .file_catalog()
            .unwrap()
            .records_for_file(initial.file.file_id)
            .unwrap()
            .len(),
        4
    );
    std::fs::write(&path, lines(6)).unwrap();
    reader.pause_next.store(true, Ordering::SeqCst);
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    std::thread::scope(|scope| {
        let file_update = scope.spawn(|| index.update_file(initial.file, &path));
        entered.wait();
        let mut batch = writer.batch();
        batch
            .insert(
                "manual-1",
                Value::from_json(serde_json::json!({"title":"shared"})).unwrap(),
            )
            .unwrap();
        writer.submit(batch).unwrap().wait_published().unwrap();
        let intermediate = index.view().unwrap();
        assert!(
            intermediate
                .get_record("entries", "manual-1", ReturnMode::Locator)
                .unwrap()
                .is_some()
        );
        assert_eq!(
            intermediate
                .file_catalog()
                .unwrap()
                .records_for_file(initial.file.file_id)
                .unwrap()
                .len(),
            4
        );
        release.wait();
        file_update.join().unwrap().unwrap();
    });
    let after = index.view().unwrap();
    assert_eq!(
        after
            .file_catalog()
            .unwrap()
            .records_for_file(initial.file.file_id)
            .unwrap()
            .len(),
        6
    );
    assert!(
        after
            .get_record("entries", "manual-1", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert!(
        before
            .get_record("entries", "manual-1", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(
        before
            .get_record("entries", "f5", ReturnMode::Locator)
            .unwrap()
            .is_none()
    );
    assert!(
        after
            .get_record("entries", "f5", ReturnMode::Locator)
            .unwrap()
            .is_some()
    );
    assert_eq!(reader.parses.load(Ordering::SeqCst), 10);
    session.finish().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn managed_file_metadata_sort_keeps_direct_missing_and_updates_local_paths() {
    let root = std::env::temp_dir().join(format!("objekte-managed-sort-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    for (name, id) in [("a.json", "z"), ("m.json", "b"), ("z.json", "a")] {
        std::fs::write(root.join("data").join(name), format!("{{\"id\":\"{id}\"}}")).unwrap();
    }
    let mut declaration = CollectionDeclaration::new("entries", Schema::default());
    declaration
        .sort_fields
        .push(objekte_search_core::model::FieldPath::parse("@file.path").unwrap());
    let mut builder = Index::builder();
    let collection = builder.declare_collection(declaration).unwrap();
    let registration = builder.register_records(collection, "direct", 1).unwrap();
    builder
        .register_file_catalog(FileCatalog::new(&root).unwrap())
        .unwrap();
    let mut definition = CollectionDefinition::folder("entries", "data", SourceFormat::Json);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    builder
        .register_file_source(collection, definition.file_source())
        .unwrap();
    let index = builder.build().unwrap();
    let session = index.ingestion();
    let writer = session.writer(registration).unwrap();
    let mut batch = writer.batch();
    batch
        .insert("unlinked", Value::String("ordinary".into()))
        .unwrap();
    writer.submit(batch).unwrap().wait_published().unwrap();
    for name in ["a.json", "m.json", "z.json"] {
        index.add_file(&root.join("data").join(name)).unwrap();
    }
    let query = |descending, offset| Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: vec![SortField::simple("@file.path", descending)],
            limit: 4,
            offset,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    let ids = |q: Query| {
        let view = index.view().unwrap();
        let optimized = view.search(&q).unwrap();
        optimized
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(query(false, 0)), ["z", "b", "a", "unlinked"]);
    assert_eq!(ids(query(true, 0)), ["a", "b", "z", "unlinked"]);
    assert_eq!(ids(query(false, 1)), ["b", "a", "unlinked"]);
    let pinned = index.view().unwrap();
    let handle = pinned
        .file_handle_for_path(std::path::Path::new("data/a.json"))
        .unwrap();
    std::fs::rename(root.join("data/a.json"), root.join("data/y.json")).unwrap();
    index.move_file(handle, &root.join("data/y.json")).unwrap();
    assert_eq!(ids(query(false, 0)), ["b", "z", "a", "unlinked"]);
    assert_eq!(
        pinned
            .search(&query(false, 0))
            .unwrap()
            .hits
            .into_iter()
            .map(|hit| hit.record_id)
            .collect::<Vec<_>>(),
        ["z", "b", "a", "unlinked"]
    );
    session.finish().unwrap();
    std::fs::remove_dir_all(root).unwrap();
}
