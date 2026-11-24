//! Owned batch-to-native-segment admission.

use std::collections::HashMap;

use rayon::ThreadPool;
use roaring::RoaringBitmap;
use serde::Serialize;

use crate::construction::batch::BatchContribution;
use crate::construction::batch::canonical_custom_text_key;
use crate::construction::inference::InferenceBuilder;
use crate::index::bigram::BigramAlphabet;
use crate::index::bigram::BigramContribution;
use crate::index::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::index::relation::RelationContribution;
use crate::index::segment::{
    IndexedCollectionSegment, SegmentAccounting, SegmentFile, SegmentId, SegmentPolicy,
};
use crate::index::sorting::SortBuilder;
use crate::inference::InferenceConfig;
use crate::inventory::{
    CollectionBuilder, DiagnosticRef, FileInventory, FileStatus, RecordClass, RecordEntry,
    append_shifted_bitmap,
};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::repository::configuration::CollectionPolicy;
use crate::repository::statistics::IncrementalSegmentProfile;
use crate::source::registry::PreparedSource;

#[cfg(test)]
use super::batch::SourceFileInput;
#[cfg(test)]
use super::file_preparation::PreparedCollectionBuild;

#[allow(
    dead_code,
    reason = "retained as the collection-wide admission reference path"
)]
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

#[allow(
    dead_code,
    reason = "retained as the collection-wide admission reference path"
)]
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
        let started = crate::measurement::timer::start_if_enabled(profile.is_some());
        let base = self.builder.next_slot()?.0;
        let count = u32::try_from(batch.records.len()).map_err(|_| "too many batch records")?;
        base.checked_add(count).ok_or("record slot overflow")?;
        let mut records = batch.records.into_iter();
        for batch_file in batch.files {
            let file_id = batch_file.entry.id;
            let path = self
                .inventory
                .required_root()
                .join(&batch_file.entry.relative_path);
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
        let started = crate::measurement::timer::start_if_enabled(profile.is_some());
        self.postings.merge_shifted(batch.postings, base)?;
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.postings_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::measurement::timer::start_if_enabled(profile.is_some());
        match (self.inference.as_mut(), batch.inference) {
            (Some(global), Some(local)) => global.merge_shifted(local, base)?,
            (None, None) => {}
            _ => return Err("inference batch mode mismatch".into()),
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.inference_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::measurement::timer::start_if_enabled(profile.is_some());
        if self.relations.len() != batch.relations.len() {
            return Err("relation batch coverage mismatch".into());
        }
        for (global, local) in self.relations.iter_mut().zip(batch.relations) {
            global.merge_shifted(local, base, count)?;
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.relations_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::measurement::timer::start_if_enabled(profile.is_some());
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
        let started = crate::measurement::timer::start_if_enabled(profile.is_some());
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
        let started = crate::measurement::timer::start_if_enabled(profile.is_some());
        append_shifted_bitmap(self.logical_records, &batch.logical_records, base)?;
        append_shifted_bitmap(self.empty_array_records, &batch.empty_array_records, base)?;
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.structural_facts_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::measurement::timer::start_if_enabled(profile.is_some());
        self.sorting.append_keys(batch.sort_keys, base, count)?;
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.sorting_ns += started.elapsed().as_nanos() as u64;
        }
        Ok(())
    }
}

/// The preparation path can admit several files before sealing one segment.
/// This avoids a permanent one-segment-per-file layout.
pub(crate) struct SegmentAdmission {
    id: SegmentId,
    collection: CollectionId,
    source_id: String,
    source_version: u32,
    files: Vec<SegmentFile>,
    file_index: HashMap<FileId, usize>,
    records: Vec<RecordEntry>,
    id_lookup: HashMap<RecordId, RecordSlot>,
    batches: Vec<SegmentBatch>,
    policy: SegmentPolicy,
    record_parses: u64,
    failed_parses: u64,
}

