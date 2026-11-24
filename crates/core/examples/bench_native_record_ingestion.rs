//! After-only record ingestion and routing measurement for the task gate.

use std::collections::BTreeMap;
use std::error::Error;
use std::hint::black_box;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::files::FileCatalog;
use objekte_search_core::index::segment::SegmentPolicy;
use objekte_search_core::ingestion::{Batch, IngestionOptions, Receipt, RecordWriter};
use objekte_search_core::inventory::{ByteRange, SourceRevision};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{FileId, LogicalType, Number, Value};
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::repository::configuration::RepositoryOptions;
use objekte_search_core::schema::{FieldDefinition, Schema};
use serde_json::json;

fn schema() -> Result<Schema, String> {
    let mut schema = Schema::default();
    for (name, indexed) in [("year", true), ("note", false)] {
        schema.register_field(FieldDefinition {
            path: vec![name.into()],
            kind: if name == "year" {
                LogicalType::Number
            } else {
                LogicalType::String
            },
            required: true,
            nullable: false,
            indexed,
            multi: false,
            element_nullable: Vec::new(),
        })?;
    }
    Ok(schema)
}

fn value(number: usize) -> Value {
    Value::Object(BTreeMap::from([
        (
            "year".into(),
            Value::Number(Number::integer(1800 + (number % 16) as i128)),
        ),
        (
            "note".into(),
            Value::String(if number % 2 == 0 { "Leipzig" } else { "Wien" }.into()),
        ),
    ]))
}

