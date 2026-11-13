use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Command, Output, Stdio};

use serde_json::Value;

fn invoke(root: &Path, args: &[&str]) -> (Output, Vec<Value>) {
    let output = Command::new(env!("CARGO_BIN_EXE_obj"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    let events = String::from_utf8(output.stdout.clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).expect("stdout line is NDJSON"))
        .collect();
    (output, events)
}

fn hits(events: &[Value]) -> Vec<&Value> {
    events
        .iter()
        .filter(|event| event["type"] == "hit")
        .collect()
}

fn managed_configuration_count(root: &Path) -> usize {
    std::fs::read_dir(root.join(".objekte/indexes"))
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .count()
}

#[test]
fn boolean_field_predicates_keep_cli_results_and_errors_exact() {
    let root = tempfile::tempdir().unwrap();
    for (name, data) in [
        ("missing", r#"{}"#),
        ("null", r#"{"x":null}"#),
        ("zero", r#"{"x":0}"#),
        ("empty", r#"{"x":""}"#),
        ("text", r#"{"x":"value"}"#),
    ] {
        std::fs::write(root.path().join(format!("{name}.json")), data).unwrap();
    }
    for (query, count) in [
        ("x:exists = true", 4),
        ("x:null = true", 1),
        ("x:? = true", 2),
        ("x:? = false", 3),
        ("x:string = true", 2),
        ("x:number = true", 1),
        ("x:null != true", 4),
    ] {
        let (output, events) = invoke(root.path(), &[query]);
        assert!(
            output.status.success(),
            "{query}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(hits(&events).len(), count, "{query}");
    }
    let (output, events) = invoke(root.path(), &["x:? > true"]);
    assert!(!output.status.success());
    assert!(events.iter().any(|event| event["type"] == "query_error"));
}

#[cfg(feature = "tui")]
#[test]
fn tui_requires_terminal_and_rejects_query_file_before_building() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.json"), br#"{"title":"one"}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_obj"))
        .current_dir(root.path())
        .arg("--tui")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("interactive input and output terminal")
    );
    assert!(output.stdout.is_empty());
    assert!(!root.path().join(".objekte").exists());
    let output = Command::new(env!("CARGO_BIN_EXE_obj"))
        .current_dir(root.path())
        .args(["--tui", "--query-file", "queries.txt"])
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--query-file"));
}

#[cfg(not(feature = "tui"))]
#[test]
fn tui_without_feature_reports_unavailable_support() {
    let output = Command::new(env!("CARGO_BIN_EXE_obj"))
        .arg("--tui")
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--tui support was not built"));
    assert!(output.stdout.is_empty());
}

#[test]
fn single_folder_schema_ignores_relations_to_unavailable_collections() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("one.json"),
        br#"{"agent":"a","title":"One"}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("schema.json"),
        serde_json::json!({
            "type":"object",
            "properties":{
                "agent":{"type":"string","index":true,"x-relation":{"collection":"agents"}},
                "title":{"type":"string"}
            }
        })
        .to_string(),
    )
    .unwrap();
    let (status, events) = invoke(root.path(), &["--schema", "schema.json", "agent = \"a\""]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(events.last().unwrap()["records"], 1);
}

#[test]
fn repository_mode_loads_collections_and_keeps_available_relations() {
    let root = tempfile::tempdir().unwrap();
    for name in ["authors", "books"] {
        std::fs::create_dir_all(root.path().join(format!("data/{name}"))).unwrap();
    }
    std::fs::create_dir_all(root.path().join(".objekte/collections")).unwrap();
    std::fs::write(
        root.path().join("data/authors/author.json"),
        br#"{"name":"Ada"}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("data/books/book.json"),
        br#"{"title":"Book","author":"author","missing_link":"orphan"}"#,
    )
    .unwrap();
    for (name, schema) in [
        (
            "authors",
            serde_json::json!({
                "type":"object",
                "properties":{"name":{"type":"string"}},
                "x-objekte":{"name":"authors","path":"data/authors","format":"json","id":"filename"}
            }),
        ),
        (
            "books",
            serde_json::json!({
                "type":"object",
                "properties":{
                    "title":{"type":"string"},
                    "author":{"type":"string","x-relation":{"collection":"authors"}},
                    "missing_link":{"type":"string","x-relation":{"collection":"missing"}}
                },
                "x-objekte":{"name":"books","path":"data/books","format":"json","id":"filename"}
            }),
        ),
    ] {
        std::fs::write(
            root.path()
                .join(format!(".objekte/collections/{name}.schema.json")),
            schema.to_string(),
        )
        .unwrap();
    }
    let (status, events) = invoke(root.path(), &["--repo", ".", "--fts", "Ada"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(
        events[0]["collections"],
        serde_json::json!(["authors", "books"])
    );
    assert_eq!(events.last().unwrap()["records"], 2);
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(hits(&events)[0]["collection"], "authors");
    assert!(
        !root
            .path()
            .join(".objekte/search-core-ascii.snapshot")
            .exists()
    );

    let (status, events) = invoke(root.path(), &["--repo", ".", "author.name = \"Ada\""]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(hits(&events)[0]["collection"], "books");

    let (status, events) = invoke(root.path(), &["--repo", ".", "missing_link = \"orphan\""]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(hits(&events)[0]["collection"], "books");

    let (status, events) = invoke(
        root.path(),
        &[
            "--repo",
            ".",
            "--auto-index",
            "--diagnostic",
            "--fts",
            "Ada",
        ],
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let diagnostics = &events.last().unwrap()["automatic_index_collections"];
    assert!(diagnostics["authors"]["paths"].is_array());
    assert!(diagnostics["books"]["paths"].is_array());

    let (status, events) = invoke(
        root.path(),
        &["--repo", ".", "--index-storage", "repository"],
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(events.last().unwrap()["type"], "run_complete");
    assert!(root.path().join(".objekte/indexes").is_dir());
    let (status, events) = invoke(
        root.path(),
        &[
            "--repo",
            ".",
            "--index-storage",
            "repository",
            "--fts",
            "Ada",
        ],
    );
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(events.last().unwrap()["build_parses"], 0);
    assert_eq!(hits(&events).len(), 1);
}

#[test]
fn repository_mode_requires_valid_collection_metadata() {
    let root = tempfile::tempdir().unwrap();
    let (status, _) = invoke(root.path(), &["--repo", ".", "--fts", "a"]);
    assert_eq!(status.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&status.stderr).contains(".objekte/collections"));

    std::fs::create_dir_all(root.path().join(".objekte/collections")).unwrap();
    std::fs::write(
        root.path().join(".objekte/collections/bad.schema.json"),
        br#"{"type":"object","properties":{},"x-objekte":{"name":"bad"}}"#,
    )
    .unwrap();
    let (status, _) = invoke(root.path(), &["--repo", ".", "--fts", "a"]);
    assert_eq!(status.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&status.stderr).contains("x-objekte.path"));
}

#[test]
fn repeated_paths_open_only_selected_collections_without_repository_configuration() {
    let root = tempfile::tempdir().unwrap();
    for name in ["authors", "books", "omitted"] {
        std::fs::create_dir_all(root.path().join(name)).unwrap();
        std::fs::write(
            root.path().join(format!("{name}/one.json")),
            format!(r#"{{"kind":"{name}","text":"needle"}}"#),
        )
        .unwrap();
    }
    let selected = ["--path", "authors", "--path", "books"];
    let (result, events) = invoke(
        root.path(),
        &[selected.as_slice(), &["--fts", "needle"]].concat(),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(events[0]["collection"], Value::Null);
    assert_eq!(
        events[0]["collections"],
        serde_json::json!(["authors", "books"])
    );
    let mut collections = hits(&events)
        .iter()
        .map(|hit| hit["collection"].as_str().unwrap())
        .collect::<Vec<_>>();
    collections.sort_unstable();
    assert_eq!(collections, ["authors", "books"]);
    assert_eq!(events.last().unwrap()["records"], 2);

    let (result, events) = invoke(
        root.path(),
        &[
            selected.as_slice(),
            &["--auto-index", "--diagnostic", "kind = \"books\""],
        ]
        .concat(),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(hits(&events)[0]["collection"], "books");
    let diagnostics = &events.last().unwrap()["automatic_index_collections"];
    assert!(diagnostics["authors"]["paths"].is_array());
    assert!(diagnostics["books"]["paths"].is_array());

    let (result, _) = invoke(
        root.path(),
        &[selected.as_slice(), &["--index-storage", "repository"]].concat(),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(root.path().join(".objekte/indexes").is_dir());
    let (result, events) = invoke(
        root.path(),
        &[
            selected.as_slice(),
            &["--index-storage", "repository", "--fts", "needle"],
        ]
        .concat(),
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 2);
    assert_eq!(events.last().unwrap()["build_parses"], 0);
}

#[test]
fn repeated_paths_reject_duplicate_collection_names() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("a/items")).unwrap();
    std::fs::create_dir_all(root.path().join("b/items")).unwrap();
    let (result, _) = invoke(
        root.path(),
        &["-p", "a/items", "-p", "b/items", "--fts", "x"],
    );
    assert_eq!(result.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&result.stderr).contains("duplicate collection name items"));
}

#[test]
fn auto_index_flag_preserves_answers_and_reports_short_string_capability() {
    let root = tempfile::tempdir().unwrap();
    for slot in 0..80 {
        let code = format!("550e8400-e29b-41d4-a716-{slot:012}");
        std::fs::write(
            root.path().join(format!("{slot:03}.json")),
            serde_json::json!({"code":code}).to_string(),
        )
        .unwrap();
    }
    let (plain_status, plain) = invoke(
        root.path(),
        &["code = \"550e8400-e29b-41d4-a716-000000000036\""],
    );
    let (auto_status, auto) = invoke(
        root.path(),
        &[
            "--auto-index",
            "code = \"550e8400-e29b-41d4-a716-000000000036\"",
        ],
    );
    assert!(plain_status.status.success());
    assert!(
        auto_status.status.success(),
        "{}",
        String::from_utf8_lossy(&auto_status.stderr)
    );
    assert_eq!(hits(&plain).len(), 1);
    assert_eq!(
        hits(&plain)[0]["file"]["record_id"],
        hits(&auto)[0]["file"]["record_id"]
    );
    assert_eq!(auto[0]["inference"], "auto_index");
    assert_eq!(
        auto.last().unwrap()["automatic_index_summary"]["short_string_fields"],
        1
    );
    assert_eq!(
        auto.last().unwrap()["automatic_index_summary"]["short_string_complete_fields"],
        1
    );
    assert_eq!(auto.last().unwrap()["build_parses"], 80);
    assert!(!root.path().join(".objekte").exists());
}

#[test]
#[ignore = "full 148,488-file Musenalm acceptance; run explicitly"]
fn musenalm_auto_index_restricts_year_across_the_whole_folder() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("examples/musenalm");
    let (status, events) = invoke(&root, &["--auto-index", "--limit", "1", "year = 1800"]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    let summary = &events.last().unwrap()["automatic_index_summary"];
    assert_eq!(summary["complete"], true);
    assert!(summary["admitted_paths"].as_u64().unwrap() > 0);
    let files = events.last().unwrap()["files"].as_u64().unwrap();
    let query = events
        .iter()
        .find(|event| event["type"] == "query_complete")
        .unwrap();
    assert!(query["counters"]["candidates"].as_u64().unwrap() < files / 10);
    assert_eq!(hits(&events).len(), 1);
}

#[test]
fn explicit_schema_and_auto_index_have_independent_cli_modes() {
    let root = tempfile::tempdir().unwrap();
    let schema = serde_json::json!({
        "type":"object",
        "properties":{
            "code":{"type":"string","index":true},
            "year":{"type":"integer"},
            "body":{"type":"string","index":false}
        }
    });
    std::fs::write(root.path().join("schema.json"), schema.to_string()).unwrap();
    std::fs::write(
        root.path().join("one.json"),
        serde_json::json!({"code":"A","year":1800,"body":"text"}).to_string(),
    )
    .unwrap();
    std::fs::write(
        root.path().join("two.json"),
        serde_json::json!({"code":"L".repeat(65),"year":1900,"body":"text"}).to_string(),
    )
    .unwrap();
    for flags in [
        vec![],
        vec!["--schema", "schema.json"],
        vec!["--auto-index"],
        vec!["--schema", "schema.json", "--auto-index"],
    ] {
        let mut args = flags.clone();
        args.push("code = \"A\"");
        let (status, events) = invoke(root.path(), &args);
        assert!(
            status.status.success(),
            "{flags:?}: {}",
            String::from_utf8_lossy(&status.stderr)
        );
        assert_eq!(hits(&events).len(), 1, "{flags:?}");
        assert_eq!(hits(&events)[0]["file"]["record_id"], "path:one.json");
        assert_eq!(
            events.last().unwrap()["files"],
            if flags.contains(&"--schema") { 2 } else { 3 }
        );
        assert_eq!(
            events[0]["schema_fields"],
            if flags.contains(&"--schema") { 3 } else { 0 }
        );
        assert_eq!(
            events[0]["inference"],
            if flags.contains(&"--auto-index") {
                "auto_index"
            } else {
                "disabled"
            }
        );
    }
    let long_query = format!("code = {}", serde_json::to_string(&"L".repeat(65)).unwrap());
    let (status, events) = invoke(root.path(), &["--schema", "schema.json", &long_query]);
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    let conflict = serde_json::json!({"type":"object","properties":{"code":{"type":"string","index":false}},"x-index-paths":["code"]});
    std::fs::write(root.path().join("conflict.json"), conflict.to_string()).unwrap();
    let (status, _) = invoke(root.path(), &["--schema", "conflict.json", "code = \"A\""]);
    assert_eq!(status.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&status.stderr).contains("conflicts"));
}

#[test]
fn schema_bytes_and_inference_mode_select_distinct_managed_indexes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"code":"A"}"#).unwrap();
    let schema_path = root.path().join("schema.json");
    let base = serde_json::json!({"type":"object","properties":{"code":{"type":"string"}}});
    std::fs::write(&schema_path, base.to_string()).unwrap();
    let (written, events) = invoke(
        root.path(),
        &[
            "--schema",
            "schema.json",
            "--auto-index",
            "--index-storage",
            "repository",
        ],
    );
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert_eq!(events.last().unwrap()["type"], "run_complete");
    assert!(root.path().join(".objekte/indexes").is_dir());
    let (loaded, events) = invoke(
        root.path(),
        &[
            "--schema",
            "schema.json",
            "--auto-index",
            "--index-storage",
            "repository",
            "code = \"A\"",
        ],
    );
    assert!(
        loaded.status.success(),
        "{}",
        String::from_utf8_lossy(&loaded.stderr)
    );
    assert_eq!(events.last().unwrap()["build_parses"], 0);
    assert_eq!(hits(&events).len(), 1);
    let (mismatch, mismatch_events) = invoke(
        root.path(),
        &[
            "--schema",
            "schema.json",
            "--index-storage",
            "repository",
            "code = \"A\"",
        ],
    );
    assert!(
        mismatch.status.success(),
        "{}",
        String::from_utf8_lossy(&mismatch.stderr)
    );
    assert_eq!(hits(&mismatch_events).len(), 1);
    assert!(
        mismatch_events.last().unwrap()["build_parses"]
            .as_u64()
            .unwrap()
            > 0
    );
    std::fs::write(&schema_path, format!("{}\n", base)).unwrap();
    let (changed, changed_events) = invoke(
        root.path(),
        &[
            "--schema",
            "schema.json",
            "--auto-index",
            "--index-storage",
            "repository",
            "code = \"A\"",
        ],
    );
    assert!(
        changed.status.success(),
        "{}",
        String::from_utf8_lossy(&changed.stderr)
    );
    assert_eq!(hits(&changed_events).len(), 1);
    assert!(
        changed_events.last().unwrap()["build_parses"]
            .as_u64()
            .unwrap()
            > 0
    );
    let (restored, restored_events) = invoke(
        root.path(),
        &[
            "--schema",
            "schema.json",
            "--auto-index",
            "--index-storage",
            "repository",
            "code = \"A\"",
        ],
    );
    assert!(restored.status.success());
    assert_eq!(restored_events.last().unwrap()["build_parses"], 0);
    assert!(managed_configuration_count(root.path()) >= 3);
}

#[test]
fn manual_and_automatic_postings_reopen_together() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("schema.json"),
        serde_json::json!({
            "type":"object", "properties":{
                "code":{"type":"string","index":true},
                "group":{"type":"string"}
            }
        })
        .to_string(),
    )
    .unwrap();
    for index in 0..4 {
        std::fs::write(
            root.path().join(format!("{index}.json")),
            serde_json::json!({"code":format!("c{index}"),"group":format!("g{index}")}).to_string(),
        )
        .unwrap();
    }
    let (built_status, built) = invoke(
        root.path(),
        &[
            "--schema",
            "schema.json",
            "--auto-index",
            "--index-storage",
            "repository",
            "group = 'g2'",
        ],
    );
    assert!(
        built_status.status.success(),
        "{}",
        String::from_utf8_lossy(&built_status.stderr)
    );
    assert_eq!(
        built.last().unwrap()["automatic_index_summary"]["admitted_paths"],
        1
    );
    let (loaded_status, loaded) = invoke(
        root.path(),
        &[
            "--schema",
            "schema.json",
            "--auto-index",
            "--index-storage",
            "repository",
            "group = 'g2'",
        ],
    );
    assert!(
        loaded_status.status.success(),
        "{}",
        String::from_utf8_lossy(&loaded_status.stderr)
    );
    assert_eq!(loaded.last().unwrap()["build_parses"], 0);
    assert_eq!(hits(&built).len(), 1);
    assert_eq!(
        hits(&built)[0]["file"]["record_id"],
        hits(&loaded)[0]["file"]["record_id"]
    );
}

#[test]
fn default_folder_admits_root_shapes_and_hidden_json() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("object.json"), br#"{"title":"needle"}"#).unwrap();
    std::fs::write(
        root.path().join("array.json"),
        br#"["needle",123,true,null]"#,
    )
    .unwrap();
    std::fs::write(root.path().join("string.json"), br#""needle""#).unwrap();
    std::fs::write(root.path().join("null.json"), b"null").unwrap();
    std::fs::write(root.path().join("bool.json"), b"true").unwrap();
    std::fs::write(root.path().join("number.json"), b"7").unwrap();
    std::fs::write(root.path().join("empty_array.json"), b"[]").unwrap();
    std::fs::write(root.path().join("empty_object.json"), b"{}").unwrap();
    std::fs::create_dir(root.path().join(".hidden")).unwrap();
    std::fs::write(
        root.path().join(".hidden/value.json"),
        br#"{"text":"needle"}"#,
    )
    .unwrap();
    std::fs::create_dir(root.path().join(".git")).unwrap();
    std::fs::write(
        root.path().join(".git/ignored.json"),
        br#"{"text":"needle"}"#,
    )
    .unwrap();
    std::fs::create_dir(root.path().join(".objekte")).unwrap();
    std::fs::write(
        root.path().join(".objekte/ignored.json"),
        br#"{"text":"needle"}"#,
    )
    .unwrap();

    let (result, events) = invoke(root.path(), &["--fts", "needle"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 4);
    assert_eq!(events.last().unwrap()["type"], "run_complete");
    assert_eq!(events.last().unwrap()["files"], 9);
    assert_eq!(events.last().unwrap()["fallback_records"], 0);
    assert_eq!(events[0]["schema_fields"], 0);
    assert_eq!(events[0]["inference"], "disabled");
    for (index, event) in events.iter().enumerate() {
        assert_eq!(event["sequence"], (index + 1) as u64);
        assert_eq!(event["protocol_version"], 2);
    }
    let (result, events) = invoke(root.path(), &["--no-recurse", "--fts", "needle"]);
    assert!(result.status.success());
    assert_eq!(hits(&events).len(), 3);
    let (result, events) = invoke(root.path(), &["@root :null"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(hits(&events)[0]["data_available"], true);
    assert_eq!(hits(&events)[0]["data"], Value::Null);
    let (result, events) = invoke(root.path(), &["@root = true"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 2);
    assert!(hits(&events).iter().any(|hit| hit["data"] == true));
    // Default universal array comparison preserves the empty-array witness.
    assert!(
        hits(&events)
            .iter()
            .any(|hit| hit["data"] == serde_json::json!([]))
    );
    let (result, events) = invoke(root.path(), &["@root = 7"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 2);
    assert!(hits(&events).iter().any(|hit| hit["data"] == 7));
    assert!(
        hits(&events)
            .iter()
            .any(|hit| hit["data"] == serde_json::json!([]))
    );
}

#[test]
fn relative_file_identity_distinguishes_duplicate_basenames() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("a")).unwrap();
    std::fs::create_dir(root.path().join("b")).unwrap();
    std::fs::write(root.path().join("a/001.json"), br#"{"text":"needle"}"#).unwrap();
    std::fs::write(root.path().join("b/001.json"), br#"{"text":"needle"}"#).unwrap();
    std::fs::write(root.path().join("ignored.jsonl"), br#"{"text":"needle"}"#).unwrap();
    std::fs::write(root.path().join("ignored.JSON"), br#"{"text":"needle"}"#).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("a/001.json", root.path().join("alias.json")).unwrap();
    let (result, events) = invoke(
        root.path(),
        &["-p", root.path().to_str().unwrap(), "--fts", "needle"],
    );
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let mut ids: Vec<String> = hits(&events)
        .iter()
        .map(|hit| hit["file"]["record_id"].as_str().unwrap().to_owned())
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        vec!["path:a/001.json".to_string(), "path:b/001.json".to_string()]
    );
}

#[test]
fn relative_source_paths_do_not_depend_on_process_directory() {
    let root = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(root.path().join("nested")).unwrap();
    std::fs::write(root.path().join("nested/one.json"), br#"{"title":"one"}"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_obj"))
        .current_dir(outside.path())
        .args([
            "-p",
            root.path().to_str().unwrap(),
            "@file.path = 'nested/one.json'",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let events: Vec<Value> = String::from_utf8(output.stdout)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(hits(&events)[0]["file"]["path"], "nested/one.json");
}

#[test]
fn nearby_json_schema_is_indexed_as_ordinary_data() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("schema.json"),
        br#"{"type":"object","description":"needle"}"#,
    )
    .unwrap();
    let (result, events) = invoke(root.path(), &["--fts", "needle"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(hits(&events)[0]["file"]["path"], "schema.json");
}

#[test]
fn search_without_index_flags_leaves_disk_cache_absent() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"text":"needle"}"#).unwrap();
    let (result, events) = invoke(root.path(), &["--fts", "needle"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(events.last().unwrap()["type"], "run_complete");
    assert_eq!(hits(&events).len(), 1);
    assert!(!root.path().join(".objekte").exists());
}

#[test]
fn temporary_and_repository_storage_open_with_the_shared_policy() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"text":"needle"}"#).unwrap();
    for mode in ["temporary", "repository"] {
        let (result, events) = invoke(root.path(), &["--index-storage", mode, "--fts", "needle"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(hits(&events).len(), 1);
        assert_eq!(events.last().unwrap()["type"], "run_complete");
        assert_eq!(events[0]["index_operation"], mode);
        let finished = events.last().unwrap();
        assert_eq!(
            finished["index_storage_status"]["synchronization"],
            "synchronized"
        );
        assert_eq!(
            finished["index_storage_status"]["source_reconciliation_complete"],
            true
        );
        assert_eq!(
            finished["index_storage_status"]["durable_generation"],
            finished["index_storage_status"]["current_generation"]
        );
    }
    assert!(root.path().join(".objekte/indexes").is_dir());
}

#[test]
fn bare_obj_errors_before_build_but_disk_mode_allows_index_only() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"text":"needle"}"#).unwrap();
    let (missing, events) = invoke(root.path(), &[]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(events.is_empty());
    assert!(!root.path().join(".objekte").exists());
    let (written, events) = invoke(root.path(), &["--index-storage", "repository"]);
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "run_start");
    assert_eq!(events[1]["type"], "run_complete");
    assert!(root.path().join(".objekte/indexes").is_dir());
}

#[cfg(feature = "detailed-profiling")]
#[test]
fn diagnostic_disk_runs_report_timing_across_a_reopen() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"text":"needle"}"#).unwrap();
    let (written, events) = invoke(
        root.path(),
        &["--index-storage", "repository", "--diagnostic"],
    );
    assert!(written.status.success());
    assert!(events.last().unwrap()["startup_ms"].is_number());
    assert!(events.last().unwrap()["run_elapsed_ms"].is_number());
    assert!(root.path().join(".objekte/indexes").is_dir());
    let (restored, events) = invoke(
        root.path(),
        &["--index-storage", "repository", "--diagnostic"],
    );
    assert!(restored.status.success());
    assert_eq!(events.last().unwrap()["build_parses"], 0);
}

#[test]
fn help_and_version_do_not_open_the_collection() {
    let root = tempfile::tempdir().unwrap();
    for flag in ["--help", "--version"] {
        let output = Command::new(env!("CARGO_BIN_EXE_obj"))
            .current_dir(root.path())
            .args(["--path", "missing-directory", flag])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{flag}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(output.stderr.is_empty());
    }
}

#[test]
fn property_names_do_not_become_full_text() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("data.json"),
        br#"{"needle":0,"other":false}"#,
    )
    .unwrap();
    let (result, events) = invoke(root.path(), &["--fts", "needle"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(hits(&events).is_empty());
}

#[test]
fn both_alphabets_keep_unicode_full_text_matches() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), "{\"name\":\"Müller\"}").unwrap();
    for alphabet in ["ascii", "unicode"] {
        let (result, events) = invoke(
            root.path(),
            &["--index-alphabet", alphabet, "--fts", "Müller"],
        );
        assert!(
            result.status.success(),
            "{alphabet}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert_eq!(hits(&events).len(), 1, "{alphabet}");
    }
}

#[test]
fn computed_sort_orders_all_results_before_emitting_rows() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("a.json"),
        br#"{"title":"match","score":2}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("b.json"),
        br#"{"title":"match","score":1}"#,
    )
    .unwrap();
    let (result, events) = invoke(root.path(), &["title ~ \"match\" SORT BY score"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let hits = hits(&events);
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0]["data"]["score"], 1);
    assert_eq!(hits[1]["data"]["score"], 2);
}

#[test]
fn diagnostic_flag_keeps_stdout_structured_and_reports_stderr_json() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"text":"needle"}"#).unwrap();
    let (result, events) = invoke(root.path(), &["--diagnostic", "--fts", "needle"]);
    assert!(result.status.success());
    assert_eq!(hits(&events).len(), 1);
    let diagnostic: Value = serde_json::from_slice(&result.stderr).unwrap();
    assert_eq!(diagnostic["level"], "info");
    assert!(diagnostic["timestamp_ms"].as_f64().is_some());
    for field in ["timestamp_ms"] {
        let text = diagnostic[field].as_number().unwrap().to_string();
        assert_eq!(text.split_once('.').unwrap().1.len(), 3, "{text}");
    }

    #[cfg(feature = "detailed-profiling")]
    assert!(diagnostic["elapsed_ms"].as_f64().is_some());
    #[cfg(not(feature = "detailed-profiling"))]
    assert!(diagnostic.get("elapsed_ms").is_none());
}

#[test]
fn protocol_timings_and_file_timestamps_are_milliseconds_with_three_decimals() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("one.json"), br#"{"text":"needle"}"#).unwrap();
    let (result, events) = invoke(root.path(), &["--fts", "needle"]);
    assert!(result.status.success());
    let three_decimals = |value: &Value| {
        let text = value.as_number().expect("numeric milliseconds").to_string();
        assert_eq!(text.split_once('.').unwrap().1.len(), 3, "{text}");
    };
    for event in &events {
        assert_eq!(event["protocol_version"], 2);
        assert!(
            !event
                .as_object()
                .unwrap()
                .keys()
                .any(|key| key.ends_with("_ns"))
        );
    }
    let hit = hits(&events)[0];
    three_decimals(&hit["file"]["modified_ms"]);
    if !hit["file"]["created_ms"].is_null() {
        three_decimals(&hit["file"]["created_ms"]);
    }
    let query = events
        .iter()
        .find(|event| event["type"] == "query_complete")
        .unwrap();
    for field in [
        "preparation_ms",
        "search_ms",
        "sink_ms",
        "all_results_ms",
        "first_result_ms",
    ] {
        if cfg!(feature = "detailed-profiling")
            || matches!(field, "all_results_ms" | "first_result_ms")
        {
            three_decimals(&query[field]);
        }
    }
    if cfg!(feature = "detailed-profiling") {
        for field in ["preparation_ms", "planning_ms"] {
            three_decimals(&query["counters"][field]);
        }
    }
    let run = events.last().unwrap();
    for field in ["metadata", "search_available", "full_available"] {
        assert!(run["index_timings_ms"][field].as_u64().is_some());
    }
    if cfg!(feature = "detailed-profiling") {
        for field in [
            "startup_ms",
            "discovery_ms",
            "extraction_index_ms",
            "finalization_ms",
            "snapshot_decode_ms",
            "reconciliation_ms",
            "snapshot_write_ms",
            "run_elapsed_ms",
        ] {
            three_decimals(&run[field]);
        }
    }
}

#[test]
fn diagnostic_lists_inferred_btree_paths_after_build_and_reopen() {
    let root = tempfile::tempdir().unwrap();
    for number in 0..80 {
        std::fs::write(
            root.path().join(format!("{number:03}.json")),
            serde_json::json!({
                "year": number,
                "enabled": number % 2 == 0,
                "code": format!("code-{number:03}"),
            })
            .to_string(),
        )
        .unwrap();
    }
    let (built, events) = invoke(
        root.path(),
        &[
            "--index-storage",
            "repository",
            "--auto-index",
            "--diagnostic",
        ],
    );
    assert!(
        built.status.success(),
        "{}",
        String::from_utf8_lossy(&built.stderr)
    );
    let paths = events.last().unwrap()["automatic_index_paths"]
        .as_array()
        .unwrap();
    let path_names = paths
        .iter()
        .map(|field| field["path"][0].as_str().unwrap())
        .collect::<Vec<_>>();
    assert!(path_names.windows(2).all(|pair| pair[0] <= pair[1]));
    for (path, kind, keys) in [
        ("year", "Number", 80),
        ("enabled", "Bool", 2),
        ("code", "String", 80),
    ] {
        let field = paths
            .iter()
            .find(|field| field["path"] == serde_json::json!([path]))
            .unwrap();
        assert_eq!(field["index_status"], "Built", "{path}");
        assert_eq!(field["kind"], kind, "{path}");
        assert_eq!(field["value_keys"], keys, "{path}");
        assert_eq!(field["covered_records"], 80, "{path}");
        assert_eq!(field["multi"], false, "{path}");
    }
    let (restored, events) = invoke(
        root.path(),
        &[
            "--index-storage",
            "repository",
            "--auto-index",
            "--diagnostic",
        ],
    );
    assert!(restored.status.success());
    assert_eq!(events.last().unwrap()["build_parses"], 0);
    assert_eq!(
        events.last().unwrap()["automatic_index_paths"],
        serde_json::json!(paths)
    );
}

#[test]
fn all_results_are_not_cut_at_fifty_and_finite_count_is_exact() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..61 {
        std::fs::write(
            root.path().join(format!("{index:03}.json")),
            br#"{"word":"needle"}"#,
        )
        .unwrap();
    }
    let (result, events) = invoke(root.path(), &["--fts", "needle"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 61);
    let complete = events
        .iter()
        .find(|event| event["type"] == "query_complete")
        .unwrap();
    assert_eq!(complete["exact_total"], 61);
    assert!(complete["counters"]["record_parses"].as_u64().unwrap() <= 61);
    let (result, events) = invoke(
        root.path(),
        &[
            "--fts", "needle", "--offset", "10", "--limit", "2", "--count",
        ],
    );
    assert!(result.status.success());
    assert_eq!(hits(&events).len(), 2);
    let complete = events
        .iter()
        .find(|event| event["type"] == "query_complete")
        .unwrap();
    assert_eq!(complete["exact_total"], 61);
    assert_eq!(hits(&events)[0]["position"], 0);
    assert_eq!(hits(&events)[0]["requested_offset"], 10);
    let (result, events) = invoke(root.path(), &["--fts", "needle", "--limit", "2"]);
    assert!(result.status.success());
    assert_eq!(hits(&events).len(), 2);
    let complete = events
        .iter()
        .find(|event| event["type"] == "query_complete")
        .unwrap();
    assert_eq!(complete["exact_total"], Value::Null);
    assert_eq!(complete["total_exact"], false);
    let (result, events) = invoke(root.path(), &["--fts", "needle", "--limit", "0", "--count"]);
    assert!(result.status.success());
    assert!(hits(&events).is_empty());
    let complete = events
        .iter()
        .find(|event| event["type"] == "query_complete")
        .unwrap();
    assert_eq!(complete["exact_total"], 61);
}

#[test]
fn root_queries_and_scalar_text_use_original_document_tokens() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(
        root.path().join("array.json"),
        b" [ \"first\", 1e1000000, true ] \n",
    )
    .unwrap();
    std::fs::write(
        root.path().join("duplicate.json"),
        b"{\"x\":\"earlier\", \"x\":\"later\"}",
    )
    .unwrap();
    let (result, events) = invoke(root.path(), &["@root[0] = \"first\""]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert!(String::from_utf8_lossy(&result.stdout).contains("1e1000000"));
    let (result, events) = invoke(root.path(), &["--fts", "1e1000000"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    let (result, events) = invoke(root.path(), &["--fts", "earlier"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    let line = String::from_utf8_lossy(&result.stdout);
    assert!(line.contains("\"x\":\"earlier\",\"x\":\"later\""));
}

#[test]
fn malformed_query_line_is_terminal_but_later_lines_execute() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"title":"needle"}"#).unwrap();
    std::fs::write(
        root.path().join("queries.txt"),
        "\u{feff}\"needle\"\r\n\"bad\\q\"\r\n title ~ \"needle\"\r\n",
    )
    .unwrap();
    let (result, events) = invoke(root.path(), &["--query-file", "queries.txt"]);
    assert_eq!(result.status.code(), Some(1));
    let starts: Vec<_> = events
        .iter()
        .filter(|event| event["type"] == "query_start")
        .collect();
    assert_eq!(starts.len(), 3);
    assert_eq!(starts[1]["line"], 2);
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "query_error")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| event["type"] == "query_complete")
            .count(),
        2
    );
    assert_eq!(events.last().unwrap()["type"], "run_error");
    let error = events
        .iter()
        .find(|event| event["type"] == "query_error")
        .unwrap();
    for value in [&error["all_results_ms"]] {
        let text = value.as_number().unwrap().to_string();
        assert_eq!(text.split_once('.').unwrap().1.len(), 3, "{text}");
    }
    assert!(
        events.last().unwrap()["index_timings_ms"]["full_available"]
            .as_u64()
            .is_some()
    );
}

#[test]
fn empty_query_file_is_an_input_error_unless_index_only_was_requested() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("queries.txt"), b"  \r\n\n").unwrap();
    let (invalid, events) = invoke(root.path(), &["--query-file", "queries.txt"]);
    assert_eq!(invalid.status.code(), Some(1));
    assert_eq!(events.last().unwrap()["type"], "run_error");
    let (index_only, events) = invoke(
        root.path(),
        &[
            "--query-file",
            "queries.txt",
            "--index-storage",
            "repository",
        ],
    );
    assert!(
        index_only.status.success(),
        "{}",
        String::from_utf8_lossy(&index_only.stderr)
    );
    assert_eq!(events.last().unwrap()["type"], "run_complete");
}

