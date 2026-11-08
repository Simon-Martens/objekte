//! Sparse, collection-local admission of prepared file contributions.

use std::collections::HashMap;

use super::*;
use crate::bigram::BigramIndex;
use crate::inference::{InferenceIndexStatus, InferenceState, InferredFieldReport};
use crate::inventory::ActiveRecords;
use crate::sorting::{SortCollection, compare_composite};


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
    pub record_parses: u64,
    pub failed_parses: u64,
}


pub(crate) struct SegmentBatch {
    base: u32,
    count: u32,
    contribution: BatchContribution,
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
            source_bytes: 0,
            limits,
            record_parses: 0,
            failed_parses: 0,
        }
    }

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
        for mut batch in prepared.contributions {
            let base = u32::try_from(self.records.len()).map_err(|_| "too many segment records")?;
            let count = u32::try_from(batch.records.len()).map_err(|_| "too many batch records")?;
            base.checked_add(count)
                .ok_or("segment record slot overflow")?;
            let mut records = std::mem::take(&mut batch.records).into_iter();
            for batch_file in std::mem::take(&mut batch.files) {
                if batch_file.entry.id != source.id
                    || batch_file.entry.relative_path != source.relative_path
                    || batch_file.entry.revision != source.revision
                    || batch_file.entry.created_ns != source.created_ns
                    || batch_file.entry.collections != [self.collection]
                {
                    return Err("prepared file identity changed".into());
                }
                let index = if batch_file.first_range {
                    if self.file_index.contains_key(&source.id) {
                        return Err("file has repeated first range".into());
                    }
                    let index = self.files.len();
                    self.files.push(SegmentFile {
                        entry: source.for_collection(self.collection),
                        records: Vec::new(),
                    });
                    self.file_index.insert(source.id, index);
                    index
                } else {
                    *self
                        .file_index
                        .get(&source.id)
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
                    if record.slot.0 as usize != expected || record.file != source.id {
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
            self.record_parses = self.record_parses.saturating_add(batch.record_parses);
            self.failed_parses = self.failed_parses.saturating_add(batch.failed_parses);
            self.batches.push(SegmentBatch {
                base,
                count,
                contribution: batch,
            });
        }
        self.source_bytes = self
            .source_bytes
            .checked_add(source.revision.size)
            .ok_or("segment source byte count overflow")?;
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
        let mut postings = PostingCollection::new(&definition.schema);
        let mut inference = InferenceBuilder::for_collection(inference_config, collection_count);
        let mut custom_postings: HashMap<_, _> = definition
            .schema
            .operations
            .iter()
            .filter(|(_, binding)| binding.operation.posting())
            .map(|(key, binding)| {
                (
                    key.clone(),
                    CustomPostingIndex {
                        index: PostingIndex::new(&definition.schema.fields[&key.0]),
                        complete: binding.operation.complete_coverage(),
                    },
                )
            })
            .collect();
        let mut raw = BigramContribution::new(alphabet);
        let mut fallback_text = BigramContribution::new(alphabet);
        let mut custom_text: HashMap<_, _> = definition
            .schema
            .custom
            .keys()
            .chain(
                definition
                    .schema
                    .operations
                    .iter()
                    .filter(|(_, binding)| {
                        binding.operation.bigram() && binding.operation.complete_coverage()
                    })
                    .map(|(key, _)| key),
            )
            .map(|key| {
                (
                    canonical_custom_text_key(&definition.schema, &key.0, &key.1),
                    BigramContribution::new(alphabet),
                )
            })
            .collect();
        let mut logical_records = RoaringBitmap::new();
        let mut empty_array_records = RoaringBitmap::new();
        let mut sorting = SortBuilder::new(
            &definition.sort_fields,
            &definition.schema,
            &definition.sort_locale,
            definition.sort_options,
            &definition.sort_overrides,
        )?;
        let mut relations = RelationContribution::for_schema(&definition.schema);
        for SegmentBatch {
            base,
            count,
            contribution: batch,
        } in self.batches
        {
            postings.merge_shifted(batch.postings, base)?;
            match (inference.as_mut(), batch.inference) {
                (Some(global), Some(local)) => global.merge_shifted(local, base)?,
                (None, None) => {}
                _ => return Err("inference segment mode mismatch".into()),
            }
            if relations.len() != batch.relations.len() {
                return Err("relation segment coverage mismatch".into());
            }
            for (global, local) in relations.iter_mut().zip(batch.relations) {
                global.merge_shifted(local, base, count)?;
            }
            if custom_postings.len() != batch.custom_postings.len() {
                return Err("custom posting segment coverage mismatch".into());
            }
            for (key, local) in batch.custom_postings {
                let global = custom_postings
                    .get_mut(&key)
                    .ok_or("custom posting segment missing")?;
                global.index.merge_shifted(local.index, base)?;
                global.complete &= local.complete;
            }
            raw.merge_shifted(batch.raw, base)?;
            fallback_text.merge_shifted(batch.fallback_text, base)?;
            if custom_text.len() != batch.custom_text.len() {
                return Err("custom bigram segment coverage mismatch".into());
            }
            for (key, index) in batch.custom_text {
                custom_text
                    .get_mut(&key)
                    .ok_or("custom bigram segment missing")?
                    .merge_shifted(index, base)?;
            }
            append_shifted_bitmap(&mut logical_records, &batch.logical_records, base)?;
            append_shifted_bitmap(&mut empty_array_records, &batch.empty_array_records, base)?;
            sorting.append_keys(batch.sort_keys, base, count)?;
        }
        let record_count = self.records.len();
        let logical_count = logical_records.len() as usize;
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
        let raw = raw.into_index_in_pool(pool)?;
        let fallback_text = fallback_text.into_index_in_pool(pool)?;
        let custom_text = custom_text
            .into_iter()
            .map(|(key, index)| index.into_index_in_pool(pool).map(|index| (key, index)))
            .collect::<Result<HashMap<_, _>, _>>()?;
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
        let sorting = sorting.finish_for_records(&self.records)?;
        let inference = inference
            .map(|builder| builder.finish(&logical_records, record_count, &definition.schema))
            .transpose()?;
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
            index.nulls.clear();
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


#[cfg(test)]
#[path = "mutation_segment.test.rs"]
mod tests;
