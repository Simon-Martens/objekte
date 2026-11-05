//! Paired first-use and repeated-query measurements for verified slot caching.

use objekte_core::{BigramEncoding, JsonQuery, JsonQueryBackend, Options, Repository, TextQuery};
use serde_json::json;
use std::{env, path::PathBuf, time::Instant};


#[cfg(feature = "allocation-profile")]
mod allocation {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::sync::atomic::{AtomicIsize, AtomicUsize, Ordering};


    pub(super) struct CountingAllocator;


    static ALLOCS: AtomicUsize = AtomicUsize::new(0);
    static BYTES: AtomicUsize = AtomicUsize::new(0);
    static LIVE: AtomicIsize = AtomicIsize::new(0);
    static PEAK: AtomicIsize = AtomicIsize::new(0);

    #[global_allocator]
    static ALLOCATOR: CountingAllocator = CountingAllocator;


    unsafe impl GlobalAlloc for CountingAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc(layout) };
            if !pointer.is_null() {
                ALLOCS.fetch_add(1, Ordering::Relaxed);
                BYTES.fetch_add(layout.size(), Ordering::Relaxed);
                let live = LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed)
                    + layout.size() as isize;
                PEAK.fetch_max(live, Ordering::Relaxed);
            }
            pointer
        }

        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            let pointer = unsafe { System.alloc_zeroed(layout) };
            if !pointer.is_null() {
                ALLOCS.fetch_add(1, Ordering::Relaxed);
                BYTES.fetch_add(layout.size(), Ordering::Relaxed);
                let live = LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed)
                    + layout.size() as isize;
                PEAK.fetch_max(live, Ordering::Relaxed);
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


    pub(super) fn begin() -> (usize, usize, isize) {
        let live = LIVE.load(Ordering::Relaxed);
        PEAK.store(live, Ordering::Relaxed);
        (
            ALLOCS.load(Ordering::Relaxed),
            BYTES.load(Ordering::Relaxed),
            live,
        )
    }

    pub(super) fn end(before: (usize, usize, isize)) -> (usize, usize, isize) {
        (
            ALLOCS.load(Ordering::Relaxed) - before.0,
            BYTES.load(Ordering::Relaxed) - before.1,
            PEAK.load(Ordering::Relaxed) - before.2,
        )
    }
}


struct Step {
    scenario: &'static str,
    label: &'static str,
    filter: &'static str,
    offset: usize,
    count: bool,
}


const STEPS: &[Step] = &[
    Step {
        scenario: "incremental",
        label: "year",
        filter: "year = 1800",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "incremental",
        label: "add_text",
        filter: "year = 1800 && annotation ~ 'Mond'",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "incremental",
        label: "add_place",
        filter: "year = 1800 && annotation ~ 'Mond' && place_statement ~ 'Berlin'",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "incremental",
        label: "replace_place",
        filter: "year = 1800 && annotation ~ 'Mond' && preferred_title ~ 'a'",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "incremental",
        label: "remove_year",
        filter: "annotation ~ 'Mond' && preferred_title ~ 'a'",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "incremental",
        label: "repeat",
        filter: "year = 1800 && annotation ~ 'Mond'",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "pagination",
        label: "first",
        filter: "created < updated",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "pagination",
        label: "second",
        filter: "created < updated",
        offset: 25,
        count: true,
    },
    Step {
        scenario: "pagination",
        label: "page_11",
        filter: "created < updated",
        offset: 250,
        count: true,
    },
    Step {
        scenario: "pagination",
        label: "page_101",
        filter: "created < updated",
        offset: 2500,
        count: true,
    },
    Step {
        scenario: "pagination",
        label: "second_again",
        filter: "created < updated",
        offset: 25,
        count: true,
    },
    Step {
        scenario: "sorting",
        label: "ascending",
        filter: "created < updated SORT BY preferred_title ASC",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "sorting",
        label: "descending",
        filter: "created < updated SORT BY preferred_title DESC",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "sorting",
        label: "ascending_again",
        filter: "created < updated SORT BY preferred_title ASC",
        offset: 25,
        count: true,
    },
    Step {
        scenario: "miss",
        label: "new_term",
        filter: "annotation ~ 'Schiller'",
        offset: 0,
        count: true,
    },
    Step {
        scenario: "miss",
        label: "another_term",
        filter: "preferred_title ~ 'Taschen'",
        offset: 0,
        count: true,
    },
];

