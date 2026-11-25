//! Public direct and managed-file ingestion with a versioned reader.

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

use objekte_loader_json::with_quoted_text;
use objekte_search_core::files::FileCatalog;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::Value;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::query_results::Payload;
use objekte_search_core::records::reader::{
    RecordProjection, RecordRead, RecordReadError, RecordReader,
};
use objekte_search_core::records::{RecordKey, RecordVersion};
use objekte_search_core::repository::configuration::CollectionDefinition;
use objekte_search_core::schema::Schema;

fn direct_records() -> Result<(), Box<dyn Error>> {
    let mut builder =
        Index::builder().with_source_registry(objekte_cli::loader::installed_sources());
    let entries = builder.collection("entries", Schema::default())?;
    let registration = builder.register_records(entries, "example.direct", 1)?;
    let index = builder.build()?;
    let session = index.ingestion();
    let writer = session.writer(registration)?;
    let mut scratch = String::new();
    let quoted = b"\"Stra\\u00dfe\"";
    let city = with_quoted_text(quoted, 0..quoted.len(), &mut scratch, |text| {
        Ok(text.to_owned())
    })?;
    let mut batch = writer.batch();
    batch.insert(
        "entry-1",
        Value::Object(BTreeMap::from([("city".into(), Value::String(city))])),
    )?;
    writer.submit(batch).unwrap().wait_published()?;
    let hit = index
        .view()?
        .get_record("entries", "entry-1", ReturnMode::Logical)?
        .ok_or("record missing")?;
    assert!(matches!(hit.payload, Some(Payload::Logical(_))));
    assert!(hit.file.is_none());
    session.finish()?;
    Ok(())
}

fn managed_file_and_direct_records() -> Result<(), Box<dyn Error>> {
    let root = std::env::temp_dir().join(format!("objekte-public-example-{}", std::process::id()));
    std::fs::create_dir_all(root.join("data"))?;
    let source = root.join("data/source.jsonl");
    std::fs::write(&source, b"{\"note\":\"alpha\"}\n{\"note\":\"beta\"}\n")?;
    let mut builder =
        Index::builder().with_source_registry(objekte_cli::loader::installed_sources());
    let entries = builder.collection("entries", Schema::default())?;
    let registration = builder.register_records(entries, "example.direct", 1)?;
    builder.register_file_catalog(FileCatalog::new(&root)?)?;
    builder.register_file_source(
        entries,
        CollectionDefinition::folder("entries", "data", objekte_loader_jsonl::JSON_LINES)
            .file_source(),
    )?;
    let index = builder.build()?;
    let session = index.ingestion();
    let writer = session.writer(registration)?;
    let mut first = writer.batch();
    first.insert("manual-1", Value::String("manual".into()))?;
    writer.submit(first).unwrap().wait_published()?;
    let imported = index.add_file(&source)?;
    let mut second = writer.batch();
    second.insert("manual-2", Value::String("later".into()))?;
    writer.submit(second).unwrap().wait_published()?;
    let view = index.view()?;
    let catalog = view.file_catalog().ok_or("catalog missing")?;
    assert_eq!(catalog.records_for_file(imported.file.file_id)?.len(), 2);
    assert!(catalog.file_for_record("entries", "manual-1")?.is_none());
    assert!(catalog.file_for_record("entries", "manual-2")?.is_none());
    session.finish()?;
    std::fs::remove_dir_all(root)?;
    Ok(())
}

struct VersionedReader {
    calls: Mutex<Vec<RecordProjection>>,
}

impl RecordReader for VersionedReader {
    fn read_projected(
        &self,
        _: &RecordKey,
        expected_version: RecordVersion,
        projection: &RecordProjection,
    ) -> Result<RecordRead, RecordReadError> {
        self.calls.lock().unwrap().push(projection.clone());
        if expected_version != RecordVersion::new(7) {
            return Err(RecordReadError::ChangedVersion {
                expected: expected_version,
                actual: Some(RecordVersion::new(7)),
            });
        }
        let value = Value::Object(BTreeMap::from([(
            "note".into(),
            Value::String("Leipzig".into()),
        )]));
        Ok(RecordRead {
            version: expected_version,
            value,
            text: Vec::new(),
            original_bytes: None,
        })
    }
}

fn versioned_reader() -> Result<(), Box<dyn Error>> {
    let reader = Arc::new(VersionedReader {
        calls: Mutex::new(Vec::new()),
    });
    let mut builder =
        Index::builder().with_source_registry(objekte_cli::loader::installed_sources());
    let entries = builder.collection("entries", Schema::default())?;
    let registration = builder.register_records(entries, "example.reader", 1)?;
    builder.register_reader(registration, reader.clone())?;
    let index = builder.build()?;
    let session = index.ingestion();
    let writer = session.writer(registration)?;
    let mut batch = writer.batch();
    batch.insert_from_reader(
        "entry-1",
        Value::Object(BTreeMap::from([(
            "note".into(),
            Value::String("Leipzig".into()),
        )])),
        RecordVersion::new(7),
    )?;
    writer.submit(batch).unwrap().wait_published()?;
    let result = index.view()?.search(&Query {
        collections: vec!["entries".into()],
        fts: None,
        filter: Some(parse_filter("note ~ 'Leipzig'")?),
        options: QueryOptions {
            sort: Vec::new(),
            offset: 0,
            limit: 10,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    })?;
    assert_eq!(result.count, Some(1));
    assert!(
        reader
            .calls
            .lock()
            .unwrap()
            .iter()
            .any(|projection| { projection.fields == [vec!["note".to_owned()]] })
    );
    session.finish()?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    direct_records()?;
    managed_file_and_direct_records()?;
    versioned_reader()?;
    Ok(())
}
