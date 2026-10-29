use super::*;

#[test]
fn stale_results_cannot_overwrite_edits_schema_changes_deletes_or_recreations() {
    let db = rusqlite::Connection::open_in_memory().unwrap();
    let table = crate::store::record_table();
    db.execute_batch(&format!("CREATE TABLE {table}(collection TEXT,id TEXT,revision INTEGER,digest TEXT,validation_schema_hash TEXT,validation_state TEXT,issues TEXT);
            INSERT INTO {table} VALUES('books','a',2,'new','schema2','pending','[]');")).unwrap();
    let result = RecordState {
        status: RecordStatus::Valid,
        issues: vec![],
    };
    for (revision, digest, schema) in [
        (1, "new", "schema2"),
        (2, "old", "schema2"),
        (2, "new", "schema1"),
    ] {
        let job = Job {
            id: "a".into(),
            revision,
            digest: digest.into(),
            bytes: vec![],
        };
        assert_eq!(apply(&db, "books", &job, schema, &result).unwrap(), 0);
    }
    let job = Job {
        id: "a".into(),
        revision: 2,
        digest: "new".into(),
        bytes: vec![],
    };
    assert_eq!(apply(&db, "books", &job, "schema2", &result).unwrap(), 1);
    assert_eq!(apply(&db, "books", &job, "schema2", &result).unwrap(), 0);
    db.execute(&format!("DELETE FROM {table}"), []).unwrap();
    assert_eq!(apply(&db, "books", &job, "schema2", &result).unwrap(), 0);
    db.execute_batch(
            &format!("INSERT INTO {table} VALUES('books','a',1,'recreated','schema2','pending','[]')"),
        )
        .unwrap();
    assert_eq!(apply(&db, "books", &job, "schema2", &result).unwrap(), 0);
    assert_eq!(
        db.query_row("SELECT validation_state FROM records", [], |r| {
            r.get::<_, String>(0)
        })
        .unwrap(),
        "pending"
    );
}
