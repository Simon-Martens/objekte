//! Assembled repository collection state.

use std::borrow::Cow;
use std::collections::HashMap;

#[cfg(test)]
use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};

use crate::index::bigram::BigramIndex;
use crate::index::memory::inference::InferenceState;
use crate::index::posting::{CustomPostingIndex, PostingCollection};
use crate::index::relation::RelationIndex;
#[cfg(test)]
use crate::index::sorting::SortCollection;
use crate::index::storage::order::declared::DeclaredOrders;
#[cfg(test)]
use crate::index::storage::segment::FILE_SLOT_LIMIT;
use crate::index::storage::segment::{
    RecordDirectoryEstimate, RecordOrigin, SegmentHandle, SegmentReader, SegmentSlotFact,
    SegmentSlotMembers, any_segment_file, read_slot_members, visit_checked_file_slots,
};
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::inference::{InferencePathDiagnostic, InferenceReport};
#[cfg(test)]
use crate::inventory::CollectionGeneration;
#[cfg(test)]
use crate::inventory::RecordClass;
use crate::inventory::{ActiveRecords, RecordEntry};
#[cfg(test)]
use crate::model::RecordId;
use crate::model::{CollectionId, FileId, RecordSlot};
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
    pub(crate) id: CollectionId,
    pub(crate) base_generation: u64,
    pub definition: CollectionDeclaration,
    /// Present only when this collection has a configured file importer.
    pub(crate) file_source: Option<FileSourceConfig>,
    /// Compatibility boundary for the existing file preparation pipeline.
    pub(crate) file_definition: Option<CollectionDefinition>,
    pub(crate) source: Option<PreparedSource>,
    pub(crate) base_records: Option<SegmentHandle>,
    #[cfg(test)]
    pub(crate) generation: CollectionGeneration,
    pub(crate) postings: PostingCollection,
    pub(crate) inference: Option<InferenceState>,
    pub(crate) global_inference: Option<GlobalInferenceSummary>,
    pub(crate) custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub(crate) raw: BigramIndex,
    pub(crate) fallback_text: BigramIndex,
    #[cfg(test)]
    pub(crate) logical_records: RoaringBitmap,
    #[cfg(test)]
    pub(crate) empty_array_records: RoaringBitmap,
    pub(crate) custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    pub(crate) text_bindings: crate::index::storage::text::TextBindingCatalog,
    pub(crate) declared_orders: DeclaredOrders,
    #[cfg(test)]
    pub(crate) sorting: SortCollection,
    /// Cached narrow gram/posting/sort estimate for status polling.
    pub(crate) index_family_bytes: usize,
    #[cfg(test)]
    pub(crate) id_lookup: HashMap<RecordId, RecordSlot>,
    #[cfg(test)]
    pub(crate) identity_order: Vec<RecordSlot>,
    pub stats: CollectionStats,
    pub(crate) initial_counts: Option<InitialCollectionCounts>,
    pub(crate) relations: Vec<RelationIndex>,
}

impl CollectionState {
    pub(crate) fn collection_id(&self) -> CollectionId {
        self.id
    }

    pub(crate) fn base_generation(&self) -> u64 {
        self.base_generation
    }

    pub(crate) fn base_slot_count(&self) -> usize {
        if let Some(base) = &self.base_records {
            return base.descriptor().slots;
        }
        #[cfg(test)]
        return self.generation.records.len();
        #[cfg(not(test))]
        0
    }

    pub(crate) fn base_slot_for_record(&self, id: &str) -> StorageResult<Option<RecordSlot>> {
        let slot = if let Some(base) = &self.base_records {
            base.slot_for_record(id)?
        } else {
            #[cfg(test)]
            {
                self.id_lookup.get(id).copied()
            }
            #[cfg(not(test))]
            None
        };
        if slot.is_some_and(|slot| slot.0 as usize >= self.base_slot_count()) {
            return Err(StorageError::invalid(
                "read base record route",
                "record ID points outside the collection base",
            ));
        }
        Ok(slot)
    }

