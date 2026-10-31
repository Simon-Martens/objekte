//! Rust projection baseline: parse every Musenalm data file, retain fields used by the views.
//! This is a selective in-memory representation, not a lossless repository cache.
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

type Result<T> = std::result::Result<T, Box<dyn Error>>;


// INFO: Selected Musenalm content fields for the projected-read benchmark.
#[derive(Deserialize)]
struct Content {
    entry: String,
    year: Option<i64>,
    #[serde(default)]
    edit_state: String,
    #[serde(default)]
    preferred_title: String,
    #[serde(default)]
    annotation: String,
    #[serde(default)]
    musenalm_type: Vec<String>,
    #[serde(default)]
    contributors: Vec<Contributor>,
}


// INFO: Contributor agent data read by the projected benchmark.
#[derive(Deserialize)]
struct Contributor {
    #[serde(default)]
    agents: String,
}


// INFO: Entry links to embedded item records in the projected benchmark.
#[derive(Deserialize)]
struct Entry {
    #[serde(default)]
    items: Vec<SourceRef>,
}


// INFO: Optional source record ID inside a projected entry.
#[derive(Deserialize)]
struct SourceRef {
    source_id: Option<String>,
}


// INFO: Agent name read by the projected benchmark.
#[derive(Deserialize)]
struct Agent {
    #[serde(default)]
    name: String,
}


// INFO: Placeholder payload for benchmark collections with no selected fields.
#[derive(Deserialize)]
struct Empty {}


// INFO: Record ID paired with a benchmark-specific projected payload.
struct Row<T> {
    id: String,
    data: T,
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
    json!({"rss_mib":kib("VmRSS:") as f64/1024.0,"peak_rss_mib":kib("VmHWM:") as f64/1024.0})
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

fn load<T: DeserializeOwned>(dir: &Path) -> Result<Vec<Row<T>>> {
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

fn century(row: &Row<Content>) -> bool {
    matches!(row.data.year, Some(1800..=1899))
}

fn edited(row: &Row<Content>) -> bool {
    century(row) && row.data.edit_state == "Edited"
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
    let root = repo.join("data");
    let total = Instant::now();
    let start = Instant::now();
    let entries: Vec<Row<Entry>> = load(&root.join("entries"))?;
    emit(
        "load_entries",
        secs(start),
        json!({"count":entries.len(),"memory":memory()}),
    );
    let start = Instant::now();
    let agents: Vec<Row<Agent>> = load(&root.join("agents"))?;
    emit(
        "load_agents",
        secs(start),
        json!({"count":agents.len(),"memory":memory()}),
    );
    let mut other_collections = HashMap::new();
    for name in ["series", "places", "items", "pages"] {
        let start = Instant::now();
        let rows: Vec<Row<Empty>> = load(&root.join(name))?;
        emit(
            &format!("load_{name}"),
            secs(start),
            json!({"count":rows.len(),"memory":memory()}),
        );
        other_collections.insert(name, rows);
    }
    let start = Instant::now();
    let contents: Vec<Row<Content>> = load(&root.join("contents"))?;
    emit(
        "load_contents",
        secs(start),
        json!({"count":contents.len(),"memory":memory()}),
    );
    let count = entries.len()
        + agents.len()
        + contents.len()
        + other_collections.values().map(Vec::len).sum::<usize>();
    emit(
        "load_total",
        secs(total),
        json!({"count":count,"memory":memory()}),
    );

    let start = Instant::now();
    let entry_ids: HashSet<&str> = entries.iter().map(|r| r.id.as_str()).collect();
    let item_ids: HashSet<&str> = other_collections["items"]
        .iter()
        .map(|r| r.id.as_str())
        .collect();
    let agent_by_id: HashMap<&str, &Row<Agent>> =
        agents.iter().map(|r| (r.id.as_str(), r)).collect();
    emit("build_id_maps", secs(start), json!({"memory":memory()}));

    type Query<'a> = (&'static str, Box<dyn Fn() -> Value + 'a>);
    let queries: [Query<'_>; 9] = [
        (
            "content_year_count",
            Box::new(|| {
                json!(
                    contents
                        .iter()
                        .filter(|r| century(r))
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
                        .filter(|r| edited(r))
                        .collect::<Vec<_>>()
                        .len()
                )
            }),
        ),
        (
            "content_title_page",
            Box::new(|| {
                let mut rows: Vec<_> = contents.iter().filter(|r| edited(r)).collect();
                rows.sort_unstable_by(|a, b| {
                    a.data
                        .preferred_title
                        .cmp(&b.data.preferred_title)
                        .then(a.id.cmp(&b.id))
                });
                json!(
                    rows[100..150]
                        .iter()
                        .map(|r| json!([r.id, r.data.preferred_title, r.data.year]))
                        .collect::<Vec<_>>()
                )
            }),
        ),
        (
            "content_type_facets",
            Box::new(|| {
                let mut counts = BTreeMap::<&str, usize>::new();
                for row in contents.iter().filter(|r| edited(r)) {
                    for ty in &row.data.musenalm_type {
                        *counts.entry(ty).or_default() += 1;
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
                        .filter(|r| r.data.preferred_title.to_lowercase().contains("almanach"))
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
                            format!("{}\n{}", r.data.preferred_title, r.data.annotation)
                                .to_lowercase()
                                .contains("almanach")
                        })
                        .count()
                )
            }),
        ),
        (
            "content_entry_links",
            Box::new(|| {
                let mut links = 0;
                let mut resolved = 0;
                for row in &contents {
                    links += 1;
                    if entry_ids.contains(row.data.entry.as_str()) {
                        resolved += 1;
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
                for row in &entries {
                    for child in &row.data.items {
                        links += 1;
                        if child
                            .source_id
                            .as_deref()
                            .is_some_and(|id| item_ids.contains(id))
                        {
                            resolved += 1;
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
                for row in &contents {
                    for contributor in &row.data.contributors {
                        if !contributor.agents.is_empty() {
                            links
                                .entry(&contributor.agents)
                                .or_default()
                                .insert(&row.id);
                        }
                    }
                }
                let mut out: Vec<_> = links
                    .into_iter()
                    .map(|(id, linked)| {
                        (
                            id,
                            agent_by_id.get(id).map(|r| r.data.name.as_str()),
                            linked.len(),
                        )
                    })
                    .collect();
                out.sort_unstable_by(|a, b| b.2.cmp(&a.2).then(a.0.cmp(b.0)));
                json!(out.into_iter().take(20).collect::<Vec<_>>())
            }),
        ),
    ];
    for (name, query) in queries {
        let (time, result) = measure(query, runs);
        emit(name, time, result);
    }
    let start = Instant::now();
    let mut view: Vec<_> = contents.iter().filter(|r| edited(r)).collect();
    view.sort_unstable_by(|a, b| {
        a.data
            .preferred_title
            .cmp(&b.data.preferred_title)
            .then(a.id.cmp(&b.id))
    });
    emit(
        "build_content_title_view",
        secs(start),
        json!({"count":view.len(),"memory":memory()}),
    );
    let (time, result) = measure(
        || {
            json!(
                view[100..150]
                    .iter()
                    .map(|r| json!([r.id, r.data.preferred_title, r.data.year]))
                    .collect::<Vec<_>>()
            )
        },
        runs,
    );
    emit("content_title_page_cached", time, result);
    emit("end_memory", 0.0, memory());
    Ok(())
}
