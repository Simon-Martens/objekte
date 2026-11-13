use super::*;
use crate::model::Number;
use crate::posting::ScalarKey;
use crate::segment_format::SegmentWriter;
use std::io::{Cursor, Write};

fn seal<K: Ord + Clone + Serialize + DeserializeOwned>(
    map: &BTreeMap<K, RoaringBitmap>,
    allowed: RoaringBitmap,
) -> SealedPostingDictionary<K> {
    let mut pack = PostingPack::new();
    let mut dictionaries = Vec::new();
    let span = append_dictionary(&mut dictionaries, &mut pack, map).unwrap();
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), 2).unwrap();
    writer
        .write_section(SectionKind::ScalarDictionaries, |target| {
            target
                .write_all(&dictionaries)
                .map_err(|error| error.to_string())
        })
        .unwrap();
    writer
        .write_section(SectionKind::PostingPayloads, |target| {
            target
                .write_all(pack.bytes())
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let reader = Arc::new(
        SegmentReader::from_owned(Arc::from(writer.finish().unwrap().into_inner())).unwrap(),
    );
    SealedPostingDictionary::open(
        reader,
        SectionKind::ScalarDictionaries,
        span,
        Arc::new(allowed),
    )
    .unwrap()
}

#[test]
fn equality_bounds_and_restriction_match_builder_map_across_leaves() {
    let mut map = BTreeMap::new();
    for number in 0..4000 {
        map.insert(
            ScalarKey::String(format!("key-{number:05}")),
            RoaringBitmap::from_iter([number % 1000, (number + 17) % 1000]),
        );
    }
    let sealed = seal(&map, RoaringBitmap::from_iter(0..1000));
    assert_eq!(sealed.len(), map.len());
    for number in [0, 1, 1500, 3999] {
        let key = ScalarKey::String(format!("key-{number:05}"));
        assert_eq!(sealed.get(&key).unwrap().as_ref(), map.get(&key));
        assert_eq!(sealed.estimated_cardinality(&key).unwrap(), Some(2));
    }
    assert_eq!(
        sealed.get(&ScalarKey::String("key-04000".into())).unwrap(),
        None
    );
    let lower = ScalarKey::String("key-01995".into());
    let upper = ScalarKey::String("key-02005".into());
    let restricted = RoaringBitmap::from_iter([12, 13, 14, 15, 16, 17, 18]);
    let mut expected = RoaringBitmap::new();
    for (_, posting) in map.range((Bound::Excluded(&lower), Bound::Included(&upper))) {
        expected |= posting & &restricted;
    }
    assert_eq!(
        sealed
            .union_bounds(
                Bound::Excluded(&lower),
                Bound::Included(&upper),
                Some(&restricted)
            )
            .unwrap(),
        expected
    );
    assert_eq!(
        sealed.get_restricted(&lower, &restricted).unwrap().unwrap(),
        map.get(&lower).unwrap() & &restricted
    );
}

#[test]
fn exact_numbers_empty_posting_and_negative_lookup_remain_distinct() {
    let values = [
        Number::parse_json_literal("-100000000000000000000000000000000000000").unwrap(),
        Number::integer(-1),
        Number::parse_json_literal("0.00000000000000000000000000001").unwrap(),
        Number::integer(1),
        Number::parse_json_literal("100000000000000000000000000000000000000").unwrap(),
    ];
    let mut map = BTreeMap::new();
    for (position, value) in values.iter().enumerate() {
        map.insert(
            ScalarKey::Number(value.clone()),
            RoaringBitmap::from_iter([position as u32]),
        );
    }
    let empty_key = ScalarKey::String("present but empty".into());
    map.insert(empty_key.clone(), RoaringBitmap::new());
    let sealed = seal(&map, RoaringBitmap::from_iter(0..5));
    assert_eq!(sealed.get(&empty_key).unwrap(), Some(RoaringBitmap::new()));
    assert_eq!(sealed.estimated_cardinality(&empty_key).unwrap(), Some(0));
    assert_eq!(
        sealed.get(&ScalarKey::Number(Number::integer(0))).unwrap(),
        None
    );
    let lower = ScalarKey::Number(Number::integer(-1));
    let upper = ScalarKey::Number(Number::integer(1));
    assert_eq!(
        sealed
            .union_bounds(Bound::Included(&lower), Bound::Excluded(&upper), None)
            .unwrap(),
        RoaringBitmap::from_iter([1, 2])
    );
}

#[test]
fn malformed_posting_is_an_error_not_an_empty_result() {
    let mut map = BTreeMap::new();
    map.insert(
        ScalarKey::String("exists".into()),
        RoaringBitmap::from_iter([1, 4]),
    );
    let mut pack = PostingPack::new();
    let mut dictionary = Vec::new();
    let span = append_dictionary(&mut dictionary, &mut pack, &map).unwrap();
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), 2).unwrap();
    writer
        .write_section(SectionKind::ScalarDictionaries, |out| {
            out.write_all(&dictionary).map_err(|e| e.to_string())
        })
        .unwrap();
    writer
        .write_section(SectionKind::PostingPayloads, |out| {
            out.write_all(pack.bytes()).map_err(|e| e.to_string())
        })
        .unwrap();
    let mut bytes = writer.finish().unwrap().into_inner();
    *bytes.last_mut().unwrap() ^= 1;
    let reader = Arc::new(SegmentReader::from_owned(Arc::from(bytes)).unwrap());
    let sealed = SealedPostingDictionary::open(
        reader,
        SectionKind::ScalarDictionaries,
        span,
        Arc::new(RoaringBitmap::from_iter(0..10)),
    )
    .unwrap();
    assert!(sealed.get(&ScalarKey::String("exists".into())).is_err());
    assert!(sealed.get(&ScalarKey::String("exists".into())).is_err());
    assert_eq!(
        sealed.get(&ScalarKey::String("absent".into())).unwrap(),
        None
    );
}
