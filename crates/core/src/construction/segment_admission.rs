//! Owned batch-to-native-segment admission.

use std::collections::HashMap;
#[cfg(feature = "detailed-profiling")]
use std::sync::atomic::AtomicU64;

use roaring::RoaringBitmap;
use serde::Serialize;

use crate::bigram::BigramContribution;
use crate::inference::InferenceBuilder;
use crate::inventory::{
    CollectionBuilder, DiagnosticRef, FileInventory, RecordClass, append_shifted_bitmap,
};
use crate::model::RecordSlot;
use crate::posting::{CustomPostingIndex, PostingCollection};
use crate::relation::RelationContribution;
use crate::sorting::SortBuilder;

use super::*;

pub(crate) struct CollectionAdmission<'a> {
    pub inventory: &'a mut FileInventory,
    pub builder: &'a mut CollectionBuilder,
    pub postings: &'a mut PostingCollection,
    pub inference: &'a mut Option<InferenceBuilder>,
    pub custom_postings: &'a mut HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: &'a mut BigramContribution,
    pub fallback_text: &'a mut BigramContribution,
    pub custom_text: &'a mut HashMap<(Vec<String>, String), BigramContribution>,
    pub logical_records: &'a mut RoaringBitmap,
    pub empty_array_records: &'a mut RoaringBitmap,
    pub sorting: &'a mut SortBuilder,
    pub relations: &'a mut Vec<RelationContribution>,
}

/// Optional aggregate timing of the ordered merge. Values from separate
/// batches are summed; `remainder_ns` includes timer and drop boundaries.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct AdmissionBreakdown {
    pub batches: u64,
    pub files: u64,
    pub records: u64,
    pub callback_ns: u64,
    pub total_ns: u64,
    /// Incremental coordinator accounting and capacity checks.
    pub coordinator_ns: u64,
    /// Collection-wide inference copy and merge outside segment admission.
    pub inference_observation_ns: u64,
    pub record_inventory_ns: u64,
    pub postings_ns: u64,
    pub inference_ns: u64,
    pub relations_ns: u64,
    pub custom_postings_ns: u64,
    pub bigrams_ns: u64,
    /// Incremental bigram merges; `bigrams_ns` is their nonadditive parent.
    pub raw_bigrams_ns: u64,
    pub fallback_bigrams_ns: u64,
    pub custom_bigrams_ns: u64,
    pub structural_facts_ns: u64,
    pub sorting_ns: u64,
    pub remainder_ns: u64,
}

impl AdmissionBreakdown {
    pub(crate) fn absorb(&mut self, other: Self) {
        self.batches += other.batches;
        self.files += other.files;
        self.records += other.records;
        self.callback_ns += other.callback_ns;
        self.total_ns += other.total_ns;
        self.coordinator_ns += other.coordinator_ns;
        self.inference_observation_ns += other.inference_observation_ns;
        self.record_inventory_ns += other.record_inventory_ns;
        self.postings_ns += other.postings_ns;
        self.inference_ns += other.inference_ns;
        self.relations_ns += other.relations_ns;
        self.custom_postings_ns += other.custom_postings_ns;
        self.bigrams_ns += other.bigrams_ns;
        self.raw_bigrams_ns += other.raw_bigrams_ns;
        self.fallback_bigrams_ns += other.fallback_bigrams_ns;
        self.custom_bigrams_ns += other.custom_bigrams_ns;
        self.structural_facts_ns += other.structural_facts_ns;
        self.sorting_ns += other.sorting_ns;
        self.remainder_ns += other.remainder_ns;
    }

    /// Exclusive incremental phases; omit the aggregate `bigrams_ns` parent.
    pub(crate) fn incremental_component_ns(&self) -> u64 {
        self.coordinator_ns
            + self.inference_observation_ns
            + self.record_inventory_ns
            + self.postings_ns
            + self.inference_ns
            + self.relations_ns
            + self.custom_postings_ns
            + self.raw_bigrams_ns
            + self.fallback_bigrams_ns
            + self.custom_bigrams_ns
            + self.structural_facts_ns
            + self.sorting_ns
    }
}

