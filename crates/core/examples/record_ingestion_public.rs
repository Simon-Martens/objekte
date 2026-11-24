//! Public record ingestion, optional file links, and a versioned reader.

use std::collections::BTreeMap;
use std::error::Error;
use std::sync::{Arc, Mutex};

use objekte_search_core::adapter::helpers::json_string::with_quoted_text;
use objekte_search_core::files::FileCatalog;
use objekte_search_core::inventory::{ByteRange, SourceRevision};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::Value;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{Query, QueryOptions, parse_filter};
use objekte_search_core::query_results::Payload;
use objekte_search_core::records::reader::{
    RecordProjection, RecordRead, RecordReadError, RecordReader,
};
use objekte_search_core::records::{RecordKey, RecordVersion};
use objekte_search_core::schema::Schema;

fn direct_records() -> Result<(), Box<dyn Error>> {
    let mut builder = Index::builder();
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

fn file_linked_records() -> Result<(), Box<dyn Error>> {
    let root = std::env::temp_dir().join(format!("objekte-public-example-{}", std::process::id()));
    std::fs::create_dir(&root)?;
    let source = root.join("source.txt");
    std::fs::write(&source, b"alpha beta")?;
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&source)?)?;
    let mut builder = Index::builder();
    let entries = builder.collection("entries", Schema::default())?;
    let registration = builder.register_records(entries, "example.linked", 1)?;
    let catalog = builder.register_file_catalog(FileCatalog::new(&root)?)?;
    builder.enable_file_links(registration, catalog)?;
    let index = builder.build()?;
    let session = index.ingestion();
    let writer = session.writer(registration)?;
    let mut batch = writer.batch();
    let file = batch.declare_file("source.txt", revision)?;
    batch.insert_with_file(
        "alpha",
        Value::String("alpha".into()),
        file,
        Some(ByteRange::checked(0, 5, revision.size)?),
    )?;
    batch.insert_with_file(
        "beta",
        Value::String("beta".into()),
        file,
        Some(ByteRange::checked(6, 10, revision.size)?),
    )?;
    writer.submit(batch).unwrap().wait_published()?;
    let view = index.view()?;
    let catalog = view.file_catalog().ok_or("catalog missing")?;
    let link = catalog
        .file_for_record("entries", "alpha")?
        .ok_or("link missing")?;
    let members = catalog.records_for_file(link.file_id)?;
    assert_eq!(members.len(), 2);
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
    let mut builder = Index::builder();
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
    file_linked_records()?;
    versioned_reader()?;
    Ok(())
}
