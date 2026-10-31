use serde_json::{Value, json};
use std::{fs, path::Path, process::Command};

fn fixture() -> tempfile::TempDir {
    let temp = tempfile::tempdir().unwrap();
    let docs = temp.path().join("data/docs");
    fs::create_dir_all(&docs).unwrap();
    for id in 0..24 {
        fs::write(
            docs.join(format!("{id}.json")),
            json!({
                "active": id % 2 == 0,
                "category": if id % 2 == 0 { "essay" } else { "letter" },
                "detail": {"rank": id},
            })
            .to_string(),
        )
        .unwrap();
    }
    let schema = schema_path(temp.path());
    fs::create_dir_all(schema.parent().unwrap()).unwrap();
    fs::write(
        schema,
        json!({
            "type": "object",
            "properties": {
                "active": {"type": "string", "index": false, "exclude_fts": true},
                "category": {"type": "string", "enum": ["old"]},
                "unused": {"type": "integer", "index": true},
            },
        })
        .to_string(),
    )
    .unwrap();
    temp
}

fn schema_path(root: &Path) -> std::path::PathBuf {
    root.join(".editor/collections/docs/schema.json")
}

fn run(root: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_objekte"))
        .arg(root)
        .args(args)
        .output()
        .unwrap()
}

fn output(root: &Path, args: &[&str]) -> Value {
    let result = run(root, args);
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    serde_json::from_slice(&result.stdout).unwrap()
}

fn read_schema(root: &Path) -> Value {
    serde_json::from_slice(&fs::read(schema_path(root)).unwrap()).unwrap()
}

#[test]
fn inferschema_reports_existing_fields_without_writing() {
    let temp = fixture();
    let original = fs::read(schema_path(temp.path())).unwrap();
    let report = output(temp.path(), &["inferschema", "docs", "--diagnostics"]);
    let fields = report["collections"][0]["fields"].as_array().unwrap();
    assert_eq!(report["collections"][0]["collection"], "docs");
    assert_eq!(report["collections"][0]["parsed"], 24);
    assert_eq!(report["collections"][0]["schema_written"], false);
    assert!(fields.iter().any(|field| field["display_path"] == "active"
        && field["kind"] == "boolean"
        && field["recommended_index"] == true));
    assert!(
        fields
            .iter()
            .any(|field| field["display_path"] == "category"
                && field["kind"] == "enum"
                && field["recommended_index"] == true)
    );
    assert_eq!(fs::read(schema_path(temp.path())).unwrap(), original);
}

#[test]
fn writeschema_updates_existing_declarations_and_preserves_other_fields() {
    let temp = fixture();
    let report = output(temp.path(), &["writeschema", "--collection", "docs"]);
    assert_eq!(report["collections"][0]["schema_written"], true);
    let schema = read_schema(temp.path());
    let fields = &schema["properties"];
    assert_eq!(
        fields["active"],
        json!({
            "type":"boolean", "index":true, "exclude_fts":true
        })
    );
    assert_eq!(fields["category"]["enum"], json!(["essay", "letter"]));
    assert_eq!(fields["category"]["index"], true);
    assert_eq!(fields["unused"], json!({"type":"integer", "index":true}));
    assert!(
        schema["x-index-paths"]
            .as_array()
            .unwrap()
            .contains(&json!("detail.rank"))
    );
    let second = output(temp.path(), &["writeschema", "docs"]);
    assert_eq!(second["collections"][0]["schema_written"], false);
}

#[test]
fn writeschema_can_update_types_without_changing_index_rules() {
    let temp = fixture();
    let report = output(temp.path(), &["writeschema", "docs", "--no-auto-index"]);
    assert_eq!(report["collections"][0]["auto_index_requested"], false);
    let schema = read_schema(temp.path());
    assert_eq!(schema["properties"]["active"]["type"], "boolean");
    assert_eq!(schema["properties"]["active"]["index"], false);
    assert!(schema["properties"]["category"].get("index").is_none());
    assert!(schema.get("x-index-paths").is_none());
}

#[test]
fn inference_commands_accept_all_collections_and_skip_schema_free_writeback() {
    let temp = fixture();
    let other = temp.path().join("data/other");
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("a.json"), r#"{"value":7}"#).unwrap();
    let report = output(temp.path(), &["inferschema", "--no-auto-index"]);
    assert_eq!(report["collections"].as_array().unwrap().len(), 2);
    let saved = output(temp.path(), &["writeschema"]);
    let collections = saved["collections"].as_array().unwrap();
    assert_eq!(collections.len(), 2);
    assert!(collections.iter().any(|collection| {
        collection["collection"] == "docs" && collection["schema_written"] == true
    }));
    assert!(collections.iter().any(|collection| {
        collection["collection"] == "other" && collection["schema_written"] == false
    }));
}

#[test]
fn inference_cli_rejects_conflicting_scope_and_write_without_schema() {
    let temp = fixture();
    let conflict = run(
        temp.path(),
        &["inferschema", "docs", "--collection", "docs"],
    );
    assert!(!conflict.status.success());
    assert!(String::from_utf8_lossy(&conflict.stderr).contains("choose positional COLLECTION"));
    let invalid = run(temp.path(), &["writeschema", "docs", "--limit", "1"]);
    assert!(!invalid.status.success());
    assert!(String::from_utf8_lossy(&invalid.stderr).contains("not valid"));
    fs::remove_file(schema_path(temp.path())).unwrap();
    let missing = run(temp.path(), &["writeschema", "docs"]);
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("existing schema"));
    let report = output(temp.path(), &["inferschema", "docs"]);
    assert_eq!(report["collections"][0]["schema_written"], false);
}
