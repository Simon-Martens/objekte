//! Owned in-memory backing for one independent stable index.
//!
//! ic-stable-structures already implements [`Memory`] for
//! `RefCell<Vec<u8>>`.  The prototype deliberately uses that owned form
//! directly instead of the `Rc`-wrapped `VectorMemory` alias so completed
//! trees can be moved between worker and coordinator threads.

use std::borrow::Cow;
use std::cell::RefCell;
use std::ops::Bound;

use ic_stable_structures::btreemap::Iter;
use ic_stable_structures::storable::Bound as StorableBound;
use ic_stable_structures::{BTreeMap, Storable};
use roaring::RoaringBitmap;

use super::codec::{
    ComponentKey, ComponentKind, FileKey, GramKey, PathKey, RecordIdentityKey, RecordKey,
    RelationOccurrenceKey, RelationTargetKey, SortEntryKey, StateKey,
};
use crate::model::Number;
use crate::posting::ScalarKey;

/// The single owned allocation behind one stable structure.
pub type IndexMemory = RefCell<Vec<u8>>;

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

    const BOUND: StorableBound = StorableBound::Unbounded;
}

/// One independently owned stable tree with role-specific key types.
pub enum ComponentTree {
    Files(BTreeMap<FileKey, StoredBytes, IndexMemory>),
    Paths(BTreeMap<PathKey, StoredBytes, IndexMemory>),
    Records(BTreeMap<RecordKey, StoredBytes, IndexMemory>),
    Identities(BTreeMap<RecordIdentityKey, StoredBytes, IndexMemory>),
    Domains(BTreeMap<StateKey, StoredBytes, IndexMemory>),
    Values(BTreeMap<ScalarKey, StoredBytes, IndexMemory>),
    Lengths(BTreeMap<Number, StoredBytes, IndexMemory>),
    States(BTreeMap<StateKey, StoredBytes, IndexMemory>),
    Normals(BTreeMap<GramKey, StoredBytes, IndexMemory>),
    Skips(BTreeMap<GramKey, StoredBytes, IndexMemory>),
    Sorts(BTreeMap<SortEntryKey, StoredBytes, IndexMemory>),
    Occurrences(BTreeMap<RelationOccurrenceKey, StoredBytes, IndexMemory>),
    RelationTargets(BTreeMap<RelationTargetKey, StoredBytes, IndexMemory>),
    RelationStates(BTreeMap<StateKey, StoredBytes, IndexMemory>),
}

pub type IndexTree = ComponentTree;

macro_rules! with_memory {
    ($tree:expr, $map:ident => $body:expr) => {
        match $tree {
            ComponentTree::Files($map) => $body,
            ComponentTree::Paths($map) => $body,
            ComponentTree::Records($map) => $body,
            ComponentTree::Identities($map) => $body,
            ComponentTree::Domains($map) => $body,
            ComponentTree::Values($map) => $body,
            ComponentTree::Lengths($map) => $body,
            ComponentTree::States($map) => $body,
            ComponentTree::Normals($map) => $body,
            ComponentTree::Skips($map) => $body,
            ComponentTree::Sorts($map) => $body,
            ComponentTree::Occurrences($map) => $body,
            ComponentTree::RelationTargets($map) => $body,
            ComponentTree::RelationStates($map) => $body,
        }
    };
}

fn wrong_key(kind: ComponentKind, key: &ComponentKey) -> ! {
    panic!(
        "stored key {} does not belong to {:?} component storage",
        key.tag(),
        kind
    )
}

impl ComponentTree {
    pub fn new(kind: ComponentKind) -> Self {
        let memory = || RefCell::new(Vec::new());
        match kind {
            ComponentKind::Files => Self::Files(BTreeMap::new(memory())),
            ComponentKind::Paths => Self::Paths(BTreeMap::new(memory())),
            ComponentKind::Records => Self::Records(BTreeMap::new(memory())),
            ComponentKind::Identities => Self::Identities(BTreeMap::new(memory())),
            ComponentKind::Domains => Self::Domains(BTreeMap::new(memory())),
            ComponentKind::Values => Self::Values(BTreeMap::new(memory())),
            ComponentKind::Lengths => Self::Lengths(BTreeMap::new(memory())),
            ComponentKind::States => Self::States(BTreeMap::new(memory())),
            ComponentKind::NormalGrams => Self::Normals(BTreeMap::new(memory())),
            ComponentKind::SkipGrams => Self::Skips(BTreeMap::new(memory())),
            ComponentKind::SortAscending | ComponentKind::SortDescending => {
                Self::Sorts(BTreeMap::new(memory()))
            }
            ComponentKind::RelationOccurrences => Self::Occurrences(BTreeMap::new(memory())),
            ComponentKind::RelationTargets => Self::RelationTargets(BTreeMap::new(memory())),
            ComponentKind::RelationStates => Self::RelationStates(BTreeMap::new(memory())),
        }
    }