impl CollectionAdmission<'_> {
    pub(crate) fn admit_profiled(
        &mut self,
        batch: BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<(), String> {
        if let Some(profile) = profile.as_deref_mut() {
            profile.batches += 1;
            profile.files += batch.files.len() as u64;
            profile.records += batch.records.len() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        let base = self.builder.next_slot()?.0;
        let count = u32::try_from(batch.records.len()).map_err(|_| "too many batch records")?;
        base.checked_add(count).ok_or("record slot overflow")?;
        let mut records = batch.records.into_iter();
        for batch_file in batch.files {
            let file_id = batch_file.entry.id;
            let path = self.inventory.root.join(&batch_file.entry.relative_path);
            if batch_file.first_range {
                self.builder.include_file(file_id)?;
            } else if !self.builder.contains_file(file_id) {
                return Err("later record range precedes first file range".into());
            }
            let mut diagnostic_refs = Vec::with_capacity(batch_file.entry.diagnostics.len());
            for mut diagnostic in batch_file.entry.diagnostics {
                if let Some(local_slot) = diagnostic.record {
                    diagnostic.record = Some(RecordSlot(
                        base.checked_add(local_slot.0)
                            .ok_or("diagnostic record slot overflow")?,
                    ));
                }
                diagnostic_refs.push(self.inventory.record_diagnostic(file_id, diagnostic)?);
            }
            let file = &mut self.inventory.files[file_id.0 as usize];
            file.status = file.status.max(batch_file.entry.status);
            file.omitted_diagnostics = file
                .omitted_diagnostics
                .saturating_add(batch_file.entry.omitted_diagnostics);
            for expected_slot in batch_file.records {
                let local = records.next().ok_or("batch record range out of bounds")?;
                if local.slot.0 as usize != expected_slot {
                    return Err("batch record order mismatch".into());
                }
                if local.file != file_id {
                    return Err("batch record file ownership mismatch".into());
                }
                let actual = if local.class == RecordClass::Logical {
                    self.builder
                        .add_record(file, local.id, local.range.start, local.range.end)
                        .map_err(|error| {
                            format!(
                                "{}:{}..{}: {error}",
                                path.display(),
                                local.range.start,
                                local.range.end,
                            )
                        })?
                } else {
                    let reference = match local.diagnostic_ref.stored_index() {
                        Some(index) => *diagnostic_refs
                            .get(index)
                            .ok_or("batch diagnostic reference out of bounds")?,
                        None => DiagnosticRef(u32::MAX),
                    };
                    self.builder
                        .add_raw_fallback(file, local.id, local.range, reference)?
                };
                if actual.0 != base + local.slot.0 {
                    return Err("batch record slot mapping mismatch".into());
                }
            }
        }
        if records.next().is_some() {
            return Err("unadmitted batch records".into());
        }
        if self.builder.next_slot()?.0 != base + count {
            return Err("incomplete batch record admission".into());
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.record_inventory_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        self.postings.merge_shifted(batch.postings, base)?;
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.postings_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        match (self.inference.as_mut(), batch.inference) {
            (Some(global), Some(local)) => global.merge_shifted(local, base)?,
            (None, None) => {}
            _ => return Err("inference batch mode mismatch".into()),
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.inference_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        if self.relations.len() != batch.relations.len() {
            return Err("relation batch coverage mismatch".into());
        }
        for (global, local) in self.relations.iter_mut().zip(batch.relations) {
            global.merge_shifted(local, base, count)?;
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.relations_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        if self.custom_postings.len() != batch.custom_postings.len() {
            return Err("custom posting batch coverage mismatch".into());
        }
        for (key, local) in batch.custom_postings {
            let global = self
                .custom_postings
                .get_mut(&key)
                .ok_or("custom posting batch missing")?;
            global.index.merge_shifted(local.index, base)?;
            global.complete &= local.complete;
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.custom_postings_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        self.raw.merge_shifted(batch.raw, base)?;
        self.fallback_text
            .merge_shifted(batch.fallback_text, base)?;
        if self.custom_text.len() != batch.custom_text.len() {
            return Err("custom bigram batch coverage mismatch".into());
        }
        for (key, index) in batch.custom_text {
            self.custom_text
                .get_mut(&key)
                .ok_or("custom bigram batch missing")?
                .merge_shifted(index, base)?;
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.bigrams_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        append_shifted_bitmap(self.logical_records, &batch.logical_records, base)?;
        append_shifted_bitmap(self.empty_array_records, &batch.empty_array_records, base)?;
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.structural_facts_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        self.sorting.append_keys(batch.sort_keys, base, count)?;
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.sorting_ns += started.elapsed().as_nanos() as u64;
        }
        Ok(())
    }
}
