mod support;
use objekte_core::{CacheMode, Freshness, Repository};
use rusqlite::Connection;
use serde_json::json;
use std::{
    fs,
    fs::FileTimes,
    sync::{Arc, Barrier},
};
use support::*;
fn database(r: &Repository) -> Connection {
    Connection::open(r.cache_directory().unwrap().join("cache.sqlite")).unwrap()
}

#[test]
fn strict_reopen_hashes_but_does_not_reparse_unchanged_sources() {
    let f = Fixture::new();
    f.repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    let mut options = f.options(CacheMode::Sqlite);
    options.freshness = Freshness::HashOnOpen;
    let v = Repository::load_repository(&f.root, options)
        .unwrap()
        .open_view("books", &all())
        .unwrap();
    assert_eq!(v.metrics.hashes, 7);
    assert_eq!(v.metrics.parses, 0);
    assert_eq!(v.metrics.rows_reused, 7);
    assert_eq!(v.metrics.rows_updated, 0);
    assert_eq!(v.metrics.index_builds, 0);
}
#[test]
fn offline_edit_with_identical_size_and_timestamp_is_found() {
    let f = Fixture::new();
    f.repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    let p = f.root.join("data/books/a.json");
    let time = fs::metadata(&p).unwrap().modified().unwrap();
    let text = fs::read_to_string(&p).unwrap();
    assert!(text.contains("Beta"));
    let edited = text.replace("Beta", "Zeta");
    assert_eq!(edited.len(), text.len());
    fs::write(&p, edited).unwrap();
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_times(FileTimes::new().set_modified(time))
        .unwrap();
    let mut options = f.options(CacheMode::Sqlite);
    options.freshness = Freshness::HashOnOpen;
    let v = Repository::load_repository(&f.root, options)
        .unwrap()
        .open_view("books", &all())
        .unwrap();
    assert_eq!(v.rows.iter().find(|r| r.id == "a").unwrap().label, "Zeta");
    assert_eq!(v.metrics.rows_updated, 1);
    assert_eq!(v.metrics.parses, 1);
    assert_eq!(v.metrics.index_rows_written, 1);
}
#[test]
fn explicit_refresh_and_hash_on_request_observe_live_edits() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    f.edit("data/books/a.json", |v| v["score"] = json!(-5));
    assert_ne!(r.open_view("books", &all()).unwrap().rows[0].id, "a");
    let delta = r.refresh("books").unwrap();
    assert_eq!(delta.rows_updated, 1);
    assert_eq!(delta.rows_reused, 6);
    assert_eq!(delta.index_rows_written, 1);
    assert_eq!(r.open_view("books", &all()).unwrap().rows[0].id, "a");
    let mut options = f.options(CacheMode::Sqlite);
    options.freshness = Freshness::HashOnRequest;
    let mut r = Repository::load_repository(&f.root, options).unwrap();
    r.open_view("books", &all()).unwrap();
    f.edit("data/books/a.json", |v| v["score"] = json!(55));
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(v.metrics.rows_updated, 1);
    assert_ne!(v.rows[0].id, "a");
}
#[test]
fn additions_deletions_and_filename_renames_are_targeted() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    fs::rename(
        f.root.join("data/books/a.json"),
        f.root.join("data/books/z.json"),
    )
    .unwrap();
    fs::remove_file(f.root.join("data/books/c.json")).unwrap();
    f.write(
        "data/books/new.json",
        &json!({"title":"New","score":-10,"payload":null}),
    );
    let m = r.refresh("books").unwrap();
    assert_eq!(m.rows_inserted, 2);
    assert_eq!(m.rows_deleted, 2);
    assert_eq!(m.rows_updated, 0);
    assert_eq!(m.index_rows_written, 2);
    let sql = r.open_view("books", &all()).unwrap();
    let direct = f
        .repo(CacheMode::Disabled)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(sql.rows, direct.rows);
    let rows = database(&r)
        .query_row(
            "SELECT count(*) FROM edges WHERE source_collection='books' AND source_id='a'",
            [],
            |r| r.get::<_, i64>(0),
        )
        .unwrap();
    assert_eq!(rows, 0);
}
#[test]
fn malformed_or_invalid_files_cannot_expose_previous_cached_rows() {
    for bad in [
        "not json",
        "{\"payload\":null,\"score\":\"wrong\"}",
        "{\"score\":1}",
        "{\"payload\":null,\"extra\":true}",
    ] {
        let f = Fixture::new();
        let mut r = f.repo(CacheMode::Sqlite);
        r.open_view("books", &all()).unwrap();
        fs::write(f.root.join("data/books/a.json"), bad).unwrap();
        r.refresh("books").unwrap();
        r.validate_collection("books").unwrap();
        let view = r.open_view("books", &all()).unwrap();
        let expected = if bad == "not json" {
            objekte_core::RecordStatus::IncorrectJson
        } else {
            objekte_core::RecordStatus::Invalid
        };
        assert_eq!(view.states["a"].status, expected);
        assert_eq!(view.total, Some(7));
        assert_ne!(
            view.rows
                .iter()
                .find(|r| r.id == "a")
                .unwrap()
                .values
                .get("title"),
            Some(&json!("Alpha"))
        );
        let state = r
            .cache_status()
            .unwrap()
            .collections
            .into_iter()
            .find(|c| c.collection == "books")
            .unwrap();
        assert_eq!(state.state, "current");
        assert!(state.verified);
        let restarted = f
            .repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .unwrap();
        assert_eq!(restarted.states["a"].status, expected);
    }
}
#[test]
fn interrupted_generation_is_reconciled_before_reuse() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    database(&r)
        .execute(
            "UPDATE collections SET state='building' WHERE name='books'",
            [],
        )
        .unwrap();
    f.edit("data/books/a.json", |v| v["score"] = json!(-1));
    drop(r);
    let mut r = f.repo(CacheMode::Sqlite);
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(v.rows[0].id, "a");
    assert_eq!(v.metrics.rows_updated, 1);
    assert_eq!(
        r.cache_status()
            .unwrap()
            .collections
            .iter()
            .find(|c| c.collection == "books")
            .unwrap()
            .state,
        "current"
    );
}
#[test]
fn transaction_failure_rolls_back_rows_and_preserves_dirty_state() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    let db = database(&r);
    db.execute_batch("CREATE TRIGGER reject_b BEFORE UPDATE ON records WHEN NEW.id='b' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").unwrap();
    f.edit("data/books/a.json", |v| v["score"] = json!(-7));
    f.edit("data/books/b.json", |v| v["score"] = json!(-8));
    assert!(r.refresh("books").is_err());
    let old: Vec<u8> = db
        .query_row(
            "SELECT json FROM records WHERE collection='books' AND id='a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&old).unwrap()["score"],
        json!(2)
    );
    assert_eq!(
        db.query_row(
            "SELECT state FROM collections WHERE name='books'",
            [],
            |r| r.get::<_, String>(0)
        )
        .unwrap(),
        "dirty"
    );
    db.execute_batch("DROP TRIGGER reject_b").unwrap();
    drop(db);
    let m = r.refresh("books").unwrap();
    assert_eq!(m.rows_updated, 2);
    assert_eq!(r.open_view("books", &all()).unwrap().rows[0].id, "b");
}
#[test]
fn schema_change_revalidates_and_changes_relation_extraction() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    f.edit(".editor/books/schema.json", |v| {
        v["properties"]["person"]
            .as_object_mut()
            .unwrap()
            .remove("x-relation");
    });
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(v.metrics.validations, 0);
    assert_eq!(v.metrics.rows_updated, 0);
    assert_eq!(v.metrics.edges_removed, 2);
    let db = database(&r);
    assert_eq!(
        db.query_row(
            "SELECT count(*) FROM edges WHERE source_collection='books' AND location='/person'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    f.edit(".editor/books/schema.json", |v| {
        v["required"].as_array_mut().unwrap().push(json!("title"))
    });
    r.open_view("books", &all()).unwrap();
    r.validate_collection("books").unwrap();
    assert!(
        r.open_view("books", &all())
            .unwrap()
            .states
            .values()
            .any(|s| s.status == objekte_core::RecordStatus::Invalid)
    );
}
#[test]
fn source_mapping_change_does_not_reuse_old_records() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    fs::create_dir(f.root.join("data/other")).unwrap();
    f.write(
        "data/other/z.json",
        &json!({"payload":null,"title":"Other","score":99}),
    );
    f.edit(".editor/books/settings.json", |v| {
        v["path"] = json!("data/other")
    });
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(ids(&v.rows), vec!["z"]);
    assert_eq!(v.rows[0].path, "data/other/z.json");
    assert_eq!(v.metrics.rows_deleted, 7);
    assert_eq!(v.metrics.rows_inserted, 1);
}
#[test]
fn deleted_cache_and_incompatible_format_rebuild_lazily() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    fs::remove_dir_all(r.cache_directory().unwrap()).unwrap();
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(v.metrics.rows_inserted, 7);
    database(&r)
        .execute("UPDATE meta SET value='old-format' WHERE key='format'", [])
        .unwrap();
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(v.metrics.rows_inserted, 7);
    assert_eq!(v.total, Some(7));
}
#[test]
fn separate_repositories_namespaces_and_cache_modes_are_isolated() {
    let f = Fixture::new();
    let mut first = f.repo(CacheMode::Sqlite);
    let expected = first.open_view("books", &all()).unwrap().rows;
    let mut options = f.options(CacheMode::Sqlite);
    options.namespace = "experiment-b".into();
    let mut second = Repository::load_repository(&f.root, options).unwrap();
    assert_ne!(
        first.cache_directory().unwrap(),
        second.cache_directory().unwrap()
    );
    assert_eq!(
        second
            .open_view("books", &all())
            .unwrap()
            .metrics
            .rows_inserted,
        7
    );
    let other = f.temp.path().join("other");
    copy(&f.root, &other);
    let mut third = Repository::load_repository(&other, f.options(CacheMode::Sqlite)).unwrap();
    assert_ne!(
        first.cache_directory().unwrap(),
        third.cache_directory().unwrap()
    );
    assert_eq!(third.open_view("books", &all()).unwrap().rows, expected);
    let clean = f.temp.path().join("disabled");
    let mut options = f.options(CacheMode::Disabled);
    options.data_dir = clean.clone();
    let mut r = Repository::load_repository(&f.root, options).unwrap();
    assert_eq!(r.open_view("books", &all()).unwrap().rows, expected);
    assert!(!clean.exists());
}
#[test]
fn wrong_repository_identity_is_rejected() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    database(&r)
        .execute(
            "UPDATE meta SET value='someone-else' WHERE key='repository'",
            [],
        )
        .unwrap();
    assert!(
        r.open_view("books", &all())
            .unwrap_err()
            .to_string()
            .contains("identity mismatch")
    );
}
#[test]
fn simultaneous_handles_serialize_reconciliation() {
    let f = Fixture::new();
    f.repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    f.edit("data/books/a.json", |v| v["score"] = json!(-9));
    let barrier = Arc::new(Barrier::new(2));
    let handles = (0..2)
        .map(|_| {
            let root = f.root.clone();
            let opts = f.options(CacheMode::Sqlite);
            let b = barrier.clone();
            std::thread::spawn(move || {
                let mut r = Repository::load_repository(root, opts).unwrap();
                b.wait();
                r.open_view("books", &all()).unwrap()
            })
        })
        .collect::<Vec<_>>();
    let views = handles
        .into_iter()
        .map(|h| h.join().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(views[0].rows, views[1].rows);
    assert_eq!(views[0].rows[0].id, "a");
    assert_eq!(views.iter().map(|v| v.metrics.rows_updated).sum::<u64>(), 1);
}
#[test]
fn get_refreshes_original_and_missing_reads_remove_stale_cache_rows() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    f.edit("data/books/a.json", |v| v["title"] = json!("Updated"));
    assert_eq!(
        r.get_record("books", "a").unwrap().unwrap().data["title"],
        "Updated"
    );
    assert_eq!(
        r.open_view("books", &all())
            .unwrap()
            .rows
            .iter()
            .find(|r| r.id == "a")
            .unwrap()
            .label,
        "Updated"
    );
    fs::remove_file(f.root.join("data/books/a.json")).unwrap();
    assert!(r.get_record("books", "a").unwrap().is_none());
    assert_eq!(r.open_view("books", &all()).unwrap().total, Some(6));
}
#[test]
fn nested_source_layout_is_rejected_instead_of_silently_omitted() {
    let f = Fixture::new();
    fs::create_dir(f.root.join("data/books/sub")).unwrap();
    f.write("data/books/sub/hidden.json", &json!({"payload":null}));
    assert!(
        f.repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .unwrap_err()
            .to_string()
            .contains("nested")
    );
}
#[cfg(unix)]
#[test]
fn symlink_sources_and_path_traversal_are_rejected() {
    use std::os::unix::fs::symlink;
    let f = Fixture::new();
    fs::remove_file(f.root.join("data/books/a.json")).unwrap();
    let secret = f.temp.path().join("external.json");
    fs::write(&secret, "{\"payload\":null}").unwrap();
    symlink(&secret, f.root.join("data/books/a.json")).unwrap();
    let mut r = f.repo(CacheMode::Sqlite);
    assert!(r.get_record("books", "a").is_err());
    assert!(r.open_view("books", &all()).is_err());
    for id in ["../people/p1", "", ".", "..", "x/y", "x\\y"] {
        assert!(r.get_record("books", id).is_err())
    }
}
#[test]
fn malformed_get_replaces_a_previously_current_view() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    fs::write(f.root.join("data/books/a.json"), "broken").unwrap();
    assert_eq!(
        r.get_record("books", "a").unwrap().unwrap().state.status,
        objekte_core::RecordStatus::IncorrectJson
    );
    assert_eq!(
        r.open_view("books", &all()).unwrap().states["a"].status,
        objekte_core::RecordStatus::IncorrectJson
    );
}

