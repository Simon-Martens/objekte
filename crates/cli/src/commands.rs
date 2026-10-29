//! Command dispatch and library calls.

use crate::{
    args::{HELP, args, data_dir},
    output::{metrics_in_milliseconds, print_view},
};
use objekte_core::{
    CacheMode, Direction, Freshness, JsonQuery, Options, Repository, Sort, ValidationMode,
    ViewRequest,
};
use serde_json::json;
use std::{
    io::Write,
    path::PathBuf,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

fn wait_for_validation(repo: &Repository, elapsed_nanos: &mut u128) -> objekte_core::Result<()> {
    let start = Instant::now();
    repo.wait_for_validation()?;
    *elapsed_nanos += start.elapsed().as_nanos();
    Ok(())
}

pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
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
    options.sql_diagnostics = a.flags.contains("--diagnostics");
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
                wait_for_validation(&repo, &mut validation_wait_nanos)?;
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
                value.as_object_mut().unwrap().remove("sql");
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
                wait_for_validation(&repo, &mut validation_wait_nanos)?;
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
                wait_for_validation(&repo, &mut validation_wait_nanos)?;
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
    if validation == "wait" || (a.flags.contains("--diagnostics") && validation == "background") {
        // INFO: Finish background validation while tracing is active so every
        // SQLite statement from this CLI invocation reaches diagnostics.
        wait_for_validation(&repo, &mut validation_wait_nanos)?;
    }
    if let Some(value) = value {
        println!("{}", serde_json::to_string_pretty(&value)?)
    }
    if a.flags.contains("--diagnostics") {
        writeln!(
            std::io::stderr().lock(),
            "{}",
            json!({"type":"diagnostics","metrics":metrics_in_milliseconds(json!(repo.metrics())),"configuration_changes":repo.configuration_changes(),"cache_directory":repo.cache_directory()?,"freshness":freshness,"sqlite_cache_mib":sqlite_cache_mib,"ingest_workers":ingest_workers,"background_workers":background_workers,"validation":validation,"validation_metrics":metrics_in_milliseconds(json!(repo.validation_metrics())),"validation_wait_ms":validation_wait_nanos as f64 / 1_000_000.0,"view_hashes":view_hashes})
        )?
    }
    Ok(())
}