/// Owned, locally slotted contribution set. `into_indexed` will merge and
/// finalize every index family before this is published by the coordinator.
pub(crate) struct CollectionSegment {
    pub id: SegmentId,
    pub collection: CollectionId,
    pub source_id: String,
    pub source_version: u32,
    pub files: Vec<SegmentFile>,
    pub file_index: HashMap<FileId, usize>,
    pub records: Vec<RecordEntry>,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub(crate) batches: Vec<SegmentBatch>,
    pub record_parses: u64,
    pub failed_parses: u64,
}

pub(crate) struct SegmentBatch {
    base: u32,
    count: u32,
    contribution: BatchContribution,
}

struct SegmentAggregate {
    postings: PostingCollection,
    postings_pristine: bool,
    inference: Option<InferenceBuilder>,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramContribution,
    fallback_text: BigramContribution,
    custom_text: HashMap<(Vec<String>, String), BigramContribution>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sorting: SortBuilder,
    relations: Vec<RelationContribution>,
}

impl SegmentAggregate {
    fn new(
        definition: &impl CollectionPolicy,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
    ) -> Result<Self, String> {
        let custom_postings = definition
            .schema()
            .operations
            .iter()
            .filter(|(_, binding)| binding.operation.posting())
            .map(|(key, binding)| {
                (
                    key.clone(),
                    CustomPostingIndex {
                        index: PostingIndex::new(&definition.schema().fields[&key.0]),
                        complete: binding.operation.complete_coverage(),
                    },
                )
            })
            .collect();
        let custom_text = definition
            .schema()
            .custom
            .keys()
            .chain(
                definition
                    .schema()
                    .operations
                    .iter()
                    .filter(|(_, binding)| {
                        binding.operation.bigram() && binding.operation.complete_coverage()
                    })
                    .map(|(key, _)| key),
            )
            .map(|key| {
                (
                    canonical_custom_text_key(definition.schema(), &key.0, &key.1),
                    BigramContribution::new(alphabet),
                )
            })
            .collect();
        Ok(Self {
            postings: PostingCollection::new(definition.schema()),
            postings_pristine: true,
            inference: InferenceBuilder::for_collection(inference_config, collection_count),
            custom_postings,
            raw: BigramContribution::new(alphabet),
            fallback_text: BigramContribution::new(alphabet),
            custom_text,
            logical_records: RoaringBitmap::new(),
            empty_array_records: RoaringBitmap::new(),
            sorting: SortBuilder::new(
                definition.sort_fields(),
                definition.schema(),
                definition.sort_locale(),
                definition.sort_options(),
                definition.sort_overrides(),
            )?,
            relations: RelationContribution::for_schema(definition.schema()),
        })
    }

