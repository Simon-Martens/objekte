use super::*;

#[test]
fn bounded_lru_evicts_without_exceeding_budget() {
    let mut cache = PreparedCache::new(280);
    let query = PreparedQuery {
        collections: Vec::new(),
    };
    cache.insert("a".into(), query.clone());
    assert!(cache.get("a").is_some());
    cache.insert("b".into(), query.clone());
    assert!(cache.used_bytes() <= 280);
    assert!(cache.get("a").is_some());
    cache.insert("c".into(), query);
    assert!(cache.get("b").is_none());
    assert!(cache.get("a").is_some());
    assert!(cache.get("c").is_some());
}
