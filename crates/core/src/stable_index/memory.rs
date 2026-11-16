//! Owned in-memory backing for one independent stable index.
//!
//! ic-stable-structures already implements [`Memory`] for
//! `RefCell<Vec<u8>>`.  The prototype deliberately uses that owned form
//! directly instead of the `Rc`-wrapped `VectorMemory` alias so completed
//! trees can be moved between worker and coordinator threads.

use std::borrow::Cow;
use std::cell::RefCell;

use ic_stable_structures::storable::Bound;
use ic_stable_structures::{BTreeMap, Storable};
use roaring::RoaringBitmap;

use super::codec::ComponentKey;

/// The single owned allocation behind one stable structure.
pub type IndexMemory = RefCell<Vec<u8>>;

/// One independently owned stable tree.
pub type IndexTree = BTreeMap<ComponentKey, StoredBytes, IndexMemory>;

/// Stable-memory allocation unit.  The persisted image includes allocator
/// free space rounded to this unit; it is not an application paging unit.
pub const MEMORY_PAGE_SIZE: usize = 65_536;

const TREE_MAGIC: &[u8; 3] = b"BTR";
const TREE_LAYOUT_V2: u8 = 2;

/// Encoded value bytes.  Values are decoded only when a caller asks for
/// their semantic contents; insert/remove may discard one without decoding.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct StoredBytes(Vec<u8>);

impl StoredBytes {
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    pub fn to_bytes(&self) -> &[u8] {
        &self.0
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Storable for StoredBytes {
    fn to_bytes(&self) -> Cow<'_, [u8]> {
        Cow::Borrowed(&self.0)
    }

    fn into_bytes(self) -> Vec<u8> {
        self.0
    }

    fn from_bytes(bytes: Cow<[u8]>) -> Self {
        Self(bytes.into_owned())
    }

    const BOUND: Bound = Bound::Unbounded;
}

/// Create a fresh empty tree.  `BTreeMap::new` initializes the V2 header in
/// one owned page rather than searching for an existing header.
pub fn new_tree() -> IndexTree {
    IndexTree::new(RefCell::new(Vec::new()))
}

/// Consume a tree and return its complete logical memory image.
///
/// Unused `Vec` capacity is not part of the image.  The image length is
/// always an allocator page multiple for a non-empty tree.
pub fn tree_payload(tree: IndexTree) -> Vec<u8> {
    tree.into_memory().into_inner()
}

/// Restore a tree from an owned payload without inserting entries.
///
/// The caller must have verified the envelope, descriptor and payload hash.
/// We still check the page alignment and V2 header here so a truncated or
/// legacy image is rejected before the library can migrate it.
pub fn load_tree(payload: Vec<u8>) -> Result<IndexTree, String> {
    validate_tree_payload(&payload)?;
    Ok(IndexTree::load(RefCell::new(payload)))
}

pub fn validate_tree_payload(payload: &[u8]) -> Result<(), String> {
    if payload.len() % MEMORY_PAGE_SIZE != 0 {
        return Err(format!(
            "stable payload length {} is not a multiple of {MEMORY_PAGE_SIZE}",
            payload.len()
        ));
    }
    if payload.is_empty() {
        return Err("empty stable payload has no tree header".into());
    }
    if payload.len() < super::codec::TREE_HEADER_BYTES {
        return Err("stable payload is shorter than the tree header".into());
    }
    if &payload[0..3] != TREE_MAGIC {
        return Err("stable payload has the wrong tree magic".into());
    }
    if payload[3] != TREE_LAYOUT_V2 {
        return Err(format!(
            "stable payload uses unsupported tree layout {}; expected {TREE_LAYOUT_V2}",
            payload[3]
        ));
    }
    Ok(())
}

pub fn bitmap_value(bitmap: &RoaringBitmap) -> Result<StoredBytes, String> {
    let mut bytes = Vec::with_capacity(bitmap.serialized_size());
    bitmap
        .serialize_into(&mut bytes)
        .map_err(|error| format!("bitmap encode failed: {error}"))?;
    Ok(StoredBytes::new(bytes))
}

pub fn decode_bitmap(value: &StoredBytes) -> Result<RoaringBitmap, String> {
    RoaringBitmap::deserialize_from(value.to_bytes())
        .map_err(|error| format!("bitmap decode failed: {error}"))
}

// The worker result types must remain ordinary Send values.  This is a
// compile-time proof; no unsafe Send/Sync implementation is used.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<IndexTree>();
    assert_send::<StoredBytes>();
    assert_send::<IndexMemory>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stable_index::codec::ComponentKey;

    #[test]
    fn owned_memory_round_trip_preserves_and_mutates() {
        let mut tree = new_tree();
        tree.insert(ComponentKey::RecordSlot(7), StoredBytes::new(vec![11]));
        let image = tree_payload(tree);
        assert_eq!(image.len() % MEMORY_PAGE_SIZE, 0);
        let mut restored = load_tree(image).expect("valid image");
        assert_eq!(
            restored.get(&ComponentKey::RecordSlot(7)),
            Some(StoredBytes::new(vec![11]))
        );
        restored.insert(ComponentKey::RecordSlot(8), StoredBytes::new(vec![12]));
        assert_eq!(
            restored.remove(&ComponentKey::RecordSlot(7)),
            Some(StoredBytes::new(vec![11]))
        );
    }

    #[test]
    fn tree_payload_rejects_legacy_or_unaligned_images() {
        assert!(load_tree(Vec::new()).is_err());
        let mut legacy = vec![0; MEMORY_PAGE_SIZE];
        legacy[0..3].copy_from_slice(b"BTR");
        legacy[3] = 1;
        assert!(load_tree(legacy).is_err());
        assert!(load_tree(vec![0; 17]).is_err());
    }
}
