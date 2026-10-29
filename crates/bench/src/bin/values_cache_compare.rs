//! Disposable values-only text cache experiment. No production cache is changed.
use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch, sinks::Bytes};
use objekte_core::{CacheMode, JsonFilter, JsonQuery, JsonQueryBackend, Options, Repository};
use rayon::prelude::*;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    time::Instant,
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;


/// One query and optional text needle from the workload.
#[derive(Deserialize)]
struct Case {
    collection: String,
    filter: String,
    /// A necessary literal in a decoded scalar value; absent means parse all files.
    needle: Option<String>,
}


/// Source and projected-text paths for one record.
struct Entry {
    id: String,
    source: PathBuf,
    text: PathBuf,
}


const SHARD_COUNT: usize = 64;


/// One disposable values-text shard and its record positions.
struct Shard {
    path: PathBuf,
    entry_indices: Vec<usize>,
}


/// Files and byte counts for the experimental text layouts.
struct ValuesCache {
    entries: Vec<Entry>,
    packed: PathBuf,
    shards: Vec<Shard>,
    text_bytes: u64,
    allocated_files_bytes: u64,
    packed_bytes: u64,
    shard_allocated_bytes: u64,
}


/// Verified match count, candidates, and page records.
struct QueryResult {
    total: usize,
    candidates: usize,
    page: Vec<(String, Value)>,
}


/// Search sink that stops at the first regex match.
#[derive(Default)]
struct FirstMatch(bool);


impl Sink for FirstMatch {
    type Error = std::io::Error;

    fn matched(&mut self, _: &Searcher, _: &SinkMatch<'_>) -> std::io::Result<bool> {
        self.0 = true;
        Ok(false)
    }
}


fn arg(name: &str, default: Option<&str>) -> Result<String> {
    let mut args = env::args().skip(1);
    while let Some(key) = args.next() {
        if key == name {
            return Ok(args.next().ok_or(format!("{name} requires a value"))?);
        }
    }
    Ok(default.ok_or(format!("missing {name}"))?.into())
}

fn parse_source(path: &Path) -> Result<Option<Value>> {
    let bytes = fs::read(path)?;
    Ok(sonic_rs::from_slice(&bytes).ok())
}

// Record boundaries are newlines; value boundaries are ASCII unit separators.
// Preserve value punctuation, but omit all JSON property names and syntax.
fn flatten(value: &Value, out: &mut String) {
    match value {
        Value::Array(items) => {
            for item in items {
                flatten(item, out);
            }
        }
        Value::Object(items) => {
            for item in items.values() {
                flatten(item, out);
            }
        }
        Value::String(s) => {
            for c in s.chars() {
                out.push(if c == '\n' || c == '\r' { '\u{1f}' } else { c });
            }
            out.push('\u{1f}');
        }
        Value::Number(n) => {
            out.push_str(&n.to_string());
            out.push('\u{1f}');
        }
        Value::Bool(v) => {
            out.push_str(if *v { "true" } else { "false" });
            out.push('\u{1f}');
        }
        Value::Null => out.push_str("null\u{1f}"),
    }
}

#[cfg(unix)]
fn allocated(path: &Path) -> Result<u64> {
    use std::os::unix::fs::MetadataExt;
    Ok(fs::metadata(path)?.blocks() * 512)
}

#[cfg(not(unix))]
fn allocated(path: &Path) -> Result<u64> {
    Ok(fs::metadata(path)?.len())
}

fn shard_for_id(id: &str) -> usize {
    // A stable hash keeps the partition independent of insertion order.
    let hash = id.bytes().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
    });
    (hash as usize) % SHARD_COUNT
}

