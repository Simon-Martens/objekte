#[path = "../../core/tests/support/mod.rs"]
mod support;
use serde_json::{Value, json};
use std::{
    fs,
    process::{Command, Output},
};
use support::*;
fn run(_f: &Fixture, args: &[&str]) -> Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_objekte"));
    c.args(args);
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
    let output = String::from_utf8_lossy(&out.stderr);
    assert!(output.starts_with("{\n  \""), "{output}");
    let diagnostics: Value = serde_json::from_slice(&out.stderr).unwrap();
    assert_eq!(diagnostics["type"], "diagnostics");
    assert_eq!(diagnostics["level"], "info");
    assert!(diagnostics["timestamp_unix_ms"].as_u64().unwrap() > 0);
    assert!(diagnostics["command_elapsed_ms"].as_f64().unwrap() >= 0.0);
    diagnostics
}

#[test]
fn singular_diagnostic_flag_reports_pretty_json_for_success_and_error() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let success = run(&f, &[p, "cache", "status", "--diagnostic"]);
    assert_eq!(diagnostics(&success)["level"], "info");
    let failed = run(&f, &[p, "view", "missing", "--diagnostic"]);
    assert!(!failed.status.success());
    let raw = String::from_utf8(failed.stderr).unwrap();
    assert!(raw.starts_with("{\n  \""));
    let report: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(report["type"], "diagnostics");
    assert_eq!(report["level"], "error");
    assert!(report["timestamp_unix_ms"].as_u64().unwrap() > 0);
    assert!(report["command_elapsed_ms"].as_f64().unwrap() >= 0.0);
    assert!(report["message"].as_str().unwrap().contains("missing"));
}

#[test]
fn cache_warm_and_rebuild_report_metrics_for_one_or_all_collections() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let single = value(run(&f, &[p, "cache", "warm", "--collection", "books"]));
    assert_millisecond_metrics(&single);
    let all = value(run(&f, &[p, "cache", "rebuild"]));
    assert_eq!(all.as_object().unwrap().len(), 3);
    for metrics in all.as_object().unwrap().values() {
        assert_millisecond_metrics(metrics);
    }
}

#[test]
fn default_cache_uses_memory_for_views_and_queries() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_objekte"))
        .args([
            p, "view", "books", "--sort", "score", "--limit", "2", "--count", "--output", "json",
        ])
        .output()
        .unwrap();
    let view = value(output);
    assert_eq!(view["total"], 7);
    assert_eq!(view["rows"].as_array().unwrap().len(), 2);
    assert!(
        view["query_plan"][0]
            .as_str()
            .unwrap()
            .contains("in-memory")
    );
    assert!(!f.data.join("repositories").exists());
    let status = Command::new(env!("CARGO_BIN_EXE_objekte"))
        .args([p, "cache", "status"])
        .output()
        .unwrap();
    assert_eq!(value(status)["mode"], "memory");
}

#[test]
fn query_accepts_undeclared_fields_and_collection_scope() {
    let f = Fixture::new();
    f.edit("data/books/a.json", |record| {
        record["unlisted"] = json!("needle")
    });
    f.edit("data/people/p1.json", |record| {
        record["unlisted"] = json!("needle")
    });
    let root = f.root.to_str().unwrap();

    let all = value(run(&f, &[root, "query", "unlisted = 'needle'", "--count"]));
    assert_eq!(all["total"], 2);

    let books = value(run(
        &f,
        &[root, "query", "books", "unlisted = 'needle'", "--count"],
    ));
    assert_eq!(books["total"], 1);
    assert_eq!(books["records"][0]["collection"], "books");

    let people = value(run(
        &f,
        &[
            root,
            "query",
            "unlisted = 'needle'",
            "--collection",
            "people",
            "--count",
        ],
    ));
    assert_eq!(people["total"], 1);
    assert_eq!(people["records"][0]["collection"], "people");
}

