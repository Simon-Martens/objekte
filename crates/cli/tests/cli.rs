#[path = "../../core/tests/support/mod.rs"]
mod support;
use serde_json::{Value, json};
use std::{
    fs,
    fs::OpenOptions,
    process::{Command, Output, Stdio},
    time::{Duration, Instant},
};
use support::*;
fn run(f: &Fixture, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_objekte"));
    c.args(args).arg("--data-dir").arg(&f.data);
    c.output().unwrap()
}

fn value(out: Output) -> Value {
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).unwrap()
}

fn diagnostics(out: &Output) -> Value {
    let entries = String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|entry| entry["type"] == "diagnostics")
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    entries.into_iter().next().unwrap()
}

fn sql_events(out: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&out.stderr)
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .filter(|entry| entry["type"] == "sql")
        .collect()
}

#[test]
fn help_version_and_invalid_arguments() {
    for arg in ["--help", "--version"] {
        let o = Command::new(env!("CARGO_BIN_EXE_objekte"))
            .arg(arg)
            .output()
            .unwrap();
        assert!(o.status.success());
        assert!(String::from_utf8_lossy(&o.stdout).contains("objekte"));
        if arg == "--help" {
            assert!(
                String::from_utf8_lossy(&o.stdout).contains("objekte REPOSITORY view COLLECTION")
            );
        }
    }
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    for args in [vec![p], vec!["view", p, "books"]] {
        let out = run(&f, &args);
        assert_eq!(out.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&out.stderr).contains("objekte REPOSITORY COMMAND"));
    }
    for args in [
        vec![p, "view"],
        vec![p, "cache"],
        vec![p, "cache", "unknown"],
        vec![p, "cache", "status", "--collection", "books"],
        vec![p, "cache", "refresh-config", "--collection", "books"],
        vec![p, "validate", "books", "people"],
        vec![p, "validate", "missing"],
        vec![p, "validate", "--collection", "books"],
        vec![p, "view", "books", "--validation", "background"],
        vec![p, "query"],
        vec![p, "query", "missing", "score=1"],
        vec![p, "query", "books", "score=1", "extra"],
        vec![p, "query", "books", "score=1", "--collection", "people"],
        vec![p, "query", "books", "score=1", "--collection", "books"],
        vec![p, "query", "score=1", "--param", "n=wrong"],
        vec![p, "query", "score=1", "--param", "n=1", "--param", "n=2"],
        vec![p, "inspect", "--param", "n=1"],
        vec![p, "view", "books", "--limt", "2"],
        vec![p, "view", "books", "--limit", "0"],
        vec![p, "view", "books", "--sort", "payload"],
        vec![p, "get", "books", "../people/p1"],
        vec![p, "view", "books", "--direction", "sideways"],
        vec![p, "inspect", "--sort", "score"],
        vec![p, "view", "books", "--filter", "score=wrong"],
        vec![p, "view", "books", "--namespace", "../oops"],
        vec![p, "view", "books", "--limit"],
        vec![p, "relations", "--collections", "--check-targets"],
        vec![p, "view", "books", "--sqlite-cache-mib"],
        vec![p, "view", "books", "--sqlite-cache-mib", "0"],
        vec![p, "view", "books", "--sqlite-cache-mib", "-1"],
        vec![p, "view", "books", "--sqlite-cache-mib", "2097152"],
        vec![p, "view", "books", "--sqlite-cache-mib", "huge"],
        vec![p, "view", "books", "--ingest-workers", "0"],
        vec![p, "view", "books", "--background-workers", "0"],
        vec![p, "view", "books", "--ingest-workers", "many"],
    ] {
        assert!(!run(&f, &args).status.success(), "{args:?}");
    }
}

