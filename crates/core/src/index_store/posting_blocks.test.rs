//! Value/length block tests.

use std::sync::Arc;

use crate::model::{CollectionId, Number, RecordSlot};
use crate::posting::ScalarKey;

use super::*;
use crate::index_store::{IndexStore, StoreConfig, read, write};

fn store() -> Arc<IndexStore> {
    Arc::new(IndexStore::new(StoreConfig::default()).expect("store"))
}

#[test]
fn boundaries_and_large_cardinalities() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let value = ScalarKey::String("common".into());
    for slot in 0..4096u32 {
        assert!(insert_member(
            &tx,
            PostingFamily::Value,
            CollectionId(0),
            1,
            &value,
            RecordSlot(slot)
        )
        .expect("insert"));
    }
    assert!(!insert_member(
        &tx,
        PostingFamily::Value,
        CollectionId(0),
        1,
        &value,
        RecordSlot(0)
    )
    .expect("duplicate"));
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        members(&view, PostingFamily::Value, CollectionId(0), 1, &value)
            .expect("members")
            .len(),
        4096
    );
    assert_eq!(
        block_count(&view, PostingFamily::Value, CollectionId(0), 1, &value).expect("blocks"),
        1
    );
    assert!(contains(
        &view,
        PostingFamily::Value,
        CollectionId(0),
        1,
        &value,
        RecordSlot(4095)
    )
    .expect("contains"));
}

#[test]
fn last_bit_removal_deletes_block_and_directory_count() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let value = ScalarKey::Number(Number::Integer(7));
    insert_member(
        &tx,
        PostingFamily::Value,
        CollectionId(0),
        2,
        &value,
        RecordSlot(4096),
    )
    .expect("insert");
    let entry = read_value_directory(&tx, CollectionId(0), 2, &value);
    assert_eq!(entry, 1);
    remove_member(
        &tx,
        PostingFamily::Value,
        CollectionId(0),
        2,
        &value,
        RecordSlot(4096),
    )
    .expect("remove");
    assert_eq!(read_value_directory(&tx, CollectionId(0), 2, &value), 0);
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert!(members(&view, PostingFamily::Value, CollectionId(0), 2, &value)
        .expect("members")
        .is_empty());
}

fn read_value_directory(
    tx: &redb::WriteTransaction,
    collection: CollectionId,
    binding: u32,
    value: &ScalarKey,
) -> u64 {
    let key = crate::index_store::keys::value_directory_key(collection.0, binding, value)
        .expect("key");
    let table = tx
        .open_table(crate::index_store::tables::VALUE_DIRECTORY)
        .expect("table");
    table
        .get(key.as_slice())
        .expect("get")
        .map(|guard| {
            crate::index_store::value_codec::decode::<
                crate::index_store::value_codec::ValueDirectoryEntry,
            >(guard.value(), "directory")
            .expect("decode")
            .cardinality
        })
        .unwrap_or(0)
}

#[test]
fn lengths_use_the_same_block_codec() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let length = ScalarKey::Number(Number::Integer(0));
    for slot in [0u32, 1, 4095, 4096, 4097] {
        insert_member(
            &tx,
            PostingFamily::Length,
            CollectionId(3),
            4,
            &length,
            RecordSlot(slot),
        )
        .expect("insert");
    }
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        members(&view, PostingFamily::Length, CollectionId(3), 4, &length)
            .expect("members"),
        vec![
            RecordSlot(0),
            RecordSlot(1),
            RecordSlot(4095),
            RecordSlot(4096),
            RecordSlot(4097)
        ]
    );
    assert_eq!(
        block_count(&view, PostingFamily::Length, CollectionId(3), 4, &length).expect("blocks"),
        2
    );
}