#[test]
fn repository_storage_reuses_and_reconciles_a_durable_generation() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"title":"needle"}"#).unwrap();
    let args = ["--index-storage", "repository"];
    let (written, written_events) = invoke(
        root.path(),
        &[args.as_slice(), &["--fts", "needle"]].concat(),
    );
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert_eq!(hits(&written_events).len(), 1);
    assert!(root.path().join(".objekte/indexes").is_dir());
    assert!(
        !root
            .path()
            .join(".objekte/search-core-ascii.snapshot")
            .exists()
    );
    let (unchanged, unchanged_events) = invoke(
        root.path(),
        &[args.as_slice(), &["--fts", "needle"]].concat(),
    );
    assert!(unchanged.status.success());
    assert_eq!(unchanged_events.last().unwrap()["build_parses"], 0);
    assert_eq!(hits(&unchanged_events).len(), 1);
    std::fs::write(
        root.path().join("data.json"),
        br#"{"title":"other, longer"}"#,
    )
    .unwrap();
    let (rebuilt, events) = invoke(
        root.path(),
        &[args.as_slice(), &["--fts", "other"]].concat(),
    );
    assert!(
        rebuilt.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    let (reopened, events) = invoke(
        root.path(),
        &[args.as_slice(), &["--fts", "other"]].concat(),
    );
    assert!(
        reopened.status.success(),
        "{}",
        String::from_utf8_lossy(&reopened.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(events.last().unwrap()["build_parses"], 0);
}

#[test]
fn custom_index_namespace_is_excluded_but_unrelated_base_files_are_sources() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"title":"needle"}"#).unwrap();
    std::fs::create_dir(root.path().join("cache")).unwrap();
    let args = ["--index-storage", "custom", "--index-directory", "cache"];
    let (written, events) = invoke(
        root.path(),
        &[args.as_slice(), &["--fts", "needle"]].concat(),
    );
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert!(root.path().join("cache/objekte-indexes").is_dir());
    std::fs::write(
        root.path().join("cache/ignored.json"),
        br#"{"title":"needle"}"#,
    )
    .unwrap();
    std::fs::write(
        root.path().join("cache/objekte-indexes/not-source.json"),
        br#"{"title":"needle"}"#,
    )
    .unwrap();
    let (loaded, events) = invoke(
        root.path(),
        &[args.as_slice(), &["--fts", "needle"]].concat(),
    );
    assert!(
        loaded.status.success(),
        "{}",
        String::from_utf8_lossy(&loaded.stderr)
    );
    assert_eq!(hits(&events).len(), 2);
}

