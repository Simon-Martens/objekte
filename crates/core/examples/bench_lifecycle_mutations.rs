//! Mutates its fixture root. Run only on an independent disposable copy.

use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use objekte_search_core::lifecycle::Index;
use objekte_search_core::lifecycle_mutation::{UpdateOptions, UpdateStrategy};
use objekte_search_core::lifecycle_repository::{CacheOpenPolicy, RepositoryOpenOptions};
use objekte_search_core::model::LogicalType;
use objekte_search_core::query::{Query, QueryOptions};
use objekte_search_core::reference::FieldDefinition;
use objekte_search_core::repository::{CollectionDefinition, IdentityPolicy, RepositoryOptions};
use objekte_search_core::source::SourceFormat;
use objekte_search_core::storage::snapshot_path;
use objekte_search_core::workload::ReturnMode;

struct QueryProbe {
    times_ns: Vec<u64>,
    errors: usize,
}

impl QueryProbe {
    fn detail(mut self) -> String {
        self.times_ns.sort_unstable();
        let n = self.times_ns.len();
        if n == 0 {
            return format!("probe_samples=0;probe_errors={}", self.errors);
        }
        let ms = |i: usize| self.times_ns[i] as f64 / 1_000_000.0;
        format!(
            "probe_samples={n};probe_p50_ms={:.4};probe_p95_ms={:.4};probe_p99_ms={:.4};probe_max_ms={:.4};probe_errors={}",
            ms(n / 2),
            ms((n - 1) * 95 / 100),
            ms((n - 1) * 99 / 100),
            ms(n - 1),
            self.errors
        )
    }
}

fn maintenance_query() -> Query {
    Query {
        collections: vec!["agents".into()],
        fts: None,
        filter: None,
        options: QueryOptions {
            sort: Vec::new(),
            limit: 50,
            offset: 500,
            count: false,
            return_mode: ReturnMode::Locator,
            evidence: false,
        },
    }
}

fn while_querying<T>(
    index: &Index,
    query: &Query,
    action: impl FnOnce() -> Result<T, Box<dyn Error>>,
) -> Result<(T, Duration, QueryProbe), Box<dyn Error>> {
    let owner = index.clone();
    let query = query.clone();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let (ready_send, ready_recv) = mpsc::channel();
    let worker = thread::spawn(move || {
        let warm = owner
            .view()
            .map_err(|error| error.to_string())
            .and_then(|view| view.search(&query));
        let expected = warm
            .as_ref()
            .map(|result| (result.count, result.hits.len()))
            .map_err(Clone::clone);
        let _ = ready_send.send(expected.as_ref().map(|_| ()).map_err(Clone::clone));
        let mut probe = QueryProbe {
            times_ns: Vec::new(),
            errors: 0,
        };
        if expected.is_err() {
            return probe;
        }
        while !worker_stop.load(Ordering::Acquire) {
            let started = Instant::now();
            let result = owner
                .view()
                .map_err(|error| error.to_string())
                .and_then(|view| view.search(&query));
            probe.times_ns.push(started.elapsed().as_nanos() as u64);
            match result {
                Ok(result) if Ok((result.count, result.hits.len())) == expected => {}
                _ => probe.errors += 1,
            }
            thread::sleep(Duration::from_millis(2));
        }
        probe
    });
    ready_recv.recv_timeout(Duration::from_secs(10))??;
    let started = Instant::now();
    let result = action();
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Release);
    let probe = worker.join().map_err(|_| "query probe panicked")?;
    Ok((result?, elapsed, probe))
}

fn batch_update(index: &Index, paths: &[PathBuf]) -> Result<(Duration, String), Box<dyn Error>> {
    let mut inputs = Vec::with_capacity(paths.len());
    for path in paths {
        let handle = index
            .view()?
            .file_handle_for_path(path)
            .ok_or("batch file missing")?;
        let mut bytes = fs::read(path)?;
        bytes.extend_from_slice(b" \n");
        fs::write(path, bytes)?;
        inputs.push((handle, path));
    }
    let started = Instant::now();
    let mut times = Vec::with_capacity(inputs.len());
    for (handle, path) in inputs {
        let one = Instant::now();
        index.update_file(handle, path)?;
        times.push(one.elapsed().as_nanos() as u64);
    }
    let elapsed = started.elapsed();
    times.sort_unstable();
    let n = times.len();
    let detail = format!(
        "batch_files={n};update_p50_ms={:.4};update_p95_ms={:.4};update_max_ms={:.4}",
        times[n / 2] as f64 / 1_000_000.0,
        times[(n - 1) * 95 / 100] as f64 / 1_000_000.0,
        times[n - 1] as f64 / 1_000_000.0,
    );
    Ok((elapsed, detail))
}

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

