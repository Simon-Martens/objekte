//! Large single-source and many small registered-source initial builds.

use std::error::Error;
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use objekte_search_core::index::segment::SegmentPolicy;
use objekte_search_core::inventory::ByteRange;
use objekte_search_core::lifecycle::{Index, IndexRevision};
use objekte_search_core::model::Value;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::repository::configuration::{
    CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::source::{
    BuildChunk, ExtractedRecord, LogicalTextCoverage, SourceFormat, SourceReader, TextDerivation,
};
use serde_json::json;

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

#[derive(Default)]
struct DecoderActivity {
    large: AtomicUsize,
    peak_large: AtomicUsize,
    neighbor: AtomicUsize,
    peak_neighbor: AtomicUsize,
}

#[derive(Default)]
struct PipeReader(Option<Arc<DecoderActivity>>);

impl SourceReader for PipeReader {
    fn name(&self) -> &'static str {
        "scale-pipe"
    }

    fn supports_streaming_build(&self) -> bool {
        true
    }

    fn text_derivation(&self) -> TextDerivation {
        TextDerivation::LogicalStringsAfterConversion
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        LogicalTextCoverage::AllStringFields
    }

    fn frame(&self, _bytes: &[u8]) -> Result<Vec<ByteRange>, String> {
        Err("the scale probe requires the registered streaming entry point".into())
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        let line = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
        let (id, fields) = line.split_once('|').ok_or("invalid pipe row")?;
        let (title, pad) = fields.split_once('|').ok_or("invalid pipe row")?;
        let active = self.0.as_ref().map(|stats| {
            let (active, peak) = if id == "neighbor" {
                (&stats.neighbor, &stats.peak_neighbor)
            } else {
                (&stats.large, &stats.peak_large)
            };
            let running = active.fetch_add(1, Ordering::Relaxed) + 1;
            peak.fetch_max(running, Ordering::Relaxed);
            active
        });
        let record = ExtractedRecord {
            value: Value::Object(
                [
                    ("id".into(), Value::String(id.into())),
                    ("title".into(), Value::String(title.into())),
                    ("pad".into(), Value::String(pad.into())),
                ]
                .into_iter()
                .collect(),
            ),
            text: Vec::new(),
        };
        if let Some(active) = active {
            active.fetch_sub(1, Ordering::Relaxed);
        }
        Ok(record)
    }

    fn stream_build_chunks(
        &self,
        input: &mut dyn Read,
        max_bytes: usize,
        max_ranges: usize,
        emit: &mut dyn FnMut(BuildChunk) -> Result<(), String>,
    ) -> Option<Result<(), String>> {
        Some((|| {
            let mut reader = BufReader::with_capacity(127, input);
            let mut line = Vec::new();
            let mut chunk = BuildChunk {
                bytes: Vec::new(),
                ranges: Vec::new(),
                file_offset: 0,
                preceding_lines: 0,
            };
            loop {
                line.clear();
                if reader
                    .read_until(b'\n', &mut line)
                    .map_err(|error| error.to_string())?
                    == 0
                {
                    break;
                }
                if !chunk.bytes.is_empty()
                    && (chunk.bytes.len() + line.len() > max_bytes
                        || chunk.ranges.len() == max_ranges)
                {
                    let next = BuildChunk {
                        bytes: Vec::new(),
                        ranges: Vec::new(),
                        file_offset: chunk.file_offset + chunk.bytes.len() as u64,
                        preceding_lines: chunk.preceding_lines + chunk.ranges.len() as u64,
                    };
                    emit(std::mem::replace(&mut chunk, next))?;
                }
                let start = chunk.bytes.len();
                chunk.bytes.extend_from_slice(&line);
                let end = chunk.bytes.len() - usize::from(line.last() == Some(&b'\n'));
                chunk.ranges.push(ByteRange {
                    start: start as u64,
                    end: end as u64,
                });
            }
            if !chunk.bytes.is_empty() {
                emit(chunk)?;
            }
            Ok(())
        })())
    }
}

