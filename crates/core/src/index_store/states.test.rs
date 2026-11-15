//! Structural-state block, mask and statistics tests.

use std::sync::Arc;

use crate::model::{CollectionId, RecordSlot};

use super::*;
use crate::index_store::bitmap_block::{decode_block, encode_block};
use crate::index_store::value_codec::{STATE_MASK_BITS, StoredContributions};
use crate::index_store::keys;
use crate::index_store::{IndexStore, StoreConfig, read};

fn store() -> Arc<IndexStore> {
    Arc::new(IndexStore::new(StoreConfig::default()).expect("store"))
}

fn mask(bits: &[StateKind]) -> u16 {
    bits.iter().fold(0u16, |mask, state| mask | (1u16 << state.tag()))
}

#[test]
fn masks_span_block_boundaries_and_multiple_states() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let masks = [(7u32, mask(&[StateKind::NonEmpty, StateKind::String]))];
    for slot in [0u32, 1, 4095, 4096, 4097, 8191] {
        crate::index_store::states::insert_masks(
            &tx,
            CollectionId(0),
            RecordSlot(slot),
            &masks,
        )
        .expect("mask");
    }
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        read::state_members(&view, CollectionId(0), 7, StateKind::NonEmpty)
            .expect("members"),
        vec![
            RecordSlot(0),
            RecordSlot(1),
            RecordSlot(4095),
            RecordSlot(4096),
            RecordSlot(4097),
            RecordSlot(8191)
        ]
    );
    assert_eq!(
        read::state_cardinality(&view, CollectionId(0), 7, StateKind::String).expect("card"),
        6
    );
    assert!(read::state_contains(
        &view,
        CollectionId(0),
        7,
        StateKind::NonEmpty,
        RecordSlot(4096)
    )
    .expect("contains"));
    assert!(!read::state_contains(
        &view,
        CollectionId(0),
        7,
        StateKind::NonEmpty,
        RecordSlot(4096 + 2)
    )
    .expect("contains"));
}

#[test]
fn sparse_to_dense_transition_keeps_canonical_value() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let mut batch = StateBatch::new();
    for slot in 0..255u32 {
        batch.set(1, StateKind::Number, slot);
    }
    apply_batch(&tx, CollectionId(0), &batch).expect("sparse");
    let key = keys::state_block(0, 1, StateKind::Number, 0);
    {
        let table = tx.open_table(super::super::tables::STATE_BLOCKS).expect("table");
        let value = table.get(key.as_slice()).expect("get").expect("present");
        assert_eq!(value.value()[1], 0, "255 must be sparse");
    }
    let mut batch = StateBatch::new();
    batch.set(1, StateKind::Number, 255);
    apply_batch(&tx, CollectionId(0), &batch).expect("dense");
    {
        let table = tx.open_table(super::super::tables::STATE_BLOCKS).expect("table");
        let value = table.get(key.as_slice()).expect("get").expect("present");
        assert_eq!(value.value()[1], 1, "256 must be dense");
        let decoded = decode_block(value.value()).expect("decode");
        assert_eq!(decoded.cardinality(), 256);
    }
    let mut batch = StateBatch::new();
    batch.clear(1, StateKind::Number, 255);
    apply_batch(&tx, CollectionId(0), &batch).expect("sparse again");
    let table = tx.open_table(super::super::tables::STATE_BLOCKS).expect("table");
    let value = table.get(key.as_slice()).expect("get").expect("present");
    assert_eq!(value.value()[1], 0, "255 must return to sparse");
    drop(value);
    drop(table);
    tx.commit().expect("commit");
}

#[test]
fn statistics_track_cardinality_and_block_transitions() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let mut batch = StateBatch::new();
    for slot in [0u32, 4095, 4096] {
        batch.set(2, StateKind::Array, slot);
    }
    apply_batch(&tx, CollectionId(0), &batch).expect("init");
    let stats = read_statistics_tx(&tx, CollectionId(0), 2, StateKind::Array).expect("stats");
    assert_eq!(stats.cardinality, 3);
    assert_eq!(stats.blocks, 2);
    let mut batch = StateBatch::new();
    batch.clear(2, StateKind::Array, 4096);
    apply_batch(&tx, CollectionId(0), &batch).expect("clear block");
    let stats = read_statistics_tx(&tx, CollectionId(0), 2, StateKind::Array).expect("stats");
    assert_eq!(stats.cardinality, 2);
    assert_eq!(stats.blocks, 1);
    let mut batch = StateBatch::new();
    batch.clear(2, StateKind::Array, 0);
    batch.clear(2, StateKind::Array, 4095);
    apply_batch(&tx, CollectionId(0), &batch).expect("clear last");
    let stats = read_statistics_tx(&tx, CollectionId(0), 2, StateKind::Array).expect("stats");
    assert_eq!(stats, StateStatistics::default());
    tx.commit().expect("commit");
}

#[test]
fn no_change_diff_does_not_rewrite_or_change_statistics() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let old = [(3, mask(&[StateKind::String, StateKind::NonEmpty]))];
    crate::index_store::states::insert_masks(&tx, CollectionId(0), RecordSlot(5), &old)
        .expect("insert");
    let key = keys::state_block(0, 3, StateKind::String, 0);
    let before = {
        let table = tx.open_table(super::super::tables::STATE_BLOCKS).expect("table");
        table
            .get(key.as_slice())
            .expect("get")
            .expect("present")
            .value()
            .to_vec()
    };
    crate::index_store::states::apply_masks(&tx, CollectionId(0), RecordSlot(5), &old, &old)
        .expect("no change");
    let after = {
        let table = tx.open_table(super::super::tables::STATE_BLOCKS).expect("table");
        table
            .get(key.as_slice())
            .expect("get")
            .expect("present")
            .value()
            .to_vec()
    };
    assert_eq!(before, after);
    let stats = read_statistics_tx(&tx, CollectionId(0), 3, StateKind::String).expect("stats");
    assert_eq!(stats.cardinality, 1);
    tx.commit().expect("commit");
}

