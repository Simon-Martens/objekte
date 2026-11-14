use std::io::Write;
use std::sync::Arc;

use super::backing::{Backing, MappedObject};
use super::dictionary::DictionaryReader;
use super::dictionary_builder::build_dictionary;
use super::format::{ObjectBuilder, ObjectKind, ObjectView, SectionKind};
use super::scalar::KeyOrder;

fn entries(count: usize) -> Vec<(Vec<u8>, Vec<u8>)> {
    (0..count)
        .map(|index| {
            (
                format!("key{:06}", index).into_bytes(),
                format!("value{index}").into_bytes(),
            )
        })
        .collect()
}

fn make_reader(bytes: Vec<u8>) -> DictionaryReader {
    let length = bytes.len() as u64;
    let backing = Backing::resident(bytes);
    DictionaryReader::open(backing, 0, length).unwrap()
}

fn mapped_reader(bytes: &[u8]) -> (tempfile::TempDir, DictionaryReader) {
    let mut builder = ObjectBuilder::new(ObjectKind::Segment);
    for kind in ObjectKind::Segment.required_sections() {
        let payload = if *kind == SectionKind::DictionaryPages {
            bytes.to_vec()
        } else {
            Vec::new()
        };
        builder.add_section(*kind, payload).unwrap();
    }
    let object = builder.finish().unwrap().into_bytes();
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("segment.obx");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(&object).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let mapped = Arc::new(MappedObject::open(&path, None).unwrap());
    let backing = Backing::mapped(mapped);
    let view = ObjectView::new(&backing).unwrap();
    let descriptor = view.section(SectionKind::DictionaryPages).unwrap();
    let reader = DictionaryReader::open(backing, descriptor.offset, descriptor.length).unwrap();
    (directory, reader)
}

#[test]
fn get_bounds_and_counts_match_the_built_entries() {
    let entries = entries(10_000);
    let dictionary = build_dictionary(entries.clone(), KeyOrder::Bytes, 8192).unwrap();
    let reader = make_reader(dictionary);
    assert_eq!(reader.entry_count(), entries.len() as u64);
    for (key, value) in &entries {
        assert_eq!(reader.get(key).unwrap().as_deref(), Some(value.as_slice()));
    }
    assert_eq!(reader.get(b"missing").unwrap(), None);
    assert_eq!(reader.first().unwrap().unwrap().0, entries[0].0);
    assert_eq!(reader.last().unwrap().unwrap().0, entries.last().unwrap().0);
    let lower = reader.lower_bound(b"key000500").unwrap().unwrap();
    assert_eq!(lower.0, b"key000500");
    let upper = reader.upper_bound(b"key000500").unwrap().unwrap();
    assert_eq!(upper.0, b"key000501");
    let missing_lower = reader.lower_bound(b"key000500x").unwrap().unwrap();
    assert_eq!(missing_lower.0, b"key000501");
    assert!(reader.contains_key(b"key009999").unwrap());
    assert!(!reader.contains_key(b"zzz").unwrap());
    reader.verify_full().unwrap();
}

#[test]
fn forward_and_reverse_cursors_cover_every_entry() {
    let entries = entries(5000);
    let dictionary = build_dictionary(entries.clone(), KeyOrder::Bytes, 4096).unwrap();
    let reader = make_reader(dictionary);
    let mut cursor = reader.cursor();
    cursor.first().unwrap();
    let mut forward = Vec::new();
    while let Some((key, _)) = cursor.current() {
        forward.push(key.to_vec());
        if !cursor.next().unwrap() {
            break;
        }
    }
    assert_eq!(forward.len(), entries.len());
    assert_eq!(forward.first().unwrap(), &entries[0].0);
    assert_eq!(forward.last().unwrap(), &entries.last().unwrap().0);

    let mut cursor = reader.cursor();
    cursor.last().unwrap();
    let mut reverse = Vec::new();
    while let Some((key, _)) = cursor.current() {
        reverse.push(key.to_vec());
        if !cursor.previous().unwrap() {
            break;
        }
    }
    reverse.reverse();
    assert_eq!(reverse, forward);
}

#[test]
fn ranges_are_inclusive_and_exclusive_as_requested() {
    let entries = entries(1000);
    let dictionary = build_dictionary(entries, KeyOrder::Bytes, 8192).unwrap();
    let reader = make_reader(dictionary);
    let range = reader
        .range(
            std::ops::Bound::Included(b"key000100".as_slice()),
            std::ops::Bound::Excluded(b"key000110".as_slice()),
        )
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(range.len(), 10);
    assert_eq!(range[0].0, b"key000100");
    assert_eq!(range[9].0, b"key000109");

    let reverse = reader
        .range_reverse(
            std::ops::Bound::Included(b"key000100".as_slice()),
            std::ops::Bound::Included(b"key000105".as_slice()),
        )
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(reverse.len(), 6);
    assert_eq!(reverse[0].0, b"key000105");
    assert_eq!(reverse[5].0, b"key000100");
}

