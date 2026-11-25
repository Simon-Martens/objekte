//! One large managed JSON Lines file through the public initial-build API.

use std::error::Error;
use std::io::{BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::index::segment::SegmentPolicy;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::repository::configuration::{
    CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::source::{SourceFormat, registry::SourceRegistry};
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

fn main() -> Result<(), Box<dyn Error>> {
    let root = PathBuf::from(
        std::env::args()
            .nth(1)
            .ok_or("usage: bench_managed_file_scale ROOT")?,
    );
    let count = 100_000usize;
    std::fs::create_dir_all(root.join("data"))?;
    let path = root.join("data/large.jsonl");
    let mut output = BufWriter::new(std::fs::File::create(&path)?);
    let mut offset = 0u64;
    let mut last_range = (0u64, 0u64);
    let padding = "x".repeat(256);
    for number in 0..count {
        let line =
            format!("{{\"id\":\"r{number:06}\",\"title\":\"Leipzig\",\"pad\":\"{padding}\"}}\n");
        let end = offset + line.len() as u64;
        last_range = (offset, end - 1);
        output.write_all(line.as_bytes())?;
        offset = end;
    }
    output.flush()?;
    drop(output);
    let mut definition = CollectionDefinition::folder("entries", "data", SourceFormat::JsonLines);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    let options = RepositoryOptions {
        query_workers: 1,
        profile_build: true,
        segment_policy: SegmentPolicy {
            target_records: NonZeroUsize::new(4_096).unwrap(),
            max_files: NonZeroUsize::new(1).unwrap(),
        },
        ..RepositoryOptions::default()
    };
    let started = Instant::now();
    let task = Index::start_build(&root, vec![definition], options, SourceRegistry::builtin())?;
    let index = task.wait_complete()?;
    let elapsed_ns = started.elapsed().as_nanos();
    let status = index.status()?;
    let view = index.view()?;
    let catalog = view.file_catalog().ok_or("catalog missing")?;
    let file = catalog
        .file_by_path(std::path::Path::new("data/large.jsonl"))
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
    if status.indexed_files != 1
        || status.live_records != count as u64
        || members.len() != count
        || (observed.start, observed.end) != last_range
        || status.build_source_reads != 1
        || status.build_source_bytes_read != offset
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
            "records": count,
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
            "completed_segments": view.segment_summaries().len(),
            "searchable_batches": task.status()?.searchable_batches,
            "last_byte_start": observed.start,
            "last_byte_end": observed.end,
            "peak_rss_kib": rss_kib("VmHWM:"),
        })
    );
    std::fs::remove_dir_all(root)?;
    Ok(())
}