#[test]
fn delete_and_reuse_same_slot_in_one_transaction_is_coalesced() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let old = [(4, mask(&[StateKind::Object]))];
    crate::index_store::states::insert_masks(&tx, CollectionId(0), RecordSlot(9), &old)
        .expect("insert");
    // Clear the old occupant and set the new occupant in one batch. Sets win.
    crate::index_store::states::apply_masks(
        &tx,
        CollectionId(0),
        RecordSlot(9),
        &old,
        &[(4, mask(&[StateKind::Array]))],
    )
    .expect("replace");
    let stats = read_statistics_tx(&tx, CollectionId(0), 4, StateKind::Object).expect("object");
    assert_eq!(stats.cardinality, 0);
    let stats = read_statistics_tx(&tx, CollectionId(0), 4, StateKind::Array).expect("array");
    assert_eq!(stats.cardinality, 1);
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert!(!read::state_contains(
        &view,
        CollectionId(0),
        4,
        StateKind::Object,
        RecordSlot(9)
    )
    .expect("object"));
    assert!(read::state_contains(
        &view,
        CollectionId(0),
        4,
        StateKind::Array,
        RecordSlot(9)
    )
    .expect("array"));
}

#[test]
fn aborted_state_batch_leaves_previous_blocks() {
    let store = store();
    {
        let tx = store.begin_write().expect("write");
        crate::index_store::states::insert_masks(
            &tx,
            CollectionId(0),
            RecordSlot(0),
            &[(0, mask(&[StateKind::NonEmpty]))],
        )
        .expect("insert");
        tx.commit().expect("commit");
    }
    let tx = store.begin_write().expect("write");
    crate::index_store::states::insert_masks(
        &tx,
        CollectionId(0),
        RecordSlot(1),
        &[(0, mask(&[StateKind::NonEmpty]))],
    )
    .expect("insert");
    tx.abort().expect("abort");
    let view = store.view().expect("view");
    assert_eq!(
        read::state_members(&view, CollectionId(0), 0, StateKind::NonEmpty).expect("members"),
        vec![RecordSlot(0)]
    );
}

#[test]
fn reverse_masks_reconstruct_exact_memberships() {
    let contributions = StoredContributions {
        record: crate::index_store::value_codec::RecordMetadata {
            id: crate::model::RecordId("r".into()),
            file: crate::model::FileId(0),
            range: crate::inventory::ByteRange { start: 0, end: 1 },
            revision: crate::inventory::SourceRevision {
                modified_ns: 0,
                size: 1,
            },
            class: crate::inventory::RecordClass::Logical,
            diagnostic_ref: 0,
            incarnation: 1,
        },
        values: Vec::new(),
        lengths: Vec::new(),
        state_masks: vec![(2, mask(&[StateKind::PathPresence, StateKind::NonEmpty]))],
        coverage: Vec::new(),
        domains: Vec::new(),
        sorts: Vec::new(),
        relations: Vec::new(),
        grams: Vec::new(),
    };
    let encoded =
        crate::index_store::contribution_codec::encode(&contributions).expect("encode");
    let decoded = crate::index_store::contribution_codec::decode(&encoded).expect("decode");
    assert_eq!(
        decoded.into_contributions(Vec::new()),
        crate::index_store::value_codec::StoredContributions {
            grams: Vec::new(),
            ..contributions.clone()
        }
    );
    let mut bad = contributions.clone();
    bad.state_masks = vec![(2, STATE_MASK_BITS + 1)];
    assert!(crate::index_store::contribution_codec::encode(&bad).is_err());
    let mut unsorted = contributions.clone();
    unsorted.state_masks = vec![(2, 1), (1, 1)];
    assert!(crate::index_store::contribution_codec::encode(&unsorted).is_err());
    let mut zero = contributions;
    zero.state_masks = vec![(2, 0)];
    assert!(crate::index_store::contribution_codec::encode(&zero).is_err());
}

#[test]
fn encoded_blocks_are_canonical_after_mutation() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let mut batch = StateBatch::new();
    for slot in 0..300u32 {
        batch.set(0, StateKind::String, slot);
    }
    apply_batch(&tx, CollectionId(0), &batch).expect("apply");
    let key = keys::state_block(0, 0, StateKind::String, 0);
    let table = tx.open_table(super::super::tables::STATE_BLOCKS).expect("table");
    let value = table.get(key.as_slice()).expect("get").expect("present");
    let decoded = decode_block(value.value()).expect("decode");
    let reencoded = encode_block(&decoded).expect("encode");
    assert_eq!(value.value(), reencoded.as_slice());
    drop(value);
    drop(table);
    tx.abort().expect("abort");
}

#[test]
fn validation_counts_state_facts() {
    let store = store();
    let tx = store.begin_write().expect("write");
    crate::index_store::states::insert_masks(
        &tx,
        CollectionId(0),
        RecordSlot(0),
        &[(0, mask(&[StateKind::PathPresence, StateKind::String]))],
    )
    .expect("insert");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        crate::index_store::states::state_fact_count(&view, CollectionId(0)).expect("facts"),
        2
    );
}