    pub fn empty_like(&self) -> Self {
        match self {
            Self::Files(_) => Self::new(ComponentKind::Files),
            Self::Paths(_) => Self::new(ComponentKind::Paths),
            Self::Records(_) => Self::new(ComponentKind::Records),
            Self::Identities(_) => Self::new(ComponentKind::Identities),
            Self::Domains(_) => Self::new(ComponentKind::Domains),
            Self::Values(_) => Self::new(ComponentKind::Values),
            Self::Lengths(_) => Self::new(ComponentKind::Lengths),
            Self::States(_) => Self::new(ComponentKind::States),
            Self::Normals(_) => Self::new(ComponentKind::NormalGrams),
            Self::Skips(_) => Self::new(ComponentKind::SkipGrams),
            Self::Sorts(_) => Self::new(ComponentKind::SortAscending),
            Self::Occurrences(_) => Self::new(ComponentKind::RelationOccurrences),
            Self::RelationTargets(_) => Self::new(ComponentKind::RelationTargets),
            Self::RelationStates(_) => Self::new(ComponentKind::RelationStates),
        }
    }

    pub fn kind(&self) -> ComponentKind {
        match self {
            Self::Files(_) => ComponentKind::Files,
            Self::Paths(_) => ComponentKind::Paths,
            Self::Records(_) => ComponentKind::Records,
            Self::Identities(_) => ComponentKind::Identities,
            Self::Domains(_) => ComponentKind::Domains,
            Self::Values(_) => ComponentKind::Values,
            Self::Lengths(_) => ComponentKind::Lengths,
            Self::States(_) => ComponentKind::States,
            Self::Normals(_) => ComponentKind::NormalGrams,
            Self::Skips(_) => ComponentKind::SkipGrams,
            Self::Sorts(_) => ComponentKind::SortAscending,
            Self::Occurrences(_) => ComponentKind::RelationOccurrences,
            Self::RelationTargets(_) => ComponentKind::RelationTargets,
            Self::RelationStates(_) => ComponentKind::RelationStates,
        }
    }

