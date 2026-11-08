//! Mutates its fixture root. Run only on an independent disposable copy.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use objekte_search_core::lifecycle::Index;
use objekte_search_core::lifecycle_mutation::{UpdateOptions, UpdateStrategy};
use objekte_search_core::lifecycle_repository::{CacheOpenPolicy, RepositoryOpenOptions};
use objekte_search_core::query::{Query, QueryOptions};
use objekte_search_core::repository::RepositoryOptions;
use objekte_search_core::storage::snapshot_path;
use objekte_search_core::workload::ReturnMode;

fn memory_kb(label: &str) -> u64 {
    fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find(|line| line.starts_with(label))
                .map(str::to_owned)
        })
        .and_then(|line| {
            line.split_whitespace()
                .nth(1)
                .and_then(|value| value.parse().ok())
        })
        .unwrap_or(0)
}

fn emit(label: &str, elapsed: Duration, index: &Index, detail: impl AsRef<str>) {
    let status = index.status().expect("index status");
    println!(
        "{label}\t{:.3}\t{}\t{}\t{}\t{}\t{}\t{}",
        elapsed.as_secs_f64() * 1000.0,
        memory_kb("VmRSS:"),
        memory_kb("VmHWM:"),
        status.revision.0,
        status.indexed_files,
        status.live_records,
        detail.as_ref().replace('\t', " ")
    );
}

fn count(index: &Index, collection: &str) -> Result<usize, Box<dyn Error>> {
    let query = Query {
        collections: vec![collection.into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 0,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    };
    Ok(index.view()?.search(&query)?.count.ok_or("missing count")?)
}

fn first_json(directory: &Path) -> Result<PathBuf, Box<dyn Error>> {
    fs::read_dir(directory)?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "json")
        })
        .min()
        .ok_or_else(|| format!("no JSON file in {}", directory.display()).into())
}

fn setup_small(root: &Path) -> Result<(), Box<dyn Error>> {
    if root.exists() {
        return Err("small fixture root must not exist yet".into());
    }
    fs::create_dir_all(root.join(".objekte/collections"))?;
    fs::create_dir_all(root.join("data/items"))?;
    fs::write(
        root.join(".objekte/collections/items.schema.json"),
        br#"{
      "type":"object", "properties":{"name":{"type":"string","x-index":true}},
      "x-objekte":{"name":"items","path":"data/items","format":"jsonl","id":"generated"}
    }"#,
    )?;
    fs::write(
        root.join("data/items/batch.jsonl"),
        b"{\"name\":\"Alpha\"}\n{\"name\":\"Beta\"}\n{\"name\":\"Gamma\"}\n",
    )?;
    Ok(())
}

