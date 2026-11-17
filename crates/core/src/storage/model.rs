use std::collections::BTreeMap;
use std::sync::Arc;

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::bigram::BigramIndex;
use crate::inference::{InferenceObservation, InferenceStateSnapshot};
use crate::inventory::{
    CollectionGeneration, FileEntry, FileInventory, FileVersion, RecordEntry,
};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::native_segment::{SegmentAccounting, SegmentId};
use crate::posting::{PostingCollectionSnapshot, PostingIndexSnapshot};
use crate::relation::RelationContribution;
use crate::repository::{CollectionStats, GlobalInferenceSummary};
use crate::sorting::SortCollection;
use crate::storage::format::sha256;

pub(crate) const SNAPSHOT_SCHEMA_VERSION: u32 = 1;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredCatalogSlot {
    pub entry: FileEntry,
    pub fingerprint: Option<[u8; 32]>,
    pub configuration: Option<[u8; 32]>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct CollectionSnapshot {
    pub name: String,
    pub generation: CollectionGeneration,
    pub postings: PostingCollectionSnapshot,
    pub inference: Option<InferenceStateSnapshot>,
    pub global_inference: Option<GlobalInferenceSummary>,
    pub custom_postings: BTreeMap<(Vec<String>, String), StoredCustomPosting>,
    pub raw: BigramIndex,
    pub fallback_text: BigramIndex,
    pub logical_records: RoaringBitmap,
    pub empty_array_records: RoaringBitmap,
    pub custom_text: BTreeMap<(Vec<String>, String), BigramIndex>,
    pub sorting: SortCollection,
    pub id_lookup: Vec<(RecordId, RecordSlot)>,
    pub identity_order: Vec<RecordSlot>,
    pub stats: CollectionStats,
    pub relations: Vec<crate::relation::RelationIndex>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredCustomPosting {
    pub index: PostingIndexSnapshot,
    pub complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredSegmentFile {
    pub entry: FileEntry,
    pub records: Vec<RecordSlot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct SegmentSnapshot {
    pub id: SegmentId,
    pub collection: CollectionId,
    pub source_id: String,
    pub source_version: u32,
    pub files: Vec<StoredSegmentFile>,
    pub records: Vec<RecordEntry>,
    pub file_index: Vec<(FileId, u32)>,
    pub id_lookup: Vec<(RecordId, RecordSlot)>,
    pub identity_order: Vec<RecordSlot>,
    pub postings: PostingCollectionSnapshot,
    pub inference_observation: Option<InferenceObservation>,
    pub inference: Option<InferenceStateSnapshot>,
    pub custom_postings: BTreeMap<(Vec<String>, String), StoredCustomPosting>,
    pub raw: BigramIndex,
    pub fallback_text: BigramIndex,
    pub custom_text: BTreeMap<(Vec<String>, String), BigramIndex>,
    pub logical_records: RoaringBitmap,
    pub empty_array_records: RoaringBitmap,
    pub sorting: SortCollection,
    pub relations: Vec<RelationContribution>,
    pub record_parses: u64,
    pub failed_parses: u64,
    pub accounting: SegmentAccounting,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct PublishedSegmentSnapshot {
    pub segment: SegmentSnapshot,
    pub active: RoaringBitmap,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct FileVersionsSnapshot {
    pub versions: Vec<FileVersion>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StorageSnapshot {
    pub schema_version: u32,
    pub logical_revision: u64,
    pub representation_generation: u64,
    pub instance_id: u64,
    pub configuration_identity: [u8; 32],
    pub source_root: String,
    pub lineage_identity: [u8; 16],
    pub complete: bool,
    pub files: FileInventory,
    pub base_fingerprints: Vec<Option<[u8; 32]>>,
    pub file_versions: FileVersionsSnapshot,
    pub catalog_slots: Vec<Option<StoredCatalogSlot>>,
    pub base_active: Vec<RoaringBitmap>,
    pub collections: Vec<CollectionSnapshot>,
    pub segments: Vec<Vec<PublishedSegmentSnapshot>>,
    pub next_segment_ordinal: u64,
    pub last_size_maintenance_revision: u64,
    pub processed_files: usize,
    pub build_parses: u64,
    pub build_failed_parses: u64,
    pub stored_objects: Vec<StoredObjectDescriptor>,
    pub counters: SnapshotCounters,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub(crate) struct SnapshotCounters {
    pub eager_file_rows: u64,
    pub eager_record_rows: u64,
    pub eager_index_descriptors: u64,
    pub eager_leaf_rows: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StoredObjectDescriptor {
    pub kind: u32,
    pub digest: [u8; 32],
    pub length: u64,
}

impl StoredObjectDescriptor {
    pub(crate) fn new(kind: u32, bytes: &[u8]) -> Self {
        Self {
            kind,
            digest: sha256(bytes),
            length: bytes.len() as u64,
        }
    }
}

pub(crate) fn validate_snapshot(snapshot: &StorageSnapshot) -> Result<(), String> {
    if snapshot.schema_version != SNAPSHOT_SCHEMA_VERSION {
        return Err("unsupported snapshot schema version".into());
    }
    if snapshot.files.files.len() != snapshot.catalog_slots.len()
        || snapshot.files.files.len() > u32::MAX as usize
    {
        return Err("snapshot file catalogue length mismatch".into());
    }
    if snapshot.base_active.len() != snapshot.collections.len()
        || snapshot.segments.len() != snapshot.collections.len()
    {
        return Err("snapshot collection domains disagree".into());
    }
    if snapshot
        .catalog_slots
        .iter()
        .enumerate()
        .any(|(ordinal, slot)| {
            slot.as_ref()
                .is_some_and(|slot| slot.entry.id.0 as usize != ordinal)
        })
    {
        return Err("snapshot catalogue row identity mismatch".into());
    }
    Ok(())
}

pub(crate) type SharedSnapshot = Arc<StorageSnapshot>;