#[test]
fn configuration_manifest_is_shared_across_cli_processes() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let first = run(&f, &[p, "cache", "refresh-config", "--diagnostics"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let diagnostics = diagnostics(&first);
    assert_eq!(diagnostics["metrics"]["source_reads"], 0);
    assert_eq!(diagnostics["metrics"]["enumerations"], 0);
    assert!(diagnostics["metrics"]["configuration_ms"].is_f64());
    assert_eq!(value(first)["inputs"].as_array().unwrap().len(), 11);
    f.edit(".editor/books/ui.json", |v| {
        v["label"] = json!("Changed while closed")
    });
    let changes = value(run(&f, &[p, "cache", "refresh-config"]));
    assert_eq!(changes["affected_collections"], json!(["books"]));
    assert_eq!(changes["inputs"][0]["kind"], "modified");
    assert_ne!(
        changes["inputs"][0]["previous_digest"],
        changes["inputs"][0]["digest"]
    );
    let status = value(run(&f, &[p, "cache", "status"]));
    assert_eq!(status["configuration_changes"]["inputs"], json!([]));
    assert!(
        status["collections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["state"] == "absent")
    );
}

#[test]
fn sqlite_page_cache_budget_is_configurable_and_reported() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    for (budget, expected) in [
        ("default", Value::Null),
        ("16", json!(16)),
        ("64", json!(64)),
    ] {
        let output = run(
            &f,
            &[
                p,
                "view",
                "books",
                "--sqlite-cache-mib",
                budget,
                "--ingest-workers",
                "2",
                "--output",
                "json",
                "--diagnostics",
            ],
        );
        assert!(output.status.success());
        let diagnostics = diagnostics(&output);
        assert_eq!(diagnostics["sqlite_cache_mib"], expected);
        assert_eq!(diagnostics["ingest_workers"], 2);
        assert_eq!(value(output)["rows"].as_array().unwrap().len(), 3);
    }
}

#[test]
fn inspect_graph_and_status_are_lazy() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let model = value(run(&f, &[p, "inspect", "--collections", "--indexes"]));
    assert_eq!(model["collections"].as_object().unwrap().len(), 3);
    assert!(!f.data.exists());
    let graph = value(run(&f, &[p, "relations", "--collections"]));
    assert_eq!(graph["cycles"], json!([["books", "people"]]));
    let status = value(run(&f, &[p, "cache", "status"]));
    assert!(
        status["collections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|c| c["state"] == "absent")
    );
    assert!(!f.data.exists());
}

#[test]
fn metadata_default_strict_mode_and_subsort_hashes_are_exposed() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    value(run(&f, &[p, "view", "books", "--output", "json"]));
    let fast = value(run(&f, &[p, "view", "books", "--output", "json"]));
    assert_eq!(fast["metrics"]["source_reads"], 0);
    assert_eq!(fast["metrics"]["manifest_hits"], 1);
    let strict = value(run(
        &f,
        &[
            p,
            "view",
            "books",
            "--reconcile",
            "hash-on-open",
            "--output",
            "json",
        ],
    ));
    assert_eq!(strict["metrics"]["hashes"], 7);
    let out = run(
        &f,
        &[
            p,
            "view",
            "books",
            "--sort",
            "status",
            "--then-sort",
            "score:desc",
            "--then-sort",
            "@id:desc",
            "--output",
            "json",
            "--diagnostics",
        ],
    );
    let diagnostics = diagnostics(&out);
    let ordered = value(out);
    assert_eq!(ordered["rows"][0]["id"], "c");
    assert_eq!(ordered["view_hashes"], diagnostics["view_hashes"]);
    assert_eq!(diagnostics["freshness"], "metadata_on_open");
    assert_ne!(ordered["view_hashes"]["sort"], fast["view_hashes"]["sort"]);
    for args in [
        vec![p, "inspect", "--then-sort", "score:asc"],
        vec![p, "view", "books", "--then-sort", "score:wrong"],
        vec![p, "view", "books", "--then-sort", "title"],
    ] {
        assert!(!run(&f, &args).status.success());
    }
}

#[test]
fn cli_views_filters_paging_json_and_ndjson_use_library_semantics() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let v = value(run(
        &f,
        &[
            p,
            "view",
            "books",
            "--sort",
            "score",
            "--direction",
            "desc",
            "--filter",
            "score>=2",
            "--limit",
            "2",
            "--count",
            "--output",
            "json",
        ],
    ));
    assert_eq!(v["total"], 3);
    assert_eq!(v["rows"][0]["id"], "c");
    assert_eq!(v["rows"][1]["id"], "a");
    let out = run(
        &f,
        &[p, "view", "books", "--limit", "2", "--output", "ndjson"],
    );
    assert!(out.status.success());
    let lines = String::from_utf8(out.stdout)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["id"], "f");
}