fn run(mode: &str, root: &Path) -> Result<(), Box<dyn Error>> {
    if std::env::var("OBJEKTE_BENCH_MUTATE_COPY").as_deref() != Ok("1") {
        return Err("set OBJEKTE_BENCH_MUTATE_COPY=1 for a disposable fixture copy".into());
    }
    if !matches!(mode, "small" | "large") {
        return Err("mode must be small or large".into());
    }
    if mode == "small" {
        setup_small(root)?;
    }
    let root = root.canonicalize()?;
    let root = root.as_path();
    let (collection, source, add_path, move_path, added_bytes) = if mode == "small" {
        (
            "items",
            root.join("data/items/batch.jsonl"),
            root.join("data/items/added.jsonl"),
            root.join("data/items/moved.jsonl"),
            b"{\"name\":\"Delta\"}\n{\"name\":\"Epsilon\"}\n".to_vec(),
        )
    } else {
        let directory = root.join("data/contents");
        let source = first_json(&directory)?;
        let bytes = fs::read(&source)?;
        (
            "contents",
            source,
            directory.join("zz-lifecycle-perf-added.json"),
            directory.join("zz-lifecycle-perf-moved.json"),
            bytes,
        )
    };
    if add_path.exists() || move_path.exists() {
        return Err("benchmark fixture already has mutation paths; use a fresh copy".into());
    }
    println!("phase\tms\trss_kb\thwm_kb\trevision\tfiles\trecords\tdetail");
    let started = Instant::now();
    let index = Index::open_repository(
        root,
        RepositoryOpenOptions {
            cache_open: CacheOpenPolicy::Ignore,
            ..Default::default()
        },
    )?;
    emit(
        "build",
        started.elapsed(),
        &index,
        format!("collection_count={}", count(&index, collection)?),
    );
    let base_count = count(&index, collection)?;

    fs::write(&add_path, &added_bytes)?;
    let started = Instant::now();
    let added = index.add_file(&add_path)?;
    emit(
        "add",
        started.elapsed(),
        &index,
        format!(
            "strategy={:?};records={}",
            added.strategy, added.indexed_records
        ),
    );

    let original = fs::read(&source)?;
    let mut update1 = original.clone();
    update1.extend_from_slice(b" \n");
    fs::write(&source, &update1)?;
    let handle = index
        .view()?
        .file_handle_for_path(&source)
        .ok_or("source handle missing")?;
    let started = Instant::now();
    let appended = index.update_file(handle, &source)?;
    emit(
        "update_append",
        started.elapsed(),
        &index,
        format!(
            "strategy={:?};fallback={:?}",
            appended.strategy, appended.fallback_reason
        ),
    );

    let mut update2 = update1.clone();
    update2.extend_from_slice(b" \n");
    fs::write(&source, &update2)?;
    let started = Instant::now();
    let verified = index.update_file_with_options(
        appended.file,
        &source,
        UpdateOptions {
            old_content: Some(&update1),
            strategy: UpdateStrategy::PreferVerified,
        },
    )?;
    emit(
        "update_verified",
        started.elapsed(),
        &index,
        format!(
            "strategy={:?};fallback={:?}",
            verified.strategy, verified.fallback_reason
        ),
    );

    fs::rename(&add_path, &move_path)?;
    let started = Instant::now();
    let moved = index.move_file(added.file, &move_path)?;
    emit(
        "move",
        started.elapsed(),
        &index,
        format!(
            "strategy={:?};records={}",
            moved.strategy, moved.indexed_records
        ),
    );

    let old_move = fs::read(&move_path)?;
    fs::remove_file(&move_path)?;
    let started = Instant::now();
    let deleted = index.delete_file_with_old_content(moved.file, &old_move)?;
    emit(
        "delete_verified",
        started.elapsed(),
        &index,
        format!(
            "strategy={:?};fallback={:?}",
            deleted.deletion.strategy, deleted.fallback_reason
        ),
    );

    let started = Instant::now();
    let compacted = index.compact()?;
    emit(
        "compact",
        started.elapsed(),
        &index,
        format!(
            "retired_slots_removed={};slots_after={}",
            compacted.retired_slots_removed, compacted.slots_after
        ),
    );
    let count_after = count(&index, collection)?;
    if count_after != base_count {
        return Err(format!("count changed: {base_count} -> {count_after}").into());
    }

    let cache = snapshot_path(root, RepositoryOptions::default().bigram_alphabet);
    let started = Instant::now();
    let (revision, stats) = index.flush_cache_with_stats(&cache)?;
    emit(
        "flush",
        started.elapsed(),
        &index,
        format!(
            "revision={};bytes={};stats_ms={:.3}",
            revision.0,
            stats.bytes_written,
            stats.total_ns as f64 / 1_000_000.0
        ),
    );
    drop(index);

    let started = Instant::now();
    let loaded = Index::open_repository(root, RepositoryOpenOptions::default())?;
    let loaded_count = count(&loaded, collection)?;
    emit(
        "decode_to_searchable",
        started.elapsed(),
        &loaded,
        format!("count={loaded_count}"),
    );
    if loaded_count != base_count {
        return Err("restored count mismatch".into());
    }
    let started = Instant::now();
    let reconciliation = loaded
        .wait_for_reconciliation()?
        .ok_or("no reconciliation task")?;
    emit(
        "reconcile_no_change",
        started.elapsed(),
        &loaded,
        format!(
            "examined={};unchanged={};issues={}",
            reconciliation.examined,
            reconciliation.unchanged,
            reconciliation.issues.len()
        ),
    );
    Ok(())
}

fn main() {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: lifecycle-perf small|large ROOT");
        std::process::exit(2);
    }
    if let Err(error) = run(&args[1], Path::new(&args[2])) {
        eprintln!("benchmark error: {error}");
        std::process::exit(1);
    }
}