    pub fn len(&self) -> u64 {
        with_memory!(self, map => map.len())
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn insert(&mut self, key: ComponentKey, value: StoredBytes) -> Option<StoredBytes> {
        match (self, key) {
            (Self::Files(map), ComponentKey::FileId(key)) => map.insert(FileKey(key), value),
            (Self::Paths(map), ComponentKey::Path(key)) => map.insert(key, value),
            (Self::Records(map), ComponentKey::RecordSlot(key)) => {
                map.insert(RecordKey(key), value)
            }
            (Self::Identities(map), ComponentKey::Identity(key)) => map.insert(key, value),
            (Self::Domains(map), ComponentKey::Domain(key)) => map.insert(StateKey(key), value),
            (Self::Values(map), ComponentKey::Scalar(key)) => map.insert(key, value),
            (Self::Lengths(map), ComponentKey::Length(key)) => map.insert(key, value),
            (Self::States(map), ComponentKey::State(key)) => map.insert(StateKey(key), value),
            (Self::Normals(map), ComponentKey::Gram(key)) => map.insert(GramKey(key), value),
            (Self::Skips(map), ComponentKey::Gram(key)) => map.insert(GramKey(key), value),
            (Self::Sorts(map), ComponentKey::Sort(key)) => map.insert(key, value),
            (Self::Occurrences(map), ComponentKey::Occurrence(key)) => map.insert(key, value),
            (Self::RelationTargets(map), ComponentKey::RelationTarget(key)) => {
                map.insert(key, value)
            }
            (Self::RelationStates(map), ComponentKey::RelationState(key)) => {
                map.insert(StateKey(key), value)
            }
            (tree, key) => wrong_key(tree.kind(), &key),
        }
    }

    pub fn get(&self, key: &ComponentKey) -> Option<StoredBytes> {
        match (self, key) {
            (Self::Files(map), ComponentKey::FileId(key)) => map.get(&FileKey(*key)),
            (Self::Paths(map), ComponentKey::Path(key)) => map.get(key),
            (Self::Records(map), ComponentKey::RecordSlot(key)) => map.get(&RecordKey(*key)),
            (Self::Identities(map), ComponentKey::Identity(key)) => map.get(key),
            (Self::Domains(map), ComponentKey::Domain(key)) => map.get(&StateKey(*key)),
            (Self::Values(map), ComponentKey::Scalar(key)) => map.get(key),
            (Self::Lengths(map), ComponentKey::Length(key)) => map.get(key),
            (Self::States(map), ComponentKey::State(key)) => map.get(&StateKey(*key)),
            (Self::Normals(map), ComponentKey::Gram(key)) => map.get(&GramKey(*key)),
            (Self::Skips(map), ComponentKey::Gram(key)) => map.get(&GramKey(*key)),
            (Self::Sorts(map), ComponentKey::Sort(key)) => map.get(key),
            (Self::Occurrences(map), ComponentKey::Occurrence(key)) => map.get(key),
            (Self::RelationTargets(map), ComponentKey::RelationTarget(key)) => map.get(key),
            (Self::RelationStates(map), ComponentKey::RelationState(key)) => {
                map.get(&StateKey(*key))
            }
            (tree, key) => wrong_key(tree.kind(), key),
        }
    }

    pub fn remove(&mut self, key: &ComponentKey) -> Option<StoredBytes> {
        match (self, key) {
            (Self::Files(map), ComponentKey::FileId(key)) => map.remove(&FileKey(*key)),
            (Self::Paths(map), ComponentKey::Path(key)) => map.remove(key),
            (Self::Records(map), ComponentKey::RecordSlot(key)) => map.remove(&RecordKey(*key)),
            (Self::Identities(map), ComponentKey::Identity(key)) => map.remove(key),
            (Self::Domains(map), ComponentKey::Domain(key)) => map.remove(&StateKey(*key)),
            (Self::Values(map), ComponentKey::Scalar(key)) => map.remove(key),
            (Self::Lengths(map), ComponentKey::Length(key)) => map.remove(key),
            (Self::States(map), ComponentKey::State(key)) => map.remove(&StateKey(*key)),
            (Self::Normals(map), ComponentKey::Gram(key)) => map.remove(&GramKey(*key)),
            (Self::Skips(map), ComponentKey::Gram(key)) => map.remove(&GramKey(*key)),
            (Self::Sorts(map), ComponentKey::Sort(key)) => map.remove(key),
            (Self::Occurrences(map), ComponentKey::Occurrence(key)) => map.remove(key),
            (Self::RelationTargets(map), ComponentKey::RelationTarget(key)) => map.remove(key),
            (Self::RelationStates(map), ComponentKey::RelationState(key)) => {
                map.remove(&StateKey(*key))
            }
            (tree, key) => wrong_key(tree.kind(), key),
        }
    }

    pub fn iter(&self) -> ComponentTreeIter<'_> {
        match self {
            Self::Files(map) => ComponentTreeIter::Files(map.iter()),
            Self::Paths(map) => ComponentTreeIter::Paths(map.iter()),
            Self::Records(map) => ComponentTreeIter::Records(map.iter()),
            Self::Identities(map) => ComponentTreeIter::Identities(map.iter()),
            Self::Domains(map) => ComponentTreeIter::Domains(map.iter()),
            Self::Values(map) => ComponentTreeIter::Values(map.iter()),
            Self::Lengths(map) => ComponentTreeIter::Lengths(map.iter()),
            Self::States(map) => ComponentTreeIter::States(map.iter()),
            Self::Normals(map) => ComponentTreeIter::Grams(map.iter()),
            Self::Skips(map) => ComponentTreeIter::Grams(map.iter()),
            Self::Sorts(map) => ComponentTreeIter::Sorts(map.iter()),
            Self::Occurrences(map) => ComponentTreeIter::Occurrences(map.iter()),
            Self::RelationTargets(map) => ComponentTreeIter::RelationTargets(map.iter()),
            Self::RelationStates(map) => ComponentTreeIter::RelationStates(map.iter()),
        }
    }

