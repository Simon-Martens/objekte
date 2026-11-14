use super::*;

#[test]
fn gram_map_backings_agree() {
    let mut building = GramMap::default();
    for (key, slot) in [(7u64, 1u32), (7, 70_000), (3, 5), (9, 300)] {
        building.insert_slot(key, slot);
    }
    let mut shared = building.clone();
    shared.freeze();
    assert!(shared.is_shared() && !building.is_shared());
    for map in [&mut building, &mut shared] {
        let source: RoaringBitmap = [0u32, 9].into_iter().collect();
        map.append_shifted(7, &source, 200_000).unwrap();
        map.append_shifted(11, &source, 0).unwrap();
        map.replace(3, RoaringBitmap::new());
        assert!(map.append_shifted(7, &source, 5).is_err());
    }
    assert_eq!(building.to_btree_map(), shared.to_btree_map());
    assert_eq!(building.len(), shared.len());
    assert_eq!(building.cardinality(7), 4);
    assert_eq!(shared.cardinality(7), 4);
    assert_eq!(
        building.get(7).map(Cow::into_owned),
        shared.get(7).map(Cow::into_owned)
    );
    let mut seen = (Vec::new(), Vec::new());
    building
        .for_each_sorted(|key, _| {
            seen.0.push(*key);
            Ok(())
        })
        .unwrap();
    shared
        .for_each_sorted(|key, _| {
            seen.1.push(*key);
            Ok(())
        })
        .unwrap();
    assert_eq!(seen.0, seen.1);
    assert_eq!(seen.0, vec![3, 7, 9, 11]);
    assert!(shared.all_bitmaps(|posting| posting.len() <= 4));
    assert!(building.serialized_bytes() > 0 && shared.serialized_bytes() > 0);
}
