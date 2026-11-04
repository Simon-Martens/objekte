use super::*;

#[test]
fn frontend_scope_invalidation_matches_only_complete_collection_names() {
    assert!(key_references_collection("books", "books"));
    assert!(key_references_collection(
        "@frontend:books\0archive",
        "archive"
    ));
    assert!(!key_references_collection(
        "@frontend:books\0archive",
        "book"
    ));
    assert!(!key_references_collection(
        "@frontend:books\0archive",
        "other"
    ));
}

fn key(expression: &[u8]) -> PredicateCacheKey {
    PredicateCacheKey {
        collection: "books".into(),
        expression: expression.to_vec(),
    }
}

#[test]
fn generation_invalidates_both_membership_and_order() {
    let mut cache = VerifiedPredicateCache::new(4096);
    let predicate = key(b"year=1820");
    cache.record_bits(predicate.clone(), vec![0b11], vec![0b01], true);
    let order = OrderedResultKey {
        predicate: predicate.clone(),
        sort: Vec::new(),
    };
    cache.insert_ordered(order.clone(), vec![0]);
    assert!(cache.get(&predicate).unwrap().complete);
    assert_eq!(
        cache.ordered(&order).map(|(slots, _)| slots),
        Some(&[0][..])
    );
    cache.invalidate("books");
    assert!(cache.get(&predicate).is_none());
    assert!(cache.ordered(&order).is_none());
}

#[test]
fn budget_evicts_entries_and_matched_is_a_subset_of_evaluated() {
    let mut cache = VerifiedPredicateCache::new(400);
    cache.record_bits(key(b"first"), vec![0b101], vec![0b001], false);
    let first = cache.get(&key(b"first")).unwrap();
    assert_eq!(first.known(0), Some(true));
    assert_eq!(first.known(1), None);
    assert_eq!(first.known(2), Some(false));
    cache.record_bits(key(b"second"), vec![0b10], vec![0b10], true);
    assert!(cache.bytes() <= 400);
    assert!(cache.evictions() > 0);
}

#[test]
fn identical_expressions_in_different_collections_never_share_slots() {
    let mut cache = VerifiedPredicateCache::new(4096);
    let books = key(b"title=Goethe");
    let other = PredicateCacheKey {
        collection: "other".into(),
        expression: books.expression.clone(),
    };
    cache.record_bits(books.clone(), vec![0b1], vec![0b1], true);
    assert!(cache.get(&other).is_none());
    cache.record_bits(other.clone(), vec![0b1], vec![0], true);
    assert_eq!(cache.get(&books).unwrap().known(0), Some(true));
    assert_eq!(cache.get(&other).unwrap().known(0), Some(false));
}
