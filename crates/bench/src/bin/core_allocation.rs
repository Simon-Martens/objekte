//! Count process allocations around one core operation in a release build.

use objekte_core::{
    CacheMode, Filter, JsonQuery, JsonQueryBackend, Operator, Options, Repository, TextMode,
    TextQuery, ViewRequest, parse_sort_terms,
};
use serde_json::{Value, json};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    env, fs,
    path::PathBuf,
    sync::atomic::{AtomicIsize, AtomicUsize, Ordering},
    time::Instant,
};


/// Process-wide byte accounting for one measured operation at a time.
struct CountingAllocator;


static ALLOCS: AtomicUsize = AtomicUsize::new(0);
static BYTES: AtomicUsize = AtomicUsize::new(0);
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;


impl CountingAllocator {
    fn record(&self, bytes: usize) {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(bytes, Ordering::Relaxed);
        let live = LIVE.fetch_add(bytes as isize, Ordering::Relaxed) + bytes as isize;
        PEAK.fetch_max(live, Ordering::Relaxed);
    }
}


unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            self.record(layout.size());
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            self.record(layout.size());
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        unsafe { System.dealloc(pointer, layout) };
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let moved = unsafe { System.realloc(pointer, layout, size) };
        if !moved.is_null() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(size, Ordering::Relaxed);
            let delta = size as isize - layout.size() as isize;
            let live = LIVE.fetch_add(delta, Ordering::Relaxed) + delta;
            PEAK.fetch_max(live, Ordering::Relaxed);
        }
        moved
    }
}


/// Timing and allocation deltas, with the live-heap high-water mark.
struct Sample {
    wall_ms: f64,
    allocations: usize,
    allocated_bytes: usize,
    peak_live_bytes: isize,
    live_before_bytes: isize,
    rss_before_kib: usize,
    rss_after_kib: usize,
    rss_high_water_kib: usize,
}


fn rss(key: &str) -> usize {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix(key)
                    .and_then(|rest| rest.split_whitespace().next())
                    .and_then(|number| number.parse().ok())
            })
        })
        .unwrap_or(0)
}

fn measure<T>(work: impl FnOnce() -> objekte_core::Result<T>) -> objekte_core::Result<(T, Sample)> {
    let before_allocs = ALLOCS.load(Ordering::Relaxed);
    let before_bytes = BYTES.load(Ordering::Relaxed);
    let live_before = LIVE.load(Ordering::Relaxed);
    let rss_before = rss("VmRSS:");
    PEAK.store(live_before, Ordering::Relaxed);
    let started = Instant::now();
    let result = work()?;
    let wall_ms = started.elapsed().as_secs_f64() * 1_000.0;
    let sample = Sample {
        wall_ms,
        allocations: ALLOCS.load(Ordering::Relaxed) - before_allocs,
        allocated_bytes: BYTES.load(Ordering::Relaxed) - before_bytes,
        peak_live_bytes: PEAK.load(Ordering::Relaxed),
        live_before_bytes: live_before,
        rss_before_kib: rss_before,
        rss_after_kib: rss("VmRSS:"),
        rss_high_water_kib: rss("VmHWM:"),
    };
    Ok((result, sample))
}

fn arg(name: &str, default: &str) -> String {
    let mut args = env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == name {
            return args.next().unwrap_or_default();
        }
    }
    default.into()
}