#[test]
fn numeric_and_string_keys_do_not_leak_across_values() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let numbers = [
        Number::Integer(-2),
        Number::Integer(0),
        Number::Integer(2),
        Number::ExactDecimal(crate::decimal::ExactDecimal::parse("2.0").expect("decimal")),
    ];
    for (index, number) in numbers.iter().enumerate() {
        insert_member(
            &tx,
            PostingFamily::Value,
            CollectionId(5),
            0,
            &ScalarKey::Number(number.clone()),
            RecordSlot(index as u32),
        )
        .expect("insert");
    }
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    // 2 and 2.0 are the same exact value and share one block.
    let two = members(
        &view,
        PostingFamily::Value,
        CollectionId(5),
        0,
        &ScalarKey::Number(Number::Integer(2)),
    )
    .expect("two");
    assert_eq!(two, vec![RecordSlot(2), RecordSlot(3)]);
    let zero = members(
        &view,
        PostingFamily::Value,
        CollectionId(5),
        0,
        &ScalarKey::Number(Number::Integer(0)),
    )
    .expect("zero");
    assert_eq!(zero, vec![RecordSlot(1)]);
}

#[test]
fn abort_restores_previous_blocks() {
    let store = store();
    {
        let tx = store.begin_write().expect("write");
        insert_member(
            &tx,
            PostingFamily::Value,
            CollectionId(0),
            0,
            &ScalarKey::String("x".into()),
            RecordSlot(0),
        )
        .expect("insert");
        tx.commit().expect("commit");
    }
    let tx = store.begin_write().expect("write");
    insert_member(
        &tx,
        PostingFamily::Value,
        CollectionId(0),
        0,
        &ScalarKey::String("x".into()),
        RecordSlot(1),
    )
    .expect("insert");
    tx.abort().expect("abort");
    let view = store.view().expect("view");
    assert_eq!(
        members(
            &view,
            PostingFamily::Value,
            CollectionId(0),
            0,
            &ScalarKey::String("x".into())
        )
        .expect("members"),
        vec![RecordSlot(0)]
    );
}

#[test]
fn directory_validation_matches_block_popcounts() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let value = ScalarKey::String("v".into());
    for slot in 0..300u32 {
        insert_member(
            &tx,
            PostingFamily::Value,
            CollectionId(0),
            0,
            &value,
            RecordSlot(slot),
        )
        .expect("insert");
    }
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        validate_directory(&view, PostingFamily::Value, CollectionId(0), 0, &value, 300)
            .expect("validate"),
        None
    );
    assert!(validate_directory(&view, PostingFamily::Value, CollectionId(0), 0, &value, 299)
        .expect("validate")
        .is_some());
}

#[test]
fn count_prefix_does_not_include_another_binding() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let value = ScalarKey::String("same".into());
    insert_member(
        &tx,
        PostingFamily::Value,
        CollectionId(0),
        1,
        &value,
        RecordSlot(1),
    )
    .expect("one");
    insert_member(
        &tx,
        PostingFamily::Value,
        CollectionId(0),
        2,
        &value,
        RecordSlot(2),
    )
    .expect("two");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        members(&view, PostingFamily::Value, CollectionId(0), 1, &value).expect("one"),
        vec![RecordSlot(1)]
    );
    assert_eq!(
        members(&view, PostingFamily::Value, CollectionId(0), 2, &value).expect("two"),
        vec![RecordSlot(2)]
    );
}

#[test]
fn read_facade_matches_block_members() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let value = ScalarKey::String("facade".into());
    write::insert_value_member(&tx, CollectionId(0), 0, &value, RecordSlot(4)).expect("insert");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        read::value_members(&view, CollectionId(0), 0, &value).expect("members"),
        vec![RecordSlot(4)]
    );
}

#[test]
fn unique_values_use_one_block_each() {
    let store = store();
    let tx = store.begin_write().expect("write");
    for index in 0..100u32 {
        insert_member(
            &tx,
            PostingFamily::Value,
            CollectionId(0),
            0,
            &ScalarKey::String(format!("unique-{index}")),
            RecordSlot(index),
        )
        .expect("insert");
    }
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    let mut blocks = 0u64;
    read::for_each_value_directory(&view, CollectionId(0), 0, |value, entry| {
        assert_eq!(entry.cardinality, 1);
        blocks += block_count(&view, PostingFamily::Value, CollectionId(0), 0, &value)
            .expect("blocks");
        Ok(true)
    })
    .expect("directory");
    assert_eq!(blocks, 100);
}
