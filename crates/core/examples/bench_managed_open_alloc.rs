//! Instrument one source-free managed open, separately from timed release runs.

#[path = "support/musenalm.rs"]
mod musenalm;

use std::alloc::{GlobalAlloc, Layout, System};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Instant;

use objekte_search_core::index_storage::{
    IndexResidency, IndexStorageLocation, IndexStoragePolicy,
};
use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::{RepositoryOptions, SnapshotMode};
use objekte_search_core::source::SourceRegistry;
use objekte_search_core::workload::BigramAlphabet;

struct CountingAllocator;

static COUNTING: AtomicBool = AtomicBool::new(false);
static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() && COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        pointer
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        let next = unsafe { System.realloc(pointer, layout, new_size) };
        if !next.is_null() && COUNTING.load(Ordering::Relaxed) {
            ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
            ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        }
        next
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut root = None;
    let mut store = None;
    let mut alphabet = BigramAlphabet::Ascii;
    let mut resident = false;
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--root" => root = Some(PathBuf::from(args.next().ok_or("--root needs a path")?)),
            "--store" => store = Some(PathBuf::from(args.next().ok_or("--store needs a path")?)),
            "--alphabet" => {
                alphabet = match args.next().ok_or("--alphabet needs a value")?.as_str() {
                    "ascii" => BigramAlphabet::Ascii,
                    "unicode" => BigramAlphabet::Unicode,
                    other => return Err(format!("unknown alphabet {other}")),
                }
            }
            "--resident" => resident = true,
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let root = root.ok_or("--root is required")?;
    let store = store.ok_or("--store is required")?;
    let definitions = musenalm::load_extended_definitions(&root)?;
    let options = RepositoryOptions {
        bigram_alphabet: alphabet,
        snapshot_mode: SnapshotMode::Disabled,
        query_workers: 32,
        ..RepositoryOptions::default()
    };
    let policy = IndexStoragePolicy {
        location: IndexStorageLocation::CustomDirectory(store),
        residency: if resident { IndexResidency::FullyResident } else { IndexResidency::OnDemand },
    };
    ALLOCATIONS.store(0, Ordering::Relaxed);
    ALLOCATED_BYTES.store(0, Ordering::Relaxed);
    COUNTING.store(true, Ordering::SeqCst);
    let started = Instant::now();
    let index = Index::open_with_storage(root, definitions, options, policy, SourceRegistry::builtin())
        .map_err(|error| error.to_string());
    let elapsed_ns = started.elapsed().as_nanos();
    COUNTING.store(false, Ordering::SeqCst);
    let index = index?;
    let status = index.status().map_err(|error| error.to_string())?;
    let timings = index.timings_ms().map_err(|error| error.to_string())?;
    if status.build_source_reads != 0 || status.build_parses != 0 {
        return Err("managed open rebuilt from sources".into());
    }
    println!("{}", serde_json::json!({
        "alphabet": alphabet,
        "resident": resident,
        "open_ns": elapsed_ns,
        "allocation_calls": ALLOCATIONS.load(Ordering::Relaxed),
        "gross_allocated_bytes": ALLOCATED_BYTES.load(Ordering::Relaxed),
        "metadata_ms": timings.metadata,
        "full_available_ms": timings.full_available,
        "source_reads": status.build_source_reads,
        "build_parses": status.build_parses,
    }));
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("managed open allocation benchmark: {error}");
        std::process::exit(1);
    }
}
