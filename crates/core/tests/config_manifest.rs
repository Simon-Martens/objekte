mod support;
use objekte_core::{CacheMode, ConfigurationChangeKind as Kind, Repository};
use rusqlite::Connection;
use serde_json::json;
use std::{collections::BTreeMap, fs, path::Path};
use support::*;

fn db(r: &Repository) -> Connection {
    Connection::open(r.cache_directory().unwrap().join("cache.sqlite")).unwrap()
}

fn inputs(db: &Connection) -> BTreeMap<String, String> {
    db.prepare("SELECT path,digest FROM inputs")
        .unwrap()
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn version(db: &Connection) -> i64 {
    db.query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap()
}

fn schema_change(f: &Fixture) {
    f.edit(".editor/books/schema.json", |v| {
        v["properties"]["new_required"] = json!({"type":"string"});
        v["required"] = json!(["payload", "new_required"]);
    });
}

#[test]
fn baseline_and_unchanged_restart_touch_no_records_or_database_pages() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    assert!(!f.data.exists());
    let initial = r.refresh_configuration().unwrap();
    assert!(initial.persistent);
    assert_eq!(initial.inputs.len(), 11);
    assert!(
        initial
            .inputs
            .iter()
            .all(|i| i.kind == Kind::Added && i.previous_digest.is_none())
    );
    assert_eq!(initial.affected_collections, ["books", "people", "spare"]);
    assert_eq!(r.metrics().enumerations, 0);
    assert_eq!(r.metrics().source_reads, 0);
    let observer = db(&r);
    let before = version(&observer);
    drop(r);
    let mut r = f.repo(CacheMode::Sqlite);
    assert!(r.refresh_configuration().unwrap().inputs.is_empty());
    assert_eq!(version(&observer), before);
    assert_eq!(r.metrics().enumerations, 0);
    assert_eq!(r.metrics().source_reads, 0);
    assert!(
        r.cache_status()
            .unwrap()
            .collections
            .iter()
            .all(|c| c.state == "absent")
    );
}

#[test]
fn observed_schema_survives_another_restart_without_claiming_it_was_applied() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    r.validate_collection("books").unwrap();
    let old_schema = r.model().collections["books"].schema_hash.clone();
    let saved = inputs(&db(&r));
    drop(r);
    schema_change(&f);
    let mut r = f.repo(CacheMode::Sqlite);
    let changes = r.refresh_configuration().unwrap();
    assert_eq!(changes.affected_collections, ["books"]);
    assert_eq!(changes.inputs.len(), 1);
    let change = &changes.inputs[0];
    assert_eq!(change.path, Path::new(".editor/books/schema.json"));
    assert_eq!(change.kind, Kind::Modified);
    assert_eq!(
        change.previous_digest.as_ref(),
        saved.get(f.root.join(&change.path).to_str().unwrap())
    );
    assert_ne!(change.previous_digest, change.digest);
    assert_eq!(r.metrics().source_reads, 0);
    assert_eq!(r.metrics().enumerations, 0);
    let status = r.cache_status().unwrap();
    let books = status
        .collections
        .iter()
        .find(|c| c.collection == "books")
        .unwrap();
    assert_eq!(
        books.applied_schema_hash.as_deref(),
        Some(old_schema.as_str())
    );
    assert_ne!(books.expected_schema_hash, old_schema);
    assert!(!books.configuration_current);
    assert!(!books.verified);
    // Observing configuration neither resets validation nor marks source inventory dirty.
    assert_eq!(books.state, "current");
    assert_eq!(books.validation.get("valid"), Some(&7));
    r.open_view("spare", &all()).unwrap();
    drop(r);
    let mut r = f.repo(CacheMode::Sqlite);
    assert!(r.refresh_configuration().unwrap().inputs.is_empty());
    let view = r.open_view("books", &all()).unwrap();
    assert_eq!(view.metrics.source_reads, 0);
    assert_eq!(view.metrics.parses, 7);
    assert!(
        view.states
            .values()
            .all(|s| s.status == objekte_core::RecordStatus::Pending)
    );
    r.validate_collection("books").unwrap();
    let status = r.cache_status().unwrap();
    let books = status
        .collections
        .iter()
        .find(|c| c.collection == "books")
        .unwrap();
    assert!(books.configuration_current);
    assert_eq!(
        books.applied_schema_hash.as_ref(),
        Some(&books.expected_schema_hash)
    );
    assert_eq!(books.validation.get("invalid"), Some(&7));
}

