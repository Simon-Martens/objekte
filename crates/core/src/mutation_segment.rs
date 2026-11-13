//! Sparse, collection-local admission of prepared file contributions.

use std::collections::HashMap;

use super::*;
use crate::bigram::BigramIndex;
use crate::build::AdmissionBreakdown;
use crate::inference::{InferenceIndexStatus, InferenceState, InferredFieldReport};
use crate::inventory::ActiveRecords;
use crate::posting::StructuralFacts;
use crate::repository::IncrementalSegmentProfile;
use crate::sorting::{CompositeSortOrder, SortCollection, SortColumn, compare_composite};


/// One physical file in a delta. File IDs remain repository-global; record
/// slots are local to this segment until the publication owner names it.
pub(crate) struct SegmentFile {
    pub entry: FileEntry,
    pub records: Vec<RecordSlot>,
}


/// The preparation path can admit several files before sealing one segment.
/// This avoids a permanent one-segment-per-file layout.
pub(crate) struct SegmentAdmission {
    collection: CollectionId,
    source_id: String,
    source_version: u32,
    files: Vec<SegmentFile>,
    file_index: HashMap<FileId, usize>,
    records: Vec<RecordEntry>,
    id_lookup: HashMap<RecordId, RecordSlot>,
    batches: Vec<SegmentBatch>,
    eager_spec: Option<EagerSpec>,
    aggregate: Option<SegmentAggregate>,
    source_bytes: u64,
    limits: SegmentLimits,
    record_parses: u64,
    failed_parses: u64,
}


/// Owned, locally slotted contribution set. `into_indexed` will merge and
/// finalize every index family before this is published by the coordinator.
pub(crate) struct CollectionSegment {
    pub collection: CollectionId,
    pub source_id: String,
    pub source_version: u32,
    pub files: Vec<SegmentFile>,
    pub file_index: HashMap<FileId, usize>,
    pub records: Vec<RecordEntry>,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub(crate) batches: Vec<SegmentBatch>,
    aggregate: Option<SegmentAggregate>,
    pub record_parses: u64,
    pub failed_parses: u64,
}


pub(crate) struct SegmentBatch {
    base: u32,
    count: u32,
    contribution: BatchContribution,
}


struct EagerSpec {
    definition: Arc<CollectionDefinition>,
    alphabet: BigramAlphabet,
    inference_config: InferenceConfig,
    collection_count: usize,
}


