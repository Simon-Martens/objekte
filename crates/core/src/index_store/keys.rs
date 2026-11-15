//! Canonical per-table key layouts.
//!
//! Each function returns a fresh byte vector. Hot loops reuse
//! [`key_codec::KeyWriter`] instead; these helpers keep read/write layout in
//! exactly one place.

use crate::model::FileId;
use crate::posting::ScalarKey;

use super::key_codec::KeyWriter;
use super::StoreError;
use super::tables::MAX_STATE_TAG;
use super::value_codec::{
    BindingId, CoverageKind, DomainKind, GramKind, RelationStateKind, SortDefinitionId, StateKind,
};

pub fn collection(id: u32) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(4);
    key.push_u32(id);
    key.into_bytes()
}

pub fn file(id: FileId) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(4);
    key.push_u32(id.0);
    key.into_bytes()
}

pub fn record(collection: u32, slot: u32) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(8);
    key.push_u32(collection);
    key.push_u32(slot);
    key.into_bytes()
}

pub fn collection_prefix(collection: u32) -> Vec<u8> {
    collection_key(collection)
}

fn collection_key(collection: u32) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(4);
    key.push_u32(collection);
    key.into_bytes()
}

pub fn field(collection: u32, path: &[String]) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(24);
    key.push_u32(collection);
    for segment in path {
        key.push_str(segment);
    }
    key.into_bytes()
}

pub fn binding(collection: u32, id: BindingId) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(8);
    key.push_u32(collection);
    key.push_u32(id);
    key.into_bytes()
}

pub fn binding_identity(collection: u32, identity: &[u8]) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(8 + identity.len());
    key.push_u32(collection);
    key.push_bytes(identity);
    key.into_bytes()
}

pub fn value_prefix(collection: u32, id: BindingId) -> Vec<u8> {
    binding(collection, id)
}

#[cfg(feature = "store-posting-rows")]
pub fn value_member(
    collection: u32,
    binding: BindingId,
    value: &ScalarKey,
    slot: u32,
) -> Result<Vec<u8>, StoreError> {
    let mut key = KeyWriter::with_capacity(48);
    key.push_u32(collection);
    key.push_u32(binding);
    key.push_scalar(value)?;
    key.push_u32(slot);
    Ok(key.into_bytes())
}

#[cfg(feature = "store-posting-rows")]
pub fn length_member(
    collection: u32,
    binding: BindingId,
    length: &ScalarKey,
    slot: u32,
) -> Result<Vec<u8>, StoreError> {
    value_member(collection, binding, length, slot)
}

pub fn value_directory_key(
    collection: u32,
    binding: BindingId,
    value: &ScalarKey,
) -> Result<Vec<u8>, StoreError> {
    let mut key = KeyWriter::with_capacity(44);
    key.push_u32(collection);
    key.push_u32(binding);
    key.push_scalar(value)?;
    Ok(key.into_bytes())
}

pub fn length_directory_key(
    collection: u32,
    binding: BindingId,
    length: &ScalarKey,
) -> Result<Vec<u8>, StoreError> {
    value_directory_key(collection, binding, length)
}

pub fn state_block(
    collection: u32,
    binding: BindingId,
    state: StateKind,
    block: u32,
) -> Vec<u8> {
    debug_assert!(state.tag() <= MAX_STATE_TAG);
    let mut key = KeyWriter::with_capacity(13);
    key.push_u32(collection);
    key.push_u32(binding);
    key.push_u8(state.tag());
    key.push_u32(block);
    key.into_bytes()
}

pub fn state_prefix(collection: u32, binding: BindingId, state: StateKind) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(9);
    key.push_u32(collection);
    key.push_u32(binding);
    key.push_u8(state.tag());
    key.into_bytes()
}

pub fn state_statistics(collection: u32, binding: BindingId, state: StateKind) -> Vec<u8> {
    state_prefix(collection, binding, state)
}

pub fn coverage_member(
    collection: u32,
    binding: BindingId,
    coverage: CoverageKind,
    slot: u32,
) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(12);
    key.push_u32(collection);
    key.push_u32(binding);
    key.push_u8(coverage.tag());
    key.push_u32(slot);
    key.into_bytes()
}

pub fn domain_member(collection: u32, domain: DomainKind, slot: u32) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(9);
    key.push_u32(collection);
    key.push_u8(domain.tag());
    key.push_u32(slot);
    key.into_bytes()
}

pub fn sort_definition(collection: u32, id: SortDefinitionId) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(8);
    key.push_u32(collection);
    key.push_u32(id);
    key.into_bytes()
}