    fn admit_parallel(
        &mut self,
        batch: BatchContribution,
        base: u32,
        count: u32,
        pool: &ThreadPool,
        profile: Option<&mut IncrementalSegmentProfile>,
    ) -> Result<(), String> {
        let Self {
            postings,
            postings_pristine,
            inference,
            custom_postings,
            raw,
            fallback_text,
            custom_text,
            logical_records,
            empty_array_records,
            sorting,
            relations,
        } = self;
        let BatchContribution {
            postings: batch_postings,
            inference: batch_inference,
            custom_postings: batch_custom_postings,
            raw: batch_raw,
            fallback_text: batch_fallback_text,
            custom_text: batch_custom_text,
            logical_records: batch_logical_records,
            empty_array_records: batch_empty_array_records,
            sort_keys: batch_sort_keys,
            relations: batch_relations,
            ..
        } = batch;
        let transfer_postings = base == 0 && *postings_pristine;
        if count != 0 {
            *postings_pristine = false;
        }
        let postings_and_custom = pool.join(
            || {
                if transfer_postings {
                    postings.transfer_pristine(batch_postings)?;
                    if let Some(profile) = profile {
                        let keys = postings.fields.len()
                            + postings
                                .fields
                                .values()
                                .map(|index| index.values.len() + index.lengths.len())
                                .sum::<usize>();
                        let members = postings
                            .fields
                            .values()
                            .flat_map(|index| {
                                [&index.presence]
                                    .into_iter()
                                    .chain(index.values.values())
                                    .chain(index.lengths.values())
                            })
                            .map(RoaringBitmap::len)
                            .sum::<u64>();
                        profile.zero_base_posting_transfers += 1;
                        profile.transferred_posting_keys += keys as u64;
                        profile.transferred_posting_members += members;
                    }
                    Ok(())
                } else {
                    postings.merge_shifted_in_pool(batch_postings, base, pool)
                }
            },
            || -> Result<(), String> {
                if custom_postings.len() != batch_custom_postings.len() {
                    return Err("custom posting segment coverage mismatch".into());
                }
                for (key, local) in batch_custom_postings {
                    let global = custom_postings
                        .get_mut(&key)
                        .ok_or("custom posting segment missing")?;
                    global.index.merge_shifted(local.index, base)?;
                    global.complete &= local.complete;
                }
                Ok(())
            },
        );
        let inference_and_domains = pool.join(
            || match (inference.as_mut(), batch_inference) {
                (Some(global), Some(local)) => global.merge_shifted(local, base),
                (None, None) => Ok(()),
                _ => Err("inference segment mode mismatch".into()),
            },
            || -> Result<(), String> {
                append_shifted_bitmap(logical_records, &batch_logical_records, base)?;
                append_shifted_bitmap(empty_array_records, &batch_empty_array_records, base)
            },
        );
        let bigrams = pool.join(
            || -> Result<(), String> {
                raw.merge_shifted(batch_raw, base)?;
                fallback_text.merge_shifted(batch_fallback_text, base)
            },
            || -> Result<(), String> {
                if custom_text.len() != batch_custom_text.len() {
                    return Err("custom bigram segment coverage mismatch".into());
                }
                for (key, index) in batch_custom_text {
                    custom_text
                        .get_mut(&key)
                        .ok_or("custom bigram segment missing")?
                        .merge_shifted(index, base)?;
                }
                Ok(())
            },
        );
        let sorting_and_relations = pool.join(
            || sorting.append_keys(batch_sort_keys, base, count),
            || -> Result<(), String> {
                if relations.len() != batch_relations.len() {
                    return Err("relation segment coverage mismatch".into());
                }
                for (global, local) in relations.iter_mut().zip(batch_relations) {
                    global.merge_shifted(local, base, count)?;
                }
                Ok(())
            },
        );
        postings_and_custom.0?;
        postings_and_custom.1?;
        inference_and_domains.0?;
        inference_and_domains.1?;
        bigrams.0?;
        bigrams.1?;
        sorting_and_relations.0?;
        sorting_and_relations.1
    }
}

/// Ordinary coalescing target. One oversized physical file remains eligible
/// as a singleton, matching fresh build's largest-file allowance.
pub(crate) type SegmentLimits = SegmentPolicy;

impl SegmentAdmission {
    #[cfg(test)]
    pub fn new(collection: CollectionId, source: &PreparedSource) -> Self {
        Self::with_limits_and_id(
            collection,
            source,
            SegmentPolicy::default(),
            SegmentId::new(1),
        )
    }

    #[cfg(test)]
    pub fn with_limits(
        collection: CollectionId,
        source: &PreparedSource,
        policy: SegmentLimits,
    ) -> Self {
        Self::with_limits_and_id(collection, source, policy, SegmentId::new(1))
    }

    pub(crate) fn with_limits_and_id(
        collection: CollectionId,
        source: &PreparedSource,
        policy: SegmentLimits,
        id: SegmentId,
    ) -> Self {
        Self {
            id,
            collection,
            source_id: source.id().into(),
            source_version: source.version(),
            files: Vec::new(),
            file_index: HashMap::new(),
            records: Vec::new(),
            id_lookup: HashMap::new(),
            batches: Vec::new(),
            policy,
            record_parses: 0,
            failed_parses: 0,
        }
    }

    #[cfg(test)]
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    pub(crate) fn at_capacity(&self) -> bool {
        self.records.len() >= self.policy.target_records.get()
            || self.files.len() >= self.policy.max_files.get()
    }