fn main() -> Result<(), Box<dyn Error>> {
    let mut args = std::env::args().skip(1);
    let root = PathBuf::from(args.next().ok_or(
        "usage: bench_managed_file_scale ROOT [jsonl|registered|registered-many|registered-mixed] [WORKERS]",
    )?);
    let mode = args.next().unwrap_or_else(|| "jsonl".into());
    let registered = match mode.as_str() {
        "jsonl" => false,
        "registered" | "registered-many" | "registered-mixed" => true,
        _ => return Err("expected jsonl, registered, registered-many, or registered-mixed".into()),
    };
    let workers = args
        .next()
        .map(|value| value.parse::<NonZeroUsize>())
        .transpose()?;
    if mode == "registered-many" {
        return many_registered_files(root, workers);
    }
    let mixed = mode == "registered-mixed";
    let neighbor_bytes = b"neighbor|Leipzig|small\n";
    let relative = if registered {
        "data/large.pipe"
    } else {
        "data/large.jsonl"
    };
    let count = 100_000usize;
    std::fs::create_dir_all(root.join("data"))?;
    let path = root.join(relative);
    let mut output = BufWriter::new(std::fs::File::create(&path)?);
    let mut offset = 0u64;
    let mut last_range = (0u64, 0u64);
    let padding = "x".repeat(if registered { 283 } else { 256 });
    for number in 0..count {
        let line = if registered {
            format!("r{number:06}|Leipzig|{padding}\n")
        } else {
            format!("{{\"id\":\"r{number:06}\",\"title\":\"Leipzig\",\"pad\":\"{padding}\"}}\n")
        };
        let end = offset + line.len() as u64;
        last_range = (offset, end - 1);
        output.write_all(line.as_bytes())?;
        offset = end;
    }
    output.flush()?;
    drop(output);
    if mixed {
        std::fs::write(root.join("data/neighbor.pipe"), neighbor_bytes)?;
    }
    let decoder_activity = Arc::new(DecoderActivity::default());
    let mut registry = objekte_cli::loader::installed_sources();
    let format = if registered {
        registry.register(
            "scale-pipe",
            1,
            "pipe",
            Arc::new(PipeReader(Some(Arc::clone(&decoder_activity)))),
        )?;
        SourceFormat::registered("scale-pipe")
    } else {
        objekte_loader_jsonl::JSON_LINES
    };
    let mut definition = CollectionDefinition::folder("entries", "data", format);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    let options = RepositoryOptions {
        query_workers: 1,
        build_workers: workers,
        profile_build: true,
        segment_policy: SegmentPolicy {
            target_records: NonZeroUsize::new(4_096).unwrap(),
            max_files: NonZeroUsize::new(1).unwrap(),
        },
        ..RepositoryOptions::default()
    };
    let started = Instant::now();
    let task = Index::start_build(&root, vec![definition], options, registry)?;
    let index = task.wait_complete()?;
    let elapsed_ns = started.elapsed().as_nanos();
    let status = index.status()?;
    let view = index.view()?;
    let catalog = view.file_catalog().ok_or("catalog missing")?;
    let file = catalog
        .file_by_path(std::path::Path::new(relative))?
        .ok_or("file missing")?;
    let members = catalog.records_for_file(file.id)?;
    let last = view
        .get_record("entries", "r099999", ReturnMode::Locator)?
        .ok_or("last record missing")?;
    let observed = last
        .file
        .ok_or("last record has no file")?
        .range
        .ok_or("last range missing")?;
    let segments = view.segment_summaries();
    let mut segment_counts = segments
        .iter()
        .map(|segment| segment.active_records)
        .collect::<Vec<_>>();
    segment_counts.sort_unstable();
    let mut expected_segments = if mixed { vec![1] } else { Vec::new() };
    expected_segments.push(1696);
    expected_segments.extend([4096; 24]);
    let file_count = if mixed { 2 } else { 1 };
    let total_bytes = offset
        + if mixed {
            neighbor_bytes.len() as u64
        } else {
            0
        };
    let publications = view.revision().0 - IndexRevision::INITIAL.0 - 1;
    if mixed {
        let tiny = view
            .get_record("entries", "neighbor", ReturnMode::Locator)?
            .ok_or("neighbor missing")?;
        if tiny.source_range()
            != Some(ByteRange {
                start: 0,
                end: neighbor_bytes.len() as u64 - 1,
            })
        {
            return Err("incorrect small neighbor locator".into());
        }
    }
    let large_peak = decoder_activity.peak_large.load(Ordering::Relaxed);
    if mixed && view.startup_stats().build_workers > 1 && large_peak < 2 {
        return Err("large registered file did not overlap its own decoders".into());
    }
    if segment_counts != expected_segments
        || status.indexed_files != file_count
        || status.live_records != (count + file_count - 1) as u64
        || publications != file_count as u64
        || members.len() != count
        || (observed.start, observed.end) != last_range
        || status.build_source_reads != file_count as u64
        || status.build_source_bytes_read != total_bytes
    {
        return Err(format!(
            "large-file invariant failed: {status:?}, range {observed:?}, members {}",
            members.len()
        )
        .into());
    }
    let profile = view.startup_stats().build_profile.as_ref();
    println!(
        "{}",
        json!({
            "mode": mode,
            "records": count,
            "build_workers": view.startup_stats().build_workers,
            "peak_active_decoders": view.startup_stats().build_peak_active_jobs,
            "peak_completed_contributions": view.startup_stats().build_peak_ready_batches,
            "stream_peak": view.startup_stats().build_stream_peak,
            "segment_populations": segment_counts,
            "file_publications": publications,
            "large_file_active_decoders": large_peak,
            "neighbor_active_decoders": decoder_activity.peak_neighbor.load(Ordering::Relaxed),
            "private_index_family_bytes": view.estimated_index_bytes(),
            "file_bytes": offset,
            "elapsed_ns": elapsed_ns,
            "records_per_second": count as f64 * 1e9 / elapsed_ns as f64,
            "indexed_files": status.indexed_files,
            "live_records": status.live_records,
            "source_content_opens": status.build_source_reads,
            "source_bytes_read": status.build_source_bytes_read,
            "peak_input_buffer_bytes": profile.map(|profile| profile.work.input_buffer_peak_capacity_bytes),
            "peak_inflight_input_reservation_bytes": view.startup_stats().peak_build_admitted_bytes,
            "framing_chunks": view.startup_stats().build_framing_chunks,
            "framing_bytes": view.startup_stats().build_framing_bytes,
            "completed_segments": segments.len(),
            "searchable_batches": task.status()?.searchable_batches,
            "last_byte_start": observed.start,
            "last_byte_end": observed.end,
            "peak_rss_kib": rss_kib("VmHWM:"),
        })
    );
    std::fs::remove_dir_all(root)?;
    Ok(())
}

