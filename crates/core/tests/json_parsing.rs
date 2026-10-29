mod support;
use objekte_core::CacheMode;
use rusqlite::Connection;
use std::fs;
use support::*;

#[test]
fn sonic_records_keep_duplicate_field_semantics_for_views_indexes_and_relations() {
    let f = Fixture::new();
    fs::write(f.root.join("data/books/a.json"),
        r#"{"title":"first","t\u0069tle":"last","score":-100,"score":100,"person":"p1","person":"p2","payload":{"large":18446744073709551616,"exponent":1e500,"same":1,"same":2}}"#).unwrap();
    let mut cached = f.repo(CacheMode::Sqlite);
    let mut direct = f.repo(CacheMode::Disabled);
    let expected = direct.open_view("books", &all()).unwrap();
    let initial = cached.open_view("books", &all()).unwrap();
    assert_eq!(initial.rows, expected.rows);
    let row = initial.rows.iter().find(|r| r.id == "a").unwrap();
    assert_eq!(row.label, "last");
    assert_eq!(row.values["score"], 100);
    for repo in [&mut direct, &mut cached] {
        let record = repo.get_record("books", "a").unwrap().unwrap();
        assert_eq!(
            record.data["payload"]["large"].to_string(),
            "18446744073709551616"
        );
        assert_eq!(record.data["payload"]["exponent"].to_string(), "1e+500");
        assert_eq!(record.data["payload"]["same"], 2);
        let references = repo.references_from("books", "a", false).unwrap();
        assert_eq!(references.edges.len(), 1);
        assert_eq!(references.edges[0].target_id, "p2");
        let mut request = all();
        request.filters.push(
            repo.model()
                .collection("books")
                .unwrap()
                .parse_filter("score=100")
                .unwrap(),
        );
        let view = repo.open_view("books", &request).unwrap();
        assert!(view.rows.iter().any(|row| row.id == "a"));
    }
    assert_eq!(
        cached.open_view("books", &all()).unwrap().rows,
        expected.rows
    );
}

#[test]
fn malformed_cached_payloads_fail_even_when_the_field_is_not_projected() {
    let f = Fixture::new();
    let mut repo = f.repo(CacheMode::Sqlite);
    repo.open_view("books", &all()).unwrap();
    let db = Connection::open(repo.cache_directory().unwrap().join("cache.sqlite")).unwrap();
    objekte_core::register_cache_sqlite_functions(&db).unwrap();
    for invalid in [r#"[1,]"#, r#"{"nested":[1,]}"#, r#"{"nested":"\x"}"#] {
        let text = format!(r#"{{"title":"valid","score":2,"payload":{invalid}}}"#);
        db.execute(
            &record_sql(
                "books",
                "UPDATE records SET json=?1 WHERE collection='books' AND id='a'",
            ),
            [text],
        )
        .unwrap();
        let error = repo.open_view("books", &all()).unwrap_err();
        assert!(error.to_string().contains("invalid cached JSON"), "{error}");
    }
}
