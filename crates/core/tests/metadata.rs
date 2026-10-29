mod support;
use objekte_core::{CacheMode, Freshness, Repository};
use rusqlite::Connection;
use serde_json::json;
use std::{fs, fs::FileTimes, time::Duration};
use support::*;

fn state(r: &Repository) -> (String, i64, String) {
    Connection::open(r.cache_directory().unwrap().join("cache.sqlite"))
        .unwrap()
        .query_row(
            "SELECT manifest_hash,revision,view_hash FROM collections WHERE name='books'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap()
}

#[test]
fn changing_page_cache_budget_reuses_existing_records_and_indexes() {
    let f = Fixture::new();
    let mut options = f.options(CacheMode::Sqlite);
    options.sqlite_cache_mib = None;
    let mut original = Repository::load_repository(&f.root, options.clone()).unwrap();
    let expected = original.open_view("books", &all()).unwrap();
    let before = state(&original);
    for budget in [Some(1), Some(16), Some(64), None] {
        options.sqlite_cache_mib = budget;
        let mut reopened = Repository::load_repository(&f.root, options.clone()).unwrap();
        let actual = reopened.open_view("books", &all()).unwrap();
        assert_eq!(actual.rows, expected.rows);
        assert_eq!(actual.view_hashes, expected.view_hashes);
        assert_eq!(actual.metrics.manifest_hits, 1);
        assert_eq!(actual.metrics.source_reads, 0);
        assert_eq!(actual.metrics.index_builds, 0);
        assert_eq!(actual.metrics.validation_nanos, 0);
        assert_eq!(state(&reopened), before);
    }
}

#[test]
fn unchanged_manifest_skips_sources_and_database_manifest_on_reopen() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    let first = r.open_view("books", &all()).unwrap();
    assert_eq!(first.metrics.enumerations, 1);
    let before = state(&r);
    let observer = Connection::open(r.cache_directory().unwrap().join("cache.sqlite")).unwrap();
    let version: i64 = observer
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap();
    drop(r);
    let mut r = f.repo(CacheMode::Sqlite);
    let next = r.open_view("books", &all()).unwrap();
    assert_eq!(next.rows, first.rows);
    assert_eq!(next.metrics.enumerations, 1);
    assert_eq!(next.metrics.manifest_hits, 1);
    assert_eq!(next.metrics.manifest_loads, 0);
    assert_eq!(next.metrics.source_reads, 0);
    assert_eq!(next.metrics.hashes, 0);
    assert_eq!(next.metrics.parses, 0);
    assert_eq!(next.metrics.metadata_reused, 7);
    assert_eq!(next.metrics.index_rows_written, 0);
    assert_eq!(state(&r), before);
    assert_eq!(
        observer
            .query_row("PRAGMA data_version", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        version,
        "unchanged reopen must not rewrite metadata or commit changed DB pages"
    );
}

#[test]
fn only_changed_candidates_are_read_and_committed_manifest_is_reusable() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    let before = state(&r);
    f.edit("data/books/a.json", |v| {
        v["title"] = json!("A different title")
    });
    let m = r.refresh("books").unwrap();
    assert_eq!(m.enumerations, 1);
    assert_eq!(m.manifest_loads, 1);
    assert_eq!(m.source_reads, 1);
    assert_eq!(m.hashes, 1);
    assert_eq!(m.validations, 0);
    assert_eq!(m.rows_updated, 1);
    assert_eq!(m.metadata_reused, 6);
    assert_ne!(state(&r).0, before.0);
    assert_eq!(state(&r).1, before.1 + 1);
    let next = f
        .repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(next.metrics.manifest_hits, 1);
    assert_eq!(next.metrics.source_reads, 0);
    assert!(next.rows.iter().any(|r| r.label == "A different title"));
}

#[test]
fn metadata_only_change_rehashes_without_reparsing_or_reindexing() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    let before = state(&r);
    let p = f.root.join("data/books/a.json");
    let modified = fs::metadata(&p).unwrap().modified().unwrap() + Duration::from_secs(2);
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    let m = r.refresh("books").unwrap();
    assert_eq!(m.source_reads, 1);
    assert_eq!(m.hashes, 1);
    assert_eq!(m.parses, 0);
    assert_eq!(m.rows_updated, 0);
    assert_eq!(m.index_rows_written, 0);
    assert_eq!(m.rows_reused, 7);
    assert_ne!(state(&r).0, before.0);
    assert_eq!(state(&r).1, before.1);
    assert_eq!(
        f.repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .unwrap()
            .metrics
            .manifest_hits,
        1
    );
}

