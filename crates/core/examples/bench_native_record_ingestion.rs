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
use objekte_search_core::inventory::ByteRange;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::model::{FileId, LogicalType, Number, RecordId, Value};
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::query::{Operator, Query, QueryOptions, parse_filter};
use objekte_search_core::query_execution::runner::DirectRouteCounters;
use objekte_search_core::records::RecordKey;
use objekte_search_core::repository::configuration::{
    CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::schema::{FieldDefinition, Schema};
use objekte_search_core::source::SourceFormat;
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
    let prebuilt_rss_kib = rss_kib("VmRSS:");
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
    let accepted_rss_kib = rss_kib("VmRSS:");
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
    let published_rss_kib = rss_kib("VmRSS:");
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
            "prebuilt_rss_kib": prebuilt_rss_kib,
            "accepted_rss_kib": accepted_rss_kib,
            "published_rss_kib": published_rss_kib,
            "overflow": overflow, "producer_accept_ns": accept_ns, "publish_drain_ns": publish_ns,
            "accepted_batches": metrics.accepted_batches, "published_batches": metrics.published_batches,
            "root_installations": metrics.root_installations,
            "active_segment_count": view.segment_summaries().iter().filter(|segment| segment.active).count(),
            "high_water_ram_bytes": metrics.high_water_ram_bytes,
            "high_water_spool_bytes": metrics.high_water_spool_bytes,
            "spool_written_bytes": metrics.spool_written_bytes,
            "spool_read_bytes": metrics.spool_read_bytes,
            "high_water_in_flight_estimated_bytes": metrics.high_water_in_flight_estimated_bytes,
            "high_water_waiting_descriptors": metrics.high_water_waiting_descriptors,
            "high_water_active_descriptors": metrics.high_water_in_flight_batches,
            "overflow_write_ns": metrics.overflow_write_ns,
            "pending_ram_bytes": metrics.pending_ram_bytes,
            "pending_spool_bytes": metrics.pending_spool_bytes,
            "segments": view.segment_summaries().len(),
            "index_family_bytes": view.estimated_index_bytes(),
            "native_storage": view.native_storage_estimate(),
            "ready_rss_kib": rss_kib("VmRSS:"),
            "peak_rss_kib": rss_kib("VmHWM:")
        })
    );
    Ok(())
}

