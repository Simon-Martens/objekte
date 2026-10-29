//! Like-for-like Rust baseline for the full Musenalm Node.js data benchmark.
//! Reads authoritative JSON files into serde_json::Value; no source writes.
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;


// INFO: Musenalm record ID and complete JSON value for the full-read benchmark.
struct Row {
    id: String,
    data: Value,
}


fn secs(start: Instant) -> f64 {
    start.elapsed().as_secs_f64()
}

fn emit(metric: &str, seconds: f64, result: Value) {
    println!(
        "{}",
        json!({"metric":metric,"seconds":seconds,"result":result})
    );
}

fn memory() -> Value {
    let status = fs::read_to_string("/proc/self/status").unwrap_or_default();
    let kib = |name: &str| {
        status
            .lines()
            .find_map(|line| {
                line.strip_prefix(name)?
                    .split_whitespace()
                    .next()?
                    .parse::<u64>()
                    .ok()
            })
            .unwrap_or(0)
    };
    json!({"rss_mib":kib("VmRSS:") as f64 / 1024.0,"peak_rss_mib":kib("VmHWM:") as f64 / 1024.0})
}

fn paths(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut pending = vec![dir.to_owned()];
    let mut files = Vec::new();
    while let Some(folder) = pending.pop() {
        for entry in fs::read_dir(folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() && entry.path().extension().is_some_and(|e| e == "json") {
                files.push(entry.path());
            }
        }
    }
    files.sort_unstable();
    Ok(files)
}

fn load(dir: &Path) -> Result<Vec<Row>> {
    paths(dir)?
        .into_iter()
        .map(|file| {
            let id = file
                .file_stem()
                .ok_or("missing ID")?
                .to_string_lossy()
                .into_owned();
            let data = serde_json::from_slice(&fs::read(file)?)?;
            Ok(Row { id, data })
        })
        .collect()
}

fn measure(mut query: impl FnMut() -> Value, runs: usize) -> (f64, Value) {
    let _ = query();
    let mut times = Vec::with_capacity(runs);
    let mut output = Value::Null;
    for _ in 0..runs {
        let start = Instant::now();
        output = query();
        times.push(secs(start));
    }
    times.sort_by(f64::total_cmp);
    (times[times.len() / 2], output)
}

fn year(row: &Row) -> Option<i64> {
    row.data.get("year")?.as_i64()
}

fn in_century(row: &Row) -> bool {
    matches!(year(row), Some(1800..=1899))
}

fn edited_century(row: &Row) -> bool {
    in_century(row) && row.data["edit_state"] == "Edited"
}

fn title(row: &Row) -> &str {
    row.data["preferred_title"].as_str().unwrap_or("")
}