    pub fn range(
        &self,
        range: (Bound<ComponentKey>, Bound<ComponentKey>),
    ) -> ComponentTreeIter<'_> {
        let (lower, upper) = range;
        macro_rules! range {
            ($map:expr, $convert:expr) => {
                $map.range((map_bound(lower, $convert), map_bound(upper, $convert)))
            };
        }
        match self {
            Self::Files(map) => ComponentTreeIter::Files(range!(map, |key| match key {
                ComponentKey::FileId(value) => FileKey(value),
                _ => panic!("file bound variant"),
            })),
            Self::Paths(map) => ComponentTreeIter::Paths(range!(map, |key| match key {
                ComponentKey::Path(value) => value,
                _ => panic!("path bound variant"),
            })),
            Self::Records(map) => ComponentTreeIter::Records(range!(map, |key| match key {
                ComponentKey::RecordSlot(value) => RecordKey(value),
                _ => panic!("record bound variant"),
            })),
            Self::Identities(map) => ComponentTreeIter::Identities(range!(map, |key| match key {
                ComponentKey::Identity(value) => value,
                _ => panic!("identity bound variant"),
            })),
            Self::Domains(map) => ComponentTreeIter::Domains(range!(map, |key| match key {
                ComponentKey::Domain(value) => StateKey(value),
                _ => panic!("domain bound variant"),
            })),
            Self::Values(map) => ComponentTreeIter::Values(range!(map, |key| match key {
                ComponentKey::Scalar(value) => value,
                _ => panic!("scalar bound variant"),
            })),
            Self::Lengths(map) => ComponentTreeIter::Lengths(range!(map, |key| match key {
                ComponentKey::Length(value) => value,
                _ => panic!("length bound variant"),
            })),
            Self::States(map) => ComponentTreeIter::States(range!(map, |key| match key {
                ComponentKey::State(value) => StateKey(value),
                _ => panic!("state bound variant"),
            })),
            Self::Normals(map) | Self::Skips(map) => {
                let iter = range!(map, |key| match key {
                    ComponentKey::Gram(value) => GramKey(value),
                    _ => panic!("gram bound variant"),
                });
                match self {
                    Self::Normals(_) => ComponentTreeIter::Grams(iter),
                    Self::Skips(_) => ComponentTreeIter::Grams(iter),
                    _ => unreachable!(),
                }
            }
            Self::Sorts(map) => ComponentTreeIter::Sorts(range!(map, |key| match key {
                ComponentKey::Sort(value) => value,
                _ => panic!("sort bound variant"),
            })),
            Self::Occurrences(map) => {
                ComponentTreeIter::Occurrences(range!(map, |key| match key {
                    ComponentKey::Occurrence(value) => value,
                    _ => panic!("occurrence bound variant"),
                }))
            }
            Self::RelationTargets(map) => {
                ComponentTreeIter::RelationTargets(range!(map, |key| match key {
                    ComponentKey::RelationTarget(value) => value,
                    _ => panic!("relation target bound variant"),
                }))
            }
            Self::RelationStates(map) => {
                ComponentTreeIter::RelationStates(range!(map, |key| match key {
                    ComponentKey::RelationState(value) => StateKey(value),
                    _ => panic!("relation state bound variant"),
                }))
            }
        }
    }

    pub fn into_memory(self) -> IndexMemory {
        with_memory!(self, map => map.into_memory())
    }
}

fn map_bound<K>(bound: Bound<ComponentKey>, convert: impl Fn(ComponentKey) -> K) -> Bound<K> {
    match bound {
        Bound::Included(key) => Bound::Included(convert(key)),
        Bound::Excluded(key) => Bound::Excluded(convert(key)),
        Bound::Unbounded => Bound::Unbounded,
    }
}

pub struct ComponentEntry {
    key: ComponentKey,
    value: StoredBytes,
}

impl ComponentEntry {
    pub fn key(&self) -> &ComponentKey {
        &self.key
    }