#[test]
fn table_output_preserves_literal_markup_and_escapes_control_characters() {
    let f = Fixture::new();
    f.edit("data/books/a.json", |v| {
        v["title"] = json!("<b>HTML</b>\n**MD**\u{1b}[31m")
    });
    let p = f.root.to_str().unwrap();
    let o = run(
        &f,
        &[
            p,
            "view",
            "books",
            "--filter",
            "@id=a",
            "--columns",
            "title",
            "--output",
            "table",
        ],
    );
    assert!(o.status.success());
    let text = String::from_utf8(o.stdout).unwrap();
    assert!(text.contains("<b>HTML</b>\\n**MD**"));
    assert!(!text.contains('\u{1b}'));
}

#[test]
fn reopen_refresh_and_fresh_namespace_are_observable() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let first = value(run(&f, &[p, "view", "books", "--output", "json"]));
    assert_eq!(first["metrics"]["rows_inserted"], 7);
    let reopened = value(run(&f, &[p, "view", "books", "--output", "json"]));
    assert_eq!(reopened["metrics"]["rows_inserted"], 0);
    assert_eq!(reopened["metrics"]["parses"], 0);
    f.edit("data/books/a.json", |v| v["score"] = json!(-99));
    let m = value(run(&f, &[p, "cache", "refresh", "--collection", "books"]));
    assert_eq!(m["rows_updated"], 1);
    let o = run(
        &f,
        &[
            p,
            "view",
            "books",
            "--fresh-cache",
            "--output",
            "json",
            "--diagnostics",
        ],
    );
    let diagnostic = diagnostics(&o);
    assert!(
        diagnostic["cache_directory"]
            .as_str()
            .unwrap()
            .contains("run-")
    );
    assert_eq!(value(o)["metrics"]["rows_inserted"], 7);
}

#[test]
fn separate_cli_processes_serialize_updates_to_one_cache() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    value(run(
        &f,
        &[
            p,
            "view",
            "books",
            "--output",
            "json",
        ],
    ));
    f.edit("data/books/a.json", |v| v["score"] = json!(-99));
    let status = value(run(&f, &[p, "cache", "status"]));
    let lock_path = std::path::Path::new(status["directory"].as_str().unwrap()).join("lock");
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(lock_path)
        .unwrap();
    fs2::FileExt::lock_exclusive(&lock).unwrap();
    let spawn = || {
        Command::new(env!("CARGO_BIN_EXE_objekte"))
            .args([
                p,
                "view",
                "books",
                "--count",
                "--output",
                "json",
                "--data-dir",
            ])
            .arg(&f.data)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    };
    let mut first = spawn();
    let mut second = spawn();
    std::thread::sleep(Duration::from_millis(100));
    assert!(first.try_wait().unwrap().is_none());
    assert!(second.try_wait().unwrap().is_none());
    fs2::FileExt::unlock(&lock).unwrap();
    let finish = |mut child: std::process::Child| {
        let deadline = Instant::now() + Duration::from_secs(15);
        while child.try_wait().unwrap().is_none() {
            if Instant::now() >= deadline {
                child.kill().unwrap();
                panic!("CLI process remained blocked after cache lock was released");
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        let output = child.wait_with_output().unwrap();
        let result = value(output);
        assert_eq!(result["total"], 7);
        assert_eq!(result["rows"][0]["id"], "a");
        result
    };
    let first = finish(first);
    let second = finish(second);
    assert_eq!(first["rows"], second["rows"]);
    assert_eq!(
        first["metrics"]["rows_updated"].as_u64().unwrap()
            + second["metrics"]["rows_updated"].as_u64().unwrap(),
        1
    );
}

#[test]
fn disabled_cache_writes_no_app_data_and_matches_sqlite() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let direct = value(run(
        &f,
        &[
            p, "view", "books", "--cache", "disabled", "--output", "json", "--count",
        ],
    ));
    assert!(!f.data.exists());
    let sql = value(run(
        &f,
        &[p, "view", "books", "--output", "json", "--count"],
    ));
    assert_eq!(direct["rows"], sql["rows"]);
    assert_eq!(direct["total"], sql["total"]);
}

#[test]
fn record_and_reference_commands_report_resolution_and_coverage() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let r = value(run(&f, &[p, "get", "books", "a"]));
    assert_eq!(r["data"]["title"], "Beta");
    let refs = value(run(
        &f,
        &[p, "relations", "--from", "books/b", "--check-targets"],
    ));
    assert!(
        refs["edges"]
            .as_array()
            .unwrap()
            .iter()
            .any(|e| e["target_state"] == "missing")
    );
    let refs = value(run(
        &f,
        &[p, "relations", "--to", "people/p1", "--complete"],
    ));
    assert_eq!(refs["edges"].as_array().unwrap().len(), 4);
    assert_eq!(refs["coverage"][0]["verified"], true);
}

