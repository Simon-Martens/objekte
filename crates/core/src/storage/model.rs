use rkyv::{Archive, Deserialize as ArchiveDeserialize, Serialize as ArchiveSerialize};
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::bigram::BigramIndex;
use crate::inference::{InferenceObservation, InferenceStateSnapshot};
use crate::inventory::{CollectionGeneration, FileEntry, FileInventory, FileVersion, RecordEntry};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::native_segment::{SegmentAccounting, SegmentId};
use crate::posting::{PostingCollectionSnapshot, PostingIndexSnapshot};
use crate::relation::RelationContribution;
use crate::repository::{CollectionStats, GlobalInferenceSummary};
use crate::sorting::SortCollection;
use crate::storage::format::sha256;

pub(crate) const SNAPSHOT_SCHEMA_VERSION: u32 = 2;
pub(crate) const METADATA_CHUNK_BYTES: usize = 8 * 1024 * 1024;

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct MetadataManifest {
    pub schema_version: u32,
    pub total_length: u64,
    pub chunks: Vec<StoredMetadataChunk>,
}

#[derive(Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct MetadataChunk {
    pub bytes: Vec<u8>,
}

#[derive(Clone, Archive, ArchiveSerialize, ArchiveDeserialize)]
pub(crate) struct StoredMetadataChunk {
    pub digest: [u8; 32],
    pub length: u64,
}

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
    #[serde(with = "crate::storage::serde_support::btree_map_sequence")]
    pub custom_postings: BTreeMap<(Vec<String>, String), StoredCustomPosting>,
    pub raw: BigramIndex,
    pub fallback_text: BigramIndex,
    pub logical_records: RoaringBitmap,
    pub empty_array_records: RoaringBitmap,
    #[serde(with = "crate::storage::serde_support::btree_map_sequence")]
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
    #[serde(with = "crate::storage::serde_support::btree_map_sequence")]
    pub custom_postings: BTreeMap<(Vec<String>, String), StoredCustomPosting>,
    pub raw: BigramIndex,
    pub fallback_text: BigramIndex,
    #[serde(with = "crate::storage::serde_support::btree_map_sequence")]
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
    if snapshot.catalog_slots.len() < snapshot.files.files.len()
        || snapshot.files.files.len() > u32::MAX as usize
    {
        return Err(format!(
            "snapshot file catalogue length mismatch: files={} catalog_slots={}",
            snapshot.files.files.len(),
            snapshot.catalog_slots.len()
        ));
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
            slot.as_ref().is_some_and(|slot| {
                slot.entry.id.0 as usize != ordinal
                    || slot.entry.id.0 as usize >= snapshot.catalog_slots.len()
            })
        })
    {
        return Err("snapshot catalogue row identity mismatch".into());
    }
    for (collection_index, (collection, active)) in snapshot
        .collections
        .iter()
        .zip(&snapshot.base_active)
        .enumerate()
    {
        let record_count = collection.generation.records.len();
        if active.max().is_some_and(|slot| slot as usize >= record_count) {
            return Err(format!(
                "base liveness for collection {collection_index} exceeds its record domain"
            ));
        }
        if collection.id_lookup.iter().any(|(_, slot)| slot.0 as usize >= record_count) {
            return Err(format!(
                "base ID lookup for collection {collection_index} points outside its record domain"
            ));
        }
    }
    for (collection_index, group) in snapshot.segments.iter().enumerate() {
        for published in group {
            let segment = &published.segment;
            if segment.collection.0 as usize != collection_index {
                return Err("segment collection route does not match its catalog group".into());
            }
            let record_count = segment.records.len();
            if segment
                .records
                .iter()
                .enumerate()
                .any(|(ordinal, record)| record.slot.0 as usize != ordinal)
            {
                return Err("segment record slots are not dense and ordered".into());
            }
            if published
                .active
                .max()
                .is_some_and(|slot| slot as usize >= record_count)
            {
                return Err("segment liveness exceeds its record domain".into());
            }
            if segment.id_lookup.iter().any(|(_, slot)| slot.0 as usize >= record_count) {
                return Err("segment ID lookup points outside its record domain".into());
            }
            if segment.identity_order.iter().any(|slot| slot.0 as usize >= record_count) {
                return Err("segment identity order points outside its record domain".into());
            }
            if segment
                .file_index
                .iter()
                .any(|(_, ordinal)| *ordinal as usize >= segment.files.len())
            {
                return Err("segment file lookup points outside its file domain".into());
            }
            if segment
                .files
                .iter()
                .flat_map(|file| &file.records)
                .any(|slot| slot.0 as usize >= record_count)
            {
                return Err("segment file membership points outside its record domain".into());
            }
            if segment
                .records
                .iter()
                .any(|record| record.file.0 as usize >= segment.files.len())
            {
                return Err("segment record points outside its file domain".into());
            }
        }
    }
    Ok(())
}
