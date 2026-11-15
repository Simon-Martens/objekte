use super::*;
use crate::model::{RecordId, RecordSlot};

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

#[test]
fn slot_table_matches_vec_in_both_backings() {
    for seed in 0..40u64 {
        let mut random = Lcg(seed.wrapping_mul(0x9E37_79B9) + 1);
        let mut table = SlotTable::new();
        let mut model: Vec<u32> = Vec::new();
        let freeze_at = random.next(40);
        for step in 0..80u32 {
            if step == freeze_at {
                table.freeze();
                assert!(table.is_shared());
            }
            match random.next(6) {
                0..=2 => {
                    let value = random.next(1_000);
                    table.push(value);
                    model.push(value);
                }
                3 => {
                    let values = (0..random.next(4))
                        .map(|_| random.next(1_000))
                        .collect::<Vec<_>>();
                    table.extend(values.iter().copied());
                    model.extend(values);
                }
                4 if !model.is_empty() => {
                    let index = random.next(model.len() as u32) as usize;
                    let value = random.next(1_000);
                    table.set(index, value);
                    model[index] = value;
                }
                _ => {}
            }
            assert_eq!(table.len(), model.len());
            assert_eq!(table.is_empty(), model.is_empty());
            assert_eq!(table.to_vec(), model);
            assert_eq!(table.last(), model.last());
            for index in 0..model.len() + 2 {
                assert_eq!(table.get(index), model.get(index));
            }
            assert_eq!(
                table.iter().copied().collect::<Vec<_>>(),
                model.clone()
            );
            let start = random.next(model.len() as u32 + 1) as usize;
            assert_eq!(
                table.iter_from(start).copied().collect::<Vec<_>>(),
                model.iter().skip(start).copied().collect::<Vec<_>>()
            );
        }
    }

    for seed in 0..20u64 {
        let mut random = Lcg(seed + 500);
        let mut values = (0..64).map(|value| value * 3).collect::<Vec<_>>();
        values.sort_unstable();
        let mut table = SlotTable::from_vec(values.clone());
        if seed % 2 == 0 {
            table.freeze();
        }
        for _ in 0..40 {
            let threshold = random.next(200);
            assert_eq!(
                table.partition_point(|value| *value <= threshold),
                values.partition_point(|value| *value <= threshold)
            );
        }
        let snapshot = table.clone();
        table.push(999);
        assert_eq!(snapshot.to_vec(), values);
    }
}

#[test]
fn slot_order_matches_vec_in_both_backings() {
    for seed in 0..40u64 {
        let mut random = Lcg(seed.wrapping_mul(0x51ED_2701) + 7);
        let mut order = SlotOrder::from_vec(Vec::new());
        let mut model: Vec<RecordSlot> = Vec::new();
        let mut keys: Vec<u32> = Vec::new();
        let freeze_at = random.next(60);
        for step in 0..120u32 {
            if step == freeze_at {
                order.freeze();
                assert!(order.is_shared());
            }
            let slot = RecordSlot(step);
            let key = random.next(1_000);
            keys.push(key);
            let target_key = key;
            order.insert_sorted(slot, |existing| {
                keys[existing.0 as usize].cmp(&target_key)
            });
            let index = model
                .iter()
                .position(|existing| keys[existing.0 as usize] > target_key)
                .unwrap_or(model.len());
            model.insert(index, slot);

            assert_eq!(order.len(), model.len());
            assert_eq!(order.to_vec(), model);
            assert_eq!(order.first(), model.first().copied());
            assert_eq!(order.last(), model.last().copied());
            for index in 0..model.len() + 2 {
                assert_eq!(order.get(index), model.get(index).copied());
            }
            assert_eq!(order.iter().collect::<Vec<_>>(), model);
            let start = random.next(model.len() as u32 + 1) as usize;
            let end = start + random.next((model.len() as u32 + 1).saturating_sub(start as u32)) as usize;
            assert_eq!(
                order.iter_range(start..end).collect::<Vec<_>>(),
                model[start..end].to_vec()
            );
            let threshold = random.next(1_000);
            assert_eq!(
                order.partition_point(|slot| keys[slot.0 as usize] <= threshold),
                model.partition_point(|slot| keys[slot.0 as usize] <= threshold)
            );
        }
        let snapshot = order.clone();
        order.insert_sorted(RecordSlot(10_000), |_| std::cmp::Ordering::Greater);
        assert_eq!(snapshot.to_vec(), model);
    }
}

#[test]
fn shared_map_matches_hash_map_in_both_backings() {
    for seed in 0..40u64 {
        let mut random = Lcg(seed.wrapping_mul(0xD1B5_4A32) + 11);
        let mut map: SharedMap<u32, u32> = SharedMap::new();
        let mut model: HashMap<u32, u32> = HashMap::new();
        let freeze_at = random.next(50);
        for step in 0..100u32 {
            if step == freeze_at {
                map.freeze();
                assert!(map.is_shared());
            }
            let key = random.next(50);
            match random.next(4) {
                0..=1 => {
                    let value = random.next(1_000);
                    assert_eq!(map.insert(key, value), model.insert(key, value));
                }
                2 => {
                    assert_eq!(map.remove(&key), model.remove(&key));
                }
                _ => {}
            }
            assert_eq!(map.len(), model.len());
            assert_eq!(map.is_empty(), model.is_empty());
            for probe in 0..55u32 {
                assert_eq!(map.get(&probe), model.get(&probe));
                assert_eq!(map.contains_key(&probe), model.contains_key(&probe));
            }
            let mut actual = map.iter().map(|(key, value)| (*key, *value)).collect::<Vec<_>>();
            let mut expected = model.iter().map(|(key, value)| (*key, *value)).collect::<Vec<_>>();
            actual.sort_unstable();
            expected.sort_unstable();
            assert_eq!(actual, expected);
        }
    }

    for frozen in [false, true] {
        let mut map: SharedMap<RecordId, RecordSlot> = SharedMap::new();
        map.insert(RecordId("alpha".into()), RecordSlot(7));
        if frozen {
            map.freeze();
        }
        assert_eq!(map.get("alpha"), Some(&RecordSlot(7)));
        assert!(map.contains_key("alpha"));
        assert_eq!(map.get("absent"), None);
    }
}