    /// False means seal this segment and begin another. A single oversized
    /// file is admitted alone; preparation already owns its index output.
    #[cfg(test)]
    pub fn can_admit(&self, _source: &SourceFileInput, prepared: &PreparedCollectionBuild) -> bool {
        if self.files.is_empty() {
            return true;
        }
        let incoming_records = prepared
            .contributions
            .iter()
            .try_fold(0usize, |total, batch| {
                total.checked_add(batch.records.len())
            });
        self.files.len() < self.policy.max_files.get()
            && incoming_records.is_some_and(|count| {
                self.policy
                    .can_grow(self.records.len(), self.files.len(), count, 0)
            })
    }

    pub(crate) fn can_admit_batch(&self, batch: &BatchContribution) -> bool {
        if self.files.is_empty() {
            return true;
        }
        self.policy.can_grow(
            self.records.len(),
            self.files.len(),
            batch.record_count(),
            batch.new_file_count(),
        )
    }

    /// Consume one prepared collection membership. The caller retains other
    /// memberships of the same physical file for their respective segments.
    #[cfg(test)]
    pub fn admit(
        &mut self,
        source: &SourceFileInput,
        prepared: PreparedCollectionBuild,
    ) -> Result<(), String> {
        if prepared.collection != self.collection
            || prepared.source_id != self.source_id
            || prepared.source_version != self.source_version
        {
            return Err("prepared collection binding changed".into());
        }
        if self.file_index.contains_key(&source.id) {
            return Err("file already admitted to segment".into());
        }
        if !self.can_admit(source, &prepared) {
            return Err("segment capacity reached; seal before admitting another file".into());
        }
        for batch in prepared.contributions {
            if batch.files.iter().any(|file| {
                file.entry.id != source.id
                    || file.entry.relative_path.as_path() != source.relative_path.as_path()
                    || file.entry.revision != source.revision
                    || file.entry.created_ns != source.created_ns
                    || file.entry.collections != [self.collection]
            }) {
                return Err("prepared file identity changed".into());
            }
            self.admit_batch(batch)?;
        }
        Ok(())
    }

    /// Admit one ordered construction batch, including batches spanning many
    /// physical files. A fresh partition builder can seal between batches;
    /// ordinary one-file admission uses the same local slot mapping.
    pub(crate) fn admit_batch(&mut self, batch: BatchContribution) -> Result<(), String> {
        self.admit_batch_profiled(batch, None)
    }

