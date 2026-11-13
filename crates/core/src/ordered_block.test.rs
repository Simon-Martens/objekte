use super::*;
use crate::model::Number;
use crate::posting::ScalarKey;
use crate::segment_format::SegmentWriter;
use std::io::Cursor;

fn open_dictionary<K: Ord + Clone + Serialize + DeserializeOwned>(
    entries: impl IntoIterator<Item = (K, PostingDescriptor)>,
) -> OrderedBlockDictionary<K> {
    let mut dictionary = Vec::new();
    write_ordered_dictionary(&mut dictionary, entries).unwrap();
    let length = dictionary.len();
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), 1).unwrap();
    writer
        .write_section(SectionKind::ScalarDictionaries, |target| {
            target
                .write_all(&dictionary)
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let bytes: Arc<[u8]> = Arc::from(writer.finish().unwrap().into_inner());
    let reader = Arc::new(SegmentReader::from_owned(bytes).unwrap());
    OrderedBlockDictionary::open(reader, SectionKind::ScalarDictionaries, 0, length).unwrap()
}

#[test]
fn seeks_across_many_leaves_and_keeps_negative_results_exact() {
    let entries = (0..4000).map(|number| {
        (
            ScalarKey::String(format!("key-{number:05}")),
            PostingDescriptor::EMPTY,
        )
    });
    let dictionary = open_dictionary(entries);
    assert_eq!(dictionary.len(), 4000);
    for number in [0, 1, 1999, 2000, 3999] {
        let key = ScalarKey::String(format!("key-{number:05}"));
        assert_eq!(
            dictionary.get(&key).unwrap(),
            Some(PostingDescriptor::EMPTY)
        );
    }
    assert_eq!(
        dictionary
            .get(&ScalarKey::String("key-04000".into()))
            .unwrap(),
        None
    );
    assert_eq!(
        dictionary
            .get(&ScalarKey::String("key-02000x".into()))
            .unwrap(),
        None
    );
    let mut cursor = dictionary
        .lower_bound(&ScalarKey::String("key-01998x".into()))
        .unwrap()
        .unwrap();
    assert_eq!(
        dictionary.entry(cursor).unwrap().0,
        ScalarKey::String("key-01999".into())
    );
    cursor = dictionary.next(cursor).unwrap();
    assert_eq!(
        dictionary.entry(cursor).unwrap().0,
        ScalarKey::String("key-02000".into())
    );
}

#[test]
fn exact_numbers_and_oversized_keys_keep_typed_order() {
    let numbers = [
        ScalarKey::Number(Number::Integer(-1)),
        ScalarKey::Number(Number::Integer(0)),
        ScalarKey::Number(Number::Integer(1)),
        ScalarKey::Number(Number::Integer(i128::MAX)),
    ];
    let dictionary = open_dictionary(
        numbers
            .iter()
            .cloned()
            .map(|key| (key, PostingDescriptor::EMPTY)),
    );
    for key in &numbers {
        assert_eq!(dictionary.get(key).unwrap(), Some(PostingDescriptor::EMPTY));
    }
    let long = "x".repeat(TARGET_LEAF_BYTES + 100);
    let dictionary = open_dictionary([
        (ScalarKey::String("a".into()), PostingDescriptor::EMPTY),
        (ScalarKey::String(long.clone()), PostingDescriptor::EMPTY),
        (ScalarKey::String("z".into()), PostingDescriptor::EMPTY),
    ]);
    assert_eq!(
        dictionary.get(&ScalarKey::String(long)).unwrap(),
        Some(PostingDescriptor::EMPTY)
    );
}

#[test]
fn rejects_unsorted_input_and_invalid_block_boundaries() {
    let mut bytes = Vec::new();
    assert!(
        write_ordered_dictionary(
            &mut bytes,
            [
                (ScalarKey::String("z".into()), PostingDescriptor::EMPTY),
                (ScalarKey::String("a".into()), PostingDescriptor::EMPTY),
            ]
        )
        .is_err()
    );

    let mut dictionary = Vec::new();
    write_ordered_dictionary(
        &mut dictionary,
        [
            (ScalarKey::String("a".into()), PostingDescriptor::EMPTY),
            (ScalarKey::String("b".into()), PostingDescriptor::EMPTY),
            (ScalarKey::String("c".into()), PostingDescriptor::EMPTY),
        ],
    )
    .unwrap();
    let offset = dictionary
        .windows(9)
        .position(|window| window == b"\x01\0\0\0\0\0\0\0b")
        .unwrap();
    dictionary[offset + 8] = b'z';
    let length = dictionary.len();
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), 1).unwrap();
    writer
        .write_section(SectionKind::ScalarDictionaries, |target| {
            target
                .write_all(&dictionary)
                .map_err(|error| error.to_string())
        })
        .unwrap();
    let reader = Arc::new(
        SegmentReader::from_owned(Arc::from(writer.finish().unwrap().into_inner())).unwrap(),
    );
    assert!(
        OrderedBlockDictionary::<ScalarKey>::open(
            reader,
            SectionKind::ScalarDictionaries,
            0,
            length
        )
        .is_err()
    );
}