fn main() -> Result<()> {
    let mut repo = PathBuf::from("examples/musenalm");
    let mut runs = 7;
    let args: Vec<_> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--repo" => repo = PathBuf::from(args.get(i + 1).ok_or("missing --repo value")?),
            "--runs" => runs = args.get(i + 1).ok_or("missing --runs value")?.parse()?,
            other => return Err(format!("unknown option: {other}").into()),
        }
        i += 2;
    }
    if runs == 0 {
        return Err("--runs must be positive".into());
    }

    let names = [
        "entries", "agents", "series", "places", "items", "pages", "contents",
    ];
    let total = Instant::now();
    let mut collections = HashMap::new();
    for name in names {
        let start = Instant::now();
        let rows = load(&repo.join("data").join(name))?;
        emit(
            &format!("load_{name}"),
            secs(start),
            json!({"count":rows.len(),"memory":memory()}),
        );
        collections.insert(name, rows);
    }
    emit(
        "load_total",
        secs(total),
        json!({"count":collections.values().map(Vec::len).sum::<usize>(),"memory":memory()}),
    );

    let index_start = Instant::now();
    let contents = &collections["contents"];
    let entries = &collections["entries"];
    let agents = &collections["agents"];
    let items = &collections["items"];
    let content_by_id: HashMap<&str, &Row> = contents.iter().map(|r| (r.id.as_str(), r)).collect();
    let item_ids: HashSet<&str> = items.iter().map(|r| r.id.as_str()).collect();
    let agent_by_id: HashMap<&str, &Row> = agents.iter().map(|r| (r.id.as_str(), r)).collect();
    emit(
        "build_id_maps",
        secs(index_start),
        json!({"memory":memory()}),
    );

    type Query<'a> = (&'static str, Box<dyn Fn() -> Value + 'a>);
    let queries: [Query<'_>; 10] = [
        (
            "content_year_count",
            Box::new(|| {
                json!(
                    contents
                        .iter()
                        .filter(|r| in_century(r))
                        .collect::<Vec<_>>()
                        .len()
                )
            }),
        ),
        (
            "content_edited_count",
            Box::new(|| {
                json!(
                    contents
                        .iter()
                        .filter(|r| edited_century(r))
                        .collect::<Vec<_>>()
                        .len()
                )
            }),
        ),
        (
            "content_title_page",
            Box::new(|| {
                let mut rows: Vec<_> = contents.iter().filter(|r| edited_century(r)).collect();
                rows.sort_unstable_by(|a, b| title(a).cmp(title(b)).then(a.id.cmp(&b.id)));
                json!(
                    rows[100..150]
                        .iter()
                        .map(|r| json!([r.id, title(r), year(r)]))
                        .collect::<Vec<_>>()
                )
            }),
        ),
        (
            "content_type_facets",
            Box::new(|| {
                let mut counts = BTreeMap::<&str, usize>::new();
                for row in contents.iter().filter(|r| edited_century(r)) {
                    if let Some(types) = row.data["musenalm_type"].as_array() {
                        for ty in types.iter().filter_map(Value::as_str) {
                            *counts.entry(ty).or_default() += 1;
                        }
                    }
                }
                json!(counts.into_iter().collect::<Vec<_>>())
            }),
        ),
        (
            "content_title_search_count",
            Box::new(|| {
                json!(
                    contents
                        .iter()
                        .filter(|r| title(r).to_lowercase().contains("almanach"))
                        .count()
                )
            }),
        ),
        (
            "content_text_search_count",
            Box::new(|| {
                json!(
                    contents
                        .iter()
                        .filter(|r| {
                            format!(
                                "{}\n{}",
                                title(r),
                                r.data["annotation"].as_str().unwrap_or("")
                            )
                            .to_lowercase()
                            .contains("almanach")
                        })
                        .count()
                )
            }),
        ),
        (
            "content_id_lookup",
            Box::new(|| {
                let row = content_by_id[contents[contents.len() / 2].id.as_str()];
                json!([[row.id, title(row), year(row)]])
            }),
        ),
        (
            "entry_content_links",
            Box::new(|| {
                let mut links = 0;
                let mut resolved = 0;
                for entry in entries {
                    if let Some(nested) = entry.data["contents"].as_array() {
                        for child in nested {
                            links += 1;
                            if child["source_id"]
                                .as_str()
                                .is_some_and(|id| content_by_id.contains_key(id))
                            {
                                resolved += 1;
                            }
                        }
                    }
                }
                json!([links, resolved, links - resolved])
            }),
        ),
        (
            "entry_item_links",
            Box::new(|| {
                let mut links = 0;
                let mut resolved = 0;
                for entry in entries {
                    if let Some(nested) = entry.data["items"].as_array() {
                        for child in nested {
                            links += 1;
                            if child["source_id"]
                                .as_str()
                                .is_some_and(|id| item_ids.contains(id))
                            {
                                resolved += 1;
                            }
                        }
                    }
                }
                json!([links, resolved, links - resolved])
            }),
        ),
        (
            "top_content_contributors",
            Box::new(|| {
                let mut links: HashMap<&str, HashSet<&str>> = HashMap::new();
                for row in contents {
                    if let Some(contributors) = row.data["contributors"].as_array() {
                        for contributor in contributors {
                            if let Some(id) =
                                contributor["agents"].as_str().filter(|s| !s.is_empty())
                            {
                                links.entry(id).or_default().insert(&row.id);
                            }
                        }
                    }
                }
                let mut result: Vec<_> = links
                    .into_iter()
                    .map(|(id, linked)| {
                        (
                            id,
                            agent_by_id.get(id).and_then(|r| r.data["name"].as_str()),
                            linked.len(),
                        )
                    })
                    .collect();
                result.sort_unstable_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(b.0)));
                json!(result.into_iter().take(20).collect::<Vec<_>>())
            }),
        ),
    ];
    for (name, query) in queries {
        let (time, result) = measure(query, runs);
        emit(name, time, result);
    }

    let view_start = Instant::now();
    let mut view: Vec<_> = contents.iter().filter(|r| edited_century(r)).collect();
    view.sort_unstable_by(|a, b| title(a).cmp(title(b)).then(a.id.cmp(&b.id)));
    emit(
        "build_content_title_view",
        secs(view_start),
        json!({"count":view.len(),"memory":memory()}),
    );
    let (time, result) = measure(
        || {
            json!(
                view[100..150]
                    .iter()
                    .map(|r| json!([r.id, title(r), year(r)]))
                    .collect::<Vec<_>>()
            )
        },
        runs,
    );
    emit("content_title_page_cached", time, result);
    emit("end_memory", 0.0, memory());
    Ok(())
}
