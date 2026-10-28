//! End-to-end cache timing and embedded JSON-engine comparison.
use jaq_all::jaq_core::unwrap_valr;
use jaq_all::json::Val as JaqValue;
use jaq_all::{data, fmts};
#[allow(dead_code)]
mod legacy_cache;
use legacy_cache as cache;
use qj::io::{InputReader, ReaderOptions};
use qj::jq::lang::execute::Jq;
use qj::jq::lang::{CompileOptions, jq_compile_args};
use qj::jq::value::Value as QjValue;
use std::ffi::OsString;
use std::fs;
use std::io::{BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn timed<T>(label: &str, f: impl FnOnce() -> T) -> T {
    let start = Instant::now();
    let result = f();
    report(label, start.elapsed());
    result
}
fn report(label: &str, elapsed: Duration) {
    println!("{label:28} {:>10.3} s", elapsed.as_secs_f64());
}
fn synthetic(root: &Path, count: usize) -> std::io::Result<()> {
    fs::create_dir_all(root.join(".editor/collections/items"))?;
    fs::create_dir_all(root.join("data/items"))?;
    fs::write(
        root.join(".editor/repository.json"),
        r#"{"collections":{"items":".editor/collections/items/settings.json"}}"#,
    )?;
    fs::write(
        root.join(".editor/collections/items/settings.json"),
        r#"{"type":"data","path":"data/items","format":"json","id":"filename"}"#,
    )?;
    for n in 0..count {
        let id = format!("{n:08}");
        let folder = root.join("data/items").join(format!("{:02x}", n % 256));
        if n < 256 {
            fs::create_dir_all(&folder)?;
        }
        let path = folder.join(format!("{id}.json"));
        let mut file = fs::File::create(path)?;
        write!(
            file,
            "{{\"title\":\"Record {n}\",\"year\":{},\"active\":{}}}",
            1600 + n % 500,
            n % 3 == 0
        )?;
    }
    Ok(())
}
fn qj_worker(files: Vec<PathBuf>, filter: &str) -> Result<usize, String> {
    let opts = CompileOptions::new(".");
    let code =
        jq_compile_args(filter.as_bytes(), &opts).map_err(|e| format!("qj compile: {e:?}"))?;
    let mut jq = Jq::new(code);
    jq.set_jq_attrs(&opts.attrs);
    let names: Vec<OsString> = files.iter().map(|p| p.as_os_str().to_owned()).collect();
    let mut input = InputReader::new(names, ReaderOptions::default());
    let mut count = 0;
    while let Some(value) = input.next() {
        jq.start(value.map_err(|e| format!("qj input: {e}"))?, 0);
        for output in &mut jq {
            output.map_err(|e| format!("qj run: {e}"))?;
            count += 1;
        }
    }
    if input.failures() != 0 {
        return Err(format!(
            "qj failed to read {} input files",
            input.failures()
        ));
    }
    Ok(count)
}
fn jaq_worker(files: Vec<PathBuf>, filter: &str) -> Result<usize, String> {
    let compiled = data::compile(filter).map_err(|_| "jaq compile error".to_owned())?;
    let runner = data::Runner::default();
    let mut count = 0;
    for path in files {
        let file = fs::File::open(&path).map_err(|e| e.to_string())?;
        let inputs = fmts::read::json::read_many(BufReader::new(file));
        data::run(
            &runner,
            &compiled,
            Default::default(),
            inputs,
            |e| e,
            |v| {
                unwrap_valr(v).map_err(|e| e.to_string())?;
                count += 1;
                Ok(())
            },
        )?;
    }
    Ok(count)
}
fn parallel(
    files: &[PathBuf],
    filter: &str,
    threads: usize,
    worker: fn(Vec<PathBuf>, &str) -> Result<usize, String>,
) -> Result<usize, String> {
    let mut groups = vec![Vec::new(); threads.max(1)];
    let n = groups.len();
    for (i, path) in files.iter().enumerate() {
        groups[i % n].push(path.clone());
    }
    std::thread::scope(|scope| {
        let handles: Vec<_> = groups
            .into_iter()
            .map(|group| scope.spawn(move || worker(group, filter)))
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .map_err(|_| "benchmark worker panicked".to_owned())?
            })
            .sum()
    })
}
fn queries(files: &[PathBuf], threads: usize, filters: &[String]) -> Result<(), String> {
    for filter in filters {
        println!("filter: {filter}");
        let qj = timed("qj embedded", || {
            parallel(files, filter, threads, qj_worker)
        })?;
        let jaq = timed("jaq embedded", || {
            parallel(files, filter, threads, jaq_worker)
        })?;
        println!("outputs: qj={qj}, jaq={jaq}");
        if qj != jaq {
            return Err("engine result counts differ".into());
        }
    }
    Ok(())
}
fn qj_slurp(files: &[PathBuf]) -> Result<QjValue, String> {
    let names: Vec<OsString> = files.iter().map(|p| p.as_os_str().to_owned()).collect();
    let mut input = InputReader::new(names, ReaderOptions::default());
    let mut rows = Vec::new();
    while let Some(value) = input.next() {
        rows.push(value.map_err(|e| format!("qj input: {e}"))?);
    }
    if input.failures() != 0 {
        return Err(format!(
            "qj failed to read {} input files",
            input.failures()
        ));
    }
    Ok(rows.into())
}
fn jaq_slurp(files: &[PathBuf]) -> Result<JaqValue, String> {
    let mut rows = Vec::new();
    for path in files {
        let file = fs::File::open(path).map_err(|e| e.to_string())?;
        for value in fmts::read::json::read_many(BufReader::new(file)) {
            rows.push(value.map_err(|e| format!("jaq input: {e}"))?);
        }
    }
    Ok(rows.into_iter().collect())
}
fn qj_global(input: QjValue, filter: &str) -> Result<Vec<serde_json::Value>, String> {
    let opts = CompileOptions::new(".");
    let code =
        jq_compile_args(filter.as_bytes(), &opts).map_err(|e| format!("qj compile: {e:?}"))?;
    let mut jq = Jq::new(code);
    jq.set_jq_attrs(&opts.attrs);
    jq.start(input, 0);
    let mut out = Vec::new();
    for value in &mut jq {
        let value = value.map_err(|e| format!("qj run: {e}"))?;
        out.push(serde_json::from_str(&value.to_json()).map_err(|e| e.to_string())?);
    }
    Ok(out)
}
fn jaq_global(input: JaqValue, filter: &str) -> Result<Vec<serde_json::Value>, String> {
    let compiled = data::compile(filter).map_err(|_| "jaq compile error".to_owned())?;
    let runner = data::Runner::default();
    let mut out = Vec::new();
    data::run(
        &runner,
        &compiled,
        Default::default(),
        std::iter::once(Ok::<_, String>(input)),
        |e| e,
        |value| {
            let value = unwrap_valr(value).map_err(|e| e.to_string())?;
            out.push(serde_json::from_str(&value.to_string()).map_err(|e| e.to_string())?);
            Ok(())
        },
    )?;
    Ok(out)
}
fn global_queries(files: &[PathBuf], filters: &[String]) -> Result<(), String> {
    for filter in filters {
        println!("global filter: {filter}");
        let qj_start = Instant::now();
        let qj_input = timed("qj slurp inputs", || qj_slurp(files))?;
        let qj_out = timed("qj global query", || qj_global(qj_input, filter))?;
        report("qj end-to-end", qj_start.elapsed());

        let jaq_start = Instant::now();
        let jaq_input = timed("jaq slurp inputs", || jaq_slurp(files))?;
        let jaq_out = timed("jaq global query", || jaq_global(jaq_input, filter))?;
        report("jaq end-to-end", jaq_start.elapsed());

        if qj_out != jaq_out {
            return Err(format!(
                "global results differ: qj={} outputs, jaq={} outputs",
                qj_out.len(),
                jaq_out.len()
            ));
        }
        let result_rows = qj_out
            .first()
            .and_then(serde_json::Value::as_array)
            .map(Vec::len);
        println!(
            "matching global outputs: {}; result rows: {}",
            qj_out.len(),
            result_rows
                .map(|n| n.to_string())
                .unwrap_or_else(|| "n/a".into())
        );
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut repo = None;
    let mut collection = None;
    let mut count = None;
    let mut work_dir = None;
    let mut filters = Vec::new();
    let mut globals = Vec::new();
    let mut no_globals = false;
    let mut threads = std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repo" | "--collection" | "--synthetic" | "--threads" | "--filter" | "--global"
            | "--work-dir" => {
                let key = &args[i];
                i += 1;
                let value = args.get(i).ok_or(format!("missing value for {key}"))?;
                match key.as_str() {
                    "--repo" => repo = Some(PathBuf::from(value)),
                    "--collection" => collection = Some(value.clone()),
                    "--synthetic" => count = Some(value.parse::<usize>()?),
                    "--threads" => threads = value.parse::<usize>()?,
                    "--filter" => filters.push(value.clone()),
                    "--global" => globals.push(value.clone()),
                    "--work-dir" => work_dir = Some(PathBuf::from(value)),
                    _ => unreachable!(),
                }
            }
            "--no-global" => no_globals = true,
            "--help" | "-h" => {
                println!(
                    "Usage: cargo run --release -p objekte-bench -- [--repo PATH --collection NAME | --synthetic N] [--work-dir PATH] [--threads N] [--filter EXPR ...] [--global EXPR ... | --no-global]"
                );
                return Ok(());
            }
            other => return Err(format!("unknown argument {other}").into()),
        }
        i += 1;
    }
    if threads == 0 {
        return Err("--threads must be positive".into());
    }
    if filters.is_empty() {
        filters = vec![
            ".id".into(),
            "select(.data.year != null and .data.year < 1800) | .id".into(),
        ];
    }
    if no_globals && !globals.is_empty() {
        return Err("--no-global conflicts with --global".into());
    }
    if globals.is_empty() && !no_globals {
        globals = vec![
            "sort_by(.data.year, .id) | .[:100] | map(.id)".into(),
            "group_by(.data.year) | map([.[0].data.year, length])".into(),
            "map(select(.data.year != null and .data.year < 1800)) | sort_by(.data.year, .id) | .[:100] | map(.id)".into(),
        ];
    }
    let temp = match work_dir {
        Some(path) => tempfile::Builder::new()
            .prefix("objekte-bench-")
            .tempdir_in(path)?,
        None => tempfile::tempdir()?,
    };
    let generated = count.is_some();
    let (root, name) = if let Some(n) = count {
        let root = temp.path().join("synthetic");
        timed("generate source files", || synthetic(&root, n))?;
        (root, "items".to_owned())
    } else {
        (
            repo.ok_or("pass --repo or --synthetic")?,
            collection.ok_or("pass --collection")?,
        )
    };
    let cache_root = temp.path().join("cache");
    println!("repository: {}", root.display());
    println!("collection: {name}; threads: {threads}");
    let first = timed("cold cache build", || {
        cache::build_collection(&root, &name, &cache_root)
    })?;
    println!("records: {}", first.index.records.len());
    let data_bytes: u64 = first
        .files()
        .iter()
        .map(|path| fs::metadata(path).map(|meta| meta.len()))
        .collect::<std::io::Result<Vec<_>>>()?
        .into_iter()
        .sum();
    let generation_dir = first.directory.join(&first.generation);
    let mut index_bytes = fs::metadata(generation_dir.join("manifest.json"))?.len();
    for shard in 0..cache::SHARDS {
        index_bytes += fs::metadata(generation_dir.join(format!("{shard:02x}.index.json")))?.len();
    }
    println!("cache bytes: data={data_bytes}, index={index_bytes}");
    let changes = timed("scan unchanged sources", || {
        cache::detect_changes(&root, &name, &cache_root)
    })?;
    println!("changed sources: {}", changes.len());
    let rebuilt = timed("full cache rebuild", || {
        cache::build_collection(&root, &name, &cache_root)
    })?;
    let id = rebuilt
        .index
        .records
        .keys()
        .next()
        .ok_or("collection is empty")?
        .clone();
    if generated {
        let path = root.join(&rebuilt.index.records[&id].path);
        fs::write(
            path,
            r#"{"title":"Changed record","year":1749,"active":true}"#,
        )?;
        let changes = timed("scan one changed source", || {
            cache::detect_changes(&root, &name, &cache_root)
        })?;
        println!("changed sources: {}", changes.len());
    }
    let refreshed = timed("one-record shard refresh", || {
        cache::refresh_record(&root, &name, &cache_root, &id)
    })?;
    println!("refreshed ID: {id}; shards: {}", cache::SHARDS);
    let opened = timed("open published generation", || {
        cache::open_generation(&root, &name, &cache_root)
    })?;
    let indexed = timed("load full ID index", || {
        cache::open_collection(&root, &name, &cache_root)
    })?;
    if opened.generation != refreshed.generation || indexed.generation != refreshed.generation {
        return Err("published generation differs from refreshed generation".into());
    }
    queries(&opened.files(), threads, &filters)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    global_queries(&opened.files(), &globals)
        .map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    Ok(())
}
