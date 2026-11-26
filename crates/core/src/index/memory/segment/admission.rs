//! Owned batch-to-native-segment admission.

use std::collections::HashMap;

use rayon::ThreadPool;
use roaring::RoaringBitmap;

use crate::construction::batch::BatchContribution;
use crate::construction::batch::canonical_custom_text_key;
use crate::index::memory::bigram::BigramAlphabet;
use crate::index::memory::bigram::placement::{GramAggregate, PlacedGrams};
use crate::index::memory::inference::InferenceBuilder;
use crate::index::memory::inventory::append_shifted_bitmap;
use crate::index::memory::posting::placement::{PostingBatches, PostingWriters};
use crate::index::memory::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::index::memory::relation::placement::{PlacedRelations, RelationAggregate};
use crate::index::memory::segment::{
    IndexedCollectionSegment, SegmentAccounting, SegmentFile, SegmentId, SegmentPolicy,
};
use crate::index::memory::sorting::SortBuilder;
use crate::index::memory::worker::NativeIndexBatch;
use crate::index::storage::posting::PostingStore;
use crate::index::storage::relation::RelationStore;
use crate::index::storage::segment::build::{BuiltSegment, SegmentBuilder, SegmentSeal};
use crate::index::storage::text::{GramBinding, GramStore};
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::inference::InferenceConfig;
use crate::inventory::{DiagnosticRef, FileStatus, RecordClass, RecordEntry};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::repository::configuration::CollectionPolicy;
use crate::repository::statistics::{AdmissionBreakdown, IncrementalSegmentProfile};
use crate::source::registry::PreparedSource;

#[cfg(test)]
use crate::construction::batch::SourceFileInput;
#[cfg(test)]
use crate::construction::file_preparation::PreparedCollectionBuild;

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
    pub records: Vec<crate::records::RecordHeader>,
    pub backing: crate::index::memory::segment::SegmentBacking,
    write_versions: super::record_versions::RecordWriteVersions,
    pub raw_covers_logical_fields: bool,
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
    placed_postings: PostingWriters,
    inference: Option<InferenceBuilder>,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: GramAggregate,
    fallback_text: GramAggregate,
    custom_text: HashMap<(Vec<String>, String), GramAggregate>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sorting: SortBuilder,
    relations: Vec<RelationAggregate>,
}