fn many_registered_files(
    root: PathBuf,
    workers: Option<NonZeroUsize>,
) -> Result<(), Box<dyn Error>> {
    let count = 8_192usize;
    let target = 64;
    std::fs::create_dir_all(root.join("data"))?;
    for number in 0..count {
        let line = format!("r{number:06}|Leipzig|sample\n");
        std::fs::write(root.join(format!("data/{number:06}.pipe")), line)?;
    }
    let mut registry = objekte_cli::loader::installed_sources();
    registry.register("scale-pipe", 1, "pipe", Arc::new(PipeReader::default()))?;
    let mut definition =
        CollectionDefinition::folder("entries", "data", SourceFormat::registered("scale-pipe"));
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    let target = NonZeroUsize::new(target).expect("nonzero policy");
    let options = RepositoryOptions {
        query_workers: 1,
        build_workers: workers,
        profile_build: true,
        segment_policy: SegmentPolicy {
            target_records: target,
            max_files: target,
        },
        ..RepositoryOptions::default()
    };
    let started = Instant::now();
    let task = Index::start_build(&root, vec![definition], options, registry)?;
    let index = task.wait_complete()?;
    let elapsed_ns = started.elapsed().as_nanos();
    let status = index.status()?;
    let view = index.view()?;
    let segments = view.segment_summaries();
    let mut populations = std::collections::BTreeMap::new();
    for segment in &segments {
        *populations.entry(segment.active_records).or_insert(0) += 1;
    }
    let catalogue = view.file_catalog().ok_or("catalogue missing")?;
    let sampled_path = format!("data/{:06}.pipe", count - 1);
    let file = catalogue
        .file_by_path(sampled_path.as_ref())?
        .ok_or("sampled file missing")?;
    let file_records = catalogue.records_for_file(file.id)?;
    let publications = view.revision().0 - IndexRevision::INITIAL.0 - 1;
    if status.indexed_files != count
        || status.live_records != count as u64
        || file_records.len() != 1
        || status.build_source_reads != count as u64
        || populations != [(target.get(), count / target.get())].into()
        || publications != (count / target.get()) as u64
    {
        return Err(format!("invalid many-file result: {status:?}, populations {populations:?}, publications {publications}").into());
    }
    println!(
        "{}",
        json!({
            "mode": "registered-many",
            "records": count,
            "indexed_files": status.indexed_files,
            "build_workers": view.startup_stats().build_workers,
            "elapsed_ns": elapsed_ns,
            "records_per_second": count as f64 * 1e9 / elapsed_ns as f64,
            "file_publications": publications,
            "source_content_opens": status.build_source_reads,
            "source_bytes_read": status.build_source_bytes_read,
            "completed_segments": segments.len(),
            "segment_population_histogram": populations,
            "stream_peak": view.startup_stats().build_stream_peak,
            "peak_rss_kib": rss_kib("VmHWM:"),
        })
    );
    std::fs::remove_dir_all(root)?;
    Ok(())
}
