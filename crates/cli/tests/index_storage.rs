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
        &[
            "--path",
            "source",
            "--save-index",
            store.to_str().unwrap(),
        ],
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
