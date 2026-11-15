//! FTS block codec, mutation and intersection tests.

use std::sync::Arc;

use crate::model::{CollectionId, RecordSlot};

use super::*;
use crate::index_store::value_codec::GramKind;
use crate::index_store::{IndexStore, StoreConfig, read};

fn store() -> Arc<IndexStore> {
    Arc::new(IndexStore::new(StoreConfig::default()).expect("store"))
}

fn block_with(offsets: &[u16]) -> Block {
    let mut block = Block::default();
    for offset in offsets {
        block.set(*offset);
    }
    block
}

#[test]
fn sparse_and_dense_boundaries_are_canonical() {
    for cardinality in [1usize, 2, 255, 256, 257, 4095, 4096] {
        let offsets = (0..cardinality)
            .map(|index| {
                if cardinality == 1 {
                    0
                } else {
                    ((index * 4095) / (cardinality - 1)) as u16
                }
            })
            .collect::<Vec<_>>();
        let block = block_with(&offsets);
        let encoded = encode_block(&block).expect("encode");
        let encoding = encoded[1];
        if cardinality <= 255 {
            assert_eq!(encoding, 0, "cardinality {cardinality} must be sparse");
        } else {
            assert_eq!(encoding, 1, "cardinality {cardinality} must be dense");
        }
        assert!(encoded.len() <= 516);
        let decoded = decode_block(&encoded).expect("decode");
        assert_eq!(decoded, block);
        assert_eq!(decoded.cardinality() as usize, cardinality);
    }
}

#[test]
fn block_offsets_zero_and_4095_round_trip() {
    let block = block_with(&[0, 4095]);
    let encoded = encode_block(&block).expect("encode");
    assert_eq!(encoded.len(), 4 + 4);
    let decoded = decode_block(&encoded).expect("decode");
    assert!(decoded.contains(0));
    assert!(decoded.contains(4095));
    assert_eq!(decoded.cardinality(), 2);
}

#[test]
fn malformed_blocks_are_rejected() {
    let mut wrong_version = encode_block(&block_with(&[1])).expect("encode");
    wrong_version[0] = 2;
    assert!(decode_block(&wrong_version).is_err());

    let mut zero_cardinality = encode_block(&block_with(&[1])).expect("encode");
    zero_cardinality[2] = 0;
    zero_cardinality[3] = 0;
    assert!(decode_block(&zero_cardinality).is_err());

    let mut unsorted = vec![1u8, 0, 2, 0, 2, 0, 1, 0];
    assert!(decode_block(&sorted_sparse(&[1, 2])).is_ok());
    unsorted[6] = 2;
    assert!(decode_block(&unsorted).is_err());

    let dense = encode_block(&block_with(&(0..256).collect::<Vec<_>>())).expect("encode");
    let mut popcount_mismatch = dense.clone();
    popcount_mismatch[4] ^= 0xFF;
    assert!(decode_block(&popcount_mismatch).is_err());

    let mut truncated = dense;
    truncated.truncate(truncated.len() - 1);
    assert!(decode_block(&truncated).is_err());

    assert!(decode_block(&[]).is_err());
    assert!(decode_block(&[1, 0, 1]).is_err());
    assert!(decode_block(&[1, 0, 1, 0, 0]).is_err());
}

fn sorted_sparse(offsets: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.push(1);
    bytes.push(0);
    bytes.extend_from_slice(&(offsets.len() as u16).to_le_bytes());
    for offset in offsets {
        bytes.extend_from_slice(&offset.to_le_bytes());
    }
    bytes
}

#[test]
fn sparse_encoding_at_dense_threshold_is_rejected() {
    let offsets = (0..256u16).collect::<Vec<_>>();
    let bytes = sorted_sparse(&offsets);
    assert!(decode_block(&bytes).is_err());
}

#[test]
fn batch_updates_aggregate_blocks_and_statistics() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let mut batch = FtsBatch::new();
    for slot in [0u32, 1, 4095, 4096, 4097, 8191] {
        batch.set(0, GramKind::Normal, 7, slot);
    }
    apply_batch(&tx, CollectionId(0), &batch).expect("apply");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        read::gram_statistics(&view, CollectionId(0), 0, GramKind::Normal, 7)
            .expect("stats")
            .expect("present"),
        (6, 2)
    );
    let candidates = candidates(
        &view,
        CollectionId(0),
        0,
        GramKind::Normal,
        &[7],
        None,
    )
    .expect("candidates")
    .expect("restriction");
    assert_eq!(
        candidates.iter().collect::<Vec<_>>(),
        vec![0, 1, 4095, 4096, 4097, 8191]
    );
}

