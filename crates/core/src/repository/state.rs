//! Assembled repository collection state.

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::index::bigram::BigramIndex;
use crate::index::posting::{CustomPostingIndex, PostingCollection};
use crate::index::relation::RelationIndex;
use crate::index::sorting::SortCollection;
use crate::inference::{InferencePathDiagnostic, InferenceReport, InferenceState};
use crate::inventory::CollectionGeneration;
use crate::model::{RecordId, RecordSlot};
use crate::source::registry::PreparedSource;

use super::configuration::CollectionDefinition;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct GlobalInferenceSummary {
    pub(crate) report: InferenceReport,
    pub(crate) diagnostics: Vec<InferencePathDiagnostic>,
}

pub struct CollectionState {
    /// Repository instance owning this pinned collection snapshot. Segment
    /// adapters use it to construct a full local-domain identity.
    pub(crate) repository_instance: u64,
    pub definition: CollectionDefinition,
    pub(crate) source: PreparedSource,
    pub generation: CollectionGeneration,
    pub postings: PostingCollection,
    pub inference: Option<InferenceState>,
    pub(crate) global_inference: Option<GlobalInferenceSummary>,
    pub custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: BigramIndex,
    pub fallback_text: BigramIndex,
    pub logical_records: RoaringBitmap,
    pub empty_array_records: RoaringBitmap,
    pub custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    pub(crate) text_bindings: crate::query_access::catalog::TextBindingCatalog,
    pub sorting: SortCollection,
    /// Cached narrow gram/posting/sort estimate for status polling.
    pub(crate) index_family_bytes: usize,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub identity_order: Vec<RecordSlot>,
    pub stats: CollectionStats,
    pub relations: Vec<RelationIndex>,
}

/// Collection size summary used by query planning.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub struct CollectionStats {
    pub file_count: usize,
    pub record_count: usize,
    pub total_file_bytes: u64,
}

impl CollectionStats {
    pub fn mean_record_bytes(self) -> f64 {
        if self.record_count == 0 {
            0.0
        } else {
            self.total_file_bytes as f64 / self.record_count as f64
        }
    }

    pub fn mean_records_per_file(self) -> f64 {
        if self.file_count == 0 {
            0.0
        } else {
            self.record_count as f64 / self.file_count as f64
        }
    }
}
