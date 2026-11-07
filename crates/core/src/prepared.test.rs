use super::*;
use std::sync::{Arc, Barrier, Mutex};

fn empty_query() -> PreparedQuery {
    PreparedQuery {
        collections: Vec::new(),
    }
}

fn insert(cache: &mut PreparedCache, key: &[u8], query: PreparedQuery) {
    let bytes = PreparedCache::entry_bytes(key, &query);
    cache.insert(key.to_vec(), query, bytes);
}

fn assert_cache_accounting(cache: &PreparedCache) {
    assert_eq!(cache.order.len(), cache.entries.len());
    assert_eq!(
        cache.bytes,
        cache
            .entries
            .values()
            .map(|entry| entry.bytes)
            .sum::<usize>()
    );
    for key in &cache.order {
        assert!(cache.entries.contains_key(key));
    }
}

#[test]
fn bounded_lru_evicts_without_exceeding_budget() {
    let mut cache = PreparedCache::new(280);
    let query = empty_query();
    insert(&mut cache, b"a", query.clone());
    assert!(cache.get(b"a").is_some());
    insert(&mut cache, b"b", query.clone());
    assert!(cache.used_bytes() <= 280);
    assert!(cache.get(b"a").is_some());
    insert(&mut cache, b"c", query);
    assert!(cache.get(b"b").is_none());
    assert!(cache.get(b"a").is_some());
    assert!(cache.get(b"c").is_some());
}

#[test]
fn repeated_insert_keeps_one_entry_and_one_charge() {
    let mut cache = PreparedCache::new(1024);
    insert(&mut cache, b"same", empty_query());
    let first_charge = cache.used_bytes();

    insert(&mut cache, b"same", empty_query());

    assert_eq!(cache.entries.len(), 1);
    assert_eq!(cache.used_bytes(), first_charge);
    assert_eq!(cache.order, VecDeque::from([b"same".to_vec()]));
    assert_cache_accounting(&cache);
}

#[test]
fn replacement_updates_charge_when_preparation_size_changes() {
    let mut cache = PreparedCache::new(4096);
    insert(&mut cache, b"same", empty_query());
    let first_charge = cache.used_bytes();
    insert(
        &mut cache,
        b"same",
        PreparedQuery {
            collections: vec![(0, None, None), (1, None, None)],
        },
    );

    assert!(cache.used_bytes() > first_charge);
    assert_cache_accounting(&cache);

    insert(&mut cache, b"same", empty_query());
    assert_eq!(cache.used_bytes(), first_charge);
    assert_cache_accounting(&cache);
}

#[test]
fn replacement_does_not_evict_a_different_recent_entry() {
    let mut probe = PreparedCache::new(1024);
    insert(&mut probe, b"a", empty_query());
    let one_charge = probe.used_bytes();
    let mut cache = PreparedCache::new(one_charge * 3);

    insert(&mut cache, b"a", empty_query());
    insert(&mut cache, b"b", empty_query());
    insert(&mut cache, b"a", empty_query());
    insert(&mut cache, b"c", empty_query());

    assert!(cache.get(b"a").is_some());
    assert!(cache.get(b"b").is_some());
    assert!(cache.get(b"c").is_some());
    assert_cache_accounting(&cache);
}

#[test]
fn simultaneous_misses_do_not_double_charge_one_key() {
    let cache = Arc::new(Mutex::new(PreparedCache::new(1024)));
    let both_missed = Arc::new(Barrier::new(2));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let both_missed = Arc::clone(&both_missed);
            std::thread::spawn(move || {
                assert!(cache.lock().unwrap().get(b"same").is_none());
                both_missed.wait();
                insert(&mut cache.lock().unwrap(), b"same", empty_query());
            })
        })
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }

    let cache = cache.lock().unwrap();
    assert_eq!(cache.entries.len(), 1);
    assert_cache_accounting(&cache);
}