/// A disposable one-record-per-file fixture lets the same original-file
/// verified update be measured at 16k, 32k, and 148,486 unrelated files.
/// The caller creates `data/000000.jsonl` through `data/{n-1:06}.jsonl`
/// with stable intrinsic IDs before invoking this mode.
fn run_locality(root: &Path) -> Result<(), Box<dyn Error>> {
    let root = root.canonicalize()?;
    let mut definition = CollectionDefinition::folder("items", "data", SourceFormat::JsonLines);
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    for (name, kind) in [("id", LogicalType::Id), ("name", LogicalType::String)] {
        definition.schema.register_field(FieldDefinition {
            path: vec![name.into()],
            kind,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })?;
    }
    println!("phase\tms\trss_kb\thwm_kb\trevision\tfiles\trecords\tdetail");
    let started = Instant::now();
    let built = Index::build(
        &root,
        vec![definition.clone()],
        RepositoryOptions::default(),
    )?;
    emit("build", started.elapsed(), &built, "synthetic=true");
    let index = if std::env::var("OBJEKTE_BENCH_RESTORE_LOCALITY").as_deref() == Ok("1") {
        let cache = root.join("locality-cache.bin");
        let started = Instant::now();
        built.flush_cache(&cache)?;
        emit("flush", started.elapsed(), &built, "synthetic=true");
        built.shutdown()?;
        let started = Instant::now();
        let restored = Index::load_cache(
            &root,
            vec![definition],
            RepositoryOptions::default(),
            &cache,
        )?;
        emit("restore", started.elapsed(), &restored, "synthetic=true");
        restored.wait_for_reconciliation()?;
        restored
    } else {
        built
    };
    let target = root.join("data/000000.jsonl");
    let mut old = fs::read(&target)?;
    let mut handle = index
        .view()?
        .file_handle_for_path(&target)
        .ok_or("synthetic target missing")?;
    for run in 0..11 {
        let next =
            format!("{{\"id\":\"item-000000\",\"name\":\"changed-{run:03}\"}}\n").into_bytes();
        fs::write(&target, &next)?;
        let started = Instant::now();
        let updated = index.update_file_with_options(
            handle,
            &target,
            UpdateOptions {
                old_content: Some(&old),
                strategy: UpdateStrategy::PreferVerified,
            },
        )?;
        let elapsed = started.elapsed();
        emit(
            "verified_original_update",
            elapsed,
            &index,
            format!(
                "sample={run};strategy={:?};fallback={:?};retired={}",
                updated.strategy, updated.fallback_reason, updated.retired_records
            ),
        );
        handle = updated.file;
        old = next;
    }
    let started = Instant::now();
    let count = count(&index, "items")?;
    emit("count", started.elapsed(), &index, format!("count={count}"));
    let started = Instant::now();
    let report = index.compact()?;
    emit(
        "compact",
        started.elapsed(),
        &index,
        format!("removed={}", report.retired_slots_removed),
    );
    index.shutdown()?;
    Ok(())
}