pub fn sort_prefix(collection: u32, id: SortDefinitionId) -> Vec<u8> {
    sort_definition(collection, id)
}

pub fn sort_entry(
    collection: u32,
    id: SortDefinitionId,
    order_key: &[u8],
    logical_id: &str,
    slot: u32,
) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(24 + order_key.len() + logical_id.len());
    key.push_u32(collection);
    key.push_u32(id);
    key.push_bytes(order_key);
    key.push_str(logical_id);
    key.push_u32(slot);
    key.into_bytes()
}

pub fn relation_outgoing(
    collection: u32,
    declaration: u32,
    source_slot: u32,
    occurrence: u32,
) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(16);
    key.push_u32(collection);
    key.push_u32(declaration);
    key.push_u32(source_slot);
    key.push_u32(occurrence);
    key.into_bytes()
}

pub fn relation_outgoing_prefix(collection: u32, declaration: u32) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(8);
    key.push_u32(collection);
    key.push_u32(declaration);
    key.into_bytes()
}

pub fn relation_incoming(
    target_collection: u32,
    target_id: &str,
    source_collection: u32,
    declaration: u32,
    source_slot: u32,
    occurrence: u32,
) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(24 + target_id.len());
    key.push_u32(target_collection);
    key.push_str(target_id);
    key.push_u32(source_collection);
    key.push_u32(declaration);
    key.push_u32(source_slot);
    key.push_u32(occurrence);
    key.into_bytes()
}

pub fn relation_state(
    collection: u32,
    declaration: u32,
    state: RelationStateKind,
    slot: u32,
) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(13);
    key.push_u32(collection);
    key.push_u32(declaration);
    key.push_u8(state.tag());
    key.push_u32(slot);
    key.into_bytes()
}

pub fn relation_state_prefix(collection: u32, declaration: u32, state: RelationStateKind) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(9);
    key.push_u32(collection);
    key.push_u32(declaration);
    key.push_u8(state.tag());
    key.into_bytes()
}

pub fn fts_key(
    collection: u32,
    binding: BindingId,
    kind: GramKind,
    gram: u64,
    block: u32,
) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(21);
    key.push_u32(collection);
    key.push_u32(binding);
    key.push_u8(kind.tag());
    key.push_u64(gram);
    key.push_u32(block);
    key.into_bytes()
}

pub fn fts_prefix(collection: u32, binding: BindingId, kind: GramKind, gram: u64) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(17);
    key.push_u32(collection);
    key.push_u32(binding);
    key.push_u8(kind.tag());
    key.push_u64(gram);
    key.into_bytes()
}

pub fn fts_statistics_key(collection: u32, binding: BindingId, kind: GramKind, gram: u64) -> Vec<u8> {
    fts_prefix(collection, binding, kind, gram)
}

pub fn record_grams(collection: u32, slot: u32, binding: BindingId, kind: GramKind) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(13);
    key.push_u32(collection);
    key.push_u32(slot);
    key.push_u32(binding);
    key.push_u8(kind.tag());
    key.into_bytes()
}

pub fn record_grams_prefix(collection: u32, slot: u32) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(8);
    key.push_u32(collection);
    key.push_u32(slot);
    key.into_bytes()
}

pub fn file_record(file: FileId, collection: u32, slot: u32) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(12);
    key.push_u32(file.0);
    key.push_u32(collection);
    key.push_u32(slot);
    key.into_bytes()
}

pub fn file_records_prefix(file: FileId) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(4);
    key.push_u32(file.0);
    key.into_bytes()
}

pub fn pending_file(collection: u32, file: FileId) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(8);
    key.push_u32(collection);
    key.push_u32(file.0);
    key.into_bytes()
}

pub fn logical_id(collection: u32, id: &str) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(4 + id.len() + 2);
    key.push_u32(collection);
    key.push_str(id);
    key.into_bytes()
}

pub fn slot_incarnation(collection: u32, slot: u32) -> Vec<u8> {
    record(collection, slot)
}

pub fn free_slot(collection: u32, slot: u32) -> Vec<u8> {
    record(collection, slot)
}

pub fn native_path(path: &[u8]) -> Vec<u8> {
    path.to_vec()
}

pub fn query_path(path_key: &str, file: FileId) -> Vec<u8> {
    let mut key = KeyWriter::with_capacity(path_key.len() + 6);
    key.push_str(path_key);
    key.push_u32(file.0);
    key.into_bytes()
}

pub fn fts_block_of(slot: u32) -> u32 {
    slot >> super::bitmap_block::BLOCK_SHIFT
}

pub fn fts_offset_of(slot: u32) -> u16 {
    (slot & super::tables::FTS_BLOCK_MASK) as u16
}