fn print(case: &str, sample: Sample, signature: Value) {
    println!(
        "{}",
        json!({
            "case":case,
            "wall_ms":sample.wall_ms,
            "allocations":sample.allocations,
            "allocated_bytes":sample.allocated_bytes,
            "peak_live_bytes":sample.peak_live_bytes,
            "live_before_bytes":sample.live_before_bytes,
            "peak_live_delta_bytes":sample.peak_live_bytes - sample.live_before_bytes,
            "rss_before_kib":sample.rss_before_kib,
            "rss_after_kib":sample.rss_after_kib,
            "rss_high_water_kib":sample.rss_high_water_kib,
            "signature":signature,
        })
    );
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let case = arg("--case", "view-indexed");
    let root = PathBuf::from(arg("--repo", "/tmp/objekte-alloc-fixture-v2"));
    let collection = arg("--collection", "items");
    let filter = arg("--filter", "category = 'all'");
    let pattern = arg("--pattern", "Äpfel");
    let sorts = parse_sort_terms(&arg("--sort", "title ASC && rank DESC"))?;
    let mut options = Options::new();
    options.total_limit = 100_000;
    if case == "view-direct" {
        options.cache = CacheMode::Disabled;
    }
    let mut repo = Repository::load_repository(&root, options.clone())?;
    if case != "cold-build" && case != "snapshot-restore" && case != "view-direct" {
        repo.warm_bigram_index(&collection)?;
    }
    let view = ViewRequest {
        filters: vec![Filter {
            field: "category".into(),
            operator: Operator::Eq,
            value: json!("all"),
        }],
        sort: Some(sorts[0].clone()),
        sub_sorts: Some(sorts[1..].to_vec()),
        limit: Some(25),
        count: case.ends_with("count") || case == "view-indexed" || case == "view-direct",
        ..Default::default()
    };
    match case.as_str() {
        "view-indexed" | "view-fallback" | "view-fallback-count" | "view-direct" => {
            let request = if case == "view-indexed" {
                ViewRequest {
                    sort: Some(parse_sort_terms("rank ASC")?[0].clone()),
                    sub_sorts: Some(Vec::new()),
                    ..view
                }
            } else {
                view
            };
            let (result, sample) = measure(|| repo.open_view(&collection, &request))?;
            print(
                &case,
                sample,
                json!({
                    "ids":result.rows.iter().map(|row| &row.id).collect::<Vec<_>>(),
                    "total":result.total,
                    "states":result.states.len(),
                    "reads":result.metrics.source_reads,
                    "plan":result.query_plan,
                }),
            );
        }
        "json-sorted" | "json-updates" => {
            let request = JsonQuery {
                filter,
                collections: Some(vec![collection.clone()]),
                sort: sorts,
                limit: 25,
                count: true,
                backend: JsonQueryBackend::Bigram,
                ..Default::default()
            };
            if case == "json-updates" {
                let (result, sample) = measure(|| {
                    let mut pages = 0;
                    let summary = repo.query_json_stream_updates(&request, |_| {
                        pages += 1;
                        Ok(())
                    })?;
                    Ok((summary, pages))
                })?;
                print(
                    &case,
                    sample,
                    json!({"total":result.0.total,"pages":result.1}),
                );
            } else {
                let (result, sample) = measure(|| repo.query_json(&request))?;
                print(
                    &case,
                    sample,
                    json!({
                        "ids":result.records.iter().map(|row| &row.id).collect::<Vec<_>>(),
                        "total":result.total,
                        "truncated":result.truncated,
                        "reads":result.metrics.source_reads,
                        "parses":result.metrics.parses,
                    }),
                );
            }
        }
        "text-sorted" | "text-updates" => {
            let request = TextQuery {
                pattern,
                mode: TextMode::Word,
                collections: Some(vec![collection.clone()]),
                sort: sorts,
                limit: 25,
                count: true,
                ..Default::default()
            };
            if case == "text-updates" {
                let (result, sample) = measure(|| {
                    let mut pages = 0;
                    let summary = repo.search_text_stream_updates(&request, |_| {
                        pages += 1;
                        Ok(())
                    })?;
                    Ok((summary, pages))
                })?;
                print(
                    &case,
                    sample,
                    json!({"total":result.0.total,"pages":result.1}),
                );
            } else {
                let (result, sample) = measure(|| repo.search_text(&request))?;
                print(
                    &case,
                    sample,
                    json!({
                        "ids":result.hits.iter().map(|hit| &hit.record.id).collect::<Vec<_>>(),
                        "total":result.total,
                        "truncated":result.truncated,
                        "reads":result.metrics.source_reads,
                        "parses":result.metrics.parses,
                    }),
                );
            }
        }
        "cold-build" => {
            let (metrics, sample) = measure(|| repo.warm_bigram_index(&collection))?;
            print(
                &case,
                sample,
                json!({"reads":metrics.source_reads,"parses":metrics.parses}),
            );
        }
        "rebuild" => {
            let (metrics, sample) = measure(|| repo.rebuild_bigram_index(&collection))?;
            print(
                &case,
                sample,
                json!({"reads":metrics.source_reads,"parses":metrics.parses}),
            );
        }
        "snapshot-save" => {
            let (path, sample) = measure(|| repo.save_bigram_cache(&collection))?;
            let sidecar = path.with_extension("secondary");
            print(
                &case,
                sample,
                json!({
                    "base_bytes":fs::metadata(path)?.len(),
                    "secondary_bytes":fs::metadata(sidecar)?.len(),
                }),
            );
        }
        "snapshot-restore" => {
            repo.warm_bigram_index(&collection)?;
            repo.save_bigram_cache(&collection)?;
            drop(repo);
            let ((mut restored, warm_metrics), sample) = measure(|| {
                let mut repository = Repository::load_repository(&root, options)?;
                let warm_metrics = repository.warm_bigram_index(&collection)?;
                Ok((repository, warm_metrics))
            })?;
            let result = restored.query_json(&JsonQuery {
                filter,
                collections: Some(vec![collection]),
                limit: 1,
                ..Default::default()
            })?;
            print(
                &case,
                sample,
                json!({
                    "first_id":result.records.first().map(|row| &row.id),
                    "snapshot_loads":warm_metrics.bigram_snapshot_loads,
                    "secondary_loads":warm_metrics.secondary_snapshot_loads,
                }),
            );
        }
        _ => return Err(format!("unknown case {case}").into()),
    }
    Ok(())
}
