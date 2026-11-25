//! Compare the shipped JSONL registration on large and small files.

use std::io::{BufWriter, Write};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::time::Instant;

use objekte_search_core::index::segment::SegmentPolicy;
use objekte_search_core::lifecycle::{Index, initial::test_hooks::track_file_activity};
use objekte_search_core::repository::configuration::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::{SourceFormat, registry::SourceRegistry};

fn write_fixture(
    root: &Path,
    files: usize,
    records: usize,
    padding_size: usize,
) -> std::io::Result<u64> {
    std::fs::create_dir(root)?;
    let mut bytes = 0;
    let padding = "x".repeat(padding_size);
    for file in 0..files {
        let mut out = BufWriter::new(std::fs::File::create(
            root.join(format!("{file:05}.jsonl")),
        )?);
        for number in 0..records {
            let line = format!("{{\"id\":\"{file:05}-{number:05}\",\"title\":\"{padding}\"}}\n");
            out.write_all(line.as_bytes())?;
            bytes += line.len() as u64;
        }
        out.flush()?;
    }
    Ok(bytes)
}

fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    status.lines().find_map(|line| {
        line.strip_prefix("VmHWM:")?
            .split_whitespace()
            .next()?
            .parse()
            .ok()
    })
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let usage = "usage: bench_jsonl_file_concurrency ROOT one|several|small WORKERS [--prepare]";
    let root = PathBuf::from(args.next().ok_or(usage)?);
    let workload = args.next().ok_or(usage)?;
    let workers = args.next().ok_or(usage)?.parse::<NonZeroUsize>()?;
    let (files, records, padding, max_files) = match workload.as_str() {
        "one" => (1, 100_000, 256, 1),
        "several" => (5, 36_000, 256, 1),
        "small" => (4_096, 4, 32, 64),
        _ => return Err(usage.into()),
    };
    if args.next().as_deref() == Some("--prepare") {
        write_fixture(&root, files, records, padding)?;
        return Ok(());
    }
    let root = root.canonicalize()?;
    let bytes: u64 = std::fs::read_dir(&root)?
        .map(|entry| entry?.metadata().map(|meta| meta.len()))
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .sum();
    let mut sources = SourceRegistry::default();
    objekte_loader_jsonl::register(&mut sources)?;
    let collection = CollectionDefinition::folder("lines", "", SourceFormat::JsonLines);
    let options = RepositoryOptions {
        build_workers: Some(workers),
        segment_policy: SegmentPolicy {
            target_records: NonZeroUsize::new(4096).unwrap(),
            max_files: NonZeroUsize::new(max_files).unwrap(),
        },
        ..Default::default()
    };
    let probe = track_file_activity(root.clone());
    let started = Instant::now();
    let index = Index::start_build(&root, vec![collection], options, sources)?.wait_complete()?;
    let build_ns = started.elapsed().as_nanos();
    let status = index.status()?;
    let view = index.view()?;
    let stats = view.startup_stats();
    assert_eq!(status.live_records, (files * records) as u64);
    assert_eq!(status.build_source_reads, files as u64);
    assert_eq!(status.build_source_bytes_read, bytes);
    // Normal batches decode at most one file per active file job. Streaming
    // decoders instead publish their individual jobs in the stream counters.
    println!(
        "{}",
        serde_json::json!({
            "workload": workload,
            "workers": stats.build_workers,
            "build_ns": build_ns as u64,
            "peak_rss_kib": peak_rss_kib(),
            "peak_active_files": stats.build_stream_peak.active_framers.max(probe.peak() as u64),
            "peak_decoder_jobs": stats.build_stream_peak.active_decoders.max(probe.peak() as u64),
            "content_opens": status.build_source_reads,
            "content_bytes": status.build_source_bytes_read,
        })
    );
    Ok(())
}
