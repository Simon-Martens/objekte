use super::*;


/// Small deterministic generator; the crate has no test dependencies.
struct Lcg(u64);


impl Lcg {
    fn next(&mut self, bound: u32) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % bound
    }
}


/// Slots cover three 65,536-slot ranges so both chunk sizes cross boundaries.
const SPAN: u32 = 3 << 16;
const KEYS: u32 = 12;

fn assert_same(model: &BTreeMap<u16, RoaringBitmap>, map: &PostingMap<u16>, random: &mut Lcg) {
    assert_eq!(map.len(), model.len());
    assert_eq!(map.is_empty(), model.is_empty());
    assert_eq!(&map.to_btree_map(), model);
    assert_eq!(map.first_key(), model.keys().next());
    assert_eq!(map.last_key(), model.keys().next_back());
    for key in 0..KEYS as u16 {
        assert_eq!(map.get(&key).map(Cow::into_owned), model.get(&key).cloned());
        assert_eq!(
            map.cardinality(&key),
            model.get(&key).map_or(0, RoaringBitmap::len)
        );
        assert_eq!(map.contains_key(&key), model.contains_key(&key));
    }
    let a = random.next(KEYS) as u16;
    let b = random.next(KEYS) as u16;
    let (low, high) = (a.min(b), a.max(b));
    let mut ranges = vec![
        (Bound::Included(&low), Bound::Included(&high)),
        (Bound::Excluded(&low), Bound::Unbounded),
        (Bound::Unbounded, Bound::Excluded(&high)),
        (Bound::Unbounded, Bound::Unbounded),
    ];
    if low != high {
        ranges.push((Bound::Included(&low), Bound::Excluded(&high)));
        ranges.push((Bound::Excluded(&low), Bound::Included(&high)));
    }
    for (lower, upper) in ranges {
        for descending in [false, true] {
            let range = model.range::<u16, _>((lower, upper));
            let expected: Vec<(u16, RoaringBitmap)> = if descending {
                range
                    .rev()
                    .map(|(key, posting)| (*key, posting.clone()))
                    .collect()
            } else {
                range
                    .map(|(key, posting)| (*key, posting.clone()))
                    .collect()
            };
            let actual: Vec<(u16, RoaringBitmap)> = map
                .range(lower, upper, descending)
                .map(|(key, posting)| (*key, posting.into_owned()))
                .collect();
            assert_eq!(actual, expected);
        }
    }
}

fn model_append(target: &mut RoaringBitmap, source: &RoaringBitmap, base: u32) -> Result<(), ()> {
    if source
        .max()
        .is_some_and(|max| base.checked_add(max).is_none())
    {
        return Err(());
    }
    let mut trial = target.clone();
    trial
        .append(source.iter().map(|slot| base + slot))
        .map_err(|_| ())?;
    *target = trial;
    Ok(())
}

/// Every operation must behave exactly like the reference `BTreeMap`, before
/// and after `freeze`, and an earlier clone must never see a later write.
#[test]
fn posting_map_matches_btree_model_in_both_backings() {
    // The forced validation feature uses tiny chunks, which makes each
    // dense split much more work; fewer seeds keep the run short.
    let seeds: u64 = if cfg!(feature = "force-shared-backing") {
        30
    } else {
        200
    };
    for seed in 0..seeds {
        let mut random = Lcg(seed.wrapping_mul(0x9E37_79B9) + 1);
        let mut model: BTreeMap<u16, RoaringBitmap> = BTreeMap::new();
        let mut map: PostingMap<u16> = PostingMap::new();
        let freeze_at = random.next(60);
        let mut snapshots = Vec::new();
        for step in 0..120u32 {
            if step == freeze_at {
                map.freeze();
                assert!(map.is_shared());
            }
            let key = random.next(KEYS) as u16;
            match random.next(10) {
                0..=4 => {
                    let slot = random.next(SPAN);
                    assert_eq!(
                        map.insert_slot(key, slot),
                        model.entry(key).or_default().insert(slot)
                    );
                }
                5 | 6 => {
                    let mut source = RoaringBitmap::new();
                    for _ in 0..random.next(6) {
                        source.insert(random.next(SPAN));
                    }
                    let base = random.next(SPAN);
                    let expected = model_append(model.entry(key).or_default(), &source, base);
                    let actual = map.append_shifted(key, &source, base).map_err(|_| ());
                    assert_eq!(actual, expected, "seed {seed} step {step}");
                }
                7 => {
                    let mut posting = RoaringBitmap::new();
                    for _ in 0..random.next(5) {
                        posting.insert(random.next(SPAN));
                    }
                    if random.next(4) == 0 {
                        posting.insert_range(0..SPAN);
                    }
                    model.insert(key, posting.clone());
                    map.replace(key, posting);
                }
                8 => {
                    let cut = random.next(KEYS) as u16;
                    model.retain(|key, _| *key != cut);
                    map.retain_keys(|key| *key != cut);
                }
                _ => snapshots.push((model.clone(), map.clone())),
            }
            assert_same(&model, &map, &mut random);
        }
        for (old_model, old_map) in &snapshots {
            assert_eq!(&old_map.to_btree_map(), old_model);
        }
        let mut frozen = map.clone();
        frozen.freeze();
        assert!(frozen == map);
        assert_eq!(
            frozen.clone().into_entries(),
            model.clone().into_iter().collect::<Vec<_>>()
        );
        assert!(frozen.all_bitmaps(|posting| posting.max().is_none_or(|max| max < 2 * SPAN)));
        frozen.clear();
        assert!(frozen.is_empty() && frozen.is_shared());
    }
}

#[test]
fn append_shifted_errors_match_the_native_helper() {
    for frozen in [false, true] {
        let mut map: PostingMap<u16> = PostingMap::new();
        if frozen {
            map.freeze();
        }
        let source: RoaringBitmap = [0u32, 5].into_iter().collect();
        map.append_shifted(1, &source, 10).unwrap();
        assert_eq!(
            map.append_shifted(1, &source, 15).unwrap_err(),
            "batch bitmap slots overlap or are out of order"
        );
        assert_eq!(
            map.append_shifted(1, &source, u32::MAX - 1).unwrap_err(),
            "record slot overflow"
        );
        // An empty source still creates the key, like `entry().or_default()`.
        map.append_shifted(2, &RoaringBitmap::new(), 0).unwrap();
        assert!(map.contains_key(&2));
        assert_eq!(map.cardinality(&2), 0);
        assert_eq!(map.len(), 2);
        assert_eq!(
            map.get(&1).unwrap().iter().collect::<Vec<_>>(),
            vec![10, 15]
        );
    }
}

#[test]
fn inverted_range_is_empty_in_the_shared_backing() {
    let mut map: PostingMap<u16> = PostingMap::new();
    map.insert_slot(3, 1);
    map.freeze();
    assert_eq!(
        map.range(Bound::Included(&5), Bound::Included(&2), false)
            .count(),
        0
    );
    assert_eq!(
        map.range(Bound::Excluded(&3), Bound::Excluded(&3), true)
            .count(),
        0
    );
}

#[test]
fn shared_backing_policy_follows_size_and_feature() {
    assert!(shared_backing_wanted(SHARED_BACKING_MIN_RECORDS));
    assert_eq!(
        shared_backing_wanted(SHARED_BACKING_MIN_RECORDS - 1),
        cfg!(feature = "force-shared-backing")
    );
}
