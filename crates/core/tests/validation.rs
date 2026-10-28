mod support;
use objekte_core::{CacheMode, Direction, Options, RecordStatus, Repository, Sort, ValidationMode};
use rusqlite::Connection;
use serde_json::{Value, json};
use std::fs;
use support::*;
fn db(r: &Repository) -> Connection {
    Connection::open(r.cache_directory().unwrap().join("cache.sqlite")).unwrap()
}
fn background(f: &Fixture) -> Repository {
    let mut o = f.options(CacheMode::Sqlite);
    o.validation = ValidationMode::Background;
    o.background_workers = 2;
    Repository::load_repository(&f.root, o).unwrap()
}
#[test]
fn defaults_follow_cpu_divisors_and_keep_64_mib() {
    let o = Options::new("unused");
    let cpus = std::thread::available_parallelism().map_or(1, |n| n.get());
    assert_eq!(o.ingest_workers, (cpus / 2).max(1));
    assert_eq!(o.background_workers, (cpus / 4).max(1));
    assert_eq!(o.sqlite_cache_mib, Some(64));
    assert_eq!(o.validation, ValidationMode::Background);
}
#[test]
fn background_is_lazy_publishes_after_commit_and_does_not_repeat_work() {
    let f = Fixture::new();
    let mut r = background(&f);
    assert!(!f.data.exists());
    assert_eq!(r.wait_for_validation().unwrap().applied, 0);
    let initial = r.open_view("books", &all()).unwrap();
    assert!(
        initial
            .states
            .values()
            .all(|s| s.status == RecordStatus::Pending)
    );
    assert_eq!(initial.metrics.validations, 0);
    assert_eq!(r.wait_for_validation().unwrap().applied, 7);
    assert_eq!(r.poll_validation_updates().unwrap(), vec!["books"]);
    assert!(r.poll_validation_updates().unwrap().is_empty());
    let ready = r.open_view("books", &all()).unwrap();
    assert!(
        ready
            .states
            .values()
            .all(|s| s.status == RecordStatus::Valid)
    );
    assert_eq!(ready.metrics.source_reads, 0);
    assert_eq!(r.wait_for_validation().unwrap().applied, 7);
    assert!(r.poll_validation_updates().unwrap().is_empty());
}
#[test]
fn pending_work_resumes_from_cache_without_source_reads() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    drop(r);
    let mut r = background(&f);
    assert_eq!(
        r.open_view("books", &all()).unwrap().metrics.source_reads,
        0
    );
    assert_eq!(r.wait_for_validation().unwrap().validated, 7);
    assert!(
        r.open_view("books", &all())
            .unwrap()
            .states
            .values()
            .all(|s| s.status == RecordStatus::Valid)
    );
}
#[test]
fn malformed_utf8_and_nonobject_json_remain_visible_and_exact() {
    let f = Fixture::new();
    let bad = b"{\"title\":\"\xff\"}";
    fs::write(f.root.join("data/books/a.json"), bad).unwrap();
    fs::write(f.root.join("data/books/b.json"), b"{broken").unwrap();
    f.write("data/books/c.json", &json!([1, 2]));
    f.write("data/books/d.json", &Value::Null);
    let mut r = f.repo(CacheMode::Sqlite);
    let page = r.open_view("books", &all()).unwrap();
    assert_eq!(page.total, Some(7));
    assert_eq!(page.states["a"].status, RecordStatus::IncorrectJson);
    assert_eq!(page.states["b"].status, RecordStatus::IncorrectJson);
    assert_eq!(page.rows.iter().find(|r| r.id == "a").unwrap().label, "a");
    assert!(
        page.rows
            .iter()
            .find(|r| r.id == "a")
            .unwrap()
            .values
            .is_empty()
    );
    let exact: Vec<u8> = db(&r)
        .query_row(
            "SELECT json FROM records WHERE collection='books' AND id='a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(exact, bad);
    assert_eq!(
        r.get_record("books", "a").unwrap().unwrap().raw.unwrap(),
        bad
    );
    assert_eq!(r.validate_collection("books").unwrap().applied, 5);
    let page = r.open_view("books", &all()).unwrap();
    for id in ["c", "d"] {
        assert_eq!(page.states[id].status, RecordStatus::Invalid);
    }
    assert_eq!(
        page.rows,
        f.repo(CacheMode::Disabled)
            .open_view("books", &all())
            .unwrap()
            .rows
    );
    f.write("data/books/a.json", &json!({"payload":null,"score":4}));
    r.refresh("books").unwrap();
    assert_eq!(
        r.open_view("books", &all()).unwrap().states["a"].status,
        RecordStatus::Pending
    );
    assert_eq!(r.validate_collection("books").unwrap().applied, 6); // cumulative worker report
    assert_eq!(
        r.get_record("books", "a").unwrap().unwrap().state.status,
        RecordStatus::Valid
    );
}
#[test]
fn all_diagnostics_and_valid_reference_occurrences_are_retained() {
    let f = Fixture::new();
    f.write(
        "data/books/a.json",
        &json!({"score":"bad","title":3,"flag":[],"count":1.5,
        "person":"../escape", "links":[{"person":"p2"},{"person":false}], "extra":true}),
    );
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    r.validate_collection("books").unwrap();
    let record = r.get_record("books", "a").unwrap().unwrap();
    assert_eq!(record.state.status, RecordStatus::Invalid);
    for pointer in [
        "/score",
        "/title",
        "/flag",
        "/count",
        "/person",
        "/links/1/person",
        "",
    ] {
        assert!(
            record.state.issues.iter().any(|i| i.location == pointer),
            "missing {pointer}: {:?}",
            record.state.issues
        );
    }
    assert_eq!(record.data["score"], json!("bad"));
    let refs = r.references_from("books", "a", false).unwrap();
    assert_eq!(refs.edges.len(), 1);
    assert_eq!(refs.edges[0].location, "/links/0/person");
    fs::write(f.root.join("data/books/a.json"), "broken").unwrap();
    r.refresh("books").unwrap();
    assert!(
        r.references_from("books", "a", false)
            .unwrap()
            .edges
            .is_empty()
    );
}
#[test]
fn wrong_types_sort_after_missing_and_never_match_typed_filters() {
    let f = Fixture::new();
    for (id, value) in [
        ("a", json!("0")),
        ("b", json!([])),
        ("c", json!({})),
        ("d", json!(true)),
    ] {
        f.edit(&format!("data/books/{id}.json"), |v| v["score"] = value);
    }
    let mut cached = f.repo(CacheMode::Sqlite);
    let mut direct = f.repo(CacheMode::Disabled);
    for direction in [Direction::Asc, Direction::Desc] {
        let mut request = all();
        request.sort = Some(Sort {
            field: "score".into(),
            direction,
        });
        let view = cached.open_view("books", &request).unwrap();
        assert_eq!(view.rows, direct.open_view("books", &request).unwrap().rows);
        assert_eq!(&ids(&view.rows)[3..], &["a", "b", "c", "d"]);
        for expression in ["score=0", "score>0", "score=null"] {
            request.filters = vec![
                cached
                    .model()
                    .collection("books")
                    .unwrap()
                    .parse_filter(expression)
                    .unwrap(),
            ];
            let view = cached.open_view("books", &request).unwrap();
            assert_eq!(view.rows, direct.open_view("books", &request).unwrap().rows);
            assert!(
                !view
                    .rows
                    .iter()
                    .any(|r| ["a", "b", "c", "d"].contains(&r.id.as_str()))
            );
        }
    }
}
#[test]
fn schema_changes_reset_diagnostics_without_source_reads_and_ui_changes_do_not() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    r.validate_collection("books").unwrap();
    f.edit(".editor/books/schema.json", |v| {
        v["required"] = json!(["payload", "title"])
    });
    let view = r.open_view("books", &all()).unwrap();
    assert_eq!(view.metrics.source_reads, 0);
    assert!(
        view.states
            .values()
            .all(|s| s.status == RecordStatus::Pending && s.issues.is_empty())
    );
    assert_eq!(r.validate_collection("books").unwrap().applied, 14);
    let ready = r.open_view("books", &all()).unwrap();
    assert!(
        ready
            .states
            .values()
            .any(|s| s.status == RecordStatus::Invalid)
    );
    f.edit(".editor/books/ui.json", |v| v["label"] = json!("renamed"));
    assert_eq!(r.open_view("books", &all()).unwrap().states, ready.states);
    assert_eq!(r.validate_collection("books").unwrap().applied, 14);
}
#[test]
fn validation_write_failure_is_reported_and_pending_work_survives() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    db(&r).execute_batch("CREATE TRIGGER validation_failure BEFORE UPDATE OF validation_state ON record_validation WHEN NEW.id='b' BEGIN SELECT RAISE(ABORT,'validation test'); END").unwrap();
    assert!(
        r.validate_collection("books")
            .unwrap_err()
            .to_string()
            .contains("validation test")
    );
    assert!(r.poll_validation_updates().is_err());
    assert_eq!(
        db(&r)
            .query_row(
                "SELECT count(*) FROM record_validation WHERE validation_state='pending'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        7
    );
    db(&r)
        .execute_batch("DROP TRIGGER validation_failure")
        .unwrap();
    drop(r);
    let mut r = background(&f);
    r.open_view("books", &all()).unwrap();
    assert_eq!(r.wait_for_validation().unwrap().applied, 7);
}
#[test]
fn shared_writer_supports_concurrent_repository_handles() {
    let f = Fixture::new();
    std::thread::scope(|s| {
        let handles = (0..4)
            .map(|i| {
                s.spawn({
                    let f = &f;
                    move || {
                        let mut r = background(f);
                        let collection = if i % 2 == 0 { "books" } else { "people" };
                        for _ in 0..3 {
                            r.open_view(collection, &all()).unwrap();
                            r.wait_for_validation().unwrap();
                        }
                        assert!(
                            r.open_view(collection, &all())
                                .unwrap()
                                .states
                                .values()
                                .all(|s| s.status == RecordStatus::Valid)
                        );
                    }
                })
            })
            .collect::<Vec<_>>();
        for h in handles {
            h.join().unwrap();
        }
    });
}

