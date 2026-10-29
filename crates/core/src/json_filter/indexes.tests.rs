use super::*;

#[test]
fn transaction_local_row_keys_verify_bytes_and_are_cleared_by_writes() {
    let temp = tempfile::tempdir().unwrap();
    let repo = crate::Repository::load_repository(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/minimal"),
        crate::Options::new(temp.path()),
    )
    .unwrap();
    let c = repo.model().collection("books").unwrap();
    let names = [crate::store::covering_view_index(c)];
    let db = Connection::open_in_memory().unwrap();
    let cache = register(&db).unwrap();
    let spec = serde_json::to_string(&field("score")).unwrap();
    let read = |raw: &str| {
        db.query_row(
            "SELECT objekte_json_key(?1,'same-id',?2)",
            rusqlite::params![raw, spec],
            |r| r.get::<_, String>(0),
        )
        .unwrap()
    };
    for maintaining in [false, true] {
        let guard = cache.build(c, &names, maintaining).unwrap();
        let first = r#"{"score":12}"#;
        assert_eq!(
            cache
                .prime(first.as_bytes(), Some(&crate::json::parse(first).unwrap()))
                .unwrap(),
            maintaining
        );
        if !maintaining {
            cache.freeze().unwrap();
        }
        // The row is read from actual bytes, not a saved source digest.
        assert_eq!(read(r#"{"score":13}"#), "N13");
        assert_eq!(read(r#"{"score":13}"#), "N13");
        let replacement = r#"{"score":"14"}"#;
        cache
            .prime(
                replacement.as_bytes(),
                Some(&crate::json::parse(replacement).unwrap()),
            )
            .unwrap();
        assert!(
            cache
                .building
                .lock()
                .unwrap()
                .as_ref()
                .unwrap()
                .rows
                .is_none()
        );
        assert_eq!(read(replacement), "S14");
        if maintaining {
            let building = cache.building.lock().unwrap();
            let building = building.as_ref().unwrap();
            assert_eq!(building.used, 0);
            assert!(building.entries.is_empty());
        }
        drop(guard);
        assert_eq!(read(r#"{"score":15}"#), "N15");
    }
}

#[test]
fn build_cache_fallback_and_guard_cleanup_preserve_results() {
    let temp = tempfile::tempdir().unwrap();
    let repo = crate::Repository::load_repository(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/fixtures/minimal"),
        crate::Options::new(temp.path()),
    )
    .unwrap();
    let c = repo.model().collection("books").unwrap();
    let db = Connection::open_in_memory().unwrap();
    let cache = register(&db).unwrap();
    for budget in [0, 1024 * 1024] {
        let guard = cache
            .build(c, &[crate::store::covering_view_index(c)], false)
            .unwrap();
        cache.building.lock().unwrap().as_mut().unwrap().budget = budget;
        let raw = r#"{"score":42,"title":"prime"}"#;
        let value = crate::json::parse(raw).unwrap();
        assert!(!cache.prime(raw.as_bytes(), Some(&value)).unwrap());
        let used = cache.building.lock().unwrap().as_ref().unwrap().used;
        assert!(!cache.prime(raw.as_bytes(), Some(&value)).unwrap());
        assert_eq!(cache.building.lock().unwrap().as_ref().unwrap().used, used);
        for (raw, expected) in [
            (r#"{"score":1,"score":1e500,"title":"alpha"}"#, "N1e+500"),
            (r#"{"score":true,"title":"beta"}"#, "B1"),
            (r#"{"score":[1,2]}"#, "M"),
            (r#"{"score":1,"ignored":[1,]}"#, "E"),
            (r#"{"score":1,"score":1e500,"title":"alpha"}"#, "N1e+500"),
        ] {
            let actual: String = db
                .query_row(
                    "SELECT objekte_json_key(?1,?2)",
                    rusqlite::params![
                        raw.as_bytes(),
                        serde_json::to_string(&field("score")).unwrap()
                    ],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(actual, expected);
        }
        {
            let state = cache.building.lock().unwrap();
            let state = state.as_ref().unwrap();
            assert!(state.used <= budget);
            assert_eq!(state.entries.is_empty(), budget == 0);
        }
        drop(guard);
        assert!(cache.building.lock().unwrap().is_none());
    }
    // SQL failures must release the temporary cache on the same unwind path.
    let failure = {
        let _guard = cache
            .build(c, &[crate::store::covering_view_index(c)], false)
            .unwrap();
        db.execute_batch("SELECT nonexistent_function()")
    };
    assert!(failure.is_err());
    assert!(cache.building.lock().unwrap().is_none());
}

#[test]
fn primed_documents_keep_all_fields_and_never_leak_across_bytes() {
    let db = Connection::open_in_memory().unwrap();
    let cache = register(&db).unwrap();
    let raw = br#"{"score":1,"score":9007199254740993,"title":"ALPHA"}"#;
    let parsed = crate::json::parse(std::str::from_utf8(raw).unwrap()).unwrap();
    cache.prime(raw, Some(&parsed)).unwrap();
    for (bytes, name, expected) in [
        (&raw[..], "score", "N9007199254740993"),
        (&raw[..], "title", "SALPHA"),
        (&b"{broken"[..], "score", "E"),
        (&raw[..], "score", "N9007199254740993"),
        (
            &br#"{"score":"9007199254740993"}"#[..],
            "score",
            "S9007199254740993",
        ),
    ] {
        let actual: String = db
            .query_row(
                "SELECT objekte_json_key(?1,?2)",
                rusqlite::params![bytes, serde_json::to_string(&field(name)).unwrap()],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(actual, expected);
    }
}