#[test]
fn killing_a_cache_builder_leaves_recoverable_state() {
    let f = Fixture::new();
    for i in 0..5000 {
        f.write(&format!("data/books/generated{i}.json"),&json!({"payload":"unchanged source ".repeat(30),"title":format!("Generated {i}"),"score":i}));
    }
    let p = f.root.to_str().unwrap();
    let status = value(run(&f, &[p, "cache", "status"]));
    let dbpath =
        std::path::PathBuf::from(status["directory"].as_str().unwrap()).join("cache.sqlite");
    let mut child = Command::new(env!("CARGO_BIN_EXE_objekte"))
        .args([
            p,
            "view",
            "books",
            "--limit",
            "1",
            "--output",
            "json",
            "--data-dir",
        ])
        .arg(&f.data)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut saw_dirty = false;
    while Instant::now() < deadline {
        if dbpath.exists()
            && let Ok(db) = rusqlite::Connection::open(&dbpath)
        {
            db.busy_timeout(Duration::from_millis(10)).unwrap();
            if db
                .query_row(
                    "SELECT state FROM collections WHERE name='books'",
                    [],
                    |r| r.get::<_, String>(0),
                )
                .ok()
                .as_deref()
                == Some("dirty")
            {
                saw_dirty = true;
                break;
            }
        }
        if child.try_wait().unwrap().is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(saw_dirty, "builder did not expose in-progress state");
    let db = rusqlite::Connection::open(&dbpath).unwrap();
    objekte_core::register_cache_sqlite_functions(&db).unwrap();
    assert_eq!(
        db.query_row("PRAGMA integrity_check", [], |r| r.get::<_, String>(0))
            .unwrap(),
        "ok"
    );
    drop(db);
    let out = value(run(
        &f,
        &[
            p, "view", "books", "--count", "--limit", "1", "--output", "json",
        ],
    ));
    assert_eq!(out["total"], 5007);
    assert_eq!(
        fs::read_dir(f.root.join("data/books")).unwrap().count(),
        5007
    );
}

#[test]
fn query_completes_pending_validation_while_view_does_not() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    f.edit("data/books/a.json", |v| v["score"] = json!("wrong"));
    fs::write(f.root.join("data/books/b.json"), "broken").unwrap();
    let read = run(
        &f,
        &[
            p,
            "view",
            "books",
            "--limit",
            "50",
            "--output",
            "json",
            "--diagnostics",
        ],
    );
    assert!(!sql_events(&read).iter().any(|event| {
        event["sql"].as_str().unwrap().contains("WITH pending AS MATERIALIZED")
    }));
    let page = value(read);
    assert_eq!(page["states"]["a"]["status"], "pending");
    assert_eq!(page["states"]["b"]["status"], "incorrect_json");
    let query = run(&f, &[p, "query", "books", "score >= 0", "--diagnostics"]);
    assert!(sql_events(&query).iter().any(|event| {
        event["sql"].as_str().unwrap().contains("SELECT id,revision,digest,json FROM records")
    }));
    assert_eq!(value(query)["records"][0]["state"]["status"], "valid");
    let direct = run(&f, &[p, "query", "books", "score >= 0", "--cache", "disabled", "--diagnostics"]);
    assert!(diagnostics(&direct)["metrics"]["validations"].as_u64().unwrap() > 0);
    assert_eq!(value(direct)["records"][0]["state"]["status"], "valid");
    let output = run(
        &f,
        &[
            p,
            "validate",
            "books",
            "--background-workers",
            "2",
            "--diagnostics",
        ],
    );
    let diagnostic = diagnostics(&output);
    assert_eq!(diagnostic["background_workers"], 2);
    assert_eq!(diagnostic["validation_metrics"]["applied"], 0);
    assert_eq!(value(output)["applied"], 0);
    let page = value(run(&f, &[p, "view", "books", "--limit", "50", "--output", "json"]));
    assert_eq!(page["states"]["a"]["status"], "invalid");
    assert_eq!(page["states"]["b"]["status"], "incorrect_json");
    assert_eq!(
        value(run(&f, &[p, "cache", "validate", "--collection", "books"]))["applied"],
        0
    );
    let status = value(run(&f, &[p, "cache", "status"]));
    let books = status["collections"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["collection"] == "books")
        .unwrap();
    assert_eq!(books["validation"]["valid"], 5);
    assert_eq!(books["validation"]["invalid"], 1);
    let record = value(run(&f, &[p, "get", "books", "a"]));
    assert_eq!(record["state"]["status"], "invalid");
    assert_eq!(record["data"]["score"], "wrong");
}