    pub fn value(&self) -> StoredBytes {
        self.value.clone()
    }

    pub fn into_parts(self) -> (ComponentKey, StoredBytes) {
        (self.key, self.value)
    }
}

pub enum ComponentTreeIter<'a> {
    Files(Iter<'a, FileKey, StoredBytes, IndexMemory>),
    Paths(Iter<'a, PathKey, StoredBytes, IndexMemory>),
    Records(Iter<'a, RecordKey, StoredBytes, IndexMemory>),
    Identities(Iter<'a, RecordIdentityKey, StoredBytes, IndexMemory>),
    Domains(Iter<'a, StateKey, StoredBytes, IndexMemory>),
    Values(Iter<'a, ScalarKey, StoredBytes, IndexMemory>),
    Lengths(Iter<'a, Number, StoredBytes, IndexMemory>),
    States(Iter<'a, StateKey, StoredBytes, IndexMemory>),
    Grams(Iter<'a, GramKey, StoredBytes, IndexMemory>),
    Sorts(Iter<'a, SortEntryKey, StoredBytes, IndexMemory>),
    Occurrences(Iter<'a, RelationOccurrenceKey, StoredBytes, IndexMemory>),
    RelationTargets(Iter<'a, RelationTargetKey, StoredBytes, IndexMemory>),
    RelationStates(Iter<'a, StateKey, StoredBytes, IndexMemory>),
}

impl Iterator for ComponentTreeIter<'_> {
    type Item = ComponentEntry;

    fn next(&mut self) -> Option<Self::Item> {
        macro_rules! next {
            ($iter:expr, $key:expr) => {
                $iter.next().map(|entry| ComponentEntry {
                    key: $key(entry.key().clone()),
                    value: entry.value(),
                })
            };
        }
        match self {
            Self::Files(iter) => next!(iter, |key: FileKey| ComponentKey::FileId(key.0)),
            Self::Paths(iter) => next!(iter, |key: PathKey| ComponentKey::Path(key)),
            Self::Records(iter) => next!(iter, |key: RecordKey| ComponentKey::RecordSlot(key.0)),
            Self::Identities(iter) => {
                next!(iter, |key: RecordIdentityKey| ComponentKey::Identity(key))
            }
            Self::Domains(iter) => next!(iter, |key: StateKey| ComponentKey::Domain(key.0)),
            Self::Values(iter) => next!(iter, |key: ScalarKey| ComponentKey::Scalar(key)),
            Self::Lengths(iter) => next!(iter, |key: Number| ComponentKey::Length(key)),
            Self::States(iter) => next!(iter, |key: StateKey| ComponentKey::State(key.0)),
            Self::Grams(iter) => next!(iter, |key: GramKey| ComponentKey::Gram(key.0)),
            Self::Sorts(iter) => next!(iter, |key: SortEntryKey| ComponentKey::Sort(key)),
            Self::Occurrences(iter) => {
                next!(iter, |key: RelationOccurrenceKey| ComponentKey::Occurrence(
                    key
                ))
            }
            Self::RelationTargets(iter) => {
                next!(iter, |key: RelationTargetKey| ComponentKey::RelationTarget(
                    key
                ))
            }
            Self::RelationStates(iter) => {
                next!(iter, |key: StateKey| ComponentKey::RelationState(key.0))
            }
        }
    }
}

impl DoubleEndedIterator for ComponentTreeIter<'_> {
    fn next_back(&mut self) -> Option<Self::Item> {
        macro_rules! next_back {
            ($iter:expr, $key:expr) => {
                $iter.next_back().map(|entry| ComponentEntry {
                    key: $key(entry.key().clone()),
                    value: entry.value(),
                })
            };
        }
        match self {
            Self::Files(iter) => next_back!(iter, |key: FileKey| ComponentKey::FileId(key.0)),
            Self::Paths(iter) => next_back!(iter, |key: PathKey| ComponentKey::Path(key)),
            Self::Records(iter) => {
                next_back!(iter, |key: RecordKey| ComponentKey::RecordSlot(key.0))
            }
            Self::Identities(iter) => {
                next_back!(iter, |key: RecordIdentityKey| ComponentKey::Identity(key))
            }
            Self::Domains(iter) => next_back!(iter, |key: StateKey| ComponentKey::Domain(key.0)),
            Self::Values(iter) => next_back!(iter, |key: ScalarKey| ComponentKey::Scalar(key)),
            Self::Lengths(iter) => next_back!(iter, |key: Number| ComponentKey::Length(key)),
            Self::States(iter) => next_back!(iter, |key: StateKey| ComponentKey::State(key.0)),
            Self::Grams(iter) => next_back!(iter, |key: GramKey| ComponentKey::Gram(key.0)),
            Self::Sorts(iter) => next_back!(iter, |key: SortEntryKey| ComponentKey::Sort(key)),
            Self::Occurrences(iter) => {
                next_back!(iter, |key: RelationOccurrenceKey| ComponentKey::Occurrence(
                    key
                ))
            }
            Self::RelationTargets(iter) => {
                next_back!(iter, |key: RelationTargetKey| ComponentKey::RelationTarget(
                    key
                ))
            }
            Self::RelationStates(iter) => {
                next_back!(iter, |key: StateKey| ComponentKey::RelationState(key.0))
            }
        }
    }
}

/// Create a fresh empty tree for one role.
pub fn new_tree(kind: ComponentKind) -> ComponentTree {
    ComponentTree::new(kind)
}

pub fn tree_payload(tree: ComponentTree) -> Vec<u8> {
    tree.into_memory().into_inner()
}

pub fn tree_image(tree: ComponentTree) -> (Vec<u8>, usize, usize) {
    let memory = tree.into_memory().into_inner();
    let capacity = memory.capacity();
    let len = memory.len();
    (memory, len, capacity)
}

pub fn load_tree(kind: ComponentKind, payload: Vec<u8>) -> Result<ComponentTree, String> {
    validate_tree_payload(&payload)?;
    let memory = RefCell::new(payload);
    Ok(match kind {
        ComponentKind::Files => ComponentTree::Files(BTreeMap::load(memory)),
        ComponentKind::Paths => ComponentTree::Paths(BTreeMap::load(memory)),
        ComponentKind::Records => ComponentTree::Records(BTreeMap::load(memory)),
        ComponentKind::Identities => ComponentTree::Identities(BTreeMap::load(memory)),
        ComponentKind::Domains => ComponentTree::Domains(BTreeMap::load(memory)),
        ComponentKind::Values => ComponentTree::Values(BTreeMap::load(memory)),
        ComponentKind::Lengths => ComponentTree::Lengths(BTreeMap::load(memory)),
        ComponentKind::States => ComponentTree::States(BTreeMap::load(memory)),
        ComponentKind::NormalGrams => ComponentTree::Normals(BTreeMap::load(memory)),
        ComponentKind::SkipGrams => ComponentTree::Skips(BTreeMap::load(memory)),
        ComponentKind::SortAscending | ComponentKind::SortDescending => {
            ComponentTree::Sorts(BTreeMap::load(memory))
        }
        ComponentKind::RelationOccurrences => ComponentTree::Occurrences(BTreeMap::load(memory)),
        ComponentKind::RelationTargets => ComponentTree::RelationTargets(BTreeMap::load(memory)),
        ComponentKind::RelationStates => ComponentTree::RelationStates(BTreeMap::load(memory)),
    })
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

const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<ComponentTree>();
    assert_send::<StoredBytes>();
    assert_send::<IndexMemory>();
};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_memory_round_trip_preserves_and_mutates() {
        let mut tree = new_tree(ComponentKind::Records);
        tree.insert(ComponentKey::RecordSlot(7), StoredBytes::new(vec![11]));
        let image = tree_payload(tree);
        assert_eq!(image.len() % MEMORY_PAGE_SIZE, 0);
        let mut restored = load_tree(ComponentKind::Records, image).expect("valid image");
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
        assert!(load_tree(ComponentKind::Records, Vec::new()).is_err());
        let mut legacy = vec![0; MEMORY_PAGE_SIZE];
        legacy[0..3].copy_from_slice(b"BTR");
        legacy[3] = 1;
        assert!(load_tree(ComponentKind::Records, legacy).is_err());
        assert!(load_tree(ComponentKind::Records, vec![0; 17]).is_err());
    }
}
