//! Command dispatch and library calls.

use crate::{
    args::{HELP, args},
    diagnostics::now_unix_ms,
    output::{
        collection_metrics_in_milliseconds, metrics_in_milliseconds, print_view, write_query_event,
    },
};
use objekte_core::{
    CacheMode, Direction, Freshness, JsonQuery, JsonQueryBackend, Options, Repository, Sort,
    ViewRequest,
};
use serde_json::json;
use std::{io::Write, time::Instant};

pub(super) fn run() -> Result<(), Box<dyn std::error::Error>> {
    let command_started = Instant::now();
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
                Some("status" | "refresh-config" | "refresh" | "warm" | "rebuild" | "validate")
            ) {
                return Err(
                    "cache requires status, refresh-config, refresh, warm, rebuild, or validate"
                        .into(),
                );
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
    let common = ["--cache", "--reconcile", "--output"];
    for key in a.values.keys() {
        let allowed = common.contains(&key.as_str())
            || match command {
                "view" => ["--sort", "--direction", "--columns", "--limit", "--offset"]
                    .contains(&key.as_str()),
                "query" => {
                    ["--collection", "--limit", "--offset", "--backend"].contains(&key.as_str())
                }
                "relations" => ["--from", "--to"].contains(&key.as_str()),
                "cache" => {
                    matches!(
                        a.pos[2].as_str(),
                        "refresh" | "warm" | "rebuild" | "validate"
                    ) && key == "--collection"
                }
                _ => false,
            };
        if !allowed {
            return Err(format!("{key} is not valid for {command}").into());
        }
    }
    for key in &a.flags {
        let allowed = key == "--diagnostics"
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
    let mut options = Options::new();
    options.cache = match get("--cache", "memory").as_str() {
        "memory" => CacheMode::Memory,
        "disabled" => CacheMode::Disabled,
        _ => return Err("cache must be memory or disabled".into()),
    };
    options.freshness = match get("--reconcile", "metadata-on-open").as_str() {
        "metadata-on-open" => Freshness::MetadataOnOpen,
        "metadata-on-request" => Freshness::MetadataOnRequest,
        "hash-on-request" => Freshness::HashOnRequest,
        _ => return Err("unknown reconciliation policy".into()),
    };
    let output = get("--output", if command == "view" { "table" } else { "json" });
    if !["table", "json", "ndjson", "events"].contains(&output.as_str()) {
        return Err("unknown output format".into());
    }
    let output_allowed = match command {
        "view" => matches!(output.as_str(), "table" | "json" | "ndjson"),
        "query" => matches!(output.as_str(), "json" | "ndjson" | "events"),
        _ => output == "json",
    };
    if !output_allowed {
        return Err("this output format is unavailable for the command".into());
    }
    let freshness = options.freshness;
    let mut validation_wait_nanos = 0;
    let repository_load_started = Instant::now();
    let mut repo = Repository::load_repository(&a.pos[0], options)?;
    let repository_load_ms = repository_load_started.elapsed().as_secs_f64() * 1_000.0;
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
                backend: match get("--backend", "auto").as_str() {
                    "auto" => JsonQueryBackend::Auto,
                    "direct" => JsonQueryBackend::Direct,
                    "grep" | "rg" => JsonQueryBackend::Grep,
                    "bigram" => JsonQueryBackend::Bigram,
                    _ => {
                        return Err("query backend must be auto, direct, grep, or bigram".into());
                    }
                },
            };
            if output == "events" {
                let stdout = std::io::stdout();
                let mut writer = stdout.lock();
                let mut delivered = 0;
                let streamed = repo.query_json_stream(&request, |ordinal, record| {
                    write_query_event(
                        &mut writer,
                        &json!({"type":"record","ordinal":ordinal,"provisional":true,"record":record}),
                    )?;
                    delivered = ordinal;
                    Ok(())
                });
                match streamed {
                    Ok(summary) => {
                        let mut event = serde_json::to_value(summary)?;
                        event["type"] = json!("complete");
                        event["provisional"] = json!(false);
                        event["metrics"] = metrics_in_milliseconds(event["metrics"].take());
                        write_query_event(&mut writer, &event)?;
                    }
                    Err(error) => {
                        let _ = write_query_event(
                            &mut writer,
                            &json!({"type":"error","message":error.to_string(),"provisional_records":delivered}),
                        );
                        return Err(error.into());
                    }
                }
                None
            } else {
                let mut result = repo.query_json(&request)?;
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
            let record = repo.get_record(&a.pos[2], &a.pos[3])?;
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
            } else if a.pos[2] == "warm" || a.pos[2] == "rebuild" {
                let metrics = match (a.pos[2].as_str(), a.values.get("--collection")) {
                    ("warm", Some(c)) => serde_json::to_value(repo.warm_bigram_index(c)?)?,
                    ("warm", None) => serde_json::to_value(repo.warm_all_bigram_indexes()?)?,
                    ("rebuild", Some(c)) => serde_json::to_value(repo.rebuild_bigram_index(c)?)?,
                    ("rebuild", None) => serde_json::to_value(repo.rebuild_all_bigram_indexes()?)?,
                    _ => unreachable!(),
                };
                Some(if a.values.contains_key("--collection") {
                    metrics_in_milliseconds(metrics)
                } else {
                    collection_metrics_in_milliseconds(metrics)
                })
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
    if let Some(value) = value {
        println!("{}", serde_json::to_string_pretty(&value)?)
    }
    if a.flags.contains("--diagnostics") {
        let command_elapsed_ms = command_started.elapsed().as_secs_f64() * 1_000.0;
        let mut diagnostics = json!({"type":"diagnostics","level":"info","timestamp_unix_ms":now_unix_ms(),"command_elapsed_ms":command_elapsed_ms,"repository_load_ms":repository_load_ms,"metrics":metrics_in_milliseconds(json!(repo.metrics())),"configuration_changes":repo.configuration_changes(),"freshness":freshness,"validation_wait_ms":(validation_wait_nanos + repo.metrics().validation_wait_nanos as u128) as f64 / 1_000_000.0,"view_hashes":view_hashes});
        let drop_started = Instant::now();
        drop(repo);
        diagnostics["repository_drop_ms"] = json!(drop_started.elapsed().as_secs_f64() * 1_000.0);
        writeln!(
            std::io::stderr().lock(),
            "{}",
            serde_json::to_string_pretty(&diagnostics)?
        )?
    }
    Ok(())
}
