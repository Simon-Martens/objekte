use objekte_core::{
    CacheMode, Direction, Freshness, JsonQuery, Options, Repository, Sort, ValidationMode,
    ViewRequest, ViewResult,
};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

const HELP: &str = "objekte — read-only file repository library driver

Usage:
  objekte REPOSITORY view COLLECTION [--sort FIELD] [--direction asc|desc]
      [--then-sort FIELD:asc|desc]
      [--filter FIELD=VALUE] [--columns a,b] [--limit N] [--offset N] [--count]
  objekte REPOSITORY get COLLECTION ID
  objekte REPOSITORY validate [COLLECTION]
  objekte REPOSITORY query [COLLECTION] FILTER [--param NAME=JSON]
      [--limit N] [--offset N] [--count]
  objekte REPOSITORY inspect [--collections] [--indexes]
  objekte REPOSITORY relations --collections
  objekte REPOSITORY relations --from COLLECTION/ID [--check-targets]
  objekte REPOSITORY relations --to COLLECTION/ID [--complete] [--check-targets]
  objekte REPOSITORY cache status
  objekte REPOSITORY cache refresh-config
  objekte REPOSITORY cache refresh --collection NAME
  objekte REPOSITORY cache validate --collection NAME

Options:
  --data-dir PATH                 Application state directory
  --cache sqlite|disabled         Default: sqlite
  --sqlite-cache-mib N|default     SQLite page-cache budget per connection (default: 64 MiB)
  --ingest-workers N              Fresh-cache workers (default: CPU threads / 2, minimum 1)
  --reconcile metadata-on-open|metadata-on-request|hash-on-open|hash-on-request
                                 Default: metadata-on-open
  --namespace NAME                Reuse a named cache (default: default)
  --fresh-cache                   Use a new namespace
  --output table|json|ndjson       Views default to table, other commands to json; timings in ms
  --background-workers N          Validation workers (default: CPU threads / 4, minimum 1)
  --validation background|manual|wait  Validation policy (default: background)
  --diagnostics                   Emit counters and floating-point millisecond timings to stderr
  --collection NAME               Query scope (alternative to positional COLLECTION); cache target
  --param NAME=JSON                Bind a typed query value (repeatable; quote JSON strings)
  --help, --version

Examples:
  objekte examples/musenalm view places --data-dir /tmp/objekte-a
  objekte examples/musenalm view contents --sort numbering --limit 20 --diagnostics
  objekte examples/musenalm view contents --filter 'year>=1800' --output json
  objekte examples/musenalm query contents 'year >= 1800' --count
";
struct Args {
    pos: Vec<String>,
    values: BTreeMap<String, String>,
    flags: BTreeSet<String>,
    filters: Vec<String>,
    sub_sorts: Vec<String>,
    parameters: BTreeMap<String, Value>,
}

fn metrics_in_milliseconds(metrics: Value) -> Value {
    let Value::Object(fields) = metrics else {
        unreachable!("library metrics serialize as an object")
    };
    Value::Object(
        fields
            .into_iter()
            .map(|(name, value)| {
                if let Some(phase) = name.strip_suffix("_nanos") {
                    let nanos = value.as_u64().expect("library timers are u64 nanoseconds");
                    (format!("{phase}_ms"), json!(nanos as f64 / 1_000_000.0))
                } else {
                    (name, value)
                }
            })
            .collect(),
    )
}

