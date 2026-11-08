//! Sparse, collection-local admission of prepared file contributions.

use std::collections::HashMap;

use super::*;
use crate::bigram::BigramIndex;
use crate::inference::InferenceState;
use crate::sorting::SortCollection;


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


#[cfg(test)]
#[path = "mutation_segment.test.rs"]
mod tests;