#[test]
fn malformed_fallback_keeps_valid_utf8_or_binary_raw_bytes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("text.json"), b"needle {").unwrap();
    std::fs::write(root.path().join("binary.json"), b"needle \xff").unwrap();
    let (result, events) = invoke(root.path(), &["--fts", "needle"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let hits = hits(&events);
    assert_eq!(hits.len(), 2);
    for hit in hits {
        assert_eq!(hit["data_available"], false);
        assert_eq!(hit["data"], Value::Null);
        assert_eq!(hit["record"]["class"], "raw_fallback");
        assert!(hit.get("raw_text").is_some() || hit.get("raw_base64").is_some());
    }
}

#[cfg(unix)]
#[test]
fn control_c_cancels_after_one_query_without_a_success_terminal() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"word":"needle"}"#).unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_obj"))
        .current_dir(root.path())
        .args(["--query-file", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"\"needle\"\n").unwrap();
    stdin.flush().unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut observed = Vec::new();
    loop {
        let mut line = String::new();
        assert_ne!(reader.read_line(&mut line).unwrap(), 0);
        let event: Value = serde_json::from_str(&line).unwrap();
        let complete = event["type"] == "query_complete";
        observed.push(event);
        if complete {
            break;
        }
    }
    assert_eq!(unsafe { libc::kill(child.id() as i32, libc::SIGINT) }, 0);
    std::thread::sleep(std::time::Duration::from_millis(20));
    drop(stdin);
    for line in reader.lines() {
        observed.push(serde_json::from_str(&line.unwrap()).unwrap());
    }
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(130));
    assert_eq!(observed.last().unwrap()["type"], "run_cancelled");
    assert!(!observed.iter().any(|event| event["type"] == "run_complete"));
}

#[test]
fn downstream_broken_pipe_stops_cleanly() {
    let root = tempfile::tempdir().unwrap();
    for index in 0..300 {
        std::fs::write(
            root.path().join(format!("{index:03}.json")),
            format!("{{\"word\":\"needle\",\"pad\":\"{}\"}}", "x".repeat(2048)),
        )
        .unwrap();
    }
    let mut child = Command::new(env!("CARGO_BIN_EXE_obj"))
        .current_dir(root.path())
        .args(["--fts", "needle"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = BufReader::new(child.stdout.take().unwrap());
    let mut line = String::new();
    assert_ne!(reader.read_line(&mut line).unwrap(), 0);
    drop(reader);
    let status = child.wait().unwrap();
    assert_eq!(status.code(), Some(0));
}