    pub(crate) fn base_record(&self, slot: RecordSlot) -> StorageResult<Cow<'_, RecordEntry>> {
        const OP: &str = "read base record";
        let invalid_slot = || StorageError::invalid(OP, "invalid record slot");
        let check_slot = |actual| {
            if actual != slot {
                return Err(StorageError::invalid(OP, "record slot mismatch"));
            }
            Ok(())
        };
        if let Some(base) = &self.base_records {
            let header = base.header(slot)?.ok_or_else(invalid_slot)?;
            check_slot(header.slot)?;
            let Some(RecordOrigin::FileRange(file)) = base.origin(slot)? else {
                return Err(StorageError::invalid(
                    OP,
                    "expected a file-backed base record",
                ));
            };
            let range = file
                .range
                .ok_or_else(|| StorageError::invalid(OP, "record origin has no byte range"))?;
            let header = header.into_owned();
            return Ok(Cow::Owned(RecordEntry {
                slot,
                id: header.id,
                file: file.file,
                range,
                revision: file.revision,
                class: header.class,
                diagnostic_ref: header.diagnostic_ref,
            }));
        }
        #[cfg(test)]
        {
            let record = self
                .generation
                .records
                .get(slot.0 as usize)
                .ok_or_else(invalid_slot)?;
            check_slot(record.slot)?;
            return Ok(Cow::Borrowed(record));
        }
        #[cfg(not(test))]
        Err(invalid_slot())
    }

    pub(crate) fn base_contains_file(&self, file: FileId) -> StorageResult<bool> {
        if let Some(base) = &self.base_records {
            return base.contains_file(file);
        }
        #[cfg(test)]
        return Ok(self.generation.files.contains(&file));
        #[cfg(not(test))]
        Ok(false)
    }

    pub(crate) fn any_base_file<E: From<StorageError>>(
        &self,
        mut matches: impl FnMut(FileId) -> Result<bool, E>,
    ) -> Result<bool, E> {
        if let Some(base) = &self.base_records {
            return any_segment_file(base.as_ref(), |file| matches(file.id));
        }
        #[cfg(test)]
        for &file in &self.generation.files {
            if matches(file)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    pub(crate) fn visit_base_file_slots(
        &self,
        file: FileId,
        visit: &mut dyn FnMut(&[RecordSlot]) -> StorageResult<()>,
    ) -> StorageResult<()> {
        if let Some(base) = &self.base_records {
            return visit_checked_file_slots(base.as_ref(), file, visit);
        }
        #[cfg(test)]
        {
            const OP: &str = "read base file membership";
            let maximum = FILE_SLOT_LIMIT.fixed_batch_len(OP, size_of::<RecordSlot>())?;
            if let Some(slots) = self.generation.file_records.records_for(file) {
                for batch in slots.chunks(maximum) {
                    if batch
                        .iter()
                        .any(|slot| slot.0 as usize >= self.base_slot_count())
                    {
                        return Err(StorageError::invalid(
                            OP,
                            "file slot outside the collection base",
                        ));
                    }
                    visit(batch)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn base_record_estimate(&self) -> RecordDirectoryEstimate {
        if let Some(base) = &self.base_records {
            return base.record_directory_estimate();
        }
        #[cfg(test)]
        return RecordDirectoryEstimate {
            record_table_capacity: self.generation.records.capacity(),
            unique_id_payload_bytes: self
                .generation
                .records
                .iter()
                .map(|record| record.id.0.len() + 2 * size_of::<usize>())
                .sum(),
            identity_lookup_capacity: self.id_lookup.capacity(),
            shared_id_references: self.id_lookup.len(),
        };
        #[cfg(not(test))]
        RecordDirectoryEstimate::default()
    }

    pub(crate) fn active_base_fallback_count(&self, active: &ActiveRecords) -> u64 {
        if active.len() == 0 {
            return 0;
        }
        if let Some(base) = &self.base_records {
            return base.active_fallback_count(active);
        }
        #[cfg(test)]
        return self
            .generation
            .records
            .iter()
            .filter(|record| {
                record.class == RecordClass::RawFallback && active.contains(record.slot)
            })
            .count() as u64;
        #[cfg(not(test))]
        0
    }

    pub(crate) fn base_slot_members(
        &self,
        fact: SegmentSlotFact,
    ) -> StorageResult<SegmentSlotMembers<'_>> {
        if let Some(base) = &self.base_records {
            return read_slot_members(base, fact);
        }
        #[cfg(test)]
        {
            let facts = match fact {
                SegmentSlotFact::Logical => &self.logical_records,
                SegmentSlotFact::EmptyArray => &self.empty_array_records,
            };
            if !facts.is_empty() {
                return SegmentSlotMembers::checked(
                    crate::query_access::membership::SlotSet::borrowed(facts),
                    self.base_slot_count(),
                );
            }
        }
        Ok(SegmentSlotMembers::empty())
    }

    pub(crate) fn base_logical_count(&self) -> StorageResult<u64> {
        Ok(self
            .base_slot_members(SegmentSlotFact::Logical)?
            .view()
            .len() as u64)
    }

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

    pub(crate) fn relation_declaration(
        &self,
        ordinal: u32,
    ) -> Option<&crate::schema::relations::RelationDefinition> {
        self.definition
            .schema
            .relations()
            .values()
            .nth(ordinal as usize)
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
        if let Some(base) = &self.base_records {
            let column = base.order_column(path)?;
            if column.is_some_and(|column| {
                column.path() != path || column.key_count() != base.descriptor().slots
            }) {
                return Err(StorageError::invalid(
                    "read base order column",
                    "invalid path or slot coverage",
                ));
            }
            return Ok(column);
        }
        #[cfg(test)]
        if let Some(order) = self.sorting.reader(path) {
            return Ok(Some(order));
        }
        Ok(self.declared_orders.column(path))
    }

    pub(crate) fn base_identity_order(
        &self,
    ) -> StorageResult<&dyn crate::index::storage::ordered_slots::OrderedSlots> {
        if let Some(base) = &self.base_records {
            if base.descriptor().slots == 0 {
                return Ok(&crate::index::storage::ordered_slots::EMPTY_ORDER);
            }
            let order = base.ordered_slots(&[], None)?.ok_or_else(|| {
                StorageError::invalid("read base record order", "identity order missing")
            })?;
            if order.len() != base.descriptor().slots {
                return Err(StorageError::invalid(
                    "read base record order",
                    "invalid slot coverage",
                ));
            }
            return Ok(order);
        }
        #[cfg(test)]
        return Ok(&self.identity_order);
        #[cfg(not(test))]
        Ok(&crate::index::storage::ordered_slots::EMPTY_ORDER)
    }

    pub(crate) fn base_composite_order(
        &self,
        fields: &[crate::query::options::SortField],
    ) -> StorageResult<Option<&dyn crate::index::storage::ordered_slots::OrderedSlots>> {
        if let Some(base) = &self.base_records {
            let order = base.ordered_slots(fields, None)?;
            if order.is_some_and(|order| order.len() != base.descriptor().slots) {
                return Err(StorageError::invalid(
                    "read base composite order",
                    "invalid slot coverage",
                ));
            }
            return Ok(order);
        }
        #[cfg(test)]
        if let Some(order) = self.sorting.composite(fields) {
            return Ok(Some(&order.order));
        }
        Ok(self.declared_orders.composite(fields))
    }

    pub(crate) fn base_index_family_estimate(&self) -> crate::lifecycle::IndexFamilyEstimate {
        if let Some(base) = &self.base_records {
            return base.index_family_estimate();
        }
        crate::index::memory::segment::statistics::index_family_estimate(
            &self.raw,
            &self.fallback_text,
            &self.custom_text,
            &self.postings,
            #[cfg(test)]
            Some(&self.sorting),
            #[cfg(not(test))]
            None,
        )
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
        if let Some(base) = &self.base_records {
            let all = ActiveRecords::full_selection(base.descriptor().slots)
                .expect("sealed base local slot count");
            return base.active_fallback_count(&all) as usize;
        }
        #[cfg(test)]
        return self
            .generation
            .records
            .iter()
            .filter(|record| record.class == RecordClass::RawFallback)
            .count();
        #[cfg(not(test))]
        0
    }

    #[doc(hidden)]
    pub fn base_empty_array_membership_bytes(&self) -> usize {
        if let Some(counts) = &self.initial_counts {
            return counts.structural_membership_bytes;
        }
        if let Some(base) = &self.base_records {
            return base.empty_array_membership_bytes();
        }
        #[cfg(test)]
        return self.empty_array_records.serialized_size();
        #[cfg(not(test))]
        0
    }

    /// Field paths registered in the base collection's declared postings.
    pub fn declared_posting_paths(&self) -> impl Iterator<Item = &[String]> + '_ {
        self.definition.schema.fields().keys().map(Vec::as_slice)
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