#[test]
fn clearing_the_last_bit_removes_block_and_statistics() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let mut batch = FtsBatch::new();
    batch.set(0, GramKind::Normal, 7, 4);
    apply_batch(&tx, CollectionId(0), &batch).expect("set");
    batch = FtsBatch::new();
    batch.clear(0, GramKind::Normal, 7, 4);
    apply_batch(&tx, CollectionId(0), &batch).expect("clear");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        read::gram_statistics(&view, CollectionId(0), 0, GramKind::Normal, 7)
            .expect("stats"),
        None
    );
    assert_eq!(
        read::fts_block(&view, CollectionId(0), 0, GramKind::Normal, 7, 0)
            .expect("block"),
        None
    );
}

#[test]
fn repeated_sets_and_clears_are_idempotent() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let mut batch = FtsBatch::new();
    batch.set(0, GramKind::Normal, 1, 3);
    batch.set(0, GramKind::Normal, 1, 3);
    apply_batch(&tx, CollectionId(0), &batch).expect("set");
    let mut batch = FtsBatch::new();
    batch.clear(0, GramKind::Normal, 1, 3);
    batch.clear(0, GramKind::Normal, 1, 3);
    apply_batch(&tx, CollectionId(0), &batch).expect("clear");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    assert_eq!(
        read::gram_statistics(&view, CollectionId(0), 0, GramKind::Normal, 1)
            .expect("stats"),
        None
    );
}

#[test]
fn normal_and_skip_and_binding_domains_are_isolated() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let mut batch = FtsBatch::new();
    batch.set(0, GramKind::Normal, 1, 1);
    batch.set(0, GramKind::Skip, 1, 2);
    batch.set(1, GramKind::Normal, 1, 3);
    apply_batch(&tx, CollectionId(0), &batch).expect("apply");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    for (binding, kind, expected) in [
        (0, GramKind::Normal, vec![1]),
        (0, GramKind::Skip, vec![2]),
        (1, GramKind::Normal, vec![3]),
    ] {
        let candidates = candidates(&view, CollectionId(0), binding, kind, &[1], None)
            .expect("candidates")
            .expect("restriction");
        assert_eq!(candidates.iter().collect::<Vec<_>>(), expected);
    }
}

#[test]
fn gram_intersection_uses_smallest_driver_and_restriction() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let mut batch = FtsBatch::new();
    for slot in 0..1000u32 {
        batch.set(0, GramKind::Normal, 1, slot);
    }
    for slot in 0..10u32 {
        batch.set(0, GramKind::Normal, 2, slot);
    }
    apply_batch(&tx, CollectionId(0), &batch).expect("apply");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    let first = super::candidates(
        &view,
        CollectionId(0),
        0,
        GramKind::Normal,
        &[1, 2],
        None,
    )
    .expect("candidates")
    .expect("restriction");
    assert_eq!(first.iter().collect::<Vec<_>>(), (0..10).collect::<Vec<_>>());

    let restriction = roaring::RoaringBitmap::from_iter([3u32, 6, 20, 30]);
    let restricted = candidates(
        &view,
        CollectionId(0),
        0,
        GramKind::Normal,
        &[1, 2],
        Some(&restriction),
    )
    .expect("candidates")
    .expect("restriction");
    assert_eq!(restricted.iter().collect::<Vec<_>>(), vec![3, 6]);
}

#[test]
fn missing_gram_is_empty_under_a_complete_binding() {
    let store = store();
    let view = store.view().expect("view");
    let candidates = candidates(
        &view,
        CollectionId(0),
        0,
        GramKind::Normal,
        &[12345],
        None,
    )
    .expect("candidates")
    .expect("restriction");
    assert!(candidates.is_empty());
}

#[test]
fn no_grams_means_no_safe_restriction() {
    let store = store();
    let view = store.view().expect("view");
    assert_eq!(
        candidates(&view, CollectionId(0), 0, GramKind::Normal, &[], None)
            .expect("candidates"),
        None
    );
}

#[test]
fn record_gram_lists_drive_exact_removal() {
    let store = store();
    let tx = store.begin_write().expect("write");
    let projections = [GramProjection {
        binding: 0,
        kind: GramKind::Normal,
        grams: vec![1, 2, 3],
    }];
    insert_record_grams(&tx, CollectionId(0), RecordSlot(0), &projections).expect("insert");
    remove_record_grams(&tx, CollectionId(0), RecordSlot(0), &projections).expect("remove");
    tx.commit().expect("commit");
    let view = store.view().expect("view");
    for gram in 1..=3 {
        assert_eq!(
            read::gram_statistics(&view, CollectionId(0), 0, GramKind::Normal, gram)
                .expect("stats"),
            None
        );
    }
}
