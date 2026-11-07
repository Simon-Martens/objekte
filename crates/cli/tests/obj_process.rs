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
        assert_eq!(event["protocol_version"], 1);
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
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(hits(&events)[0]["data"], true);
    let (result, events) = invoke(root.path(), &["@root = 7"]);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(hits(&events)[0]["data"], 7);
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
        vec!["a/001.json".to_string(), "b/001.json".to_string()]
    );
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
    assert_eq!(events.last().unwrap()["snapshot_action"], "built_in_memory");
    assert!(!root.path().join(".objekte").exists());
}

#[test]
fn bare_obj_errors_before_build_but_index_only_writes() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"text":"needle"}"#).unwrap();
    let (missing, events) = invoke(root.path(), &[]);
    assert_eq!(missing.status.code(), Some(2));
    assert!(events.is_empty());
    assert!(!root.path().join(".objekte").exists());
    let (written, events) = invoke(root.path(), &["--write-index"]);
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["type"], "run_start");
    assert_eq!(events[1]["type"], "run_complete");
    assert_eq!(events[1]["snapshot_action"], "built_and_written");
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
    assert!(diagnostic["timestamp_ms"].as_u64().is_some());
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
        &["--query-file", "queries.txt", "--write-index"],
    );
    assert!(
        index_only.status.success(),
        "{}",
        String::from_utf8_lossy(&index_only.stderr)
    );
    assert_eq!(events.last().unwrap()["type"], "run_complete");
}

#[test]
fn explicit_snapshot_flags_do_not_silently_rebuild_load_only() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"title":"needle"}"#).unwrap();
    let (missing, _) = invoke(root.path(), &["--load-index", "--fts", "needle"]);
    assert_eq!(missing.status.code(), Some(2));
    let (written, written_events) = invoke(root.path(), &["--write-index", "--fts", "needle"]);
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert_eq!(hits(&written_events).len(), 1);
    let (loaded, loaded_events) = invoke(root.path(), &["--load-index", "--fts", "needle"]);
    assert!(
        loaded.status.success(),
        "{}",
        String::from_utf8_lossy(&loaded.stderr)
    );
    assert_eq!(hits(&loaded_events).len(), 1);
    std::fs::write(
        root.path().join("data.json"),
        br#"{"title":"other, longer"}"#,
    )
    .unwrap();
    let (stale, _) = invoke(root.path(), &["--load-index", "--fts", "needle"]);
    assert_eq!(stale.status.code(), Some(2));
    let (rebuilt, events) = invoke(
        root.path(),
        &["--load-index", "--write-index", "--fts", "other"],
    );
    assert!(
        rebuilt.status.success(),
        "{}",
        String::from_utf8_lossy(&rebuilt.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
}

#[test]
fn custom_index_directory_is_excluded_and_can_be_restored() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.json"), br#"{"title":"needle"}"#).unwrap();
    let (written, events) = invoke(root.path(), &["--write-index=cache", "--fts", "needle"]);
    assert!(
        written.status.success(),
        "{}",
        String::from_utf8_lossy(&written.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    std::fs::write(
        root.path().join("cache/ignored.json"),
        br#"{"title":"needle"}"#,
    )
    .unwrap();
    let (loaded, events) = invoke(root.path(), &["--load-index=cache", "--fts", "needle"]);
    assert!(
        loaded.status.success(),
        "{}",
        String::from_utf8_lossy(&loaded.stderr)
    );
    assert_eq!(hits(&events).len(), 1);
    assert_eq!(events.last().unwrap()["snapshot_action"], "restored");
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