#[test]
fn query_opens_a_discovered_repository_without_configuration_or_schema() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("data/books/part")).unwrap();
    fs::write(temp.path().join("data/books/a.json"), r#"{"title":"Moon"}"#).unwrap();
    fs::write(
        temp.path().join("data/books/part/b.json"),
        r#"{"title":"Moon"}"#,
    )
    .unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_objekte"))
        .args([
            temp.path().to_str().unwrap(),
            "query",
            "title = 'Moon'",
            "--count",
        ])
        .output()
        .unwrap();
    let result = value(output);
    assert_eq!(result["total"], 2);
    assert_eq!(result["records"][0]["collection"], "books");
    assert_eq!(result["records"][1]["collection"], "books/part");
    assert_eq!(result["records"][0]["state"]["status"], "no_schema");
    let scoped = value(
        Command::new(env!("CARGO_BIN_EXE_objekte"))
            .args([
                temp.path().to_str().unwrap(),
                "query",
                "books/part",
                "title = 'Moon'",
                "--count",
            ])
            .output()
            .unwrap(),
    );
    assert_eq!(scoped["total"], 1);
    assert_eq!(scoped["records"][0]["id"], "b");
}

#[test]
fn default_schema_groups_nested_cli_records_and_accepts_relative_id() {
    let temp = tempfile::tempdir().unwrap();
    fs::create_dir_all(temp.path().join("data/books/part")).unwrap();
    fs::create_dir_all(temp.path().join(".editor/collections/books")).unwrap();
    fs::write(temp.path().join("data/books/a.json"), r#"{"title":"A"}"#).unwrap();
    fs::write(
        temp.path().join("data/books/part/a.json"),
        r#"{"title":"B"}"#,
    )
    .unwrap();
    fs::write(
        temp.path().join(".editor/collections/books/schema.json"),
        r#"{"type":"object","properties":{"title":{"type":"string"}}}"#,
    )
    .unwrap();
    let root = temp.path().to_str().unwrap();
    let query = value(
        Command::new(env!("CARGO_BIN_EXE_objekte"))
            .args([root, "query", "books", "title >= 'A'", "--count"])
            .output()
            .unwrap(),
    );
    assert_eq!(query["total"], 2);
    assert_eq!(query["records"][1]["id"], "part/a");
    let record = value(
        Command::new(env!("CARGO_BIN_EXE_objekte"))
            .args([root, "get", "books", "part/a"])
            .output()
            .unwrap(),
    );
    assert_eq!(record["path"], "data/books/part/a.json");
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
        vec![p, "view", "books", "--cache", "segments"],
        vec![p, "query", "books", "score=1", "--backend", "segments"],
        vec![p, "view", "books", "--limit"],
        vec![p, "relations", "--collections", "--check-targets"],
        vec![p, "view", "books", "--ingest-workers", "0"],
        vec![p, "view", "books", "--background-workers", "0"],
        vec![p, "view", "books", "--ingest-workers", "many"],
    ] {
        assert!(!run(&f, &args).status.success(), "{args:?}");
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
    assert_eq!(
        fast["metrics"]["source_reads"],
        fast["rows"].as_array().unwrap().len()
    );
    let strict = value(run(
        &f,
        &[
            p,
            "view",
            "books",
            "--reconcile",
            "hash-on-request",
            "--output",
            "json",
        ],
    ));
    assert_eq!(strict["rows"], fast["rows"]);
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
fn disabled_cache_writes_no_app_data_and_matches_memory() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let direct = value(run(
        &f,
        &[
            p, "view", "books", "--cache", "disabled", "--output", "json", "--count",
        ],
    ));
    assert!(!f.data.exists());
    let memory = value(run(
        &f,
        &[p, "view", "books", "--output", "json", "--count"],
    ));
    assert_eq!(direct["rows"], memory["rows"]);
    assert_eq!(direct["total"], memory["total"]);
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
fn query_and_view_validate_on_demand_without_stored_states() {
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
    let page = value(read);
    assert_eq!(page["states"]["a"]["status"], "invalid");
    assert_eq!(page["states"]["b"]["status"], "incorrect_json");
    let query = run(&f, &[p, "query", "books", "score >= 0", "--diagnostics"]);
    assert_eq!(value(query)["records"][0]["state"]["status"], "valid");
    let direct = run(
        &f,
        &[
            p,
            "query",
            "books",
            "score >= 0",
            "--cache",
            "disabled",
            "--diagnostics",
        ],
    );
    assert!(
        diagnostics(&direct)["metrics"]["validations"]
            .as_u64()
            .unwrap()
            > 0
    );
    assert_eq!(value(direct)["records"][0]["state"]["status"], "valid");
    let output = run(&f, &[p, "validate", "books", "--diagnostics"]);
    let diagnostic = diagnostics(&output);
    assert!(diagnostic["validation_wait_ms"].as_f64().unwrap() >= 0.0);
    assert_eq!(value(output)["applied"], 6);
    let page = value(run(
        &f,
        &[p, "view", "books", "--limit", "50", "--output", "json"],
    ));
    assert_eq!(page["states"]["a"]["status"], "invalid");
    assert_eq!(page["states"]["b"]["status"], "incorrect_json");
    assert_eq!(
        value(run(&f, &[p, "cache", "validate", "--collection", "books"]))["applied"],
        6
    );
    let record = value(run(&f, &[p, "get", "books", "a"]));
    assert_eq!(record["state"]["status"], "pending");
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
    assert!(
        status["collections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|collection| collection["state"] == "absent")
    );
    // The all-collections form must also reconcile later source edits.
    f.edit("data/books/a.json", |v| v["score"] = json!(2));
    let result = value(run(&f, &[p, "validate"]));
    assert!(result["applied"].as_u64().unwrap() > 1);
    assert_millisecond_metrics(&result);
    let status = value(run(&f, &[p, "cache", "status"]));
    assert!(
        status["collections"]
            .as_array()
            .unwrap()
            .iter()
            .all(|collection| collection["state"] == "absent")
    );
    assert!(
        value(run(&f, &[p, "validate"]))["applied"]
            .as_u64()
            .unwrap()
            > 0
    );
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
    assert!(diagnostic["metrics"]["validations"].as_u64().unwrap() > 0);
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
    assert_eq!(strings["total"], 1);
    assert_eq!(strings["records"][0]["collection"], "people");
    assert_eq!(strings["records"][0]["id"], "p1");
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
    for cache in ["memory", "disabled"] {
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
fn bigram_cli_backend_matches_default_query_results() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    let args = [
        p,
        "query",
        "books",
        "title ~ 'Alpha' && score >= 2",
        "--count",
    ];
    let expected = value(run(&f, &args));
    assert!(!f.data.exists());
    let actual_output = run(
        &f,
        &[
            p,
            "query",
            "books",
            "title ~ 'Alpha' && score >= 2",
            "--count",
            "--backend",
            "bigram",
            "--diagnostics",
        ],
    );
    let diagnostic = diagnostics(&actual_output);
    #[cfg(feature = "index-timings")]
    for phase in [
        "bigram_inventory_ms",
        "bigram_slots_ms",
        "bigram_scan_ms",
        "bigram_project_worker_ms",
        "bigram_add_worker_ms",
        "bigram_compress_ms",
        "bigram_typed_ms",
        "bigram_view_ms",
    ] {
        assert!(
            diagnostic["metrics"][phase].as_f64().unwrap() > 0.0,
            "{phase}"
        );
    }

    #[cfg(not(feature = "index-timings"))]
    assert!(diagnostic["metrics"].get("bigram_inventory_ms").is_none());
    let actual = value(actual_output);
    assert_eq!(actual["total"], 1);
    assert_eq!(actual["total"], expected["total"]);
    assert_eq!(actual["records"], expected["records"]);
    assert_eq!(actual["query_plan"], expected["query_plan"]);
    assert!(
        expected["query_plan"][0]
            .as_str()
            .unwrap()
            .contains("bigram")
    );
    let direct = value(run(
        &f,
        &[
            p,
            "query",
            "books",
            "title ~ 'Alpha' && score >= 2",
            "--count",
            "--backend",
            "direct",
        ],
    ));
    assert_eq!(actual["records"], direct["records"]);
}

#[test]
fn query_events_stream_numbered_records_and_completion_metadata() {
    let f = Fixture::new();
    let p = f.root.to_str().unwrap();
    for cache in ["memory", "disabled"] {
        let args = [
            p,
            "query",
            "books",
            "score >= 2",
            "--count",
            "--offset",
            "1",
            "--limit",
            "2",
            "--cache",
            cache,
        ];
        let collected = value(run(&f, &args));
        let mut event_args = args.to_vec();
        event_args.extend(["--output", "events"]);
        let output = run(&f, &event_args);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let events = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<Value>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(events.len(), 3, "{cache}");
        for (index, event) in events[..2].iter().enumerate() {
            assert_eq!(event["type"], "record");
            assert_eq!(event["ordinal"], index + 1);
            assert_eq!(event["provisional"], true);
            assert_eq!(event["record"], collected["records"][index]);
        }
        let complete = &events[2];
        assert_eq!(complete["type"], "complete");
        assert_eq!(complete["provisional"], false);
        assert_eq!(complete["returned"], 2);
        assert_eq!(complete["total"], collected["total"]);
        assert_eq!(complete["filter_hash"], collected["filter_hash"]);
        assert_eq!(complete["dependencies"], collected["dependencies"]);
        assert_eq!(complete["query_plan"], collected["query_plan"]);
        assert!(complete["metrics"]["query_ms"].is_number());
    }
}

#[test]
fn query_events_report_late_errors_after_provisional_records() {
    let f = Fixture::new();
    let books = f.root.join("data/books");
    for entry in fs::read_dir(&books).unwrap() {
        fs::remove_file(entry.unwrap().path()).unwrap();
    }
    for number in 0..600 {
        fs::write(
            books.join(format!("r{number:03}.json")),
            serde_json::to_vec(&json!({"title":"hit","payload":null})).unwrap(),
        )
        .unwrap();
    }
    fs::write(
        books.join("zbad.json"),
        serde_json::to_vec(&json!({"title":["hit","other"],"payload":null})).unwrap(),
    )
    .unwrap();
    let p = f.root.to_str().unwrap();
    let output = run(
        &f,
        &[
            p,
            "query",
            "books",
            "title = 'hit'",
            "--count",
            "--limit",
            "2",
            "--output",
            "events",
            "--cache",
            "memory",
        ],
    );
    assert!(!output.status.success());
    let events = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["record"]["id"], "r000");
    assert_eq!(events[1]["record"]["id"], "r001");
    assert_eq!(events[2]["type"], "error");
    assert_eq!(events[2]["provisional_records"], 2);
    assert!(
        events[2]["message"]
            .as_str()
            .unwrap()
            .contains("multiple values")
    );
}

#[test]
fn full_text_search_modes_and_manual_warm_are_available_in_cli() {
    let fixture = Fixture::new();
    fixture.edit(".editor/books/schema.json", |schema| {
        schema["properties"]["title"]["x-full-text"] = json!("markdown");
    });
    fixture.edit("data/books/a.json", |record| {
        record["title"] = json!("**He**lp");
    });
    let root = fixture.root.to_str().unwrap();
    let visible = value(run(
        &fixture,
        &[
            root, "search", "books", "Help", "--count", "--field", "title",
        ],
    ));
    assert_eq!(visible["total"], 1);
    assert_eq!(visible["hits"][0]["record"]["id"], "a");
    assert_eq!(visible["hits"][0]["fields"], json!(["title"]));
    let combined = value(run(
        &fixture,
        &[
            root,
            "query",
            "books",
            "score >= 1 && title:fts ~ 'Help'",
            "--count",
        ],
    ));
    assert_eq!(combined["total"], 1);
    assert_eq!(combined["records"][0]["id"], "a");
    let direct = value(run(
        &fixture,
        &[
            root, "search", "books", "Help", "--cache", "disabled", "--count",
        ],
    ));
    assert_eq!(direct["total"], 1);
    assert!(direct["query_plan"][0].as_str().unwrap().contains("direct"));
    let diagnostic_output = run(
        &fixture,
        &[root, "search", "books", "Help", "--diagnostics"],
    );
    assert!(diagnostic_output.status.success());
    let diagnostic = diagnostics(&diagnostic_output);
    assert_eq!(diagnostic["level"], "info");
    assert!(diagnostic["timestamp_unix_ms"].as_u64().is_some());

    let raw = value(run(
        &fixture,
        &[
            root,
            "search",
            "books",
            r"\*\*He\*\*lp",
            "--mode",
            "raw-regex",
            "--count",
        ],
    ));
    assert_eq!(raw["total"], 1);
    let clean = value(run(
        &fixture,
        &[
            root,
            "search",
            "books",
            r"\*\*He\*\*lp",
            "--mode",
            "regex",
            "--count",
        ],
    ));
    assert_eq!(clean["total"], 0);

    let output = run(
        &fixture,
        &[
            root, "search", "books", "Help", "--count", "--output", "events",
        ],
    );
    assert!(output.status.success());
    let events = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "hit");
    assert_eq!(events[0]["provisional"], true);
    assert_eq!(events[1]["type"], "complete");
    assert_eq!(events[1]["total"], 1);

    let warmed = value(run(
        &fixture,
        &[root, "cache", "warm-text", "--collection", "books"],
    ));
    assert!(warmed["bigram_open_ms"].as_f64().unwrap() > 0.0);
    assert_eq!(warmed["fts_open_ms"], 0.0);
}

#[test]
fn cli_full_text_searches_undeclared_values_and_honors_exclusion() {
    let fixture = Fixture::new();
    fixture.edit(".editor/books/schema.json", |schema| {
        schema["properties"]["secret"] = json!({"type":"string","exclude_fts":true});
    });
    fixture.edit("data/books/a.json", |record| {
        record["custom_text"] = json!("Comet");
        record["secret"] = json!("Forbidden");
    });
    let root = fixture.root.to_str().unwrap();
    let found = value(run(
        &fixture,
        &[root, "search", "books", "Comet", "--count"],
    ));
    assert_eq!(found["total"], 1);
    assert_eq!(found["hits"][0]["fields"], json!(["custom_text"]));
    let excluded = value(run(
        &fixture,
        &[root, "search", "books", "Forbidden", "--count"],
    ));
    assert_eq!(excluded["total"], 0);
    let ordinary = value(run(
        &fixture,
        &[root, "query", "books", "secret ~ 'Forbidden'", "--count"],
    ));
    assert_eq!(ordinary["total"], 1);
}

#[test]
fn cli_can_warm_and_search_a_schema_free_collection() {
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("data/books");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("a.json"), r#"{"extra":{"note":"Comet"}}"#).unwrap();
    let root = temp.path().to_str().unwrap();
    let warmed = value(
        Command::new(env!("CARGO_BIN_EXE_objekte"))
            .args([root, "cache", "warm-text", "--collection", "books"])
            .output()
            .unwrap(),
    );
    assert!(warmed["fts_open_ms"].as_f64().unwrap() > 0.0);
    let found = value(
        Command::new(env!("CARGO_BIN_EXE_objekte"))
            .args([root, "search", "books", "Comet", "--count"])
            .output()
            .unwrap(),
    );
    assert_eq!(found["total"], 1);
    assert_eq!(found["hits"][0]["fields"], json!(["extra.note"]));
}

#[test]
fn diagnostics_reports_bigram_query() {
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
            "--backend",
            "bigram",
            "--diagnostics",
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(diagnostics(&out)["type"], "diagnostics");
    let result = value(out);
    assert_eq!(result["total"], 3);
    assert!(result["query_plan"][0].as_str().unwrap().contains("bigram"));
    assert_eq!(result["metrics"]["grep_scans"], 0);
    assert!(
        result["query_plan"][1]
            .as_str()
            .unwrap()
            .contains("exact scalar")
    );

    let direct = run(
        &f,
        &[
            p,
            "query",
            "books",
            "score >= 2",
            "--cache",
            "disabled",
            "--backend",
            "direct",
            "--diagnostics",
        ],
    );
    assert!(direct.status.success());
}
