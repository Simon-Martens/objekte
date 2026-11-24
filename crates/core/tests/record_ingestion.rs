use std::collections::BTreeMap;

use objekte_search_core::ingestion::{IngestionOptions, SubmitError};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{LogicalType, Number, Value};
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::query_results::Payload;
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