    pub(crate) fn admit_batch_profiled(
        &mut self,
        mut batch: BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<(), String> {
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let base = u32::try_from(self.records.len()).map_err(|_| "too many segment records")?;
        let count = u32::try_from(batch.records.len()).map_err(|_| "too many batch records")?;
        base.checked_add(count)
            .ok_or("segment record slot overflow")?;
        let mut records = std::mem::take(&mut batch.records).into_iter();
        for batch_file in std::mem::take(&mut batch.files) {
            if batch_file.entry.collections != [self.collection] {
                return Err("prepared collection binding changed".into());
            }
            let file_id = batch_file.entry.id;
            let index = if batch_file.first_range || !self.file_index.contains_key(&file_id) {
                if batch_file.first_range && self.file_index.contains_key(&file_id) {
                    return Err("file has repeated first range".into());
                }
                let index = self.files.len();
                let mut entry = batch_file.entry.clone();
                entry.status = FileStatus::Valid;
                entry.diagnostics.clear();
                entry.omitted_diagnostics = 0;
                self.files.push(SegmentFile {
                    entry,
                    records: Vec::new(),
                });
                self.file_index.insert(file_id, index);
                index
            } else {
                *self
                    .file_index
                    .get(&file_id)
                    .ok_or("later range before first")?
            };
            let file = &mut self.files[index];
            file.entry.status = file.entry.status.max(batch_file.entry.status);
            file.entry.omitted_diagnostics = file
                .entry
                .omitted_diagnostics
                .saturating_add(batch_file.entry.omitted_diagnostics);
            let mut references = Vec::with_capacity(batch_file.entry.diagnostics.len());
            for mut diagnostic in batch_file.entry.diagnostics {
                if let Some(slot) = diagnostic.record {
                    diagnostic.record = Some(RecordSlot(
                        base.checked_add(slot.0).ok_or("diagnostic slot overflow")?,
                    ));
                }
                references.push(file.entry.record_diagnostic(diagnostic)?);
            }
            for expected in batch_file.records {
                let mut record = records.next().ok_or("batch record range out of bounds")?;
                if record.slot.0 as usize != expected || record.file != file_id {
                    return Err("batch record ownership or order mismatch".into());
                }
                record.slot = RecordSlot(
                    base.checked_add(record.slot.0)
                        .ok_or("segment slot overflow")?,
                );
                if record.class == RecordClass::RawFallback {
                    record.diagnostic_ref = match record.diagnostic_ref.stored_index() {
                        Some(local) => *references
                            .get(local)
                            .ok_or("fallback diagnostic reference out of bounds")?,
                        None => DiagnosticRef(u32::MAX),
                    };
                }
                if self
                    .id_lookup
                    .insert(record.id.clone(), record.slot)
                    .is_some()
                {
                    let path = self
                        .file_index
                        .get(&record.file)
                        .and_then(|index| self.files.get(*index))
                        .map(|file| file.entry.relative_path.as_path().display().to_string())
                        .unwrap_or_else(|| format!("file {}", record.file.0));
                    return Err(format!(
                        "duplicate record ID in segment: {} ({path})",
                        record.id.0
                    ));
                }
                file.records.push(record.slot);
                self.records.push(record);
            }
        }
        if records.next().is_some() || self.records.len() != (base + count) as usize {
            return Err("incomplete segment batch admission".into());
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.record_inventory_ns += started.elapsed().as_nanos() as u64;
        }
        self.record_parses = self.record_parses.saturating_add(batch.record_parses);
        self.failed_parses = self.failed_parses.saturating_add(batch.failed_parses);
        self.batches.push(SegmentBatch {
            base,
            count,
            contribution: batch,
        });
        Ok(())
    }

    pub fn finish(self) -> CollectionSegment {
        CollectionSegment {
            id: self.id,
            collection: self.collection,
            source_id: self.source_id,
            source_version: self.source_version,
            files: self.files,
            file_index: self.file_index,
            records: self.records,
            id_lookup: self.id_lookup,
            batches: self.batches,
            record_parses: self.record_parses,
            failed_parses: self.failed_parses,
        }
    }
}

impl CollectionSegment {
    pub fn into_indexed(
        self,
        definition: &impl CollectionPolicy,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
        pool: &ThreadPool,
    ) -> Result<IndexedCollectionSegment, String> {
        self.into_indexed_profiled(
            definition,
            alphabet,
            inference_config,
            collection_count,
            pool,
            None,
        )
    }

    pub fn into_indexed_profiled(
        self,
        definition: &impl CollectionPolicy,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
        pool: &ThreadPool,
        profile: Option<&mut IncrementalSegmentProfile>,
    ) -> Result<IndexedCollectionSegment, String> {
        self.into_indexed_profiled_inner(
            definition,
            alphabet,
            inference_config,
            collection_count,
            pool,
            profile,
            false,
        )
        .map(|(index, _)| index)
    }

    pub(crate) fn into_indexed_observation_profiled(
        self,
        definition: &impl CollectionPolicy,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
        pool: &ThreadPool,
        profile: Option<&mut IncrementalSegmentProfile>,
    ) -> Result<
        (
            IndexedCollectionSegment,
            Option<crate::construction::inference::InferenceObservation>,
        ),
        String,
    > {
        self.into_indexed_profiled_inner(
            definition,
            alphabet,
            inference_config,
            collection_count,
            pool,
            profile,
            true,
        )
    }