fn run(mode: &str, root: &Path) -> Result<(), Box<dyn Error>> {
    if std::env::var("OBJEKTE_BENCH_MUTATE_COPY").as_deref() != Ok("1") {
        return Err("set OBJEKTE_BENCH_MUTATE_COPY=1 for a disposable fixture copy".into());
    }
    if mode == "locality" {
        return run_locality(root);
    }
    if !matches!(mode, "small" | "large" | "stress") {
        return Err("mode must be small, large, stress, or locality".into());
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
    let probe_query = (mode == "stress").then(maintenance_query);
    if let Some(query) = &probe_query {
        let (_, elapsed, probe) = while_querying(&index, query, || {
            thread::sleep(Duration::from_millis(500));
            Ok(())
        })?;
        emit("query_steady", elapsed, &index, probe.detail());
    }

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

    if let Some(query) = &probe_query {
        let mut paths = fs::read_dir(root.join("data/contents"))?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect::<Vec<_>>();
        paths.sort();
        if paths.len() < 4097 {
            return Err("stress fixture needs at least 4097 JSON files".into());
        }
        let verified_path = &paths[136];
        let verified_handle = index
            .view()?
            .file_handle_for_path(verified_path)
            .ok_or("verified base file missing")?;
        let old_content = fs::read(verified_path)?;
        let mut changed = old_content.clone();
        changed.extend_from_slice(b" \n");
        fs::write(verified_path, changed)?;
        let started = Instant::now();
        let verified_base = index.update_file_with_options(
            verified_handle,
            verified_path,
            UpdateOptions {
                old_content: Some(&old_content),
                strategy: UpdateStrategy::PreferVerified,
            },
        )?;
        emit(
            "update_verified_base",
            started.elapsed(),
            &index,
            format!(
                "strategy={:?};fallback={:?}",
                verified_base.strategy, verified_base.fallback_reason,
            ),
        );
        // The historical base target above shares a partition with the first
        // append-updated file. This second target is in a different, still
        // dense base partition and measures eligible slot preservation.
        let isolated_path = &paths[4096];
        let isolated_handle = index
            .view()?
            .file_handle_for_path(isolated_path)
            .ok_or("untouched base file missing")?;
        let isolated_old = fs::read(isolated_path)?;
        let mut isolated_new = isolated_old.clone();
        isolated_new.extend_from_slice(b" \n");
        fs::write(isolated_path, isolated_new)?;
        let started = Instant::now();
        let isolated = index.update_file_with_options(
            isolated_handle,
            isolated_path,
            UpdateOptions {
                old_content: Some(&isolated_old),
                strategy: UpdateStrategy::PreferVerified,
            },
        )?;
        emit(
            "update_verified_untouched_base",
            started.elapsed(),
            &index,
            format!(
                "strategy={:?};fallback={:?}",
                isolated.strategy, isolated.fallback_reason
            ),
        );
        let (elapsed, detail) = batch_update(&index, &paths[..8])?;
        emit("batch_small_8", elapsed, &index, detail);
        let (elapsed, detail) = batch_update(&index, &paths[8..136])?;
        emit("batch_large_128", elapsed, &index, detail);

        let mut handle = index
            .view()?
            .file_handle_for_path(&source)
            .ok_or("repeated-update file missing")?;
        let started = Instant::now();
        let mut times = Vec::new();
        for _ in 0..32 {
            let mut bytes = fs::read(&source)?;
            bytes.extend_from_slice(b" \n");
            fs::write(&source, bytes)?;
            let one = Instant::now();
            handle = index.update_file(handle, &source)?.file;
            times.push(one.elapsed().as_nanos() as u64);
        }
        let elapsed = started.elapsed();
        times.sort_unstable();
        emit(
            "repeat_update_32",
            elapsed,
            &index,
            format!(
                "updates=32;api_p50_ms={:.4};api_p95_ms={:.4};api_max_ms={:.4}",
                times[16] as f64 / 1_000_000.0,
                times[31 * 95 / 100] as f64 / 1_000_000.0,
                times[31] as f64 / 1_000_000.0,
            ),
        );
        if count(&index, collection)? != base_count {
            return Err("stress updates changed logical record count".into());
        }
        let (report, elapsed, probe) =
            while_querying(&index, query, || Ok(index.start_reconcile()?.wait()?))?;
        if report.cancelled || !report.issues.is_empty() || report.omitted_issues > 0 {
            return Err("stress reconciliation incomplete".into());
        }
        emit(
            "reconcile_with_query",
            elapsed,
            &index,
            format!(
                "examined={};unchanged={};{}",
                report.examined,
                report.unchanged,
                probe.detail()
            ),
        );
    }

    let (compacted, compact_elapsed, compact_probe) = if let Some(query) = &probe_query {
        let (report, elapsed, probe) = while_querying(&index, query, || Ok(index.compact()?))?;
        (report, elapsed, probe.detail())
    } else {
        let started = Instant::now();
        (index.compact()?, started.elapsed(), String::new())
    };
    emit(
        "compact",
        compact_elapsed,
        &index,
        format!(
            "retired_slots_removed={};slots_after={};{}",
            compacted.retired_slots_removed, compacted.slots_after, compact_probe
        ),
    );
    let count_after = count(&index, collection)?;
    if count_after != base_count {
        return Err(format!("count changed: {base_count} -> {count_after}").into());
    }
    if let Some(query) = &probe_query {
        let (report, elapsed, probe) = while_querying(&index, query, || Ok(index.rebuild()?))?;
        if report.live_records != index.status()?.live_records {
            return Err("rebuild record count mismatch".into());
        }
        emit(
            "rebuild_with_query",
            elapsed,
            &index,
            format!("attempts={};{}", report.attempts, probe.detail()),
        );
    }

    let cache = snapshot_path(root, RepositoryOptions::default().bigram_alphabet);
    let ((revision, stats), flush_elapsed, flush_probe) = if let Some(query) = &probe_query {
        let (result, elapsed, probe) =
            while_querying(&index, query, || Ok(index.flush_cache_with_stats(&cache)?))?;
        (result, elapsed, probe.detail())
    } else {
        let started = Instant::now();
        (
            index.flush_cache_with_stats(&cache)?,
            started.elapsed(),
            String::new(),
        )
    };
    emit(
        "flush",
        flush_elapsed,
        &index,
        format!(
            "revision={};bytes={};stats_ms={:.3};{}",
            revision.0,
            stats.bytes_written,
            stats.total_ns as f64 / 1_000_000.0,
            flush_probe,
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
        eprintln!("usage: lifecycle-perf small|large|stress|locality ROOT");
        std::process::exit(2);
    }
    if let Err(error) = run(&args[1], Path::new(&args[2])) {
        eprintln!("benchmark error: {error}");
        std::process::exit(1);
    }
}