#[test]
fn deletion_only_needs_no_source_reads_and_empty_manifest_is_reusable() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    for e in fs::read_dir(f.root.join("data/books")).unwrap() {
        fs::remove_file(e.unwrap().path()).unwrap();
    }
    let m = r.refresh("books").unwrap();
    assert_eq!(m.source_reads, 0);
    assert_eq!(m.rows_deleted, 7);
    let v = f
        .repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(v.total, Some(0));
    assert_eq!(v.metrics.manifest_hits, 1);
}

#[test]
fn metadata_on_request_observes_edits_without_explicit_refresh() {
    let f = Fixture::new();
    let mut options = f.options(CacheMode::Sqlite);
    options.freshness = Freshness::MetadataOnRequest;
    let mut r = Repository::load_repository(&f.root, options).unwrap();
    r.open_view("books", &all()).unwrap();
    f.edit("data/books/a.json", |v| v["score"] = json!(-100));
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(v.rows[0].id, "a");
    assert_eq!(v.metrics.hashes, 1);
    assert_eq!(
        r.open_view("books", &all()).unwrap().metrics.manifest_hits,
        1
    );
}

#[test]
fn dirty_or_partial_state_cannot_take_the_manifest_shortcut() {
    for state in ["dirty", "partial", "building"] {
        let f = Fixture::new();
        let mut r = f.repo(CacheMode::Sqlite);
        r.open_view("books", &all()).unwrap();
        let db = Connection::open(r.cache_directory().unwrap().join("cache.sqlite")).unwrap();
        db.execute(
            "UPDATE collections SET state=?1 WHERE name='books'",
            [state],
        )
        .unwrap();
        let v = f
            .repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .unwrap();
        assert_eq!(v.metrics.manifest_hits, 0);
        assert_eq!(v.metrics.hashes, 7);
        assert_eq!(v.metrics.rows_updated, 0);
    }
}

#[test]
fn schema_and_index_changes_reuse_cached_bytes_but_rederive_required_parts() {
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
    assert_eq!(v.metrics.source_reads, 0);
    assert_eq!(v.metrics.validations, 0);
    assert_eq!(v.metrics.edges_removed, 2);
    f.edit(".editor/books/ui.json", |v| {
        v["list"]["sortable"]
            .as_array_mut()
            .unwrap()
            .push(json!("type"))
    });
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(v.metrics.source_reads, 0);
    assert_eq!(v.metrics.validations, 0);
    assert_eq!(v.metrics.index_rows_written, 7);
    assert!(v.metrics.index_builds > 0);
}

#[test]
fn a_complete_relationship_cache_can_build_view_indexes_without_source_reads() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.references_to("people", "p1", true, false).unwrap();
    let v = r.open_view("books", &all()).unwrap();
    assert_eq!(v.metrics.source_reads, 0);
    assert_eq!(v.metrics.hashes, 0);
    assert_eq!(v.metrics.index_rows_written, 7);
    assert!(v.metrics.index_builds > 0);
    assert_eq!(
        v.rows,
        f.repo(CacheMode::Disabled)
            .open_view("books", &all())
            .unwrap()
            .rows
    );
}

#[test]
fn failed_changes_do_not_publish_a_new_manifest() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    let before = state(&r);
    f.edit("data/books/a.json", |v| v["score"] = json!(42));
    let db = rusqlite::Connection::open(r.cache_directory().unwrap().join("cache.sqlite")).unwrap();
    db.execute_batch(&record_sql("books", "CREATE TRIGGER fail_update BEFORE UPDATE ON records WHEN NEW.id='a' BEGIN SELECT RAISE(ABORT,'test'); END")).unwrap();
    assert!(r.refresh("books").is_err());
    assert_eq!(state(&r), before);
    assert!(
        f.repo(CacheMode::Sqlite)
            .open_view("books", &all())
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn unix_ctime_detects_equal_size_edits_with_restored_mtime_in_metadata_mode() {
    let f = Fixture::new();
    f.repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    let p = f.root.join("data/books/a.json");
    let modified = fs::metadata(&p).unwrap().modified().unwrap();
    let text = fs::read_to_string(&p).unwrap();
    fs::write(&p, text.replace("Beta", "Zeta")).unwrap();
    fs::File::options()
        .write(true)
        .open(&p)
        .unwrap()
        .set_times(FileTimes::new().set_modified(modified))
        .unwrap();
    let v = f
        .repo(CacheMode::Sqlite)
        .open_view("books", &all())
        .unwrap();
    assert_eq!(v.metrics.source_reads, 1);
    assert!(v.rows.iter().any(|r| r.label == "Zeta"));
}