#[test]
fn parallel_parsing_and_validation_handle_the_supported_nesting_limit() {
    // The foreground projection also uses Sonic. Match its upstream deep-input
    // test stack; ingestion and validation still run on their own library workers.
    std::thread::Builder::new()
        .stack_size(16 * 1024 * 1024)
        .spawn(|| {
            let f = Fixture::new();
            // Opaque payload; both the ingestion pool and validation pool decode it.
            let text = format!("{{\"payload\":{}0{}}}", "[".repeat(120), "]".repeat(120));
            fs::write(f.root.join("data/books/a.json"), text).unwrap();
            let mut r = background(&f);
            r.open_view("books", &all()).unwrap();
            assert_eq!(r.wait_for_validation().unwrap().applied, 7);
            assert_eq!(
                r.open_view("books", &all()).unwrap().states["a"].status,
                RecordStatus::Valid
            );
        })
        .unwrap()
        .join()
        .unwrap();
}

#[test]
fn deletion_and_recreation_replace_both_payload_and_validation_identity() {
    let f = Fixture::new();
    let mut r = f.repo(CacheMode::Sqlite);
    r.open_view("books", &all()).unwrap();
    r.validate_collection("books").unwrap();
    fs::remove_file(f.root.join("data/books/a.json")).unwrap();
    r.refresh("books").unwrap();
    assert_eq!(
        db(&r)
            .query_row(
                "SELECT count(*) FROM record_validation WHERE collection='books' AND id='a'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    fs::write(
        f.root.join("data/books/a.json"),
        r#"{"payload":null,"score":1e500}"#,
    )
    .unwrap();
    assert_eq!(
        r.get_record("books", "a").unwrap().unwrap().state.status,
        RecordStatus::Pending
    );
    r.validate_collection("books").unwrap();
    let record = r.get_record("books", "a").unwrap().unwrap();
    assert_eq!(record.state.status, RecordStatus::Invalid);
    assert_eq!(record.data["score"].to_string(), "1e+500");
    assert!(
        record
            .state
            .issues
            .iter()
            .any(|i| i.location == "/score" && i.code == "numeric_domain")
    );
}