fn routing(segment_count: usize, count: usize, root: PathBuf) -> Result<(), Box<dyn Error>> {
    if count == 0 || count % 64 != 0 || ![1, 8, 64].contains(&segment_count) {
        return Err("routing expects 64 file groups and 1, 8 or 64 target segments".into());
    }
    std::fs::create_dir_all(root.join("data"))?;
    let records_per_file = count / 64;
    let mut files = Vec::new();
    let mut selective_size = 0u64;
    for group in 0..64 {
        let name = format!("data/group-{group:02}.jsonl");
        let mut bytes = Vec::new();
        let mut ranges = Vec::new();
        for offset in 0..records_per_file {
            let number = group * records_per_file + offset;
            let start = bytes.len() as u64;
            let pad = if group == 63 && offset == 0 {
                ",\"pad\":\"xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx\""
            } else {
                ""
            };
            bytes.extend_from_slice(
                format!(
                    "{{\"id\":\"r{number:05}\",\"year\":{},\"note\":\"{}\"{pad}}}\n",
                    1800 + number % 16,
                    if number % 2 == 0 { "Leipzig" } else { "Wien" }
                )
                .as_bytes(),
            );
            ranges.push(ByteRange {
                start,
                end: bytes.len() as u64 - 1,
            });
        }
        std::fs::write(root.join(&name), bytes)?;
        if group == 63 {
            selective_size = std::fs::metadata(root.join(&name))?.len();
        }
        files.push((name, ranges));
    }
    std::fs::write(root.join("data/empty.jsonl"), [])?;
    let mut builder = Index::builder().with_options(RepositoryOptions {
        segment_policy: SegmentPolicy {
            target_records: NonZeroUsize::new(count / segment_count).unwrap(),
            max_files: NonZeroUsize::new(64).unwrap(),
        },
        query_workers: 1,
        ..Default::default()
    });
    let collection = builder.collection("records", schema()?)?;
    let registration = builder.register_records(collection, "benchmark.direct", 1)?;
    builder.register_file_catalog(FileCatalog::new(&root)?)?;
    let mut definition = CollectionDefinition::folder("records", "data", SourceFormat::JsonLines);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    builder.register_file_source(collection, definition.file_source())?;
    let index = builder.build()?;
    let session = index.ingestion();
    let writer = session.writer(registration)?;
    let publish_direct = |id: &str| -> Result<(), Box<dyn Error>> {
        let mut batch = writer.batch();
        batch.insert(id, value(0))?;
        writer
            .submit(batch)
            .map_err(|_| "batch admission refused")?
            .wait_published()?;
        Ok(())
    };
    publish_direct("manual-1")?;
    for (group, (name, _)) in files.iter().enumerate() {
        index.add_file(&root.join(name))?;
        if group == 31 {
            publish_direct("manual-2")?;
        }
    }
    index.add_file(&root.join("data/empty.jsonl"))?;
    publish_direct("manual-3")?;
    let view = index.view()?;
    let catalog = view.file_catalog().ok_or("catalog missing")?;
    let selected_files = catalog.files_by_size(Operator::Eq, selective_size)?;
    if selected_files.len() != 1
        || selected_files[0].relative_path != std::path::Path::new(&files[63].0)
    {
        return Err("size index selected wrong file".into());
    }
    let metadata_query = |filter: String| -> Result<Query, Box<dyn Error>> {
        Ok(Query {
            collections: vec!["records".into()],
            fts: None,
            filter: Some(parse_filter(&filter)?),
            options: QueryOptions {
                sort: Vec::new(),
                offset: 0,
                limit: 10,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        })
    };
    let selected = view.search(&metadata_query(format!("@file.size = {selective_size}"))?)?;
    let composed = view.search(&metadata_query(format!(
        "@file.size = {selective_size} || @record.id = 'manual-1'"
    ))?)?;
    if selected.count != Some(records_per_file)
        || composed.count != Some(records_per_file + 1)
        || selected.counters.files_opened != 0
        || selected.counters.bytes_read != 0
        || selected.counters.file_restrictions_resolved != 1
        || selected.counters.file_restriction_matching_files != 1
        || selected.counters.file_restrictions_broad != 0
        || selected.counters.file_membership_lists_probed != 1
        || composed.counters.files_opened != 0
    {
        return Err(format!(
            "selective metadata query mismatch: selected {:?} {:?}, composed {:?} {:?}",
            selected.count, selected.counters, composed.count, composed.counters
        )
        .into());
    }
    let observed_segments = view.segment_summaries().len();
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
    let mut route_counts = DirectRouteCounters::default();
    for operation in 0..10_000 {
        let id = if operation % 2 == 0 {
            format!("r{:05}", (operation / 2) % count)
        } else {
            format!("missing{:05}", operation / 2)
        };
        let observed = view.get_record_with_route_counts_for_measurement(
            "records",
            &id,
            ReturnMode::Locator,
            &mut route_counts,
        )?;
        if operation % 2 == 0 {
            let number = (operation / 2) % count;
            let group = number / records_per_file;
            let offset = number % records_per_file;
            let expected_file = catalog
                .file_by_path(std::path::Path::new(&files[group].0))
                .ok_or("file missing")?;
            let hit = observed.ok_or("route lost record")?;
            let origin = hit.file.ok_or("route lost file")?;
            if hit.record_id != id
                || origin.file_id != expected_file.id
                || origin.range != Some(files[group].1[offset])
            {
                return Err(format!("wrong managed-file origin for {id}").into());
            }
        } else if observed.is_some() {
            return Err(format!("unexpected record {id}").into());
        }
    }
    let started = Instant::now();
    let mut members = 0usize;
    for (group, (name, _)) in files.iter().enumerate() {
        let file = catalog
            .file_by_path(std::path::Path::new(name))
            .ok_or("file missing")?;
        let actual = catalog.records_for_file(file.id)?;
        let mut expected = (0..records_per_file)
            .map(|offset| {
                RecordKey::new(
                    collection,
                    RecordId(format!("r{:05}", group * records_per_file + offset).into()),
                )
            })
            .collect::<Vec<_>>();
        expected.sort();
        if actual != expected {
            return Err(format!("wrong reverse members for {name}").into());
        }
        members += actual.len();
    }
    let reverse_ns = started.elapsed().as_nanos();
    let empty = catalog
        .file_by_path(std::path::Path::new("data/empty.jsonl"))
        .ok_or("empty file missing")?;
    if !catalog.records_for_file(empty.id)?.is_empty()
        || catalog.records_for_file(FileId(u32::MAX)).is_ok()
        || members != count
        || hits != 5_000
        || route_counts.route_key_probes != 10_000
        || route_counts.route_segment_visits != 5_000
    {
        return Err("managed-file routing mismatch".into());
    }
    println!(
        "{}",
        json!({
            "mode": "routing", "workload": "managed_file", "records": count,
            "requested_segments": segment_count, "segments": observed_segments,
            "lookup_operations": 10000, "lookup_hits": hits, "lookup_ns": lookup_ns,
            "route_key_probes": route_counts.route_key_probes,
            "route_segment_visits": route_counts.route_segment_visits,
            "unrelated_segment_visits": route_counts.route_segment_visits.saturating_sub(hits as u64),
            "reverse_members": members, "reverse_ns": reverse_ns,
            "metadata_size_candidates": selected_files.len(),
            "metadata_record_candidates": selected.counters.candidates,
            "metadata_matching_records": selected.count,
            "metadata_resolutions": selected.counters.file_restrictions_resolved,
            "metadata_membership_probes": selected.counters.file_membership_lists_probed,
            "metadata_broad_fallbacks": selected.counters.file_restrictions_broad,
            "metadata_source_opens": selected.counters.files_opened,
            "metadata_source_bytes": selected.counters.bytes_read,
            "mixed_or_count": composed.count,
            "index_family_bytes": view.estimated_index_bytes(),
            "native_storage": view.native_storage_estimate(),
            "ready_rss_kib": rss_kib("VmRSS:"), "peak_rss_kib": rss_kib("VmHWM:")
        })
    );
    session.finish()?;
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