fn rss_kib(field: &str) -> Option<u64> {
    std::fs::read_to_string("/proc/self/status")
        .ok()?
        .lines()
        .find(|line| line.starts_with(field))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn batches(
    writer: &RecordWriter,
    count: usize,
    batch_size: usize,
    producers: usize,
) -> Result<Vec<Vec<Batch>>, String> {
    let mut by_producer = (0..producers).map(|_| Vec::new()).collect::<Vec<_>>();
    for start in (0..count).step_by(batch_size) {
        let mut batch = writer.batch();
        for number in start..(start + batch_size).min(count) {
            batch.insert(format!("r{number:05}"), value(number))?;
        }
        by_producer[(start / batch_size) % producers].push(batch);
    }
    Ok(by_producer)
}

fn submit_all(writer: RecordWriter, batches: Vec<Batch>) -> Result<Vec<Receipt>, String> {
    let mut receipts = Vec::with_capacity(batches.len());
    for batch in batches {
        receipts.push(
            writer
                .submit(batch)
                .map_err(|_| "batch admission refused")?,
        );
    }
    Ok(receipts)
}

fn ingest(
    batch_size: usize,
    producers: usize,
    overflow: bool,
    count: usize,
    spool_parent: PathBuf,
) -> Result<(), Box<dyn Error>> {
    if batch_size == 0 || producers == 0 || count == 0 {
        return Err("batch size, producers and count must be positive".into());
    }
    let mut builder = Index::builder().with_options(RepositoryOptions {
        query_workers: 1,
        ..Default::default()
    });
    let collection = builder.collection("records", schema()?)?;
    let registration = builder.register_records(collection, "benchmark.records", 1)?;
    let index = builder.build()?;
    let session = index.ingestion_with_options(IngestionOptions {
        pending_ram_bytes: if overflow {
            1024 * 1024
        } else {
            64 * 1024 * 1024
        },
        max_batch_bytes: 64 * 1024 * 1024,
        spool_bytes: if overflow {
            256 * 1024 * 1024
        } else {
            4 * 1024 * 1024 * 1024
        },
        spool_directory: Some(spool_parent),
    })?;
    if overflow {
        session.pause_worker_for_measurement()?;
    }
    let writer = session.writer(registration)?;
    let inputs = batches(&writer, count, batch_size, producers)?;
    let producer_started = Instant::now();
    let handles = inputs
        .into_iter()
        .map(|input| {
            let writer = writer.clone();
            std::thread::spawn(move || submit_all(writer, input))
        })
        .collect::<Vec<_>>();
    let mut receipts = Vec::new();
    for handle in handles {
        receipts.extend(handle.join().map_err(|_| "producer thread panicked")??);
    }
    let accept_ns = producer_started.elapsed().as_nanos();
    let before_release = session.metrics();
    if overflow && before_release.high_water_spool_bytes == 0 {
        return Err("overflow run did not use the spool".into());
    }
    let publish_started = Instant::now();
    if overflow {
        session.resume_worker_for_measurement();
    }
    for receipt in &receipts {
        receipt.wait_published()?;
    }
    session.finish()?;
    let publish_ns = publish_started.elapsed().as_nanos();
    let metrics = session.metrics();
    let view = index.view()?;
    if index.status()?.live_records != count as u64 {
        return Err("published record count mismatch".into());
    }
    for number in 0..count {
        let id = format!("r{number:05}");
        if view
            .get_record("records", &id, ReturnMode::Locator)?
            .is_none()
        {
            return Err(format!("missing published key: {id}").into());
        }
    }
    println!(
        "{}",
        json!({
            "mode": "ingest", "records": count, "batch_size": batch_size, "producers": producers,
            "overflow": overflow, "producer_accept_ns": accept_ns, "publish_drain_ns": publish_ns,
            "accepted_batches": metrics.accepted_batches, "published_batches": metrics.published_batches,
            "high_water_ram_bytes": metrics.high_water_ram_bytes,
            "high_water_spool_bytes": metrics.high_water_spool_bytes,
            "spool_written_bytes": metrics.spool_written_bytes,
            "spool_read_bytes": metrics.spool_read_bytes,
            "high_water_in_flight_estimated_bytes": metrics.high_water_in_flight_estimated_bytes,
            "overflow_write_ns": metrics.overflow_write_ns,
            "pending_ram_bytes": metrics.pending_ram_bytes,
            "pending_spool_bytes": metrics.pending_spool_bytes,
            "segments": view.segment_summaries().len(),
            "index_bytes": view.estimated_index_bytes(), "ready_rss_kib": rss_kib("VmRSS:"),
            "peak_rss_kib": rss_kib("VmHWM:")
        })
    );
    Ok(())
}

fn routing(segment_count: usize, count: usize, root: PathBuf) -> Result<(), Box<dyn Error>> {
    if count == 0
        || count % 64 != 0
        || ![1, 8, 64].contains(&segment_count)
        || count % segment_count != 0
    {
        return Err("routing expects a multiple of 64 records and 1, 8 or 64 segments".into());
    }
    std::fs::create_dir(&root)?;
    let mut files = Vec::new();
    for group in 0..count / 64 {
        let name = format!("group-{group:04}.bin");
        std::fs::write(root.join(&name), [b'x'; 64])?;
        let revision = SourceRevision::from_metadata(&std::fs::metadata(root.join(&name))?)?;
        files.push((name, revision));
    }
    let empty = root.join("empty.bin");
    std::fs::write(&empty, [])?;
    let empty_revision = SourceRevision::from_metadata(&std::fs::metadata(&empty)?)?;
    let mut builder = Index::builder().with_options(RepositoryOptions {
        segment_policy: SegmentPolicy {
            target_records: NonZeroUsize::new(count / segment_count).unwrap(),
            max_files: NonZeroUsize::new(count).unwrap(),
        },
        query_workers: 1,
        ..Default::default()
    });
    let collection = builder.collection("records", schema()?)?;
    let registration = builder.register_records(collection, "benchmark.linked", 1)?;
    let catalog = builder.register_file_catalog(FileCatalog::new(&root)?)?;
    builder.enable_file_links(registration, catalog)?;
    let index = builder.build()?;
    let session = index.ingestion();
    let writer = session.writer(registration)?;
    let mut batch = writer.batch();
    for (group, (name, revision)) in files.iter().enumerate() {
        let file = batch.declare_file(name, *revision)?;
        for offset in 0..64 {
            let number = group * 64 + offset;
            batch.insert_with_file(
                format!("r{number:05}"),
                value(number),
                file,
                Some(ByteRange::checked(
                    offset as u64,
                    offset as u64 + 1,
                    revision.size,
                )?),
            )?;
        }
    }
    batch.declare_file("empty.bin", empty_revision)?;
    writer.submit(batch).unwrap().wait_published()?;
    session.finish()?;
    let view = index.view()?;
    let observed_segments = view.segment_summaries().len();
    if observed_segments != segment_count {
        return Err(format!("wanted {segment_count} segments, found {observed_segments}").into());
    }
    let started = Instant::now();
    let mut hits = 0usize;
    for operation in 0..10_000 {
        let id = if operation % 2 == 0 {
            format!("r{:05}", (operation / 2) % count)
        } else {
            format!("missing{:05}", operation / 2)
        };
        hits += usize::from(
            view.get_record("records", &id, ReturnMode::Locator)?
                .is_some(),
        );
    }
    black_box(hits);
    let lookup_ns = started.elapsed().as_nanos();
    let catalog = view.file_catalog().ok_or("catalog missing")?;
    let started = Instant::now();
    let mut members = 0usize;
    for (name, _) in &files {
        let file = catalog
            .file_by_path(std::path::Path::new(name))
            .ok_or("file missing")?;
        members += catalog.records_for_file(file.id)?.len();
    }
    let reverse_ns = started.elapsed().as_nanos();
    let empty = catalog
        .file_by_path(std::path::Path::new("empty.bin"))
        .ok_or("empty file missing")?;
    if !catalog.records_for_file(empty.id)?.is_empty()
        || catalog.records_for_file(FileId(u32::MAX)).is_ok()
        || members != count
        || hits != 5_000
    {
        return Err("routing membership mismatch".into());
    }
    println!(
        "{}",
        json!({
            "mode": "routing", "records": count, "segments": observed_segments,
            "lookup_operations": 10000, "lookup_hits": hits, "lookup_ns": lookup_ns,
            "route_key_probes": 10000, "route_segment_visits": hits,
            "reverse_members": members, "reverse_ns": reverse_ns,
            "index_bytes": view.estimated_index_bytes(), "ready_rss_kib": rss_kib("VmRSS:"),
            "peak_rss_kib": rss_kib("VmHWM:")
        })
    );
    std::fs::remove_dir_all(root)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    match args.as_slice() {
        [mode, batch_size, producers, overflow, count, directory] if mode == "ingest" => ingest(
            batch_size.parse()?, producers.parse()?, overflow == "overflow", count.parse()?, PathBuf::from(directory),
        ),
        [mode, segments, count, directory] if mode == "routing" => routing(
            segments.parse()?, count.parse()?, PathBuf::from(directory),
        ),
        _ => Err("usage: ingest BATCH_SIZE PRODUCERS normal|overflow COUNT SPOOL_PARENT | routing SEGMENTS COUNT NEW_ROOT".into()),
    }
}
