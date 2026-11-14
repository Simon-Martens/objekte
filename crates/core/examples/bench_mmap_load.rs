//! Warm and cold mapped-load latency on a synthetic JSONL collection.
//!
//! Cold runs evict each store file from the page cache with POSIX
//! `fadvise(DONTNEED)` immediately before Load. The measurement is still a
//! process-local approximation of a cold filesystem cache.

use std::path::{Path, PathBuf};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use objekte_search_core::SaveOptions;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::repository::{CollectionDefinition, RepositoryOptions};
use objekte_search_core::source::{SourceFormat, SourceRegistry};

fn temporary(label: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "objekte-{label}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn evict(path: &Path) {
    if path.is_dir() {
        for entry in std::fs::read_dir(path).unwrap().flatten() {
            evict(&entry.path());
        }
        return;
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::fd::AsRawFd;
        if let Ok(file) = std::fs::File::open(path) {
            // SAFETY: the descriptor is valid for this call. DONTNEED only
            // advises the kernel and does not alter file contents.
            unsafe {
                libc::posix_fadvise(file.as_raw_fd(), 0, 0, libc::POSIX_FADV_DONTNEED);
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
    }
}

fn load(directory: &Path, definitions: &[CollectionDefinition], options: RepositoryOptions) -> f64 {
    let started = Instant::now();
    let index = Index::load_from(
        directory,
        definitions.to_vec(),
        options,
        SourceRegistry::builtin(),
    )
    .unwrap();
    let elapsed = started.elapsed().as_secs_f64() * 1000.0;
    drop(index);
    elapsed
}

fn main() {
    let records = std::env::var("OBJEKTE_STORAGE_RECORDS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(20_000);
    let repetitions = std::env::var("OBJEKTE_STORAGE_REPS")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(5)
        .max(1);
    let root = temporary("mmap-bench-source");
    let data = root.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let mut text = String::new();
    for index in 0..records {
        text.push_str(&format!(
            "{{\"id\":\"r{index:06}\",\"text\":\"value {index}\",\"year\":{}}}\n",
            2000 + index % 30
        ));
    }
    std::fs::write(data.join("records.jsonl"), text).unwrap();
    let definitions = vec![CollectionDefinition::folder(
        "records",
        "data",
        SourceFormat::JsonLines,
    )];
    let options = RepositoryOptions::default();
    let index = Index::build(&root, definitions.clone(), options).unwrap();
    let store = temporary("mmap-bench-store");
    index.save_to(&store, SaveOptions::default()).unwrap();
    drop(index);

    // One discarded load brings the mapped object headers and allocator state
    // into the same steady condition as the measured runs.
    let warmup_ms = load(&store, &definitions, options);
    let warm = (0..repetitions)
        .map(|_| load(&store, &definitions, options))
        .collect::<Vec<_>>();
    let cold = (0..repetitions)
        .map(|_| {
            evict(&store);
            load(&store, &definitions, options)
        })
        .collect::<Vec<_>>();
    let mean = |values: &[f64]| values.iter().sum::<f64>() / values.len() as f64;
    println!(
        "{}",
        serde_json::json!({
            "records": records,
            "repetitions": repetitions,
            "warmup_ms": warmup_ms,
            "warm_ms": warm,
            "cold_ms": cold,
            "warm_mean_ms": mean(&warm),
            "cold_mean_ms": mean(&cold),
            "cold_slowdown_over_warm": mean(&cold) / mean(&warm),
        })
    );
    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(store);
}
