use super::*;

#[test]
fn background_writer_does_not_recreate_a_deleted_cache() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let writer = Writer::open(&cache, temp.path(), Some(64), false).unwrap();
    std::fs::remove_dir_all(&cache).unwrap();
    assert!(writer.enter_existing(Some(64), false).is_err());
    assert!(!cache.exists());
    assert!(writer.enter(Some(64), false).is_ok());
    assert!(cache.exists());
}

#[test]
fn page_cache_budget_is_connection_local_and_keeps_wal_and_durability() {
    let temp = tempfile::tempdir().unwrap();
    let cache = temp.path().join("cache");
    let default: i64 = Connection::open_in_memory()
        .unwrap()
        .query_row("PRAGMA cache_size", [], |r| r.get(0))
        .unwrap();
    for budget in [None, Some(1), Some(16), Some(64), None] {
        let store = Store::open(&cache, temp.path(), budget, false).unwrap();
        assert_eq!(
            store
                .db
                .query_row("PRAGMA cache_size", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            budget.map_or(default, |mib| -i64::from(mib) * 1024)
        );
        assert_eq!(
            store
                .db
                .query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "wal"
        );
        assert_eq!(
            store
                .db
                .query_row("PRAGMA synchronous", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
    }
}