fn build_cache(source_dir: &Path, dest: &Path) -> Result<ValuesCache> {
    let files_dir = dest.join("files");
    fs::create_dir_all(&files_dir)?;
    let packed = dest.join("packed.txt");
    let mut pack = BufWriter::new(File::create(&packed)?);
    let mut shard_writers = (0..SHARD_COUNT)
        .map(|i| -> Result<_> {
            let path = dest.join(format!("shard-{i:02}.txt"));
            Ok((
                path.clone(),
                BufWriter::new(File::create(path)?),
                Vec::new(),
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let mut sources = fs::read_dir(source_dir)?
        .map(|e| e.map(|e| e.path()))
        .collect::<std::io::Result<Vec<_>>>()?;
    sources.retain(|p| p.extension().is_some_and(|e| e == "json"));
    sources.sort_by_key(|p| p.file_stem().unwrap().to_os_string());
    let mut entries = Vec::with_capacity(sources.len());
    let mut text_bytes = 0;
    let mut allocated_files_bytes = 0;
    for source in sources {
        let id = source
            .file_stem()
            .unwrap()
            .to_str()
            .ok_or("non-UTF-8 ID")?
            .to_owned();
        let text = files_dir.join(format!("{id}.txt"));
        let mut body = String::new();
        if let Some(value) = parse_source(&source)? {
            flatten(&value, &mut body);
        }
        body.push('\n');
        fs::write(&text, body.as_bytes())?;
        pack.write_all(body.as_bytes())?;
        let shard = &mut shard_writers[shard_for_id(&id)];
        shard.1.write_all(body.as_bytes())?;
        shard.2.push(entries.len());
        text_bytes += body.len() as u64;
        allocated_files_bytes += allocated(&text)?;
        entries.push(Entry { id, source, text });
    }
    pack.flush()?;
    let packed_bytes = fs::metadata(&packed)?.len();
    let mut shards = Vec::with_capacity(SHARD_COUNT);
    let mut shard_allocated_bytes = 0;
    for (path, mut writer, entry_indices) in shard_writers {
        writer.flush()?;
        drop(writer);
        shard_allocated_bytes += allocated(&path)?;
        shards.push(Shard {
            path,
            entry_indices,
        });
    }
    Ok(ValuesCache {
        entries,
        packed,
        shards,
        text_bytes,
        allocated_files_bytes,
        packed_bytes,
        shard_allocated_bytes,
    })
}

fn matcher(case: &Case) -> Result<Option<RegexMatcher>> {
    // The historical experiment stores only original values. A Unicode fold
    // can create an ASCII needle absent from those bytes (ß -> ss).
    if case.filter.contains(":ignorecase(") {
        return Ok(None);
    }
    case.needle
        .as_ref()
        .map(|s| RegexMatcher::new(&format!("(?i:{})", regex::escape(s))).map_err(Into::into))
        .transpose()
}

fn verify(path: &Path, filter: &JsonFilter) -> Result<bool> {
    match parse_source(path)? {
        Some(value) => Ok(filter.matches(&value)?),
        None => Ok(false),
    }
}

fn page(cache: &ValuesCache, hits: &[usize], candidates: usize) -> Result<QueryResult> {
    let mut records = Vec::new();
    for &index in hits.iter().take(50) {
        let entry = &cache.entries[index];
        let value = parse_source(&entry.source)?.ok_or("matched source became invalid")?;
        records.push((entry.id.clone(), value));
    }
    Ok(QueryResult {
        total: hits.len(),
        candidates,
        page: records,
    })
}

fn per_file(cache: &ValuesCache, case: &Case, source_text: bool) -> Result<QueryResult> {
    let filter = JsonFilter::compile(&case.filter, &BTreeMap::new())?;
    let matcher = matcher(case)?;
    let outcomes = cache
        .entries
        .par_iter()
        .map_init(
            || {
                SearcherBuilder::new()
                    .line_number(false)
                    .bom_sniffing(false)
                    .build()
            },
            |searcher, entry| -> Result<(bool, bool)> {
                let candidate = if let Some(matcher) = &matcher {
                    let mut sink = FirstMatch::default();
                    searcher.search_path(
                        matcher,
                        if source_text {
                            &entry.source
                        } else {
                            &entry.text
                        },
                        &mut sink,
                    )?;
                    sink.0
                } else {
                    true
                };
                Ok((candidate, candidate && verify(&entry.source, &filter)?))
            },
        )
        .collect::<Result<Vec<_>>>()?;
    let candidates = outcomes.iter().filter(|(candidate, _)| *candidate).count();
    let hits = outcomes
        .iter()
        .enumerate()
        .filter_map(|(i, (_, matched))| matched.then_some(i))
        .collect::<Vec<_>>();
    page(cache, &hits, candidates)
}

fn packed(cache: &ValuesCache, case: &Case) -> Result<QueryResult> {
    let filter = JsonFilter::compile(&case.filter, &BTreeMap::new())?;
    let candidates = if let Some(matcher) = matcher(case)? {
        let mut candidates = Vec::new();
        let mut searcher = SearcherBuilder::new().bom_sniffing(false).build();
        searcher.search_path(
            &matcher,
            &cache.packed,
            Bytes(|line, _| {
                candidates.push((line - 1) as usize);
                Ok(true)
            }),
        )?;
        candidates
    } else {
        (0..cache.entries.len()).collect()
    };
    let outcomes = candidates
        .par_iter()
        .map(|&i| verify(&cache.entries[i].source, &filter))
        .collect::<Result<Vec<_>>>()?;
    let hits = candidates
        .iter()
        .zip(outcomes)
        .filter_map(|(&i, matched)| matched.then_some(i))
        .collect::<Vec<_>>();
    page(cache, &hits, candidates.len())
}

fn sharded(cache: &ValuesCache, case: &Case) -> Result<QueryResult> {
    let filter = JsonFilter::compile(&case.filter, &BTreeMap::new())?;
    let mut candidates = if let Some(matcher) = matcher(case)? {
        cache
            .shards
            .par_iter()
            .map_init(
                || SearcherBuilder::new().bom_sniffing(false).build(),
                |searcher, shard| -> Result<Vec<usize>> {
                    let mut hits = Vec::new();
                    searcher.search_path(
                        &matcher,
                        &shard.path,
                        Bytes(|line, _| {
                            hits.push(shard.entry_indices[(line - 1) as usize]);
                            Ok(true)
                        }),
                    )?;
                    Ok(hits)
                },
            )
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
    } else {
        (0..cache.entries.len()).collect()
    };
    candidates.sort_unstable();
    let outcomes = candidates
        .par_iter()
        .map(|&i| verify(&cache.entries[i].source, &filter))
        .collect::<Result<Vec<_>>>()?;
    let hits = candidates
        .iter()
        .zip(outcomes)
        .filter_map(|(&i, matched)| matched.then_some(i))
        .collect::<Vec<_>>();
    page(cache, &hits, candidates.len())
}

fn compare(
    expected: &objekte_core::JsonQueryResult,
    actual: &QueryResult,
    label: &str,
    case: &Case,
) -> Result<()> {
    if expected.total != Some(actual.total)
        || expected.records.len() != actual.page.len()
        || expected
            .records
            .iter()
            .zip(&actual.page)
            .any(|(a, (id, data))| &a.id != id || &a.data != data)
    {
        return Err(format!("{label} result differs for {}", case.filter).into());
    }
    Ok(())
}

fn median(values: &mut [f64]) -> f64 {
    values.sort_by(f64::total_cmp);
    values[values.len() / 2]
}

fn benchmark_shard_update(cache: &ValuesCache) -> Result<()> {
    let entry_index = cache.entries.len() / 2;
    let entry = &cache.entries[entry_index];
    let shard = &cache.shards[shard_for_id(&entry.id)];
    let line_index = shard
        .entry_indices
        .binary_search(&entry_index)
        .map_err(|_| "entry missing from shard")?;
    let original = fs::read(&shard.path)?;
    let mut times = Vec::new();
    for iteration in 0..20 {
        let started = Instant::now();
        let mut value = parse_source(&entry.source)?.ok_or("update source is invalid")?;
        if iteration % 2 == 0 {
            let title = value
                .get("preferred_title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned();
            value["preferred_title"] = Value::String(format!("{title} [edited]"));
        }
        let mut replacement = String::new();
        flatten(&value, &mut replacement);
        replacement.push('\n');

        let bytes = fs::read(&shard.path)?;
        let mut start = 0;
        for _ in 0..line_index {
            start += bytes[start..]
                .iter()
                .position(|&byte| byte == b'\n')
                .ok_or("shard line missing")?
                + 1;
        }
        let end = start
            + bytes[start..]
                .iter()
                .position(|&byte| byte == b'\n')
                .ok_or("shard line missing")?
            + 1;
        let temp = shard.path.with_extension("next");
        let mut writer = BufWriter::new(File::create(&temp)?);
        writer.write_all(&bytes[..start])?;
        writer.write_all(replacement.as_bytes())?;
        writer.write_all(&bytes[end..])?;
        writer.flush()?;
        writer.get_ref().sync_all()?;
        drop(writer);
        fs::rename(&temp, &shard.path)?;
        File::open(shard.path.parent().ok_or("shard has no parent")?)?.sync_all()?;
        times.push(started.elapsed().as_secs_f64() * 1000.0);
    }
    if fs::read(&shard.path)? != original {
        return Err("shard update did not restore original bytes".into());
    }
    println!(
        "{}",
        json!({
            "type":"shard_update", "shard_count":SHARD_COUNT,
            "shard_bytes":original.len(), "records_in_shard":shard.entry_indices.len(),
            "median_ms":median(&mut times), "runs":times.len(),
            "includes":"one source Sonic parse, text flatten, shard read/rewrite, file fsync, rename, directory fsync",
            "excludes":"typed indexes, metadata, snapshot manifest"
        })
    );
    Ok(())
}

fn main() -> Result<()> {
    let root = PathBuf::from(arg("--repo", Some("examples/musenalm"))?);
    let workload = PathBuf::from(arg(
        "--workload",
        Some("crates/bench/workloads/musenalm-grep.json"),
    )?);
    let data_dir = PathBuf::from(arg("--data-dir", Some("target/json-query-grep-cache"))?);
    let work_dir = PathBuf::from(arg("--work-dir", Some("target"))?);
    let runs: usize = arg("--runs", Some("3"))?.parse()?;
    if runs == 0 {
        return Err("--runs must be positive".into());
    }
    let cases: Vec<Case> = serde_json::from_slice(&fs::read(workload)?)?;
    let collection = &cases.first().ok_or("empty workload")?.collection;
    if cases.iter().any(|c| &c.collection != collection) {
        return Err("this values-cache runner requires one collection".into());
    }
    fs::create_dir_all(&work_dir)?;
    let temp = tempfile::tempdir_in(&work_dir)?;
    let source_dir = root.join("data").join(collection);
    let started = Instant::now();
    let cache = build_cache(&source_dir, temp.path())?;
    println!(
        "{}",
        json!({
            "type":"build", "collection":collection, "records":cache.entries.len(),
            "build_ms":started.elapsed().as_secs_f64()*1000.0,
            "text_logical_bytes":cache.text_bytes,
            "per_file_allocated_bytes":cache.allocated_files_bytes,
            "packed_bytes":cache.packed_bytes,
            "shard_count":SHARD_COUNT,
            "shard_allocated_bytes":cache.shard_allocated_bytes
        })
    );
    if arg("--mode", Some("queries"))? == "update" {
        return benchmark_shard_update(&cache);
    }
    let mut options = Options::new(data_dir);
    options.cache = CacheMode::Segments;
    let mut repo = Repository::load_repository(root, options)?;
    for case in &cases {
        let request = JsonQuery {
            filter: case.filter.clone(),
            collections: Some(vec![case.collection.clone()]),
            count: true,
            backend: JsonQueryBackend::Segments,
            ..Default::default()
        };
        let segment = repo.query_json(&request)?;
        let raw = repo.query_json(&JsonQuery {
            backend: JsonQueryBackend::Grep,
            ..request.clone()
        })?;
        if segment.total != raw.total
            || serde_json::to_value(&segment.records)? != serde_json::to_value(&raw.records)?
        {
            return Err(format!("raw backend differs for {}", case.filter).into());
        }
        compare(
            &segment,
            &per_file(&cache, case, true)?,
            "raw control",
            case,
        )?;
        compare(&segment, &per_file(&cache, case, false)?, "per-file", case)?;
        compare(&segment, &packed(&cache, case)?, "packed", case)?;
        compare(&segment, &sharded(&cache, case)?, "sharded", case)?;
    }
    for case in cases {
        let mut segment_ms = Vec::new();
        let mut raw_ms = Vec::new();
        let mut raw_control_ms = Vec::new();
        let mut files_ms = Vec::new();
        let mut packed_ms = Vec::new();
        let mut sharded_ms = Vec::new();
        let mut candidates = (0, 0, 0, 0);
        for run in 0..runs {
            let order = if run % 2 == 0 {
                [0, 1, 2, 3, 4, 5]
            } else {
                [5, 4, 3, 2, 1, 0]
            };
            for engine in order {
                let start = Instant::now();
                match engine {
                    0 | 1 => {
                        let request = JsonQuery {
                            filter: case.filter.clone(),
                            collections: Some(vec![case.collection.clone()]),
                            count: true,
                            backend: if engine == 0 {
                                JsonQueryBackend::Segments
                            } else {
                                JsonQueryBackend::Grep
                            },
                            ..Default::default()
                        };
                        let result = repo.query_json(&request)?;
                        if engine == 0 {
                            segment_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                        } else {
                            raw_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                        }
                        if result.total.is_none() {
                            return Err("missing count".into());
                        }
                    }
                    2 => {
                        let result = per_file(&cache, &case, true)?;
                        raw_control_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                        candidates.0 = result.candidates;
                    }
                    3 => {
                        let result = per_file(&cache, &case, false)?;
                        files_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                        candidates.1 = result.candidates;
                    }
                    4 => {
                        let result = packed(&cache, &case)?;
                        packed_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                        candidates.2 = result.candidates;
                    }
                    5 => {
                        let result = sharded(&cache, &case)?;
                        sharded_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                        candidates.3 = result.candidates;
                    }
                    _ => unreachable!(),
                }
            }
        }
        println!(
            "{}",
            json!({
                "type":"query", "filter":case.filter, "needle":case.needle,
                "raw_control_candidates":candidates.0,
                "file_candidates":candidates.1, "packed_candidates":candidates.2,
                "sharded_candidates":candidates.3,
                "segment_ms":median(&mut segment_ms), "raw_grep_ms":median(&mut raw_ms),
                "raw_control_ms":median(&mut raw_control_ms),
                "values_files_ms":median(&mut files_ms), "values_packed_ms":median(&mut packed_ms),
                "values_sharded_ms":median(&mut sharded_ms),
                "runs":runs
            })
        );
    }
    Ok(())
}