impl SegmentAggregate {
    fn new(
        definition: &dyn CollectionPolicy,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
        posting_store: Option<std::sync::Arc<dyn PostingStore>>,
        gram_store: Option<std::sync::Arc<dyn GramStore>>,
        relation_store: Option<std::sync::Arc<dyn RelationStore>>,
    ) -> Result<Self, String> {
        let placed = PostingBatches::new(definition.schema(), posting_store);
        let custom_postings = definition
            .schema()
            .operations
            .iter()
            .filter(|(key, binding)| {
                binding.operation.posting() && !placed.has(&key.0, Some(&key.1))
            })
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
                let key = canonical_custom_text_key(definition.schema(), &key.0, &key.1);
                let writer = GramAggregate::new(
                    alphabet,
                    GramBinding::Field(key.0.clone(), key.1.clone()),
                    gram_store.as_ref(),
                )?;
                Ok((key, writer))
            })
            .collect::<StorageResult<_>>()?;
        Ok(Self {
            postings: PostingCollection::new_filtered(definition.schema(), |path| {
                !placed.has(path, None)
            }),
            placed_postings: PostingWriters::from_batch(&placed)?,
            postings_pristine: true,
            inference: InferenceBuilder::for_collection(inference_config, collection_count),
            custom_postings,
            raw: GramAggregate::new(alphabet, GramBinding::Raw, gram_store.as_ref())?,
            fallback_text: GramAggregate::new(
                alphabet,
                GramBinding::Fallback,
                gram_store.as_ref(),
            )?,
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
            relations: definition
                .schema()
                .relations()
                .values()
                .map(|definition| RelationAggregate::new(definition, relation_store.as_ref()))
                .collect::<StorageResult<_>>()?,
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
            placed_postings,
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
        let NativeIndexBatch {
            postings: batch_postings,
            placed_postings: batch_placed,
            inference: batch_inference,
            custom_postings: batch_custom_postings,
            raw: batch_raw,
            gram_store: _,
            fallback_text: batch_fallback_text,
            custom_text: batch_custom_text,
            logical_records: batch_logical_records,
            empty_array_records: batch_empty_array_records,
            sort_keys: batch_sort_keys,
            relations: batch_relations,
            relation_store: _,
        } = *NativeIndexBatch::take(batch.indexes)?;
        placed_postings.push(batch_placed, base, count)?;
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
        // A file's initial source range and its first range in this draft
        // differ when one file spans more than one segment.
        let new_files = batch
            .files
            .iter()
            .filter(|file| !self.file_index.contains_key(&file.entry.id))
            .count();
        self.policy.can_grow(
            self.records.len(),
            self.files.len(),
            batch.record_count(),
            new_files,
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
    #[cfg(test)]
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
        let mut records = Vec::with_capacity(self.records.len());
        let mut locators = Vec::with_capacity(self.records.len());
        for record in self.records {
            locators.push(crate::index::memory::segment::FileRangeLocator {
                file: record.file,
                range: Some(record.range),
                revision: record.revision,
            });
            records.push(crate::records::RecordHeader {
                slot: record.slot,
                id: record.id,
                class: record.class,
                diagnostic_ref: record.diagnostic_ref,
            });
        }
        CollectionSegment {
            id: self.id,
            collection: self.collection,
            source_id: self.source_id,
            source_version: self.source_version,
            files: self.files,
            file_index: self.file_index,
            records,
            backing: crate::index::memory::segment::SegmentBacking::FileRange { locators },
            write_versions: Default::default(),
            raw_covers_logical_fields: true,
            id_lookup: self.id_lookup,
            batches: self.batches,
            record_parses: self.record_parses,
            failed_parses: self.failed_parses,
        }
    }
}

impl SegmentBuilder for SegmentAdmission {
    fn file_count(&self) -> usize {
        self.file_count()
    }

    fn record_count(&self) -> usize {
        self.records.len()
    }

    fn source_bytes(&self) -> u64 {
        self.files
            .iter()
            .map(|file| file.entry.revision.size)
            .fold(0, u64::saturating_add)
    }

    fn at_capacity(&self) -> bool {
        self.at_capacity()
    }

    fn can_admit(&self, batch: &BatchContribution) -> StorageResult<bool> {
        Ok(self.can_admit_batch(batch))
    }

    fn admit(
        &mut self,
        batch: BatchContribution,
        profile: Option<&mut AdmissionBreakdown>,
    ) -> StorageResult<()> {
        self.admit_batch_profiled(batch, profile)
            .map_err(|error| StorageError::invalid("admit file segment", error))
    }

    fn seal(self: Box<Self>, input: SegmentSeal<'_>) -> StorageResult<BuiltSegment> {
        self.finish().seal(input)
    }
}

impl CollectionSegment {
    pub(crate) fn seal(mut self, input: SegmentSeal<'_>) -> StorageResult<BuiltSegment> {
        self.id = input.id;
        let (index, observation) = self
            .into_indexed_profiled_inner(
                input.definition,
                input.options.bigram_alphabet,
                input.options.inference,
                input.collection_count,
                input.pool,
                input.profile,
                input.observe_inference,
                input.declared_orders,
            )
            .map_err(|error| StorageError::invalid("seal segment", error))?;
        Ok(BuiltSegment {
            index: std::sync::Arc::new(index),
            observation: observation.map(|value| {
                std::sync::Arc::new(value)
                    as crate::index::storage::inference::InferenceObservationHandle
            }),
        })
    }

    /// Admit one typed batch as an ordinary native segment. The logical
    /// contribution uses the same finalization and index validation as files.
    pub(crate) fn from_retained(
        id: SegmentId,
        collection: CollectionId,
        source_id: String,
        source_version: u32,
        registration: u32,
        raw_covers_logical_fields: bool,
        records: Vec<crate::records::RecordHeader>,
        bodies: crate::index::storage::segment::RetainedBlock,
        contribution: BatchContribution,
    ) -> Result<Self, String> {
        if records.is_empty() || records.len() != bodies.len() {
            return Err("retained batch is empty or has mismatched body count".into());
        }
        let count = u32::try_from(records.len()).map_err(|_| "too many retained records")?;
        let backing = crate::index::memory::segment::SegmentBacking::Retained {
            blocks: vec![bodies],
            addresses: (0..count)
                .map(|offset| crate::index::memory::segment::RetainedAddress { block: 0, offset })
                .collect(),
            links: vec![None; count as usize],
            registration,
        };
        Self::from_native(
            id,
            collection,
            source_id,
            source_version,
            raw_covers_logical_fields,
            records,
            backing,
            contribution,
        )
    }

    pub(crate) fn from_provider(
        id: SegmentId,
        collection: CollectionId,
        source_id: String,
        source_version: u32,
        registration: u32,
        raw_covers_logical_fields: bool,
        records: Vec<crate::records::RecordHeader>,
        versions: Vec<crate::records::RecordVersion>,
        contribution: BatchContribution,
    ) -> Result<Self, String> {
        if records.is_empty() || records.len() != versions.len() {
            return Err("provider batch is empty or has mismatched version count".into());
        }
        let backing = crate::index::memory::segment::SegmentBacking::Provider {
            versions,
            links: vec![None; records.len()],
            registration,
        };
        Self::from_native(
            id,
            collection,
            source_id,
            source_version,
            raw_covers_logical_fields,
            records,
            backing,
            contribution,
        )
    }

    fn from_native(
        id: SegmentId,
        collection: CollectionId,
        source_id: String,
        source_version: u32,
        raw_covers_logical_fields: bool,
        records: Vec<crate::records::RecordHeader>,
        backing: crate::index::memory::segment::SegmentBacking,
        contribution: BatchContribution,
    ) -> Result<Self, String> {
        let count = u32::try_from(records.len()).map_err(|_| "too many native records")?;
        let mut id_lookup = HashMap::with_capacity(records.len());
        for (slot, record) in records.iter().enumerate() {
            if record.slot.0 as usize != slot {
                return Err("retained record slots out of order".into());
            }
            if id_lookup.insert(record.id.clone(), record.slot).is_some() {
                return Err(format!(
                    "duplicate record ID in retained segment: {}",
                    record.id.0
                ));
            }
        }
        Ok(Self {
            id,
            collection,
            source_id,
            source_version,
            files: Vec::new(),
            file_index: HashMap::new(),
            records,
            backing,
            write_versions: super::record_versions::RecordWriteVersions::new(count as usize)?,
            raw_covers_logical_fields,
            id_lookup,
            batches: vec![SegmentBatch {
                base: 0,
                count,
                contribution,
            }],
            record_parses: 0,
            failed_parses: 0,
        })
    }

    #[cfg(test)]
    pub fn into_indexed(
        self,
        definition: &dyn CollectionPolicy,
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

    #[cfg(test)]
    pub fn into_indexed_profiled(
        self,
        definition: &dyn CollectionPolicy,
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
            None,
        )
        .map(|(index, _)| index)
    }

    #[cfg(test)]
    pub(crate) fn into_indexed_observation_profiled(
        self,
        definition: &dyn CollectionPolicy,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
        pool: &ThreadPool,
        profile: Option<&mut IncrementalSegmentProfile>,
    ) -> Result<
        (
            IndexedCollectionSegment,
            Option<crate::index::memory::inference::InferenceObservation>,
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
            None,
        )
    }

    fn into_indexed_profiled_inner(
        self,
        definition: &dyn CollectionPolicy,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
        pool: &ThreadPool,
        mut profile: Option<&mut IncrementalSegmentProfile>,
        capture_observation: bool,
        declared_orders: Option<
            std::sync::Arc<dyn crate::index::storage::order::DeclaredOrderStore>,
        >,
    ) -> Result<
        (
            IndexedCollectionSegment,
            Option<crate::index::memory::inference::InferenceObservation>,
        ),
        String,
    > {
        let phase = crate::measurement::timer::start_if_enabled(profile.is_some());
        let first_indexes = self
            .batches
            .first()
            .map(|batch| batch.contribution.indexes.as_ref());
        let posting_store = first_indexes.and_then(|indexes| indexes.posting_store());
        let mut aggregate = SegmentAggregate::new(
            definition,
            alphabet,
            inference_config,
            collection_count,
            posting_store.clone(),
            first_indexes.and_then(|indexes| indexes.gram_store()),
            first_indexes.and_then(|indexes| indexes.relation_store()),
        )?;
        if let Some(store) = declared_orders {
            aggregate.sorting.select_store(store)?;
        }
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
            placed_postings,
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
        let mut placed_relations = PlacedRelations::default();
        let relations = relations
            .into_iter()
            .enumerate()
            .map(|(ordinal, relation)| relation.seal(ordinal, &mut placed_relations))
            .collect::<StorageResult<_>>()?;
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
        let mut placed_grams = PlacedGrams::default();
        let raw = raw.seal(pool, &mut placed_grams)?;
        let fallback_text = fallback_text.seal(pool, &mut placed_grams)?;
        let custom_text = custom_text
            .into_iter()
            .map(|(key, index)| {
                index
                    .seal(pool, &mut placed_grams)
                    .map(|index| (key, index))
            })
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
        sorting.refresh_file_status_for_records(&self.records, |slot| {
            let file = self.backing.file_locator(slot)?.file;
            Some(
                self.files
                    .iter()
                    .find(|entry| entry.entry.id == file)
                    .map_or(FileStatus::Valid.code(), |entry| entry.entry.status.code()),
            )
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
                    posting_store.as_ref(),
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
        let backing = self.backing;
        let mut index = IndexedCollectionSegment {
            id: self.id,
            collection: self.collection,
            source_id: self.source_id,
            source_version: self.source_version,
            files: self.files,
            file_index: self.file_index,
            records: self.records,
            backing,
            write_versions: self.write_versions,
            raw_covers_logical_fields: self.raw_covers_logical_fields,
            id_lookup: self.id_lookup,
            identity_order,
            postings: postings.into(),
            placed_postings: placed_postings.finish(record_count, logical_count as u64)?,
            inference,
            custom_postings,
            raw: raw.into(),
            placed_grams,
            fallback_text: fallback_text.into(),
            custom_text,
            logical_records,
            empty_array_records,
            sorting: sorting.into(),
            relations,
            placed_relations,
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