#[test]
fn long_keys_and_values_use_overflow_pages() {
    let long_key = b"key000010".repeat(2_000);
    let long_value = vec![b'v'; 20_000];
    let mut entries = entries(20);
    entries.insert(11, (long_key.clone(), long_value.clone()));
    let dictionary = build_dictionary(entries.clone(), KeyOrder::Bytes, 8192).unwrap();
    let reader = make_reader(dictionary);
    assert_eq!(
        reader.get(&long_key).unwrap().as_deref(),
        Some(long_value.as_slice())
    );
    assert_eq!(reader.entry_count(), 21);
    reader.verify_full().unwrap();
}

#[test]
fn empty_and_single_entry_dictionaries_work() {
    let empty = build_dictionary(Vec::new(), KeyOrder::Bytes, 8192).unwrap();
    let reader = make_reader(empty);
    assert_eq!(reader.entry_count(), 0);
    assert!(reader.get(b"anything").unwrap().is_none());
    assert!(reader.first().unwrap().is_none());
    assert!(reader.last().unwrap().is_none());
    reader.verify_full().unwrap();

    let single = build_dictionary(
        vec![(b"only".to_vec(), b"value".to_vec())],
        KeyOrder::Bytes,
        8192,
    )
    .unwrap();
    let reader = make_reader(single);
    assert_eq!(reader.get(b"only").unwrap().unwrap(), b"value");
    assert_eq!(reader.first().unwrap().unwrap().0, b"only");
    assert_eq!(reader.last().unwrap().unwrap().0, b"only");
}

#[test]
fn large_dictionary_has_multiple_levels() {
    let entries = entries(100_000);
    let dictionary = build_dictionary(entries, KeyOrder::Bytes, 4096).unwrap();
    let reader = make_reader(dictionary);
    assert!(reader.header().depth >= 2);
    assert!(reader.page_count() > 50);
    assert_eq!(
        reader.get(b"key050000").unwrap().unwrap(),
        b"value50000".to_vec()
    );
    reader.verify_full().unwrap();
}

#[test]
fn numeric_scalar_dictionary_orders_by_number() {
    let values = [-100i64, -1, 0, 1, 2, 10, 100];
    let entries = values
        .iter()
        .map(|value| {
            (
                super::scalar::encode_scalar_key(&crate::posting::ScalarKey::Number(
                    crate::model::Number::Integer(*value as i128),
                ))
                .unwrap(),
                value.to_string().into_bytes(),
            )
        })
        .collect::<Vec<_>>();
    let dictionary = build_dictionary(entries, KeyOrder::Scalar, 8192).unwrap();
    let reader = make_reader(dictionary);
    let key = super::scalar::encode_scalar_key(&crate::posting::ScalarKey::Number(
        crate::model::Number::Integer(5),
    ))
    .unwrap();
    assert_eq!(reader.lower_bound(&key).unwrap().unwrap().1, b"10");
    let key = super::scalar::encode_scalar_key(&crate::posting::ScalarKey::Number(
        crate::model::Number::Integer(-2),
    ))
    .unwrap();
    assert_eq!(reader.lower_bound(&key).unwrap().unwrap().1, b"-1");
}

#[test]
fn resident_and_mapped_readers_agree() {
    let entries = entries(10_000);
    let dictionary = build_dictionary(entries, KeyOrder::Bytes, 8192).unwrap();
    let resident = make_reader(dictionary.clone());
    let (_directory, mapped) = mapped_reader(&dictionary);
    assert_eq!(resident.entry_count(), mapped.entry_count());
    for key in [b"key000000".as_slice(), b"key005555", b"key009999", b"zzz"] {
        assert_eq!(resident.get(key).unwrap(), mapped.get(key).unwrap());
        assert_eq!(
            resident.lower_bound(key).unwrap(),
            mapped.lower_bound(key).unwrap()
        );
        assert_eq!(
            resident.upper_bound(key).unwrap(),
            mapped.upper_bound(key).unwrap()
        );
    }
    let resident_entries = resident
        .range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mapped_entries = mapped
        .range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(resident_entries, mapped_entries);
}

#[test]
fn corrupt_root_and_pages_are_rejected() {
    let entries = entries(1000);
    let dictionary = build_dictionary(entries, KeyOrder::Bytes, 4096).unwrap();
    let mut bad_root = dictionary.clone();
    bad_root[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
    let length = bad_root.len() as u64;
    let backing = Backing::resident(bad_root);
    assert!(DictionaryReader::open(backing, 0, length).is_err());

    let mut bad_length = dictionary.clone();
    let total = bad_length.len();
    let root = u64::from_le_bytes(bad_length[16..24].try_into().unwrap()) as usize;
    bad_length[root + 24..root + 28].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(DictionaryReader::open(Backing::resident(bad_length), 0, total as u64).is_err());

    let mut truncated = dictionary;
    truncated.truncate(100);
    assert!(DictionaryReader::open(Backing::resident(truncated), 0, 100).is_err());
}

#[test]
fn builder_rejects_unsorted_entries() {
    let entries = vec![(b"b".to_vec(), Vec::new()), (b"a".to_vec(), Vec::new())];
    assert!(build_dictionary(entries, KeyOrder::Bytes, 8192).is_err());
}
