use std::sync::atomic::Ordering;
use std::sync::{Arc, Barrier};

use super::*;

fn config(location: StoreLocation) -> StoreConfig {
    StoreConfig {
        location,
        configuration_digest: [3; 32],
        alphabet: "ascii".into(),
        source_root: b"/fixture".to_vec(),
        build_identity: [4; 32],
    }
}

#[test]
fn memory_mode_creates_no_database_files() {
    let store = RocksStore::create(config(StoreLocation::Memory)).expect("memory store");
    let host_directory = store
        .memory_host_directory()
        .expect("memory host directory")
        .to_path_buf();
    assert!(host_directory.is_dir());
    assert_eq!(
        std::fs::read_dir(&host_directory)
            .expect("host directory")
            .count(),
        0
    );

    let key = Namespace::Records.physical(&[1]);
    let transaction = store.transaction();
    transaction.put(&key, b"value").expect("put");
    transaction.commit().expect("commit");
    assert_eq!(
        store
            .view()
            .expect("view")
            .get(Namespace::Records, &[1])
            .unwrap(),
        Some(b"value".to_vec())
    );
    drop(store);
    assert!(!host_directory.exists());
}

#[test]
fn a_new_memory_environment_does_not_recover_old_data() {
    let first = RocksStore::create(config(StoreLocation::Memory)).expect("first");
    let transaction = first.transaction();
    transaction
        .put(Namespace::Records.physical(&[9]), b"old")
        .expect("put");
    transaction.commit().expect("commit");
    drop(first);

    let second = RocksStore::create(config(StoreLocation::Memory)).expect("second");
    assert_eq!(
        second
            .view()
            .unwrap()
            .get(Namespace::Records, &[9])
            .unwrap(),
        None
    );
}

#[test]
fn snapshots_keep_one_consistent_version() {
    let store = RocksStore::create(config(StoreLocation::Memory)).expect("store");
    let key = Namespace::Records.physical(&[2]);
    let transaction = store.transaction();
    transaction.put(&key, b"old").unwrap();
    transaction.commit().unwrap();
    let old = store.view().expect("old view");

    let transaction = store.transaction();
    transaction.put(&key, b"new").unwrap();
    transaction.commit().unwrap();
    let new = store.view().expect("new view");

    assert_eq!(
        old.get(Namespace::Records, &[2]).unwrap(),
        Some(b"old".to_vec())
    );
    assert_eq!(
        new.get(Namespace::Records, &[2]).unwrap(),
        Some(b"new".to_vec())
    );
    assert!(old.sequence_number() < new.sequence_number());
}

#[test]
fn concurrent_transactions_preserve_distinct_bits_in_one_value() {
    let store = Arc::new(RocksStore::create(config(StoreLocation::Memory)).expect("store"));
    let key = Namespace::StateBlocks.physical(&[7]);
    let transaction = store.transaction();
    transaction.put(&key, [0u8]).unwrap();
    transaction.commit().unwrap();
    let barrier = Arc::new(Barrier::new(3));

    let mut workers = Vec::new();
    for bit in [0u8, 1u8] {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let key = key.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            let transaction = store.transaction();
            let mut value = transaction
                .get_for_update(&key, true)
                .expect("lock")
                .expect("value")[0];
            value |= 1 << bit;
            transaction.put(&key, [value]).expect("put");
            transaction.commit().expect("commit");
        }));
    }
    barrier.wait();
    for worker in workers {
        worker.join().expect("worker");
    }
    let view = store.view().unwrap();
    assert_eq!(
        view.get(Namespace::StateBlocks, &[7]).unwrap(),
        Some(vec![3])
    );
}

#[test]
fn namespace_iterator_does_not_cross_logical_namespaces() {
    let store = RocksStore::create(config(StoreLocation::Memory)).expect("store");
    let transaction = store.transaction();
    for (namespace, suffix) in [
        (Namespace::Records, b"a".as_slice()),
        (Namespace::Records, b"b".as_slice()),
        (Namespace::LogicalIds, b"a".as_slice()),
    ] {
        transaction
            .put(namespace.physical(suffix), b"x")
            .expect("put");
    }
    transaction.commit().unwrap();
    let view = store.view().unwrap();
    let mut iterator = view.namespace_prefix(Namespace::Records, b"").unwrap();
    iterator.seek_to_first();
    let mut keys = Vec::new();
    while iterator.valid() {
        keys.push(iterator.key().unwrap().to_vec());
        iterator.next();
    }
    iterator.status().expect("iterator status");
    assert_eq!(
        keys,
        [
            Namespace::Records.physical(b"a"),
            Namespace::Records.physical(b"b")
        ]
    );
}

#[test]
fn unbounded_and_one_sided_iterator_queries_are_namespace_clamped() {
    let store = RocksStore::create(config(StoreLocation::Memory)).expect("store");
    let transaction = store.transaction();
    for (namespace, key) in [
        (Namespace::Records, b"a".as_slice()),
        (Namespace::Records, b"b".as_slice()),
        (Namespace::LogicalIds, b"a".as_slice()),
    ] {
        transaction.put(namespace.physical(key), b"x").unwrap();
    }
    transaction.commit().unwrap();
    let view = store.view().unwrap();
    for (lower, upper) in [
        (None, None),
        (None, Some(b"b".as_slice())),
        (Some(b"a".as_slice()), None),
    ] {
        let mut iterator = view.iterator(Namespace::Records, lower, upper).unwrap();
        iterator.seek_to_first();
        while iterator.valid() {
            assert_eq!(
                super::namespace::namespace_of(iterator.key().unwrap()),
                Some(Namespace::Records)
            );
            iterator.next();
        }
        iterator.status().unwrap();
    }
}

#[test]
fn file_mode_flush_reopen_preserves_complete_values() {
    let path = std::env::temp_dir().join(format!(
        "objekte-rocksdb-file-{}-{}",
        std::process::id(),
        super::NEXT_INSTANCE.load(Ordering::Relaxed)
    ));
    let store = RocksStore::create(config(StoreLocation::File(path.clone()))).expect("file store");
    let transaction = store.transaction();
    transaction
        .put(Namespace::Files.physical(&[5]), b"durable")
        .unwrap();
    transaction.commit().unwrap();
    store.flush_wal(true).unwrap();
    store.flush_memtable().unwrap();
    drop(store);

    let reopened = RocksStore::reopen_file(path.clone(), config(StoreLocation::File(path.clone())))
        .expect("reopen");
    assert_eq!(
        reopened
            .view()
            .unwrap()
            .get(Namespace::Files, &[5])
            .unwrap(),
        Some(b"durable".to_vec())
    );
    drop(reopened);
    std::fs::remove_dir_all(path).expect("cleanup");
}