fn args() -> Result<Args, String> {
    let mut out = Args {
        pos: Vec::new(),
        values: BTreeMap::new(),
        flags: BTreeSet::new(),
        filters: Vec::new(),
        sub_sorts: Vec::new(),
        parameters: BTreeMap::new(),
    };
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--data-dir"
            | "--cache"
            | "--reconcile"
            | "--namespace"
            | "--output"
            | "--sort"
            | "--direction"
            | "--columns"
            | "--limit"
            | "--offset"
            | "--collection"
            | "--from"
            | "--to"
            | "--filter"
            | "--param"
            | "--then-sort"
            | "--sqlite-cache-mib"
            | "--ingest-workers"
            | "--background-workers"
            | "--validation" => {
                let v = args
                    .next()
                    .filter(|s| !s.starts_with("--"))
                    .ok_or_else(|| format!("{a} requires a value"))?;
                if a == "--param" {
                    let (name, raw) = v.split_once('=').ok_or("--param requires NAME=JSON")?;
                    if name.is_empty() {
                        return Err("--param requires a name".into());
                    }
                    let value = serde_json::from_str(raw)
                        .map_err(|e| format!("invalid --param JSON: {e}"))?;
                    if out.parameters.insert(name.into(), value).is_some() {
                        return Err(format!("duplicate parameter {name}"));
                    }
                } else if a == "--filter" {
                    out.filters.push(v)
                } else if a == "--then-sort" {
                    out.sub_sorts.push(v)
                } else if out.values.insert(a.clone(), v).is_some() {
                    return Err(format!("duplicate option {a}"));
                }
            }
            "--collections" | "--indexes" | "--count" | "--complete" | "--check-targets"
            | "--fresh-cache" | "--diagnostics" => {
                out.flags.insert(a);
            }
            "--help" | "-h" => {
                print!("{HELP}");
                std::process::exit(0)
            }
            "--version" | "-V" => {
                println!("objekte {}", env!("CARGO_PKG_VERSION"));
                std::process::exit(0)
            }
            a if a.starts_with('-') => return Err(format!("unknown option {a}")),
            _ => out.pos.push(a),
        }
    }
    Ok(out)
}
fn data_dir() -> Result<PathBuf, String> {
    if let Some(p) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(p).join("objekte"));
    }
    if cfg!(windows) {
        return std::env::var_os("LOCALAPPDATA")
            .map(|p| PathBuf::from(p).join("objekte"))
            .ok_or_else(|| "set --data-dir".into());
    }
    let home = std::env::var_os("HOME").ok_or_else(|| "set --data-dir".to_owned())?;
    Ok(PathBuf::from(home).join(if cfg!(target_os = "macos") {
        "Library/Application Support/objekte"
    } else {
        ".local/share/objekte"
    }))
}
fn cell(value: &Value) -> String {
    let text = value.to_string();
    let mut chars = text.chars();
    let mut out = chars.by_ref().take(100).collect::<String>();
    if chars.next().is_some() {
        out.push('…')
    }
    out
}
fn print_view(view: &ViewResult, output: &str) -> Result<(), Box<dyn std::error::Error>> {
    match output {
        "json" => {
            let mut value = serde_json::to_value(view)?;
            value["metrics"] = metrics_in_milliseconds(value["metrics"].take());
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        "ndjson" => {
            for row in &view.rows {
                let mut value = serde_json::to_value(row)?;
                value["state"] = serde_json::to_value(&view.states[&row.id])?;
                println!("{}", serde_json::to_string(&value)?)
            }
        }
        "table" => {
            let mut headers = vec!["ID".to_owned(), "STATUS".to_owned()];
            headers.extend(view.columns.iter().map(|s| cell(&json!(s))));
            let rows = view
                .rows
                .iter()
                .map(|r| {
                    [
                        cell(&json!(r.id)),
                        cell(&serde_json::to_value(view.states[&r.id].status).unwrap()),
                    ]
                    .into_iter()
                    .chain(view.columns.iter().map(|f| {
                        r.values
                            .get(f)
                            .map(cell)
                            .unwrap_or_else(|| "<missing>".into())
                    }))
                    .collect::<Vec<_>>()
                })
                .collect::<Vec<_>>();
            let widths = (0..headers.len())
                .map(|i| {
                    std::iter::once(headers[i].chars().count())
                        .chain(rows.iter().map(|r| r[i].chars().count()))
                        .max()
                        .unwrap()
                })
                .collect::<Vec<_>>();
            let print = |row: &[String]| {
                println!(
                    "{}",
                    row.iter()
                        .enumerate()
                        .map(|(i, s)| format!("{}{}", s, " ".repeat(widths[i] - s.chars().count())))
                        .collect::<Vec<_>>()
                        .join(" | ")
                )
            };
            print(&headers);
            println!(
                "{}",
                widths
                    .iter()
                    .map(|w| "-".repeat(*w))
                    .collect::<Vec<_>>()
                    .join("-+-")
            );
            for row in rows {
                print(&row)
            }
            if let Some(total) = view.total {
                println!("{} rows shown; {total} matching records", view.rows.len())
            }
        }
        _ => return Err("output must be table, json, or ndjson".into()),
    }
    Ok(())
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let a = args()?;
    if a.pos.is_empty() {
        print!("{HELP}");
        return Ok(());
    }
    let command = a
        .pos
        .get(1)
        .ok_or(
            "expected a command after REPOSITORY; usage: objekte REPOSITORY COMMAND; run --help",
        )?
        .as_str();
    let expected = match command {
        "cache" => {
            if !matches!(
                a.pos.get(2).map(String::as_str),
                Some("status" | "refresh-config" | "refresh" | "validate")
            ) {
                return Err("cache requires status, refresh-config, refresh, or validate".into());
            }
            3
        }
        "validate" if a.pos.len() == 2 || a.pos.len() == 3 => a.pos.len(),
        "validate" => return Err("usage: objekte REPOSITORY validate [COLLECTION]".into()),
        "query" if a.pos.len() == 3 || a.pos.len() == 4 => a.pos.len(),
        "query" => return Err("usage: objekte REPOSITORY query [COLLECTION] FILTER".into()),
        "view" => 3,
        "get" => 4,
        "inspect" | "relations" => 2,
        _ => {
            return Err(format!(
                "unknown command {command}; usage: objekte REPOSITORY COMMAND; run --help"
            )
            .into());
        }
    };
    if a.pos.len() != expected {
        return Err(format!("wrong arguments for {command}; run --help").into());
    }
    if command == "query" && a.pos.len() == 4 && a.values.contains_key("--collection") {
        return Err("choose positional COLLECTION or --collection, not both".into());
    }
    let common = [
        "--data-dir",
        "--cache",
        "--sqlite-cache-mib",
        "--ingest-workers",
        "--background-workers",
        "--validation",
        "--reconcile",
        "--namespace",
        "--output",
    ];
    for key in a.values.keys() {
        let allowed = common.contains(&key.as_str())
            || match command {
                "view" => ["--sort", "--direction", "--columns", "--limit", "--offset"]
                    .contains(&key.as_str()),
                "query" => ["--collection", "--limit", "--offset"].contains(&key.as_str()),
                "relations" => ["--from", "--to"].contains(&key.as_str()),
                "cache" => {
                    matches!(a.pos[2].as_str(), "refresh" | "validate") && key == "--collection"
                }
                _ => false,
            };
        if !allowed {
            return Err(format!("{key} is not valid for {command}").into());
        }
    }
    for key in &a.flags {
        let allowed = ["--fresh-cache", "--diagnostics"].contains(&key.as_str())
            || match command {
                "view" | "query" => key == "--count",
                "inspect" => ["--collections", "--indexes"].contains(&key.as_str()),
                "relations" => {
                    ["--collections", "--complete", "--check-targets"].contains(&key.as_str())
                }
                _ => false,
            };
        if !allowed {
            return Err(format!("{key} is not valid for {command}").into());
        }
    }
    if command != "view" && (!a.filters.is_empty() || !a.sub_sorts.is_empty()) {
        return Err("filters and sub-sorts require view".into());
    }
    if command != "query" && !a.parameters.is_empty() {
        return Err("--param requires query".into());
    }
    let get =
        |key: &str, default: &str| a.values.get(key).cloned().unwrap_or_else(|| default.into());
    let mut options = Options::new(if let Some(p) = a.values.get("--data-dir") {
        PathBuf::from(p)
    } else {
        data_dir()?
    });
    options.cache = match get("--cache", "sqlite").as_str() {
        "sqlite" => CacheMode::Sqlite,
        "disabled" => CacheMode::Disabled,
        _ => return Err("cache must be sqlite or disabled".into()),
    };
    options.freshness = match get("--reconcile", "metadata-on-open").as_str() {
        "metadata-on-open" => Freshness::MetadataOnOpen,
        "metadata-on-request" => Freshness::MetadataOnRequest,
        "hash-on-open" => Freshness::HashOnOpen,
        "hash-on-request" => Freshness::HashOnRequest,
        _ => return Err("unknown reconciliation policy".into()),
    };
    if let Some(value) = a.values.get("--sqlite-cache-mib") {
        options.sqlite_cache_mib = if value == "default" {
            None
        } else {
            Some(
                value
                    .parse()
                    .map_err(|_| "--sqlite-cache-mib requires a positive integer or 'default'")?,
            )
        };
    }
    if let Some(value) = a.values.get("--ingest-workers") {
        options.ingest_workers = value
            .parse()
            .map_err(|_| "--ingest-workers requires a positive integer")?;
    }
    if let Some(value) = a.values.get("--background-workers") {
        options.background_workers = value
            .parse()
            .map_err(|_| "--background-workers requires a positive integer")?;
    }
    let validation = get("--validation", "background");
    options.validation = match validation.as_str() {
        "background" | "wait" => ValidationMode::Background,
        "manual" => ValidationMode::Manual,
        _ => return Err("--validation requires background, manual, or wait".into()),
    };
    options.namespace = get("--namespace", "default");
    if a.flags.contains("--fresh-cache") {
        if a.values.contains_key("--namespace") {
            return Err("choose --fresh-cache or --namespace".into());
        }
        options.namespace = format!(
            "run-{}-{}",
            std::process::id(),
            SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos()
        );
    }
    let output = get("--output", if command == "view" { "table" } else { "json" });
    if !["table", "json", "ndjson"].contains(&output.as_str()) {
        return Err("unknown output format".into());
    }
    if command != "view" && output != "json" && !(command == "query" && output == "ndjson") {
        return Err("this command supports --output json; ndjson requires view/query and table requires view".into());
    }
    let freshness = options.freshness;
    let sqlite_cache_mib = options.sqlite_cache_mib;
    let ingest_workers = options.ingest_workers;
    let background_workers = options.background_workers;
    let mut validation_wait_nanos = 0;
    let mut repo = Repository::load_repository(&a.pos[0], options)?;
    let mut view_hashes = None;
    let value = match command {
        "inspect" => Some(serde_json::to_value(repo.model())?),
        "query" => {
            let (filter, collection) = if a.pos.len() == 4 {
                (&a.pos[3], Some(&a.pos[2]))
            } else {
                (&a.pos[2], a.values.get("--collection"))
            };
            let request = JsonQuery {
                filter: filter.clone(),
                parameters: a.parameters.clone(),
                collections: collection.map(|c| vec![c.clone()]),
                limit: get("--limit", "50").parse()?,
                offset: get("--offset", "0").parse()?,
                count: a.flags.contains("--count"),
            };
            let mut result = repo.query_json(&request)?;
            if validation == "wait" {
                let t = std::time::Instant::now();
                repo.wait_for_validation()?;
                validation_wait_nanos += t.elapsed().as_nanos();
                result = repo.query_json(&request)?;
            }
            if output == "ndjson" {
                for record in &result.records {
                    println!("{}", serde_json::to_string(record)?);
                }
                None
            } else {
                result.metrics = repo.metrics().clone();
                let mut value = serde_json::to_value(result)?;
                value["metrics"] = metrics_in_milliseconds(value["metrics"].take());
                Some(value)
            }
        }
        "validate" => {
            let t = std::time::Instant::now();
            let result = if let Some(collection) = a.pos.get(2) {
                repo.validate_collection(collection)?
            } else {
                repo.validate_all()?
            };
            validation_wait_nanos += t.elapsed().as_nanos();
            Some(metrics_in_milliseconds(serde_json::to_value(result)?))
        }
        "get" => {
            let mut record = repo.get_record(&a.pos[2], &a.pos[3])?;
            if validation == "wait" {
                let t = std::time::Instant::now();
                repo.wait_for_validation()?;
                validation_wait_nanos += t.elapsed().as_nanos();
                record = repo.get_record(&a.pos[2], &a.pos[3])?;
            }
            Some(serde_json::to_value(record)?)
        }
        "view" => {
            let c = repo.model().collection(&a.pos[2])?;
            let sort = if a.values.contains_key("--sort") || a.values.contains_key("--direction") {
                Some(Sort {
                    field: get("--sort", &c.list.default_sort.field),
                    direction: match get("--direction", "asc").as_str() {
                        "asc" => Direction::Asc,
                        "desc" => Direction::Desc,
                        _ => return Err("direction must be asc or desc".into()),
                    },
                })
            } else {
                None
            };
            let request = ViewRequest {
                filters: a
                    .filters
                    .iter()
                    .map(|f| c.parse_filter(f))
                    .collect::<objekte_core::Result<_>>()?,
                sort,
                sub_sorts: if a.sub_sorts.is_empty() {
                    None
                } else {
                    Some(
                        a.sub_sorts
                            .iter()
                            .map(|s| {
                                let (field, direction) = s
                                    .rsplit_once(':')
                                    .ok_or("--then-sort requires FIELD:asc or FIELD:desc")?;
                                let direction = match direction {
                                    "asc" => Direction::Asc,
                                    "desc" => Direction::Desc,
                                    _ => return Err("invalid sub-sort direction"),
                                };
                                Ok(Sort {
                                    field: field.into(),
                                    direction,
                                })
                            })
                            .collect::<Result<Vec<_>, &str>>()?,
                    )
                },
                columns: a
                    .values
                    .get("--columns")
                    .map(|s| s.split(',').map(str::to_owned).collect()),
                limit: a.values.get("--limit").map(|s| s.parse()).transpose()?,
                offset: get("--offset", "0").parse()?,
                count: a.flags.contains("--count"),
            };
            let mut view = repo.open_view(&a.pos[2], &request)?;
            if validation == "wait" {
                let t = std::time::Instant::now();
                repo.wait_for_validation()?;
                validation_wait_nanos += t.elapsed().as_nanos();
                view = repo.open_view(&a.pos[2], &request)?;
            }
            view.metrics = repo.metrics().clone();
            view_hashes = Some(view.view_hashes.clone());
            print_view(&view, &output)?;
            None
        }
        "relations" => {
            let modes = usize::from(a.flags.contains("--collections"))
                + usize::from(a.values.contains_key("--from"))
                + usize::from(a.values.contains_key("--to"));
            if modes != 1 {
                return Err("relations requires exactly one of --collections, --from, --to".into());
            }
            if a.flags.contains("--collections") {
                if a.flags.contains("--complete") || a.flags.contains("--check-targets") {
                    return Err("record flags cannot be used with --collections".into());
                }
                Some(serde_json::to_value(&repo.model().graph)?)
            } else {
                let from = a.values.contains_key("--from");
                let key = if from { "--from" } else { "--to" };
                let (collection, id) = a.values[key]
                    .split_once('/')
                    .ok_or("reference must be COLLECTION/ID")?;
                let check = a.flags.contains("--check-targets");
                let refs = if from {
                    if a.flags.contains("--complete") {
                        return Err("--complete requires --to".into());
                    }
                    repo.references_from(collection, id, check)?
                } else {
                    repo.references_to(collection, id, a.flags.contains("--complete"), check)?
                };
                Some(serde_json::to_value(refs)?)
            }
        }
        "cache" => {
            if a.pos[2] == "status" {
                Some(serde_json::to_value(repo.cache_status()?)?)
            } else if a.pos[2] == "refresh-config" {
                Some(serde_json::to_value(repo.refresh_configuration()?)?)
            } else {
                let c = a
                    .values
                    .get("--collection")
                    .ok_or("refresh requires --collection")?;
                if a.pos[2] == "validate" {
                    let t = std::time::Instant::now();
                    let result = repo.validate_collection(c)?;
                    validation_wait_nanos += t.elapsed().as_nanos();
                    Some(metrics_in_milliseconds(serde_json::to_value(result)?))
                } else {
                    Some(metrics_in_milliseconds(serde_json::to_value(
                        repo.refresh(c)?,
                    )?))
                }
            }
        }
        _ => unreachable!(),
    };
    if validation == "wait" {
        let t = std::time::Instant::now();
        repo.wait_for_validation()?;
        validation_wait_nanos += t.elapsed().as_nanos();
    }
    if let Some(value) = value {
        println!("{}", serde_json::to_string_pretty(&value)?)
    }
    if a.flags.contains("--diagnostics") {
        eprintln!(
            "{}",
            json!({"metrics":metrics_in_milliseconds(json!(repo.metrics())),"configuration_changes":repo.configuration_changes(),"cache_directory":repo.cache_directory()?,"freshness":freshness,"sqlite_cache_mib":sqlite_cache_mib,"ingest_workers":ingest_workers,"background_workers":background_workers,"validation":validation,"validation_metrics":metrics_in_milliseconds(json!(repo.validation_metrics())),"validation_wait_ms":validation_wait_nanos as f64 / 1_000_000.0,"view_hashes":view_hashes})
        )
    }
    Ok(())
}
fn main() {
    if let Err(e) = run() {
        eprintln!("{e}");
        std::process::exit(2)
    }
}