#[test]
fn failed_transactions_do_not_report_rolled_back_writes_as_committed() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    r.reset_metrics();
    database(&r).execute_batch("CREATE TRIGGER fail_second BEFORE UPDATE ON records WHEN NEW.id='b' BEGIN SELECT RAISE(ABORT,'failure'); END;").unwrap();
    f.edit("data/books/a.json", |v| v["score"] = json!(7));
    f.edit("data/books/b.json", |v| v["score"] = json!(8));
    assert!(r.refresh("books").is_err());
    assert_eq!(r.metrics().rows_updated, 0);
    assert_eq!(r.metrics().index_rows_written, 0);
    assert!(r.metrics().source_reads >= 2);
}
#[test]
fn missing_collection_directory_is_an_error_not_a_missing_record() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Disabled);
    fs::remove_dir_all(f.root.join("data/books")).unwrap();
    assert!(r.get_record("books", "a").is_err());
}
#[test]
fn a_corrupt_database_does_not_prevent_explicit_direct_reads() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    let path = r.cache_directory().unwrap().join("cache.sqlite");
    fs::write(path, "not a SQLite database").unwrap();
    assert!(r.open_view("books", &all()).is_err());
    assert_eq!(
        f.repo(CacheMode::Disabled)
            .open_view("books", &all())
            .unwrap()
            .total,
        Some(7)
    );
}
