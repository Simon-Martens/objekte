use super::*;
#[test]
fn bounded_entries_include_complete_empty_results() {
    let mut cache = VerifiedCache::new(300);
    cache.insert("empty".into(), Vec::new());
    assert_eq!(cache.get("empty").unwrap().members.len(), 0);
    cache.insert("other".into(), Vec::new());
    cache.insert("third".into(), Vec::new());
    assert!(cache.used_bytes() <= 300);
    assert!(cache.get("empty").is_none());
    cache.set_budget(0);
    assert!(!cache.enabled());
    assert!(cache.get("other").is_none());
}