struct SegmentAggregate {
    postings: PostingCollection,
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
        definition: &CollectionDefinition,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
    ) -> Result<Self, String> {
        let custom_postings = definition.schema.operations.iter()
            .filter(|(_, binding)| binding.operation.posting())
            .map(|(key, binding)| (key.clone(), CustomPostingIndex {
                index: PostingIndex::new(&definition.schema.fields[&key.0]),
                complete: binding.operation.complete_coverage(),
            })).collect();
        let custom_text = definition.schema.custom.keys()
            .chain(definition.schema.operations.iter()
                .filter(|(_, binding)| binding.operation.bigram() && binding.operation.complete_coverage())
                .map(|(key, _)| key))
            .map(|key| (canonical_custom_text_key(&definition.schema, &key.0, &key.1), BigramContribution::new(alphabet)))
            .collect();
        Ok(Self {
            postings: PostingCollection::new(&definition.schema),
            inference: InferenceBuilder::for_collection(inference_config, collection_count),
            custom_postings,
            raw: BigramContribution::new(alphabet),
            fallback_text: BigramContribution::new(alphabet),
            custom_text,
            logical_records: RoaringBitmap::new(),
            empty_array_records: RoaringBitmap::new(),
            sorting: SortBuilder::new(
                &definition.sort_fields, &definition.schema, &definition.sort_locale,
                definition.sort_options, &definition.sort_overrides,
            )?,
            relations: RelationContribution::for_schema(&definition.schema),
        })
    }

    fn admit(&mut self, batch: BatchContribution, base: u32, count: u32) -> Result<(), String> {
        self.admit_profiled(batch, base, count, None)
    }

    fn admit_profiled(
        &mut self,
        batch: BatchContribution,
        base: u32,
        count: u32,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<(), String> {
        let phase = profile.as_ref().map(|_| Instant::now());
        self.postings.merge_shifted(batch.postings, base)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.postings_ns += started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
        match (self.inference.as_mut(), batch.inference) {
            (Some(global), Some(local)) => global.merge_shifted(local, base)?,
            (None, None) => {}
            _ => return Err("inference segment mode mismatch".into()),
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.inference_ns += started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
        if self.relations.len() != batch.relations.len() {
            return Err("relation segment coverage mismatch".into());
        }
        for (global, local) in self.relations.iter_mut().zip(batch.relations) {
            global.merge_shifted(local, base, count)?;
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.relations_ns += started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
        if self.custom_postings.len() != batch.custom_postings.len() {
            return Err("custom posting segment coverage mismatch".into());
        }
        for (key, local) in batch.custom_postings {
            let global = self.custom_postings.get_mut(&key).ok_or("custom posting segment missing")?;
            global.index.merge_shifted(local.index, base)?;
            global.complete &= local.complete;
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.custom_postings_ns += started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
        self.raw.merge_shifted(batch.raw, base)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.raw_bigrams_ns += started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
        self.fallback_text.merge_shifted(batch.fallback_text, base)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.fallback_bigrams_ns += started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
        if self.custom_text.len() != batch.custom_text.len() {
            return Err("custom bigram segment coverage mismatch".into());
        }
        for (key, index) in batch.custom_text {
            self.custom_text.get_mut(&key).ok_or("custom bigram segment missing")?
                .merge_shifted(index, base)?;
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.custom_bigrams_ns += started.elapsed().as_nanos() as u64;
            profile.bigrams_ns = profile.raw_bigrams_ns
                + profile.fallback_bigrams_ns
                + profile.custom_bigrams_ns;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
        append_shifted_bitmap(&mut self.logical_records, &batch.logical_records, base)?;
        append_shifted_bitmap(&mut self.empty_array_records, &batch.empty_array_records, base)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.structural_facts_ns += started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
        self.sorting.append_keys(batch.sort_keys, base, count)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.sorting_ns += started.elapsed().as_nanos() as u64;
        }
        Ok(())
    }
}


/// Ordinary coalescing target. One oversized physical file remains eligible
/// as a singleton, matching fresh build's largest-file allowance.
#[derive(Clone, Copy)]
pub(crate) struct SegmentLimits {
    pub files: usize,
    pub records: usize,
    pub source_bytes: u64,
}


impl Default for SegmentLimits {
    fn default() -> Self {
        Self {
            files: 256,
            records: 16_384,
            source_bytes: 64 * 1024 * 1024,
        }
    }
}


/// Complete local search structures. The publication owner assigns a segment
/// ID and combines it with the base and other live segments in one IndexView.
pub(crate) struct IndexedCollectionSegment {
    pub collection: CollectionId,
    pub source_id: String,
    pub source_version: u32,
    pub files: Vec<SegmentFile>,
    pub file_index: HashMap<FileId, usize>,
    pub records: Vec<RecordEntry>,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub identity_order: Vec<RecordSlot>,
    pub postings: PostingCollection,
    pub inference: Option<InferenceState>,
    pub custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: BigramIndex,
    pub fallback_text: BigramIndex,
    pub custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    pub logical_records: RoaringBitmap,
    pub empty_array_records: RoaringBitmap,
    pub sorting: SortCollection,
    /// Source references retain target logical IDs for later merged-view
    /// resolution; existing `RelationIndex::resolve` needs full collections.
    pub relations: Vec<RelationContribution>,
    pub record_parses: u64,
    pub failed_parses: u64,
}


impl SegmentAdmission {
    pub fn new(collection: CollectionId, source: &PreparedSource) -> Self {
        Self::with_limits(collection, source, SegmentLimits::default())
    }

    pub fn with_limits(
        collection: CollectionId,
        source: &PreparedSource,
        limits: SegmentLimits,
    ) -> Self {
        Self {
            collection,
            source_id: source.id().into(),
            source_version: source.version(),
            files: Vec::new(),
            file_index: HashMap::new(),
            records: Vec::new(),
            id_lookup: HashMap::new(),
            batches: Vec::new(),
            eager_spec: None,
            aggregate: None,
            source_bytes: 0,
            limits,
            record_parses: 0,
            failed_parses: 0,
        }
    }

    /// Initial construction merges each completed worker contribution into
    /// the current bounded segment immediately. It never retains both all
    /// worker-local gram maps and a second full merged map at sealing time.
    pub(crate) fn with_limits_eager(
        collection: CollectionId,
        source: &PreparedSource,
        limits: SegmentLimits,
        definition: Arc<CollectionDefinition>,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
    ) -> Self {
        let mut admission = Self::with_limits(collection, source, limits);
        admission.eager_spec = Some(EagerSpec {
            definition,
            alphabet,
            inference_config,
            collection_count,
        });
        admission
    }

    #[cfg(test)]
    pub fn record_count(&self) -> usize {
        self.records.len()
    }

    pub fn file_count(&self) -> usize {
        self.files.len()
    }

    /// False means seal this segment and begin another. A single oversized
    /// file is admitted alone; preparation already owns its index output.
    pub fn can_admit(&self, source: &SourceFileInput, prepared: &PreparedCollectionBuild) -> bool {
        if self.files.is_empty() {
            return true;
        }
        let incoming_records = prepared
            .contributions
            .iter()
            .try_fold(0usize, |total, batch| {
                total.checked_add(batch.records.len())
            });
        self.files.len() < self.limits.files
            && incoming_records
                .and_then(|count| self.records.len().checked_add(count))
                .is_some_and(|count| count <= self.limits.records)
            && self
                .source_bytes
                .checked_add(source.revision.size)
                .is_some_and(|bytes| bytes <= self.limits.source_bytes)
    }

    pub(crate) fn can_admit_batch(&self, batch: &BatchContribution) -> bool {
        if self.files.is_empty() {
            return true;
        }
        self.files
            .len()
            .checked_add(batch.new_file_count())
            .is_some_and(|count| count <= self.limits.files)
            && self
                .records
                .len()
                .checked_add(batch.record_count())
                .is_some_and(|count| count <= self.limits.records)
            && self
                .source_bytes
                .checked_add(batch.new_file_bytes())
                .is_some_and(|bytes| bytes <= self.limits.source_bytes)
    }

    /// Consume one prepared collection membership. The caller retains other
    /// memberships of the same physical file for their respective segments.
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
        let phase = profile.as_ref().map(|_| Instant::now());
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
            let index = if batch_file.first_range {
                if self.file_index.contains_key(&file_id) {
                    return Err("file has repeated first range".into());
                }
                self.source_bytes = self
                    .source_bytes
                    .checked_add(batch_file.entry.revision.size)
                    .ok_or("segment source byte count overflow")?;
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
                    return Err("duplicate record ID in segment".into());
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
        if let Some(spec) = &self.eager_spec {
            if self.aggregate.is_none() {
                self.aggregate = Some(SegmentAggregate::new(
                    &spec.definition,
                    spec.alphabet,
                    spec.inference_config,
                    spec.collection_count,
                )?);
            }
            self.aggregate
                .as_mut()
                .expect("eager aggregate initialized")
                .admit_profiled(batch, base, count, profile)?;
        } else {
            self.batches.push(SegmentBatch {
                base,
                count,
                contribution: batch,
            });
        }
        Ok(())
    }

    pub fn seal(self) -> CollectionSegment {
        CollectionSegment {
            collection: self.collection,
            source_id: self.source_id,
            source_version: self.source_version,
            files: self.files,
            file_index: self.file_index,
            records: self.records,
            id_lookup: self.id_lookup,
            batches: self.batches,
            aggregate: self.aggregate,
            record_parses: self.record_parses,
            failed_parses: self.failed_parses,
        }
    }
}


impl CollectionSegment {
    pub fn into_indexed(
        self,
        definition: &CollectionDefinition,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
        pool: &ThreadPool,
    ) -> Result<IndexedCollectionSegment, String> {
        self.into_indexed_profiled(definition, alphabet, inference_config, collection_count, pool, None)
    }

    pub fn into_indexed_profiled(
        self,
        definition: &CollectionDefinition,
        alphabet: BigramAlphabet,
        inference_config: InferenceConfig,
        collection_count: usize,
        pool: &ThreadPool,
        mut profile: Option<&mut IncrementalSegmentProfile>,
    ) -> Result<IndexedCollectionSegment, String> {
        let phase = profile.as_ref().map(|_| Instant::now());
        let mut aggregate = match self.aggregate {
            Some(aggregate) => aggregate,
            None => SegmentAggregate::new(definition, alphabet, inference_config, collection_count)?,
        };
        for SegmentBatch { base, count, contribution } in self.batches {
            aggregate.admit(contribution, base, count)?;
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.aggregate_merge_ns = started.elapsed().as_nanos() as u64;
        }
        let SegmentAggregate {
            mut postings, inference, mut custom_postings, raw, fallback_text,
            custom_text, logical_records, empty_array_records, sorting, relations,
        } = aggregate;
        let record_count = self.records.len();
        let logical_count = logical_records.len() as usize;
        let phase = profile.as_ref().map(|_| Instant::now());
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
        let phase = profile.as_ref().map(|_| Instant::now());
        let raw = raw.into_index_in_pool(pool)?;
        let fallback_text = fallback_text.into_index_in_pool(pool)?;
        let custom_text = custom_text
            .into_iter()
            .map(|(key, index)| index.into_index_in_pool(pool).map(|index| (key, index)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.bigram_finish_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
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
        let phase = profile.as_ref().map(|_| Instant::now());
        let sorting = sorting.finish_for_records(&self.records)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.sorting_finish_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
        let inference = inference
            .map(|builder| builder.finish(&logical_records, record_count, &definition.schema))
            .transpose()?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.inference_finish_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = profile.as_ref().map(|_| Instant::now());
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
        Ok(IndexedCollectionSegment {
            collection: self.collection,
            source_id: self.source_id,
            source_version: self.source_version,
            files: self.files,
            file_index: self.file_index,
            records: self.records,
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
        })
    }
}


impl IndexedCollectionSegment {
    pub fn file(&self, id: FileId) -> Option<&SegmentFile> {
        self.files.get(*self.file_index.get(&id)?)
    }

    pub fn record(&self, slot: RecordSlot) -> Option<&RecordEntry> {
        self.records.get(slot.0 as usize)
    }

    pub fn file_records(&self, id: FileId) -> Option<&[RecordSlot]> {
        Some(&self.file(id)?.records)
    }
}


/// The offsets translate old shard-local slots to the consolidated local
/// namespace. Publication must translate each old active mask with them.
pub(crate) struct ConsolidatedSegment {
    pub index: IndexedCollectionSegment,
    pub offsets: Vec<u32>,
}


impl ConsolidatedSegment {
    /// Preserve retired local slots exactly when publishing the consolidated
    /// shard. Call before replacing old shards in one immutable root.
    pub fn translate_active(&self, masks: &[&ActiveRecords]) -> Result<ActiveRecords, String> {
        if masks.len() != self.offsets.len() {
            return Err("consolidation active-mask count mismatch".into());
        }
        let mut retired = Vec::new();
        for (index, mask) in masks.iter().enumerate() {
            let start = self.offsets[index];
            let end = self
                .offsets
                .get(index + 1)
                .copied()
                .unwrap_or(self.index.records.len() as u32);
            if mask.slot_count() != end - start {
                return Err("consolidation active-mask coverage mismatch".into());
            }
            for slot in 0..mask.slot_count() {
                if !mask.contains(RecordSlot(slot)) {
                    retired.push(RecordSlot(
                        start
                            .checked_add(slot)
                            .ok_or("consolidation active slot overflow")?,
                    ));
                }
            }
        }
        ActiveRecords::all(self.index.records.len())?.retired(&retired)
    }
}


fn merge_bigram_index(
    target: &mut BigramIndex,
    source: &BigramIndex,
    base: u32,
) -> Result<(), String> {
    if target.alphabet != source.alphabet || target.record_count != base {
        return Err("bigram segment representation or coverage mismatch".into());
    }
    target.record_count = base
        .checked_add(source.record_count)
        .ok_or("bigram slot overflow")?;
    for (key, posting) in &source.normal {
        append_shifted_bitmap(target.normal.entry(*key).or_default(), posting, base)?;
    }
    for (key, posting) in &source.skip {
        append_shifted_bitmap(target.skip.entry(*key).or_default(), posting, base)?;
    }
    Ok(())
}

fn consolidate_inference(
    shards: &[&IndexedCollectionSegment],
    logical: &RoaringBitmap,
    record_count: usize,
    offsets: &[u32],
) -> Result<Option<InferenceState>, String> {
    let states = shards
        .iter()
        .map(|shard| shard.inference.as_ref())
        .collect::<Option<Vec<_>>>();
    let Some(states) = states else {
        if shards.iter().any(|shard| shard.inference.is_some()) {
            return Err("inference segment mode mismatch".into());
        }
        return Ok(None);
    };
    let first = *states.first().ok_or("no inference segments")?;
    let mode = first.report.mode;
    let policy = first.report.policy;
    if states
        .iter()
        .any(|state| state.report.mode != mode || state.report.policy != policy)
    {
        return Err("inference segment configuration mismatch".into());
    }
    let complete = states.iter().all(|state| state.report.complete);
    let mut fields: HashMap<Vec<String>, InferredFieldReport> = HashMap::new();
    for state in &states {
        for source in &state.report.fields {
            let target = fields.entry(source.path.clone()).or_insert_with(|| {
                let mut field = source.clone();
                field.scalar_records = 0;
                field.array_records = 0;
                field.object_records = 0;
                field.eligible_records = 0;
                field.unavailable_records = 0;
                field.present_records = 0;
                field.missing_records = 0;
                field.null_occurrence_records = 0;
                field.exact_null_records = 0;
                field.empty_array_records = 0;
                field.empty_selection_records = 0;
                field.value_occurrences = 0;
                field.nonblank_occurrences = 0;
                field.nonblank_records = 0;
                field.short_string_observed_occurrences = 0;
                field.short_string_covered_records = 0;
                field.short_string_unresolved_records = 0;
                field.summary_bytes_estimate = 0;
                field.index_bytes_estimate = 0;
                field.retained_bytes_estimate = 0;
                field
            });
            for kind in &source.observed_kinds {
                if !target.observed_kinds.contains(kind) {
                    target.observed_kinds.push(kind.clone());
                }
            }
            target.scalar_records = target.scalar_records.saturating_add(source.scalar_records);
            target.array_records = target.array_records.saturating_add(source.array_records);
            target.object_records = target.object_records.saturating_add(source.object_records);
            target.array_nesting = target.array_nesting.max(source.array_nesting);
            target.integer_only = match (target.integer_only, source.integer_only) {
                (Some(left), Some(right)) => Some(left && right),
                _ => None,
            };
            target.present_records = target
                .present_records
                .saturating_add(source.present_records);
            target.null_occurrence_records = target
                .null_occurrence_records
                .saturating_add(source.null_occurrence_records);
            target.exact_null_records = target
                .exact_null_records
                .saturating_add(source.exact_null_records);
            target.empty_array_records = target
                .empty_array_records
                .saturating_add(source.empty_array_records);
            target.empty_selection_records = target
                .empty_selection_records
                .saturating_add(source.empty_selection_records);
            target.value_occurrences = target
                .value_occurrences
                .saturating_add(source.value_occurrences);
            target.nonblank_occurrences = target
                .nonblank_occurrences
                .saturating_add(source.nonblank_occurrences);
            target.nonblank_records = target
                .nonblank_records
                .saturating_add(source.nonblank_records);
            target.short_string_observed_occurrences = target
                .short_string_observed_occurrences
                .saturating_add(source.short_string_observed_occurrences);
            target.summary_bytes_estimate = target
                .summary_bytes_estimate
                .saturating_add(source.summary_bytes_estimate);
            target.index_bytes_estimate = target
                .index_bytes_estimate
                .saturating_add(source.index_bytes_estimate);
        }
    }
    let mut indexes = HashMap::new();
    let mut empty_selections = HashMap::new();
    let mut short_string_coverage = HashMap::new();
    if complete {
        let seed = shards
            .iter()
            .position(|shard| !shard.logical_records.is_empty());
        for (path, initial) in seed
            .into_iter()
            .flat_map(|position| &states[position].indexes)
        {
            if states.iter().zip(shards).any(|(state, shard)| {
                !shard.logical_records.is_empty()
                    && state.indexes.get(path).is_none_or(|index| {
                        index.kind != initial.kind || index.multi != initial.multi
                    })
            }) {
                continue;
            }
            let mut index = initial.clone();
            index.presence.clear();
            index.empty_selections.clear();
            index.nulls.clear();
            if let Some(facts) = &mut index.structural {
                facts.postings.clear();
            }
            index.values.clear();
            index.lengths.clear();
            index.covered_records = 0;
            let mut empty = RoaringBitmap::new();
            let mut short = None;
            for ((state, shard), base) in states.iter().zip(shards).zip(offsets) {
                if shard.logical_records.is_empty() {
                    continue;
                }
                index.merge_shifted(state.indexes[path].clone(), *base)?;
                append_shifted_bitmap(
                    &mut empty,
                    state
                        .empty_selections
                        .get(path)
                        .ok_or("inferred empty coverage missing")?,
                    *base,
                )?;
                if let Some(source) = state.short_string_coverage.get(path) {
                    let target = short.get_or_insert_with(RoaringBitmap::new);
                    append_shifted_bitmap(target, source, *base)?;
                }
            }
            index.finish_stats();
            index.validate_eligible(record_count, logical.len() as usize)?;
            index.validate_domain(logical)?;
            empty_selections.insert(path.clone(), empty);
            if let Some(short) = short {
                short_string_coverage.insert(path.clone(), short);
            }
            indexes.insert(path.clone(), index);
        }
    }
    let eligible = logical.len();
    let unavailable = record_count as u64 - eligible;
    let mut fields = fields.into_values().collect::<Vec<_>>();
    fields.sort_by(|left, right| left.path.cmp(&right.path));
    for field in &mut fields {
        field.eligible_records = eligible;
        field.unavailable_records = unavailable;
        field.missing_records = eligible.saturating_sub(field.present_records);
        field.distinct_nonblank_strings = None;
        if let Some(index) = indexes.get(&field.path) {
            field.index_status = InferenceIndexStatus::Built;
            field.complete = true;
            field.present_records = index.presence.len();
            field.exact_null_records = index.nulls.len();
            field.empty_selection_records = empty_selections[&field.path].len();
            if !field.observed_kinds.contains(&index.kind) {
                field.observed_kinds.push(index.kind.clone());
            }
            field.short_string_covered_records = short_string_coverage
                .get(&field.path)
                .map_or(0, RoaringBitmap::len);
            field.short_string_distinct_values = if field.short_string_covered_records > 0 {
                index.values.len()
            } else {
                0
            };
        } else {
            field.index_status = InferenceIndexStatus::Incomplete;
            field.complete = false;
            field.reason = "consolidated without a complete inferred posting".into();
            field.index_bytes_estimate = 0;
            field.short_string_covered_records = 0;
            field.short_string_distinct_values = 0;
        }
        field.short_string_unresolved_records = eligible - field.short_string_covered_records;
        field.retained_bytes_estimate = field
            .summary_bytes_estimate
            .saturating_add(field.index_bytes_estimate);
    }
    let mut report = first.report.clone();
    report.eligible_records = eligible;
    report.unavailable_records = unavailable;
    report.omitted_paths = states.iter().fold(0u64, |total, state| {
        total.saturating_add(state.report.omitted_paths)
    });
    report.complete = complete;
    report.observed_paths = fields.len();
    report.admitted_paths = indexes.len();
    report.dropped_paths = fields
        .iter()
        .filter(|field| {
            matches!(
                field.index_status,
                InferenceIndexStatus::Incompatible
                    | InferenceIndexStatus::Incomplete
                    | InferenceIndexStatus::BudgetExceeded
                    | InferenceIndexStatus::BelowThreshold
            )
        })
        .count();
    report.summary_bytes_estimate = fields.iter().fold(0usize, |total, field| {
        total.saturating_add(field.summary_bytes_estimate)
    });
    report.index_bytes_estimate = fields.iter().fold(0usize, |total, field| {
        total.saturating_add(field.index_bytes_estimate)
    });
    report.retained_bytes_estimate = report
        .summary_bytes_estimate
        .saturating_add(report.index_bytes_estimate);
    report.fields = fields;
    Ok(Some(InferenceState {
        report,
        indexes,
        empty_selections,
        short_string_coverage,
    }))
}


impl IndexedCollectionSegment {
    /// Combine already indexed shards without opening or parsing source files.
    /// Shards with overlapping FileIds or logical IDs must first be resolved
    /// by the coordinator's active-version policy. `max_records` bounds this
    /// consolidation's temporary copying and future shard scan cost.
    pub fn consolidate(
        shards: &[&Self],
        max_records: usize,
    ) -> Result<ConsolidatedSegment, String> {
        let first = *shards.first().ok_or("no segments to consolidate")?;
        let total_records = shards.iter().try_fold(0usize, |total, shard| {
            total
                .checked_add(shard.records.len())
                .ok_or("segment record count overflow")
        })?;
        if total_records > max_records || total_records > u32::MAX as usize {
            return Err("consolidation record budget exceeded".into());
        }
        let mut offsets = Vec::with_capacity(shards.len());
        let mut files = Vec::new();
        let mut file_index = HashMap::new();
        let mut records = Vec::with_capacity(total_records);
        let mut id_lookup = HashMap::new();
        let mut postings = first.postings.clone();
        let mut custom_postings = first.custom_postings.clone();
        let mut raw = first.raw.clone();
        let mut fallback_text = first.fallback_text.clone();
        let mut custom_text = first.custom_text.clone();
        let mut logical_records = RoaringBitmap::new();
        let mut empty_array_records = RoaringBitmap::new();
        let mut sorting = first.sorting.clone();
        let mut relations = first.relations.clone();
        let mut record_parses = 0u64;
        let mut failed_parses = 0u64;
        for (number, shard) in shards.iter().enumerate() {
            if shard.collection != first.collection
                || shard.source_id != first.source_id
                || shard.source_version != first.source_version
            {
                return Err("segment collection or source binding mismatch".into());
            }
            let base = u32::try_from(records.len()).map_err(|_| "segment slot overflow")?;
            offsets.push(base);
            for file in &shard.files {
                let mut entry = file.entry.clone();
                if file_index.insert(entry.id, files.len()).is_some() {
                    return Err("overlapping physical file in consolidation".into());
                }
                for diagnostic in &mut entry.diagnostics {
                    if let Some(slot) = diagnostic.record {
                        diagnostic.record = Some(RecordSlot(
                            base.checked_add(slot.0).ok_or("diagnostic slot overflow")?,
                        ));
                    }
                }
                let mut owned = Vec::with_capacity(file.records.len());
                for slot in &file.records {
                    owned.push(RecordSlot(
                        base.checked_add(slot.0).ok_or("file slot overflow")?,
                    ));
                }
                files.push(SegmentFile {
                    entry,
                    records: owned,
                });
            }
            for record in &shard.records {
                let mut record = record.clone();
                record.slot = RecordSlot(
                    base.checked_add(record.slot.0)
                        .ok_or("record slot overflow")?,
                );
                if id_lookup.insert(record.id.clone(), record.slot).is_some() {
                    return Err("overlapping record ID in consolidation".into());
                }
                records.push(record);
            }
            append_shifted_bitmap(&mut logical_records, &shard.logical_records, base)?;
            append_shifted_bitmap(&mut empty_array_records, &shard.empty_array_records, base)?;
            if number != 0 {
                postings.merge_shifted(shard.postings.clone(), base)?;
                if custom_postings.len() != shard.custom_postings.len() {
                    return Err("custom posting segment coverage mismatch".into());
                }
                for (key, source) in &shard.custom_postings {
                    let target = custom_postings
                        .get_mut(key)
                        .ok_or("custom posting segment missing")?;
                    target.index.merge_shifted(source.index.clone(), base)?;
                    target.complete &= source.complete;
                }
                merge_bigram_index(&mut raw, &shard.raw, base)?;
                merge_bigram_index(&mut fallback_text, &shard.fallback_text, base)?;
                if custom_text.len() != shard.custom_text.len() {
                    return Err("custom bigram segment coverage mismatch".into());
                }
                for (key, source) in &shard.custom_text {
                    merge_bigram_index(
                        custom_text
                            .get_mut(key)
                            .ok_or("custom bigram segment missing")?,
                        source,
                        base,
                    )?;
                }
                if relations.len() != shard.relations.len() {
                    return Err("relation segment coverage mismatch".into());
                }
                for (target, source) in relations.iter_mut().zip(&shard.relations) {
                    target.merge_shifted(source.clone(), base, shard.records.len() as u32)?;
                }
                if sorting.columns.len() != shard.sorting.columns.len()
                    || sorting.composites.len() != shard.sorting.composites.len()
                {
                    return Err("sort segment coverage mismatch".into());
                }
                for (path, source) in &shard.sorting.columns {
                    let target = sorting.columns.get_mut(path).ok_or("sort column missing")?;
                    if target.kind != source.kind
                        || target.locale != source.locale
                        || target.options != source.options
                        || target.collation_version != source.collation_version
                    {
                        return Err("sort column representation mismatch".into());
                    }
                    target.keys.extend_from_slice(&source.keys);
                }
                for (target, source) in sorting.composites.iter().zip(&shard.sorting.composites) {
                    if target.fields != source.fields {
                        return Err("composite sort representation mismatch".into());
                    }
                }
            }
            record_parses = record_parses.saturating_add(shard.record_parses);
            failed_parses = failed_parses.saturating_add(shard.failed_parses);
        }
        postings.finish_stats();
        let logical_count = logical_records.len() as usize;
        postings.validate_eligible(records.len(), logical_count)?;
        if logical_count != records.len() {
            postings.validate_domain(&logical_records)?;
        }
        for custom in custom_postings.values_mut() {
            custom.index.finish_stats();
            custom
                .index
                .validate_eligible(records.len(), logical_count)?;
            if logical_count != records.len() {
                custom.index.validate_domain(&logical_records)?;
            }
        }
        raw.validate(records.len())?;
        fallback_text.validate(records.len())?;
        if logical_count != records.len() {
            raw.validate_domain(&logical_records)?;
        }
        let fallback_slots: RoaringBitmap = records
            .iter()
            .filter(|record| record.class == RecordClass::RawFallback)
            .map(|record| record.slot.0)
            .collect();
        fallback_text.validate_domain(&fallback_slots)?;
        for index in custom_text.values() {
            index.validate(records.len())?;
            if logical_count != records.len() {
                index.validate_domain(&logical_records)?;
            }
        }
        for column in sorting.columns.values_mut() {
            if column.keys.len() != records.len() {
                return Err("sort column coverage mismatch".into());
            }
            column.ascending = records.iter().map(|record| record.slot).collect();
            column.ascending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], false, column.options)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
            column.descending = records.iter().map(|record| record.slot).collect();
            column.descending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], true, column.options)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
        }
        for composite in &mut sorting.composites {
            let columns = composite
                .fields
                .iter()
                .map(|field| {
                    sorting
                        .columns
                        .get(&field.path)
                        .map(|column| (column, field.descending))
                        .ok_or("composite sort column missing")
                })
                .collect::<Result<Vec<_>, _>>()?;
            composite.order = records.iter().map(|record| record.slot).collect();
            composite.order.sort_by(|a, b| {
                compare_composite(&columns, *a, *b)
                    .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
            });
        }
        let mut identity_order = records.iter().map(|record| record.slot).collect::<Vec<_>>();
        identity_order.sort_by(|a, b| records[a.0 as usize].id.cmp(&records[b.0 as usize].id));
        let inference = consolidate_inference(shards, &logical_records, records.len(), &offsets)?;
        Ok(ConsolidatedSegment {
            index: IndexedCollectionSegment {
                collection: first.collection,
                source_id: first.source_id.clone(),
                source_version: first.source_version,
                files,
                file_index,
                records,
                id_lookup,
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
                record_parses,
                failed_parses,
            },
            offsets,
        })
    }
}


/// A dense active-only shard plus a locator translation for callers that
/// retained an old local slot. Retired slots map to `None`.
pub(crate) struct CompactedSegment {
    pub index: IndexedCollectionSegment,
    pub active: ActiveRecords,
    pub old_to_new: Vec<Option<RecordSlot>>,
}


pub(crate) struct PostingProjection {
    postings: PostingCollection,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
}


fn empty_projected_posting(source: &PostingIndex, logical_count: u32) -> PostingIndex {
    PostingIndex {
        path: source.path.clone(),
        kind: source.kind.clone(),
        multi: source.multi,
        presence: RoaringBitmap::new(),
        empty_selections: RoaringBitmap::new(),
        nulls: RoaringBitmap::new(),
        structural: source.structural.as_ref().map(|_| StructuralFacts::default()),
        values: Default::default(),
        lengths: Default::default(),
        value_complete: source.value_complete,
        length_complete: source.length_complete,
        empty_string_complete: source.empty_string_complete,
        covered_records: logical_count,
        stats: Default::default(),
    }
}

fn scatter_posting_key<K: Ord + Clone>(
    target: &mut std::collections::BTreeMap<K, RoaringBitmap>,
    key: &K,
    slot: u32,
) {
    if let Some(bitmap) = target.get_mut(key) {
        bitmap.insert(slot);
    } else {
        let mut bitmap = RoaringBitmap::new();
        bitmap.insert(slot);
        target.insert(key.clone(), bitmap);
    }
}

fn project_bitmap(
    source: &RoaringBitmap,
    slots: &[Option<RecordSlot>],
) -> Result<RoaringBitmap, String> {
    project_bitmap_in_range(source, slots, None)
}

fn project_bitmap_in_range(
    source: &RoaringBitmap,
    slots: &[Option<RecordSlot>],
    bounds: Option<(u32, u32)>,
) -> Result<RoaringBitmap, String> {
    let mut projected = RoaringBitmap::new();
    let Some((first, last)) = bounds else {
        for old in source.iter() {
            if let Some(slot) = slots
                .get(old as usize)
                .ok_or("posting slot outside segment")?
            {
                projected.insert(slot.0);
            }
        }
        return Ok(projected);
    };
    for old in source.range(first..=last) {
        if let Some(slot) = slots
            .get(old as usize)
            .ok_or("posting slot outside segment")?
        {
            projected.insert(slot.0);
        }
    }
    Ok(projected)
}

fn project_posting(
    index: &PostingIndex,
    slots: &[Option<RecordSlot>],
    logical_count: u32,
    bounds: Option<(u32, u32)>,
) -> Result<PostingIndex, String> {
    let mut projected = PostingIndex {
        path: index.path.clone(),
        kind: index.kind.clone(),
        multi: index.multi,
        presence: project_bitmap_in_range(&index.presence, slots, bounds)?,
        empty_selections: project_bitmap_in_range(&index.empty_selections, slots, bounds)?,
        nulls: project_bitmap_in_range(&index.nulls, slots, bounds)?,
        structural: index.structural.as_ref().map(|_| StructuralFacts::default()),
        values: Default::default(),
        lengths: Default::default(),
        value_complete: index.value_complete,
        length_complete: index.length_complete,
        empty_string_complete: index.empty_string_complete,
        covered_records: logical_count,
        stats: Default::default(),
    };
    for (key, bitmap) in &index.values {
        let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
        if !kept.is_empty() {
            projected.values.insert(key.clone(), kept);
        }
    }
    for (key, bitmap) in &index.lengths {
        let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
        if !kept.is_empty() {
            projected.lengths.insert(key.clone(), kept);
        }
    }
    if let (Some(source), Some(target)) = (&index.structural, &mut projected.structural) {
        for (key, bitmap) in &source.postings {
            let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
            if !kept.is_empty() {
                target.postings.insert(*key, kept);
            }
        }
    }
    projected.covered_records = logical_count;
    projected.finish_stats();
    Ok(projected)
}

fn project_bigram(
    index: &BigramIndex,
    slots: &[Option<RecordSlot>],
    new_count: u32,
    bounds: Option<(u32, u32)>,
) -> Result<BigramIndex, String> {
    let mut projected = BigramIndex::new(index.alphabet);
    projected.record_count = new_count;
    for (key, bitmap) in &index.normal {
        let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
        if !kept.is_empty() {
            projected.normal.insert(*key, kept);
        }
    }
    for (key, bitmap) in &index.skip {
        let kept = project_bitmap_in_range(bitmap, slots, bounds)?;
        if !kept.is_empty() {
            projected.skip.insert(*key, kept);
        }
    }
    Ok(projected)
}

fn project_sorting(
    source: &SortCollection,
    slots: &[Option<RecordSlot>],
    records: &[RecordEntry],
) -> Result<SortCollection, String> {
    let mut sorting = SortCollection {
        columns: HashMap::with_capacity(source.columns.len()),
        composites: source
            .composites
            .iter()
            .map(|order| CompositeSortOrder {
                fields: order.fields.clone(),
                order: Vec::new(),
            })
            .collect(),
    };
    for (path, original) in &source.columns {
        let mut column = SortColumn {
            path: original.path.clone(),
            kind: original.kind.clone(),
            keys: Vec::new(),
            ascending: Vec::new(),
            descending: Vec::new(),
            locale: original.locale.clone(),
            options: original.options,
            collation_version: original.collation_version.clone(),
        };
        let old_keys = &original.keys;
        if old_keys.len() != slots.len() {
            return Err("sort projection coverage mismatch".into());
        }
        let mut keys = std::iter::repeat_with(|| None)
            .take(records.len())
            .collect::<Vec<_>>();
        for (old, key) in old_keys.iter().enumerate() {
            if let Some(slot) = slots[old] {
                let target = keys
                    .get_mut(slot.0 as usize)
                    .ok_or("sort projection slot out of bounds")?;
                if target.replace(key.clone()).is_some() {
                    return Err("duplicate sort projection slot".into());
                }
            }
        }
        column.keys = keys
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or("sort projection slot mismatch")?;
        column.ascending = records.iter().map(|record| record.slot).collect();
        column.ascending.sort_by(|a, b| {
            column.keys[a.0 as usize]
                .compare_with(&column.keys[b.0 as usize], false, column.options)
                .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
        });
        column.descending = records.iter().map(|record| record.slot).collect();
        column.descending.sort_by(|a, b| {
            column.keys[a.0 as usize]
                .compare_with(&column.keys[b.0 as usize], true, column.options)
                .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
        });
        sorting.columns.insert(path.clone(), column);
    }
    for composite in &mut sorting.composites {
        let columns = composite
            .fields
            .iter()
            .map(|part| {
                sorting
                    .columns
                    .get(&part.path)
                    .map(|column| (column, part.descending))
                    .ok_or("composite sort column missing")
            })
            .collect::<Result<Vec<_>, _>>()?;
        composite.order = records.iter().map(|record| record.slot).collect();
        composite.order.sort_by(|a, b| {
            compare_composite(&columns, *a, *b)
                .then_with(|| records[a.0 as usize].id.cmp(&records[b.0 as usize].id))
        });
    }
    Ok(sorting)
}

fn project_inference(
    source: &Option<InferenceState>,
    slots: &[Option<RecordSlot>],
    logical: &RoaringBitmap,
    new_count: usize,
    bounds: Option<(u32, u32)>,
) -> Result<Option<InferenceState>, String> {
    let Some(source) = source else {
        return Ok(None);
    };
    let mut state = InferenceState {
        report: source.report.clone(),
        indexes: HashMap::new(),
        empty_selections: HashMap::new(),
        short_string_coverage: HashMap::new(),
    };
    let eligible = u32::try_from(logical.len()).map_err(|_| "inference slot overflow")?;
    state.indexes = source
        .indexes
        .iter()
        .map(|(path, index)| {
            project_posting(index, slots, eligible, bounds).map(|index| (path.clone(), index))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    state.empty_selections = source
        .empty_selections
        .iter()
        .map(|(path, bitmap)| {
            project_bitmap_in_range(bitmap, slots, bounds).map(|bitmap| (path.clone(), bitmap))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    state.short_string_coverage = source
        .short_string_coverage
        .iter()
        .map(|(path, bitmap)| {
            project_bitmap_in_range(bitmap, slots, bounds).map(|bitmap| (path.clone(), bitmap))
        })
        .collect::<Result<HashMap<_, _>, _>>()?;
    let eligible = logical.len();
    let unavailable = new_count as u64 - eligible;
    state.report.eligible_records = eligible;
    state.report.unavailable_records = unavailable;
    for field in &mut state.report.fields {
        field.eligible_records = eligible;
        field.unavailable_records = unavailable;
        field.distinct_nonblank_strings = None;
        if let Some(index) = state.indexes.get(&field.path) {
            field.index_status = InferenceIndexStatus::Built;
            field.complete = true;
            field.present_records = index.presence.len();
            field.exact_null_records = index.nulls.len();
            field.null_occurrence_records = field.exact_null_records;
            field.empty_selection_records = state.empty_selections[&field.path].len();
            field.short_string_covered_records = state
                .short_string_coverage
                .get(&field.path)
                .map_or(0, RoaringBitmap::len);
            field.short_string_distinct_values = if field.short_string_covered_records > 0 {
                index.values.len()
            } else {
                0
            };
            field.scalar_records = if index.multi {
                0
            } else {
                field.present_records
            };
            field.array_records = if index.multi {
                field.present_records
            } else {
                0
            };
        } else {
            field.index_status = InferenceIndexStatus::Incomplete;
            field.complete = false;
            field.reason = "retired records removed from incomplete inference summary".into();
            field.present_records = 0;
            field.null_occurrence_records = 0;
            field.exact_null_records = 0;
            field.empty_selection_records = 0;
            field.short_string_covered_records = 0;
            field.short_string_distinct_values = 0;
            field.scalar_records = 0;
            field.array_records = 0;
            field.index_bytes_estimate = 0;
        }
        field.missing_records = eligible - field.present_records;
        field.short_string_unresolved_records = eligible - field.short_string_covered_records;
        field.object_records = 0;
        field.value_occurrences = 0;
        field.nonblank_occurrences = 0;
        field.nonblank_records = 0;
        field.short_string_observed_occurrences = 0;
        field.retained_bytes_estimate = field
            .summary_bytes_estimate
            .saturating_add(field.index_bytes_estimate);
    }
    state.report.admitted_paths = state.indexes.len();
    state.report.dropped_paths = state
        .report
        .fields
        .iter()
        .filter(|field| {
            matches!(
                field.index_status,
                InferenceIndexStatus::Incompatible
                    | InferenceIndexStatus::Incomplete
                    | InferenceIndexStatus::BudgetExceeded
                    | InferenceIndexStatus::BelowThreshold
            )
        })
        .count();
    state.report.summary_bytes_estimate = state.report.fields.iter().fold(0usize, |sum, field| {
        sum.saturating_add(field.summary_bytes_estimate)
    });
    state.report.index_bytes_estimate = state.report.fields.iter().fold(0usize, |sum, field| {
        sum.saturating_add(field.index_bytes_estimate)
    });
    state.report.retained_bytes_estimate = state
        .report
        .summary_bytes_estimate
        .saturating_add(state.report.index_bytes_estimate);
    Ok(Some(state))
}


impl IndexedCollectionSegment {
    /// Scatter typed posting dictionaries through the partition map once.
    /// Repeatedly projecting the complete source dictionary for each output
    /// shard is quadratic in the number of partitions for unique IDs.
    pub(crate) fn project_postings_for_partitions(
        &self,
        masks: &[ActiveRecords],
    ) -> Result<Vec<PostingProjection>, String> {
        let mut mapping = vec![None; self.records.len()];
        let mut logical_counts = vec![0u32; masks.len()];
        for (part, mask) in masks.iter().enumerate() {
            if mask.slot_count() as usize != self.records.len() {
                return Err("partition posting mask coverage mismatch".into());
            }
            for (local, old) in mask.bitmap().iter().enumerate() {
                let location = mapping
                    .get_mut(old as usize)
                    .ok_or("partition posting slot outside source")?;
                if location.replace((part, RecordSlot(local as u32))).is_some() {
                    return Err("partition posting slot selected twice".into());
                }
                if self.logical_records.contains(old) {
                    logical_counts[part] += 1;
                }
            }
        }
        let mut projected = logical_counts
            .into_iter()
            .map(|logical_count| PostingProjection {
                postings: PostingCollection {
                    fields: self
                        .postings
                        .fields
                        .iter()
                        .map(|(path, index)| {
                            (path.clone(), empty_projected_posting(index, logical_count))
                        })
                        .collect(),
                },
                custom_postings: self
                    .custom_postings
                    .iter()
                    .map(|(key, custom)| {
                        (
                            key.clone(),
                            CustomPostingIndex {
                                index: empty_projected_posting(&custom.index, logical_count),
                                complete: custom.complete,
                            },
                        )
                    })
                    .collect(),
            })
            .collect::<Vec<_>>();
        for (path, index) in &self.postings.fields {
            for old in index.presence.iter() {
                if let Some((part, slot)) = mapping[old as usize] {
                    projected[part]
                        .postings
                        .fields
                        .get_mut(path)
                        .unwrap()
                        .presence
                        .insert(slot.0);
                }
            }
            for old in index.empty_selections.iter() {
                if let Some((part, slot)) = mapping[old as usize] {
                    projected[part]
                        .postings
                        .fields
                        .get_mut(path)
                        .unwrap()
                        .empty_selections
                        .insert(slot.0);
                }
            }
            for old in index.nulls.iter() {
                if let Some((part, slot)) = mapping[old as usize] {
                    projected[part]
                        .postings
                        .fields
                        .get_mut(path)
                        .unwrap()
                        .nulls
                        .insert(slot.0);
                }
            }
            if let Some(facts) = &index.structural {
                for (key, bitmap) in &facts.postings {
                    for old in bitmap.iter() {
                        if let Some((part, slot)) = mapping[old as usize] {
                            projected[part]
                                .postings
                                .fields
                                .get_mut(path)
                                .unwrap()
                                .structural
                                .as_mut()
                                .unwrap()
                                .postings
                                .entry(*key)
                                .or_default()
                                .insert(slot.0);
                        }
                    }
                }
            }
            for (key, bitmap) in &index.values {
                for old in bitmap.iter() {
                    if let Some((part, slot)) = mapping[old as usize] {
                        scatter_posting_key(
                            &mut projected[part]
                                .postings
                                .fields
                                .get_mut(path)
                                .unwrap()
                                .values,
                            key,
                            slot.0,
                        );
                    }
                }
            }
            for (key, bitmap) in &index.lengths {
                for old in bitmap.iter() {
                    if let Some((part, slot)) = mapping[old as usize] {
                        scatter_posting_key(
                            &mut projected[part]
                                .postings
                                .fields
                                .get_mut(path)
                                .unwrap()
                                .lengths,
                            key,
                            slot.0,
                        );
                    }
                }
            }
        }
        for (key, custom) in &self.custom_postings {
            let index = &custom.index;
            for old in index.presence.iter() {
                if let Some((part, slot)) = mapping[old as usize] {
                    projected[part]
                        .custom_postings
                        .get_mut(key)
                        .unwrap()
                        .index
                        .presence
                        .insert(slot.0);
                }
            }
            for old in index.nulls.iter() {
                if let Some((part, slot)) = mapping[old as usize] {
                    projected[part]
                        .custom_postings
                        .get_mut(key)
                        .unwrap()
                        .index
                        .nulls
                        .insert(slot.0);
                }
            }
            for (value, bitmap) in &index.values {
                for old in bitmap.iter() {
                    if let Some((part, slot)) = mapping[old as usize] {
                        scatter_posting_key(
                            &mut projected[part]
                                .custom_postings
                                .get_mut(key)
                                .unwrap()
                                .index
                                .values,
                            value,
                            slot.0,
                        );
                    }
                }
            }
            for (value, bitmap) in &index.lengths {
                for old in bitmap.iter() {
                    if let Some((part, slot)) = mapping[old as usize] {
                        scatter_posting_key(
                            &mut projected[part]
                                .custom_postings
                                .get_mut(key)
                                .unwrap()
                                .index
                                .lengths,
                            value,
                            slot.0,
                        );
                    }
                }
            }
        }
        for part in &mut projected {
            for index in part.postings.fields.values_mut() {
                index.finish_stats();
            }
            for custom in part.custom_postings.values_mut() {
                custom.index.finish_stats();
            }
        }
        Ok(projected)
    }

    pub(crate) fn compact_active_with_postings(
        &self,
        active: &ActiveRecords,
        max_scanned_records: usize,
        retain_file: impl Fn(&FileEntry) -> bool,
        projection: PostingProjection,
    ) -> Result<CompactedSegment, String> {
        self.compact_active_mapped(
            active,
            max_scanned_records,
            retain_file,
            None,
            Some(projection),
        )
    }

    /// Build a dense active-only shard without opening source data. The file
    /// predicate distinguishes a current zero-record file from an obsolete
    /// historical incarnation with no live records.
    pub fn compact_active(
        &self,
        active: &ActiveRecords,
        max_scanned_records: usize,
        retain_file: impl Fn(&FileEntry) -> bool,
    ) -> Result<CompactedSegment, String> {
        self.compact_active_mapped(active, max_scanned_records, retain_file, None, None)
    }

    /// Reorder a dense bounded shard without changing its physical slot count.
    /// Used to put a verified replacement into the old record's local slot.
    pub(crate) fn remap_dense(&self, new_slots: &[RecordSlot]) -> Result<Self, String> {
        if new_slots.len() != self.records.len() {
            return Err("slot remap coverage mismatch".into());
        }
        let active = ActiveRecords::all(self.records.len())?;
        Ok(self
            .compact_active_mapped(&active, self.records.len(), |_| true, Some(new_slots), None)?
            .index)
    }

    fn compact_active_mapped(
        &self,
        active: &ActiveRecords,
        max_scanned_records: usize,
        retain_file: impl Fn(&FileEntry) -> bool,
        new_slots: Option<&[RecordSlot]>,
        posting_projection: Option<PostingProjection>,
    ) -> Result<CompactedSegment, String> {
        if active.slot_count() as usize != self.records.len() {
            return Err("compaction active-mask coverage mismatch".into());
        }
        if self.records.len() > max_scanned_records {
            return Err("compaction record budget exceeded".into());
        }
        if new_slots.is_some_and(|slots| slots.len() != self.records.len()) {
            return Err("slot remap coverage mismatch".into());
        }
        let mut old_to_new = Vec::with_capacity(self.records.len());
        let mut next = 0u32;
        for record in &self.records {
            if record.slot.0 as usize != old_to_new.len() {
                return Err("compaction record slots out of order".into());
            }
            if active.contains(record.slot) {
                old_to_new.push(Some(
                    new_slots
                        .map(|slots| slots[record.slot.0 as usize])
                        .unwrap_or(RecordSlot(next)),
                ));
                next = next.checked_add(1).ok_or("compaction slot overflow")?;
            } else {
                old_to_new.push(None);
            }
        }
        let mut seen = vec![false; next as usize];
        for slot in old_to_new.iter().flatten() {
            let used = seen
                .get_mut(slot.0 as usize)
                .ok_or("slot remap outside active domain")?;
            if std::mem::replace(used, true) {
                return Err("duplicate slot remap target".into());
            }
        }
        let mut files = Vec::new();
        let mut file_index = HashMap::new();
        let mut diagnostic_refs: HashMap<FileId, Vec<Option<DiagnosticRef>>> = HashMap::new();
        for file in &self.files {
            let retained = retain_file(&file.entry);
            let mut owned = Vec::new();
            for old in &file.records {
                let mapped = old_to_new
                    .get(old.0 as usize)
                    .ok_or("file record slot out of bounds")?;
                if let Some(slot) = mapped {
                    owned.push(*slot);
                }
            }
            owned.sort_unstable();
            if !retained {
                if !owned.is_empty() {
                    return Err("active record belongs to obsolete file".into());
                }
                continue;
            }
            let mut entry = file.entry.clone();
            let old_diagnostics = std::mem::take(&mut entry.diagnostics);
            entry.status = FileStatus::Valid;
            let omitted = entry.omitted_diagnostics;
            entry.omitted_diagnostics = 0;
            let mut refs = Vec::with_capacity(old_diagnostics.len());
            for mut diagnostic in old_diagnostics {
                if let Some(old) = diagnostic.record {
                    let new = old_to_new
                        .get(old.0 as usize)
                        .ok_or("diagnostic slot out of bounds")?;
                    diagnostic.record = *new;
                }
                refs.push(Some(entry.record_diagnostic(diagnostic)?));
            }
            entry.omitted_diagnostics = entry.omitted_diagnostics.saturating_add(omitted);
            if omitted > 0 {
                entry.status = entry.status.max(file.entry.status);
            }
            diagnostic_refs.insert(entry.id, refs);
            file_index.insert(entry.id, files.len());
            files.push(SegmentFile {
                entry,
                records: owned,
            });
        }
        let mut records = vec![None; next as usize];
        let mut id_lookup = HashMap::new();
        for record in &self.records {
            let Some(slot) = old_to_new[record.slot.0 as usize] else {
                continue;
            };
            if !file_index.contains_key(&record.file) {
                return Err("active record file missing".into());
            }
            let mut compacted = record.clone();
            compacted.slot = slot;
            if compacted.class == RecordClass::RawFallback {
                compacted.diagnostic_ref = match compacted.diagnostic_ref.stored_index() {
                    Some(old) => diagnostic_refs
                        .get(&compacted.file)
                        .and_then(|refs| refs.get(old))
                        .and_then(|reference| *reference)
                        .ok_or("active fallback diagnostic removed")?,
                    None => DiagnosticRef(u32::MAX),
                };
            }
            if id_lookup.insert(compacted.id.clone(), slot).is_some() {
                return Err("duplicate active record ID during compaction".into());
            }
            let target = records
                .get_mut(slot.0 as usize)
                .ok_or("compacted record target outside active domain")?;
            if target.replace(compacted).is_some() {
                return Err("duplicate compacted record target".into());
            }
        }
        let records = records
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or("compacted record target missing")?;
        let bounds = old_to_new
            .iter()
            .position(Option::is_some)
            .zip(old_to_new.iter().rposition(Option::is_some))
            .map(|(first, last)| (first as u32, last as u32));
        let logical_records = project_bitmap(&self.logical_records, &old_to_new)?;
        let empty_array_records = project_bitmap(&self.empty_array_records, &old_to_new)?;
        let logical_count =
            u32::try_from(logical_records.len()).map_err(|_| "logical slot overflow")?;
        let (postings, custom_postings) = if let Some(projection) = posting_projection {
            (projection.postings, projection.custom_postings)
        } else {
            let postings = PostingCollection {
                fields: self
                    .postings
                    .fields
                    .iter()
                    .map(|(path, index)| {
                        project_posting(index, &old_to_new, logical_count, bounds)
                            .map(|projected| (path.clone(), projected))
                    })
                    .collect::<Result<HashMap<_, _>, _>>()?,
            };
            let custom_postings = self
                .custom_postings
                .iter()
                .map(|(key, custom)| {
                    project_posting(&custom.index, &old_to_new, logical_count, bounds).map(
                        |index| {
                            (
                                key.clone(),
                                CustomPostingIndex {
                                    index,
                                    complete: custom.complete,
                                },
                            )
                        },
                    )
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
            (postings, custom_postings)
        };
        let raw = project_bigram(&self.raw, &old_to_new, next, bounds)?;
        let fallback_text = project_bigram(&self.fallback_text, &old_to_new, next, bounds)?;
        let custom_text = self
            .custom_text
            .iter()
            .map(|(key, index)| {
                project_bigram(index, &old_to_new, next, bounds).map(|index| (key.clone(), index))
            })
            .collect::<Result<HashMap<_, _>, _>>()?;
        let sorting = project_sorting(&self.sorting, &old_to_new, &records)?;
        let inference = project_inference(
            &self.inference,
            &old_to_new,
            &logical_records,
            records.len(),
            bounds,
        )?;
        let relations = self
            .relations
            .iter()
            .map(|relation| relation.project_slots(&old_to_new))
            .collect::<Result<Vec<_>, _>>()?;
        let mut identity_order = records.iter().map(|record| record.slot).collect::<Vec<_>>();
        identity_order.sort_by(|a, b| records[a.0 as usize].id.cmp(&records[b.0 as usize].id));
        postings.validate_eligible(records.len(), logical_records.len() as usize)?;
        if logical_records.len() as usize != records.len() {
            postings.validate_domain(&logical_records)?;
        }
        raw.validate(records.len())?;
        fallback_text.validate(records.len())?;
        for index in custom_text.values() {
            index.validate(records.len())?;
        }
        let index = IndexedCollectionSegment {
            collection: self.collection,
            source_id: self.source_id.clone(),
            source_version: self.source_version,
            files,
            file_index,
            records,
            id_lookup,
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
        };
        Ok(CompactedSegment {
            active: ActiveRecords::all(index.records.len())?,
            index,
            old_to_new,
        })
    }
}


#[cfg(test)]
#[path = "mutation_segment.test.rs"]
mod tests;
