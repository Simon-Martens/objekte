//! Flat worker output consumed by the single redb admission coordinator.

use roaring::RoaringBitmap;
use serde::Serialize;
use std::sync::Arc;

use super::value_codec::{BindingDescriptor, FieldCoverage, GramKind, RelationStateKind};
use crate::build::{BatchFile, BuildWorkProfile};
use crate::inventory::RecordEntry;
use crate::model::{FieldPath, Number};
use crate::posting::ScalarKey;
use crate::sorting::SortKey;

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedBinding {
    pub(crate) descriptor: BindingDescriptor,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedValue {
    pub(crate) slot: u32,
    pub(crate) binding: u32,
    pub(crate) value: ScalarKey,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedLength {
    pub(crate) slot: u32,
    pub(crate) binding: u32,
    pub(crate) length: Number,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedState {
    pub(crate) slot: u32,
    pub(crate) binding: u32,
    pub(crate) mask: u16,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedSort {
    pub(crate) slot: u32,
    pub(crate) path: FieldPath,
    pub(crate) key: SortKey,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedRelation {
    pub(crate) slot: u32,
    pub(crate) path: Vec<String>,
    pub(crate) occurrence_ordinal: u32,
    pub(crate) target_collection: String,
    pub(crate) target_id: String,
    pub(crate) positions: Vec<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedRelationState {
    pub(crate) slot: u32,
    pub(crate) path: Vec<String>,
    pub(crate) state: RelationStateKind,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct PreparedGramSet {
    pub(crate) slot: u32,
    pub(crate) binding: u32,
    pub(crate) kind: GramKind,
    pub(crate) grams: Vec<u64>,
}

pub(crate) struct PreparedDescriptors {
    pub(crate) bindings: Vec<PreparedBinding>,
    pub(crate) coverage: Vec<(u32, FieldCoverage)>,
}

pub(crate) struct PreparedStoreBatch {
    pub(crate) files: Vec<BatchFile>,
    pub(crate) file_fingerprints: Vec<(crate::model::FileId, [u8; 32])>,
    pub(crate) descriptors: Arc<PreparedDescriptors>,
    pub(crate) records: Vec<RecordEntry>,
    pub(crate) values: Vec<PreparedValue>,
    pub(crate) lengths: Vec<PreparedLength>,
    pub(crate) states: Vec<PreparedState>,
    pub(crate) sorts: Vec<PreparedSort>,
    pub(crate) relations: Vec<PreparedRelation>,
    pub(crate) relation_states: Vec<PreparedRelationState>,
    pub(crate) grams: Vec<PreparedGramSet>,
    pub(crate) inference: Option<crate::inference::InferenceBuilder>,
    pub(crate) logical_records: RoaringBitmap,
    pub(crate) empty_array_records: RoaringBitmap,
    pub(crate) record_parses: u64,
    pub(crate) failed_parses: u64,
    pub(crate) source_reads: u64,
    pub(crate) source_bytes_read: u64,
    pub(crate) profile: BuildWorkProfile,
    pub(crate) retained_capacity_bytes: usize,
}

impl crate::build::ScheduledBatch for PreparedStoreBatch {
    fn retained_capacity_bytes(&self) -> usize {
        self.retained_capacity_bytes
    }
}
