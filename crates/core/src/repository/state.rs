//! Assembled repository collection state.

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::index::bigram::BigramIndex;
use crate::index::memory::inference::InferenceState;
use crate::index::posting::{CustomPostingIndex, PostingCollection};
use crate::index::relation::RelationIndex;
use crate::index::sorting::SortCollection;
use crate::inference::{InferencePathDiagnostic, InferenceReport};
use crate::inventory::CollectionGeneration;
use crate::model::{RecordId, RecordSlot};
use crate::source::registry::PreparedSource;

use super::configuration::{CollectionDeclaration, CollectionDefinition, FileSourceConfig};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct GlobalInferenceSummary {
    pub(crate) report: InferenceReport,
    pub(crate) diagnostics: Vec<InferencePathDiagnostic>,
}

pub(crate) struct InitialCollectionCounts {
    pub fallback_records: usize,
    pub structural_membership_bytes: usize,
}

pub struct CollectionState {
    /// Repository instance owning this pinned collection snapshot. Segment
    /// adapters use it to construct a full local-domain identity.
    pub(crate) repository_instance: u64,
    pub definition: CollectionDeclaration,
    /// Present only when this collection has a configured file importer.
    pub(crate) file_source: Option<FileSourceConfig>,
    /// Compatibility boundary for the existing file preparation pipeline.
    pub(crate) file_definition: Option<CollectionDefinition>,
    pub(crate) source: Option<PreparedSource>,
    pub(crate) generation: CollectionGeneration,
    pub(crate) postings: PostingCollection,
    pub(crate) inference: Option<InferenceState>,
    pub(crate) global_inference: Option<GlobalInferenceSummary>,
    pub(crate) custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub(crate) raw: BigramIndex,
    pub(crate) fallback_text: BigramIndex,
    pub(crate) logical_records: RoaringBitmap,
    pub(crate) empty_array_records: RoaringBitmap,
    pub(crate) custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    pub(crate) text_bindings: crate::index::storage::text::TextBindingCatalog,
    pub(crate) sorting: SortCollection,
    /// Cached narrow gram/posting/sort estimate for status polling.
    pub(crate) index_family_bytes: usize,
    pub(crate) id_lookup: HashMap<RecordId, RecordSlot>,
    pub(crate) identity_order: Vec<RecordSlot>,
    pub stats: CollectionStats,
    pub(crate) initial_counts: Option<InitialCollectionCounts>,
    pub(crate) relations: Vec<RelationIndex>,
}

impl CollectionState {
    pub(crate) fn text_access_policy(&self) -> crate::index::storage::text::TextAccessPolicy<'_> {
        crate::index::storage::text::TextAccessPolicy {
            schema: &self.definition.schema,
            bindings: &self.text_bindings,
            logical_text_complete: self.source.as_ref().is_some_and(|source| {
                source.reader().logical_text_coverage()
                    == crate::source::LogicalTextCoverage::AllStringFields
            }),
        }
    }

    pub(crate) fn relation_reader(
        &self,
        ordinal: usize,
    ) -> Option<&dyn crate::index::storage::relation::RelationReader> {
        self.relations
            .get(ordinal)
            .map(|relation| relation as &dyn crate::index::storage::relation::RelationReader)
    }

    pub(crate) fn order_column(
        &self,
        path: &crate::model::FieldPath,
    ) -> crate::index::storage::transfer::StorageResult<
        Option<&dyn crate::index::storage::order::OrderColumnReader>,
    > {
        Ok(self.sorting.reader(path))
    }

    /// Counts for the base collection before segment overlays.
    pub fn base_file_count(&self) -> usize {
        self.stats.file_count
    }

    pub fn base_record_count(&self) -> usize {
        self.stats.record_count
    }

    pub fn base_fallback_record_count(&self) -> usize {
        if let Some(counts) = &self.initial_counts {
            return counts.fallback_records;
        }
        self.generation
            .records
            .iter()
            .filter(|record| record.class == crate::inventory::RecordClass::RawFallback)
            .count()
    }

    #[doc(hidden)]
    pub fn base_empty_array_membership_bytes(&self) -> usize {
        self.initial_counts.as_ref().map_or_else(
            || self.empty_array_records.serialized_size(),
            |counts| counts.structural_membership_bytes,
        )
    }

    /// Field paths registered in the base collection's declared postings.
    pub fn declared_posting_paths(&self) -> impl Iterator<Item = &[String]> + '_ {
        self.postings.fields.keys().map(Vec::as_slice)
    }

    /// Summaries of inferred dictionaries in the base collection.
    pub fn base_inferred_postings(
        &self,
    ) -> Option<impl Iterator<Item = InferredPostingSummary<'_>> + '_> {
        self.inference.as_ref().map(|state| {
            state
                .indexes
                .iter()
                .map(|(path, index)| InferredPostingSummary {
                    path,
                    keys: index.values.len(),
                    memberships: index.values.values().map(|slots| slots.len()).sum(),
                    covered_records: index.covered_records,
                    min_key: index.stats.min_key.as_ref(),
                    max_key: index.stats.max_key.as_ref(),
                })
        })
    }

    /// File importer configuration, when this collection has one.
    pub fn file_source(&self) -> Option<&FileSourceConfig> {
        self.file_source.as_ref()
    }

    /// Existing file adapter paths only. Direct record paths inspect the
    /// optional registration and never request file discovery settings.
    pub(crate) fn required_file_source(&self) -> &FileSourceConfig {
        self.file_source
            .as_ref()
            .expect("file source configuration required by file adapter")
    }

    pub(crate) fn required_file_definition(&self) -> &CollectionDefinition {
        self.file_definition
            .as_ref()
            .expect("file definition required by file adapter")
    }

    pub(crate) fn required_file_reader(&self) -> &PreparedSource {
        self.source
            .as_ref()
            .expect("file reader required by file adapter")
    }
}

/// Cardinalities for one inferred value dictionary.
pub struct InferredPostingSummary<'a> {
    pub path: &'a [String],
    pub keys: usize,
    pub memberships: u64,
    pub covered_records: u32,
    pub min_key: Option<&'a crate::index::posting::ScalarKey>,
    pub max_key: Option<&'a crate::index::posting::ScalarKey>,
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