fn assert_millisecond_metrics(metrics: &Value) {
    let fields = metrics.as_object().unwrap();
    assert!(fields.keys().any(|key| key.ends_with("_ms")));
    for (name, value) in fields {
        assert!(!name.ends_with("_nanos"), "unconverted timer: {name}");
        if name.ends_with("_ms") {
            assert!(value.is_f64(), "{name}: {value}");
            assert!(value.as_f64().unwrap() >= 0.0);
        } else {
            assert!(value.is_u64(), "counter {name}: {value}");
        }
    }
}

#[test]
fn validate_accepts_one_or_all_collections_and_waits() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    f.edit("data/books/a.json", |v| v["score"] = json!("wrong"));
    fs::write(f.root.join("data/books/b.json"), "broken").unwrap();
    let single = value(run(&f, &[p, "validate", "books"]));
    assert_eq!(single["applied"], 6);
    assert_millisecond_metrics(&single);
    let status = value(run(&f, &[p, "cache", "status"]));
    for collection in status["collections"].as_array().unwrap() {
        if collection["collection"] == "books" {
            assert_eq!(collection["validation"]["valid"], 5);
            assert_eq!(collection["validation"]["invalid"], 1);
            assert_eq!(collection["validation"]["incorrect_json"], 1);
            assert!(collection["validation"]["pending"].is_null());
        } else {
            assert_eq!(collection["state"], "absent");
        }
    }
    // The all-collections form must also reconcile later source edits.
    f.edit("data/books/a.json", |v| v["score"] = json!(2));
    let result = value(run(&f, &[p, "validate"]));
    assert!(result["applied"].as_u64().unwrap() > 1);
    assert_millisecond_metrics(&result);
    let status = value(run(&f, &[p, "cache", "status"]));
    for collection in status["collections"].as_array().unwrap() {
        assert_eq!(collection["state"], "current");
        assert!(collection["validation"]["pending"].is_null());
        if collection["collection"] == "books" {
            assert_eq!(collection["validation"]["valid"], 6);
        }
    }
    assert_eq!(value(run(&f, &[p, "validate"]))["applied"], 0);
}

