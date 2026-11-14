use roaring::RoaringBitmap;

use super::record_set::{
    BLOCK_DESCRIPTOR_LEN, RECORD_SET_DESCRIPTOR_LEN, RecordSetRef, RecordSetTag, encode_record_set,
};

fn view(
    encoded: &super::record_set::EncodedRecordSet,
    domain: u32,
) -> super::record_set::RecordSetView<'_> {
    encoded.reference.view(&encoded.arena, domain).unwrap()
}

fn expected(slots: &[u32]) -> RoaringBitmap {
    slots.iter().copied().collect()
}

#[test]
fn small_forms_are_detected_deterministically() {
    let empty = encode_record_set(&[], 10).unwrap();
    assert_eq!(empty.reference.tag, RecordSetTag::Empty);
    assert!(view(&empty, 10).is_empty());

    let all_slots = [0u32, 1, 2, 3, 4];
    let all = encode_record_set(&all_slots, 5).unwrap();
    assert_eq!(all.reference.tag, RecordSetTag::All);
    assert_eq!(all.reference.cardinality(), 5);

    let range = encode_record_set(&[10, 11, 12, 13], 100).unwrap();
    assert_eq!(range.reference.tag, RecordSetTag::Range);
    assert!(view(&range, 100).contains(10).unwrap());
    assert!(view(&range, 100).contains(13).unwrap());
    assert!(!view(&range, 100).contains(14).unwrap());

    let inline = encode_record_set(&[1, 9, 20], 100).unwrap();
    assert_eq!(inline.reference.tag, RecordSetTag::Inline);
    assert!(view(&inline, 100).contains(9).unwrap());
    assert!(!view(&inline, 100).contains(10).unwrap());
}

#[test]
fn block_codecs_round_trip_across_boundaries() {
    let mut slots = (0u32..=65_535).collect::<Vec<_>>();
    slots.push(100_000);
    slots.push(100_001);
    let encoded = encode_record_set(&slots, 200_000).unwrap();
    assert_eq!(encoded.reference.tag, RecordSetTag::Blocks);
    assert!(encoded.reference.b >= BLOCK_DESCRIPTOR_LEN as u64 * 2);
    let view = view(&encoded, 200_000);
    view.verify_full().unwrap();
    assert!(view.contains(0).unwrap());
    assert!(view.contains(65_535).unwrap());
    assert!(view.contains(100_000).unwrap());
    assert!(!view.contains(65_536).unwrap());
    assert_eq!(view.cardinality(), slots.len() as u64);
    let collected = view.ids().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(collected, slots);
}

#[test]
fn array_bitmap_and_run_shapes_decode_correctly() {
    let sparse = (0..4096u32).map(|value| value * 13).collect::<Vec<_>>();
    let dense = (100_000u32..105_000).collect::<Vec<_>>();
    let random = (0..5000u32)
        .map(|value| 200_000 + (value.wrapping_mul(2654435761) & 0xffff))
        .collect::<Vec<_>>();
    let mut random = random;
    random.sort_unstable();
    random.dedup();
    for slots in [sparse, dense, random] {
        let encoded = encode_record_set(&slots, 400_000).unwrap();
        let view = view(&encoded, 400_000);
        view.verify_full().unwrap();
        let collected = view.ids().collect::<Result<Vec<_>, _>>().unwrap();
        assert_eq!(collected, slots);
    }
}

#[test]
fn full_block_run_is_representable() {
    let slots = (0u32..65_536).chain(300_000..300_010).collect::<Vec<_>>();
    let encoded = encode_record_set(&slots, 400_000).unwrap();
    let view = view(&encoded, 400_000);
    view.verify_full().unwrap();
    assert_eq!(view.cardinality(), slots.len() as u64);
    assert!(view.contains(65_535).unwrap());
    assert!(view.contains(300_009).unwrap());
    assert!(!view.contains(300_010).unwrap());
}

#[test]
fn set_operations_match_roaring() {
    let left_slots = (0..2000u32)
        .map(|value| value * 7)
        .chain(90_000..92_000)
        .collect::<Vec<_>>();
    let right_slots = (0..3000u32)
        .map(|value| value * 5)
        .chain(91_000..93_000)
        .collect::<Vec<_>>();
    let left = encode_record_set(&left_slots, 200_000).unwrap();
    let right = encode_record_set(&right_slots, 200_000).unwrap();
    let left_view = view(&left, 200_000);
    let right_view = view(&right, 200_000);
    let left_roaring = expected(&left_slots);
    let right_roaring = expected(&right_slots);

    let mut union = RoaringBitmap::new();
    left_view.union_into(&mut union).unwrap();
    right_view.union_into(&mut union).unwrap();
    assert_eq!(union, &left_roaring | &right_roaring);
    assert_eq!(
        left_view.intersection_count(&right_view).unwrap(),
        (&left_roaring & &right_roaring).len()
    );
    assert_eq!(
        left_view.intersect_into(&right_view).unwrap(),
        &left_roaring & &right_roaring
    );
    assert_eq!(
        left_view.difference_into(&right_view).unwrap(),
        &left_roaring - &right_roaring
    );
}

#[test]
fn all_and_range_intersection_counts_are_exact() {
    let all = encode_record_set(&(0..500u32).collect::<Vec<_>>(), 500).unwrap();
    let range = encode_record_set(&(100..300u32).collect::<Vec<_>>(), 500).unwrap();
    let all_view = view(&all, 500);
    let range_view = view(&range, 500);
    assert_eq!(all_view.intersection_count(&range_view).unwrap(), 200);
    assert_eq!(range_view.intersection_count(&all_view).unwrap(), 200);
}

#[test]
fn descriptor_corruption_is_rejected() {
    let encoded = encode_record_set(&[1, 5, 9, 20], 100).unwrap();
    let mut bytes = encoded.reference.encode();
    bytes[0] = 99;
    assert!(RecordSetRef::decode(&bytes).is_err());

    let mut bad_cardinality = encoded.reference.encode();
    bad_cardinality[24..32].copy_from_slice(&99u64.to_le_bytes());
    let reference = RecordSetRef::decode(&bad_cardinality).unwrap();
    assert!(reference.view(&encoded.arena, 100).is_err());

    let mut short = vec![0u8; RECORD_SET_DESCRIPTOR_LEN - 1];
    assert!(RecordSetRef::decode(&short).is_err());
    short.push(0);
    assert!(RecordSetRef::decode(&short).is_ok());
}

#[test]
fn block_payload_corruption_is_rejected() {
    let slots = (0u32..1000).chain(200_000..200_010).collect::<Vec<_>>();
    let mut encoded = encode_record_set(&slots, 300_000).unwrap();
    assert_eq!(encoded.reference.tag, RecordSetTag::Blocks);
    // The first block is a run; make its length overflow the 65,536-ID block.
    let payload = encoded.reference.b as usize;
    encoded.arena[payload + 2..payload + 4].copy_from_slice(&u16::MAX.to_le_bytes());
    let view = view(&encoded, 300_000);
    assert!(view.verify_full().is_err());
}

#[test]
fn inline_and_range_validation_rejects_bad_domains() {
    assert!(encode_record_set(&[4, 2], 10).is_err());
    assert!(encode_record_set(&[10], 10).is_err());
    assert!(RecordSetRef::range(5, 4).is_err());
    let reference = RecordSetRef::range(0, 11).unwrap();
    assert!(reference.view(&[], 10).is_err());
}