#[test]
fn schema_and_data_changes_while_closed_use_both_manifests() {
    let f = Fixture::new();
    f.repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    schema_change(&f);
    f.edit("data/books/a.json", |v| {
        v["title"] = json!("Edited while closed")
    });
    let mut r = f.repo(CacheMode::Sqlite);
    assert_eq!(r.refresh_configuration().unwrap().inputs.len(), 1);
    assert_eq!(r.metrics().source_reads, 0);
    let view = r.open_view("books", &all()).unwrap();
    assert_eq!(view.metrics.source_reads, 1);
    assert!(
        view.rows
            .iter()
            .any(|row| row.label == "Edited while closed")
    );
}

#[test]
fn formatting_and_unrelated_changes_preserve_verified_views() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    let schema = r.model().collections["books"].schema_hash.clone();
    fs::write(
        f.root.join(".editor/books/schema.json"),
        serde_json::to_vec(&f.read(".editor/books/schema.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        r.refresh_configuration().unwrap().affected_collections,
        ["books"]
    );
    assert_eq!(r.model().collections["books"].schema_hash, schema);
    f.edit(".editor/spare/ui.json", |v| v["label"] = json!("New label"));
    assert_eq!(
        r.refresh_configuration().unwrap().affected_collections,
        ["spare"]
    );
    let view = r.open_view("books", &all()).unwrap();
    assert_eq!(view.metrics.enumerations, 0);
    assert_eq!(view.metrics.parses, 0);
    assert_eq!(view.metrics.index_builds, 0);
    assert!(
        r.cache_status()
            .unwrap()
            .collections
            .iter()
            .find(|c| c.collection == "books")
            .unwrap()
            .verified
    );
}

#[test]
fn shared_files_and_repository_defaults_identify_all_consumers() {
    let f = Fixture::new();
    // Spare shares both files with people but still has its own source/settings.
    f.edit(".editor/spare/settings.json", |v| {
        v["schema"] = json!(".editor/people/schema.json");
        v["ui"] = json!(".editor/people/ui.json");
    });
    f.repo(CacheMode::Sqlite).refresh_configuration().unwrap();
    f.edit(".editor/people/ui.json", |v| v["label"] = json!("Shared"));
    let changes = f.repo(CacheMode::Sqlite).refresh_configuration().unwrap();
    assert_eq!(changes.affected_collections, ["people", "spare"]);
    f.edit(".editor/ui.json", |v| v["defaults"]["pageSize"] = json!(12));
    let changes = f.repo(CacheMode::Sqlite).refresh_configuration().unwrap();
    assert_eq!(changes.affected_collections, ["books", "people", "spare"]);
}

#[test]
fn removed_and_added_configurations_update_manifest_and_ownership() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.refresh_configuration().unwrap();
    let observer = db(&r);
    let registry = f.read(".editor/repository.json");
    // A live handle must reload successfully even though an old input disappeared.
    f.edit(".editor/repository.json", |v| {
        v["collections"].as_object_mut().unwrap().remove("spare");
    });
    fs::remove_file(f.root.join(".editor/spare/schema.json")).unwrap();
    let changes = r.refresh_configuration().unwrap();
    let removed = changes
        .inputs
        .iter()
        .filter(|c| c.kind == Kind::Removed)
        .collect::<Vec<_>>();
    assert_eq!(removed.len(), 3);
    assert!(
        removed
            .iter()
            .all(|c| c.collections == ["spare"] && c.digest.is_none())
    );
    assert!(!inputs(&observer).keys().any(|p| p.contains("/spare/")));
    assert_eq!(
        observer
            .query_row(
                "SELECT count(*) FROM input_collections WHERE collection='spare'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    drop(r);
    f.write(".editor/repository.json", &registry);
    f.write(
        ".editor/spare/schema.json",
        &json!({"type":"object", "properties":{"name":{"type":"string"}}}),
    );
    let changes = f.repo(CacheMode::Sqlite).refresh_configuration().unwrap();
    assert_eq!(
        changes
            .inputs
            .iter()
            .filter(|c| c.kind == Kind::Added)
            .count(),
        3
    );
    assert_eq!(
        observer
            .query_row(
                "SELECT count(*) FROM input_collections WHERE collection='spare'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        5
    );
}

#[test]
fn invalid_or_missing_required_configuration_keeps_previous_manifest() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.refresh_configuration().unwrap();
    let observer = db(&r);
    let before = inputs(&observer);
    let schema = f.read(".editor/books/schema.json");
    fs::write(f.root.join(".editor/books/schema.json"), "{broken").unwrap();
    assert!(r.refresh_configuration().is_err());
    assert_eq!(inputs(&observer), before);
    fs::remove_file(f.root.join(".editor/books/schema.json")).unwrap();
    assert!(r.refresh_configuration().is_err());
    assert_eq!(inputs(&observer), before);
    f.write(".editor/books/schema.json", &schema);
    assert_eq!(r.refresh_configuration().unwrap().inputs.len(), 1); // formatting changed
}

#[test]
fn transaction_failure_rolls_back_digests_and_consumers_then_retry_redetects_changes() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.refresh_configuration().unwrap();
    let observer = db(&r);
    let before = inputs(&observer);
    f.edit(".editor/spare/settings.json", |v| {
        v["schema"] = json!(".editor/people/schema.json")
    });
    observer.execute_batch("CREATE TRIGGER fail_ownership BEFORE INSERT ON input_collections BEGIN SELECT RAISE(FAIL, 'manifest failure'); END;").unwrap();
    assert!(
        r.refresh_configuration()
            .unwrap_err()
            .to_string()
            .contains("manifest failure")
    );
    assert_eq!(inputs(&observer), before);
    assert_eq!(
        observer
            .query_row(
                "SELECT count(*) FROM input_collections WHERE path LIKE '%/spare/schema.json'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    observer
        .execute_batch("DROP TRIGGER fail_ownership")
        .unwrap();
    drop(r);
    let mut r = f.repo(CacheMode::Sqlite);
    let changes = r.refresh_configuration().unwrap();
    assert_eq!(changes.affected_collections, ["spare"]);
    assert_eq!(changes.inputs.len(), 2); // changed settings, former schema removed
    assert!(r.refresh_configuration().unwrap().inputs.is_empty());
}

#[test]
fn collection_update_failure_cannot_erase_pending_schema_change() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    let observer = db(&r);
    let old = r.model().collections["books"].schema_hash.clone();
    schema_change(&f);
    r.refresh_configuration().unwrap();
    observer.execute_batch(&record_sql("books", "CREATE TRIGGER fail_record BEFORE UPDATE ON records BEGIN SELECT RAISE(FAIL, 'record failure'); END;")).unwrap();
    assert!(r.open_view("books", &all()).is_err());
    assert_eq!(
        observer
            .query_row(
                "SELECT schema_hash FROM collections WHERE name='books'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        old
    );
    drop(r);
    observer.execute_batch("DROP TRIGGER fail_record").unwrap();
    let mut r = f.repo(CacheMode::Sqlite);
    assert!(r.refresh_configuration().unwrap().inputs.is_empty());
    r.open_view("books", &all()).unwrap();
    assert_ne!(
        observer
            .query_row(
                "SELECT schema_hash FROM collections WHERE name='books'",
                [],
                |r| r.get::<_, String>(0)
            )
            .unwrap(),
        old
    );
}

#[test]
fn older_caches_gain_ownership_without_losing_applied_versions() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    let observer = db(&r);
    drop(r);
    // INFO: A pre-migration cache has neither ownership nor migration history.
    observer
        .execute_batch("DROP TABLE input_collections; DROP TABLE migrations")
        .unwrap();
    schema_change(&f);
    let mut r = f.repo(CacheMode::Sqlite);
    // No previous ownership: conservative fallback, still lazy data reconciliation.
    assert_eq!(
        r.refresh_configuration().unwrap().affected_collections,
        ["books", "people", "spare"]
    );
    assert_eq!(
        r.open_view("books", &all()).unwrap().metrics.source_reads,
        0
    );
    assert_eq!(
        observer
            .query_row(
                &record_sql(
                    "books",
                    "SELECT count(*) FROM records WHERE collection='books'"
                ),
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        7
    );
}

#[test]
fn disabled_cache_tracks_in_memory_without_creating_application_data() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Disabled);
    assert!(r.refresh_configuration().unwrap().inputs.is_empty());
    schema_change(&f);
    let changes = r.refresh_configuration().unwrap();
    assert!(!changes.persistent);
    assert_eq!(changes.affected_collections, ["books"]);
    assert!(r.refresh_configuration().unwrap().inputs.is_empty());
    assert!(!f.data.exists());
    assert_eq!(r.metrics().source_reads, 0);
}
