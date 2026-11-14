//! End-to-end CLI storage protocol.

use std::process::Command;

use serde_json::Value;

fn invoke(root: &std::path::Path, args: &[&str]) -> (std::process::ExitStatus, Vec<Value>, String) {
    let output = Command::new(env!("CARGO_BIN_EXE_obj"))
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    let events = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();
    (
        output.status,
        events,
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn cli_builds_saves_loads_checks_and_resaves() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let store = root.path().join("store");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("a.json"), br#"{"text":"Alpha"}"#).unwrap();
    std::fs::write(source.join("b.json"), br#"{"text":"Beta"}"#).unwrap();

    let (status, events, error) = invoke(
        root.path(),
        &["--path", "source", "--save-index", store.to_str().unwrap()],
    );
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "index_saved"));
    assert!(store.join("CURRENT").exists());

    let (status, events, error) = invoke(
        root.path(),
        &[
            "--path",
            "source",
            "--load-index",
            store.to_str().unwrap(),
            "--fts",
            "Alpha",
        ],
    );
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "index_loaded"));
    assert!(events.iter().any(|event| event["type"] == "hit"));

    std::fs::write(source.join("c.json"), br#"{"text":"Gamma"}"#).unwrap();
    let (status, events, error) = invoke(
        root.path(),
        &[
            "--path",
            "source",
            "--load-index",
            store.to_str().unwrap(),
            "--check-index",
            "--save-index",
            store.to_str().unwrap(),
            "--fts",
            "Gamma",
        ],
    );
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "index_checked"));
    assert!(events.iter().any(|event| event["type"] == "index_saved"));
    assert!(events.iter().any(|event| event["type"] == "hit"));

    let (status, events, error) = invoke(
        root.path(),
        &[
            "--path",
            "source",
            "--load-index",
            store.to_str().unwrap(),
            "--fts",
            "Gamma",
        ],
    );
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "hit"));
}

#[test]
fn an_explicit_load_failure_never_falls_back_to_building() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("a.json"), br#"{"text":"Alpha"}"#).unwrap();
    let missing = root.path().join("missing-index");
    let (status, events, error) = invoke(
        root.path(),
        &[
            "--path",
            "source",
            "--load-index",
            missing.to_str().unwrap(),
            "--fts",
            "Alpha",
        ],
    );
    assert_eq!(status.code(), Some(1), "{error}");
    assert!(!events.iter().any(|event| event["type"] == "hit"));
    assert!(!source.join(".objekte").exists());
}

#[test]
fn a_failed_save_does_not_run_queries() {
    let root = tempfile::tempdir().unwrap();
    std::fs::create_dir(root.path().join("source")).unwrap();
    std::fs::write(root.path().join("source/a.json"), br#"{"text":"Alpha"}"#).unwrap();
    let (status, events, error) = invoke(
        root.path(),
        &[
            "--path",
            "source",
            "--save-index",
            root.path().join("source").to_str().unwrap(),
            "--fts",
            "Alpha",
        ],
    );
    assert_eq!(status.code(), Some(1), "{error}");
    assert!(!events.iter().any(|event| event["type"] == "hit"));
}

#[test]
fn custom_index_opens_from_its_saved_recipe_without_source_arguments() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let store = root.path().join("store");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("a.json"), br#"{"text":"Alpha"}"#).unwrap();
    std::fs::write(source.join("b.json"), br#"{"text":"Beta"}"#).unwrap();
    let (status, events, error) = invoke(
        root.path(),
        &["--path", "source", "--save-index", store.to_str().unwrap()],
    );
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "index_saved"));

    // A custom index carries its opening recipe; no --path or --repo is needed.
    let (status, events, error) = invoke(
        root.path(),
        &["--load-index", store.to_str().unwrap(), "--fts", "Alpha"],
    );
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "index_loaded"));
    assert!(events.iter().any(|event| event["type"] == "hit"));

    // Storage-only opening also emits status from the recipe alone.
    let (status, events, error) = invoke(root.path(), &["--load-index", store.to_str().unwrap()]);
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "index_status"));

    // The hydrated recipe is identical, so re-saving the loaded publication is a no-op.
    let (status, events, error) = invoke(
        root.path(),
        &[
            "--load-index",
            store.to_str().unwrap(),
            "--save-index",
            store.to_str().unwrap(),
        ],
    );
    assert!(status.success(), "{error}");
    let saved = events
        .iter()
        .find(|event| event["type"] == "index_saved")
        .expect("index_saved event");
    assert_eq!(saved["unchanged"], true);
    assert_eq!(saved["bytes_written"], 0);
}

#[test]
fn saved_recipe_supplies_schema_inference_and_alphabet_options() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let store = root.path().join("store");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("a.json"), br#"{"text":"Alpha","year":2024}"#).unwrap();
    std::fs::write(
        root.path().join("schema.json"),
        br#"{
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "type": "object",
            "properties": {"text": {"type": "string"}, "year": {"type": "integer"}}
        }"#,
    )
    .unwrap();
    let (status, events, error) = invoke(
        root.path(),
        &[
            "--path",
            "source",
            "--schema",
            "schema.json",
            "--auto-index",
            "--index-alphabet",
            "unicode",
            "--save-index",
            store.to_str().unwrap(),
        ],
    );
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "index_saved"));

    // The saved recipe supplies the explicit schema, auto-index, and alphabet.
    let (status, events, error) = invoke(
        root.path(),
        &["--load-index", store.to_str().unwrap(), "year = 2024"],
    );
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "hit"));
    let start = events
        .iter()
        .find(|event| event["type"] == "run_start")
        .unwrap();
    assert_eq!(start["alphabet"], "unicode");
    assert_eq!(start["inference"], "auto_index");
    assert_eq!(start["schema_fields"], 2);

    // Explicit contradictory interpretation is an argument error.
    let (status, events, error) = invoke(
        root.path(),
        &[
            "--load-index",
            store.to_str().unwrap(),
            "--index-alphabet",
            "ascii",
            "--fts",
            "Alpha",
        ],
    );
    assert_eq!(status.code(), Some(2), "{error}");
    assert!(events.is_empty());
    assert!(error.contains("conflicts with the saved index"), "{error}");
}

#[test]
fn explicit_source_arguments_confirm_a_saved_recipe() {
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    let store = root.path().join("store");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("a.json"), br#"{"text":"Alpha"}"#).unwrap();
    let (status, _, error) = invoke(
        root.path(),
        &["--path", "source", "--save-index", store.to_str().unwrap()],
    );
    assert!(status.success(), "{error}");
    let (status, events, error) = invoke(
        root.path(),
        &[
            "--path",
            "source",
            "--load-index",
            store.to_str().unwrap(),
            "--fts",
            "Alpha",
        ],
    );
    assert!(status.success(), "{error}");
    assert!(events.iter().any(|event| event["type"] == "hit"));

    std::fs::create_dir(root.path().join("other")).unwrap();
    std::fs::write(root.path().join("other/a.json"), br#"{"text":"Alpha"}"#).unwrap();
    let (status, events, error) = invoke(
        root.path(),
        &[
            "--path",
            "other",
            "--load-index",
            store.to_str().unwrap(),
            "--fts",
            "Alpha",
        ],
    );
    assert_eq!(status.code(), Some(2), "{error}");
    assert!(events.is_empty());
}