    fn into_indexed_profiled_inner(
        self,
        definition: &impl CollectionPolicy,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
        pool: &ThreadPool,
        mut profile: Option<&mut IncrementalSegmentProfile>,
        capture_observation: bool,
    ) -> Result<
        (
            IndexedCollectionSegment,
            Option<crate::construction::inference::InferenceObservation>,
        ),
        String,
    > {
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let mut aggregate =
            SegmentAggregate::new(definition, alphabet, inference_config, collection_count)?;
        for SegmentBatch {
            base,
            count,
            contribution,
        } in self.batches
        {
            aggregate.admit_parallel(contribution, base, count, pool, profile.as_deref_mut())?;
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.aggregate_merge_ns = started.elapsed().as_nanos() as u64;
        }
        let SegmentAggregate {
            mut postings,
            postings_pristine: _,
            inference,
            mut custom_postings,
            raw,
            fallback_text,
            custom_text,
            logical_records,
            empty_array_records,
            sorting,
            relations,
        } = aggregate;
        let record_count = self.records.len();
        let logical_count = logical_records.len() as usize;
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        postings.finish_stats();
        postings.validate_eligible(record_count, logical_count)?;
        if logical_count != record_count {
            postings.validate_domain(&logical_records)?;
        }
        if !(&empty_array_records - &logical_records).is_empty() {
            return Err("empty-array structural bitmap exceeds logical records".into());
        }
        for custom in custom_postings.values_mut() {
            custom.index.finish_stats();
            custom
                .index
                .validate_eligible(record_count, logical_count)?;
            if logical_count != record_count {
                custom.index.validate_domain(&logical_records)?;
            }
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.postings_finish_validation_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let raw = raw.into_index_in_pool(pool)?;
        let fallback_text = fallback_text.into_index_in_pool(pool)?;
        let custom_text = custom_text
            .into_iter()
            .map(|(key, index)| index.into_index_in_pool(pool).map(|index| (key, index)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.bigram_finish_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        raw.validate(record_count)?;
        fallback_text.validate(record_count)?;
        if logical_count != record_count {
            raw.validate_domain(&logical_records)?;
        }
        let fallback_slots: RoaringBitmap = self
            .records
            .iter()
            .filter(|record| record.class == RecordClass::RawFallback)
            .map(|record| record.slot.0)
            .collect();
        fallback_text.validate_domain(&fallback_slots)?;
        for index in custom_text.values() {
            index.validate(record_count)?;
            if logical_count != record_count {
                index.validate_domain(&logical_records)?;
            }
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.bigram_validation_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let mut sorting = sorting.finish_for_records(&self.records)?;
        sorting.refresh_file_status_for_records(&self.records, |file| {
            self.files
                .iter()
                .find(|entry| entry.entry.id == file)
                .map_or(FileStatus::Valid.code(), |entry| entry.entry.status.code())
        });
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.sorting_finish_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let (inference, inference_observation) = match inference {
            Some(builder) => {
                let (state, observation) = builder.finish_candidates_with_observation(
                    &logical_records,
                    record_count,
                    definition.schema(),
                    capture_observation,
                )?;
                (Some(state), observation)
            }
            None => (None, None),
        };
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.inference_finish_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let mut identity_order = self
            .records
            .iter()
            .map(|record| record.slot)
            .collect::<Vec<_>>();
        identity_order.sort_by(|a, b| {
            self.records[a.0 as usize]
                .id
                .cmp(&self.records[b.0 as usize].id)
        });
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.identity_order_ns = started.elapsed().as_nanos() as u64;
        }
        let backing = crate::index::segment::SegmentBacking::file_ranges(&self.records);
        let mut index = IndexedCollectionSegment {
            id: self.id,
            collection: self.collection,
            source_id: self.source_id,
            source_version: self.source_version,
            files: self.files,
            file_index: self.file_index,
            records: self.records,
            backing,
            id_lookup: self.id_lookup,
            identity_order,
            postings,
            inference,
            custom_postings,
            raw,
            fallback_text,
            custom_text,
            logical_records,
            empty_array_records,
            sorting,
            relations,
            record_parses: self.record_parses,
            failed_parses: self.failed_parses,
            accounting: SegmentAccounting::default(),
        };
        index.recompute_accounting();
        Ok((index, inference_observation))
    }
}

#[cfg(test)]
#[path = "segment_admission.test.rs"]
mod tests;