fn arg(name: &str, default: &str) -> String {
    let mut args = env::args().skip(1);
    while let Some(key) = args.next() {
        if key == name {
            return args.next().expect("option requires a value");
        }
    }
    default.into()
}

fn measure<T>(
    work: impl FnOnce() -> objekte_core::Result<T>,
) -> objekte_core::Result<(T, f64, (usize, usize, isize))> {
    #[cfg(feature = "allocation-profile")]
    let allocation_before = allocation::begin();
    let started = Instant::now();
    let result = work()?;
    let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
    #[cfg(feature = "allocation-profile")]
    let allocations = allocation::end(allocation_before);
    #[cfg(not(feature = "allocation-profile"))]
    let allocations = (0, 0, 0);
    Ok((result, elapsed_ms, allocations))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = PathBuf::from(arg("--repo", "examples/musenalm"));
    let budget_mib: usize = arg("--cache-mib", "64").parse()?;
    let run: usize = arg("--run", "0").parse()?;
    let mut options = Options::new();
    options.query_cache_bytes = budget_mib.saturating_mul(1024 * 1024);
    options.bigram_encoding =
        BigramEncoding::parse(&arg("--encoding", "ascii")).ok_or("invalid bigram encoding")?;
    let mut repo = Repository::load_repository(&root, options)?;
    let collection = arg("--collection", "contents");
    repo.warm_bigram_index(&collection)?;
    let text_pattern = arg("--text-pattern", "");
    println!(
        "{}",
        json!({"type":"setup", "run":run, "cache_mib":budget_mib, "repo":root})
    );
    if !text_pattern.is_empty() {
        repo.warm_text_index(&collection)?;
        for repetition in 0..2 {
            let request = TextQuery {
                pattern: text_pattern.clone(),
                collections: Some(vec![collection.clone()]),
                count: true,
                limit: 25,
                ..Default::default()
            };
            let (result, elapsed_ms, allocations) = measure(|| repo.search_text(&request))?;
            println!(
                "{}",
                json!({
                    "type":"text_step", "run":run, "cache_mib":budget_mib,
                    "collection":collection, "pattern":text_pattern, "repetition":repetition,
                    "ms":elapsed_ms,
                    "total":result.total, "truncated":result.truncated,
                    "ids":result.hits.iter().map(|hit| &hit.record.id).collect::<Vec<_>>(),
                    "scans":result.metrics.grep_scans,
                    "parses":result.metrics.parses,
                    "predicate_cache_hits":result.metrics.predicate_cache_hits,
                    "ordered_result_hits":result.metrics.ordered_result_hits,
                    "cache_bytes":repo.query_cache_bytes(),
                    "allocations":allocations.0,
                    "allocated_bytes":allocations.1,
                    "peak_live_delta_bytes":allocations.2
                })
            );
        }
        return Ok(());
    }
    for step in STEPS {
        let request = JsonQuery {
            filter: step.filter.into(),
            collections: Some(vec!["contents".into()]),
            count: step.count,
            offset: step.offset,
            limit: 25,
            backend: JsonQueryBackend::Bigram,
            ..Default::default()
        };
        let (result, elapsed_ms, allocations) = measure(|| repo.query_json(&request))?;
        println!(
            "{}",
            json!({
                "type":"step", "run":run, "cache_mib":budget_mib,
                "scenario":step.scenario, "label":step.label,
                "filter":step.filter, "offset":step.offset,
                "ms":elapsed_ms,
                "total":result.total, "truncated":result.truncated,
                "ids":result.records.iter().map(|record| &record.id).collect::<Vec<_>>(),
                "source_reads":result.metrics.source_reads,
                "scans":result.metrics.grep_scans,
                "parses":result.metrics.parses,
                "predicate_cache_hits":result.metrics.predicate_cache_hits,
                "ordered_result_hits":result.metrics.ordered_result_hits,
                "partial_evaluation_hits":result.metrics.partial_evaluation_hits,
                "cache_evictions":result.metrics.cache_evictions,
                "cache_bytes":repo.query_cache_bytes(),
                "allocations":allocations.0,
                "allocated_bytes":allocations.1,
                "peak_live_delta_bytes":allocations.2
            })
        );
    }
    Ok(())
}