#[test]
fn every_cli_metrics_output_uses_milliseconds_without_changing_record_fields() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    f.edit(".editor/books/schema.json", |v| {
        v["properties"]["duration_nanos"] = json!({"type":"integer"});
    });
    f.edit("data/books/a.json", |v| {
        v["duration_nanos"] = json!(1234567);
        v["payload"] = json!({"metrics":{"read_nanos":7654321}});
    });
    let out = run(
        &f,
        &[
            p,
            "view",
            "books",
            "--columns",
            "duration_nanos",
            "--limit",
            "50",
            "--output",
            "json",
            "--diagnostics",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let diagnostics = diagnostics(&out);
    let view = value(out);
    assert_eq!(view["metrics"], diagnostics["metrics"]);
    assert_millisecond_metrics(&view["metrics"]);
    assert_millisecond_metrics(&diagnostics["validation_metrics"]);
    assert!(diagnostics["validation_wait_ms"].is_f64());
    let row = view["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["id"] == "a")
        .unwrap();
    assert_eq!(row["values"]["duration_nanos"], 1234567);
    let record = value(run(&f, &[p, "get", "books", "a"]));
    assert_eq!(record["data"]["payload"]["metrics"]["read_nanos"], 7654321);
    for args in [
        vec![p, "cache", "refresh", "--collection", "books"],
        vec![p, "cache", "validate", "--collection", "books"],
        vec![p, "validate", "books"],
        vec![p, "validate"],
    ] {
        assert_millisecond_metrics(&value(run(&f, &args)));
    }
}

#[test]
fn validate_without_cache_keeps_application_directory_absent() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let out = run(&f, &[p, "validate", "--cache", "disabled", "--diagnostics"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let diagnostics = diagnostics(&out);
    assert!(diagnostics["metrics"]["validations"].as_u64().unwrap() > 7);
    assert_millisecond_metrics(&diagnostics["metrics"]);
    assert_millisecond_metrics(&value(out));
    assert!(!f.data.exists());
}

#[test]
fn arbitrary_json_queries_support_parameters_scope_and_millisecond_diagnostics() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    f.edit(
        "data/books/a.json",
        |v| v["payload"] = json!({"year":1800,"end":1900,"name":"ALPHA","tags":["poet","editor"]}),
    );
    f.edit("data/people/p1.json", |v| {
        v["payload"] = json!({"year":"1800"})
    });
    let out = run(
        &f,
        &[
            p,
            "query",
            "payload.year=$year && payload.tags ?= 'poet'",
            "--param",
            "year=1800",
            "--count",
            "--diagnostics",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let diagnostic = diagnostics(&out);
    let result = value(out);
    assert_millisecond_metrics(&result["metrics"]);
    assert_eq!(result["metrics"], diagnostic["metrics"]);
    assert!(diagnostic["validation_wait_ms"].as_f64().unwrap() > 0.0);
    assert_eq!(result["total"], 1);
    assert_eq!(result["records"][0]["collection"], "books");
    assert_eq!(result["records"][0]["id"], "a");
    assert_eq!(
        result["dependencies"]["collections"],
        json!(["books", "people", "spare"])
    );
    let strings = value(run(
        &f,
        &[
            p,
            "query",
            "payload.year=$year",
            "--param",
            r#"year="1800""#,
            "--count",
        ],
    ));
    assert_eq!(strings["total"], 0);
    let scoped = value(run(
        &f,
        &[
            p,
            "query",
            "exists(payload)",
            "--collection",
            "books",
            "--count",
        ],
    ));
    assert!(
        scoped["records"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["collection"] == "books")
    );
    for cache in ["sqlite", "disabled"] {
        let positional = value(run(
            &f,
            &[
                p,
                "query",
                "books",
                "exists(payload)",
                "--cache",
                cache,
                "--count",
            ],
        ));
        assert_eq!(positional["total"], scoped["total"]);
        assert_eq!(positional["dependencies"]["collections"], json!(["books"]));
        let identities = |result: &Value| {
            result["records"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| (r["collection"].clone(), r["id"].clone()))
                .collect::<Vec<_>>()
        };
        assert_eq!(identities(&positional), identities(&scoped));
    }
    let extended = value(run(
        &f,
        &[
            p,
            "query",
            "books",
            "payload.year < payload.end && payload.tags:length = 2 && payload.tags:each !~ 'admin%' && payload.name:lower = 'alpha'",
            "--count",
        ],
    ));
    assert_eq!(extended["total"], 1);
    assert_eq!(extended["records"][0]["id"], "a");
    let out = run(
        &f,
        &[
            p,
            "query",
            "payload.year=1800",
            "--cache",
            "disabled",
            "--output",
            "ndjson",
        ],
    );
    let record = value(out);
    assert_eq!(record["id"], "a");
}

#[test]
fn diagnostics_streams_every_executed_sql_statement_and_keeps_query_json_small() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let out = run(
        &f,
        &[
            p,
            "query",
            "books",
            "score >= 2",
            "--count",
            "--diagnostics",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let events = sql_events(&out);
    assert!(
        events.len() > 10,
        "expected cache setup, writes, and query SQL"
    );
    for event in &events {
        assert!(event["sql"].as_str().is_some_and(|sql| !sql.is_empty()));
        assert!(event["duration_ms"].as_f64().is_some_and(|ms| ms >= 0.0));
    }
    assert!(
        events
            .iter()
            .any(|event| event["sql"].as_str().unwrap().contains("PRAGMA"))
    );
    assert!(
        events
            .iter()
            .any(|event| event["sql"].as_str().unwrap().contains("INSERT INTO"))
    );
    assert!(events.iter().any(|event| {
        event["sql"]
            .as_str()
            .unwrap()
            .contains("WITH hits AS MATERIALIZED")
    }));
    assert_eq!(diagnostics(&out)["type"], "diagnostics");
    let result = value(out);
    assert!(result.get("sql").is_none());
    assert_eq!(result["total"], 3);

    let direct = run(
        &f,
        &[
            p,
            "query",
            "books",
            "score >= 2",
            "--cache",
            "disabled",
            "--diagnostics",
        ],
    );
    assert!(sql_events(&direct).is_empty());
    assert!(value(direct).get("sql").is_none());
}
