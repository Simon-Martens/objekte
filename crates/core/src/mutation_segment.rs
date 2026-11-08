//! Sparse, collection-local admission of prepared file contributions.

use std::collections::HashMap;

use super::*;


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
    batches: Vec<(u32, BatchContribution)>,
    record_parses: u64,
    failed_parses: u64,
}


/// Owned, locally slotted contribution set. `finish_indexes` will merge and
/// finalize every index family before this is published by the coordinator.
pub(crate) struct CollectionSegment {
    pub collection: CollectionId,
    pub source_id: String,
    pub source_version: u32,
    pub files: Vec<SegmentFile>,
    pub records: Vec<RecordEntry>,
    pub id_lookup: HashMap<RecordId, RecordSlot>,
    pub(crate) batches: Vec<(u32, BatchContribution)>,
    pub record_parses: u64,
    pub failed_parses: u64,
}


impl SegmentAdmission {
    pub fn new(collection: CollectionId, source: &PreparedSource) -> Self {
        Self {
            collection,
            source_id: source.id().into(),
            source_version: source.version(),
            files: Vec::new(),
            file_index: HashMap::new(),
            records: Vec::new(),
            id_lookup: HashMap::new(),
            batches: Vec::new(),
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
            self.batches.push((base, batch));
        }
        Ok(())
    }

    pub fn seal(self) -> CollectionSegment {
        CollectionSegment {
            collection: self.collection,
            source_id: self.source_id,
            source_version: self.source_version,
            files: self.files,
            records: self.records,
            id_lookup: self.id_lookup,
            batches: self.batches,
            record_parses: self.record_parses,
            failed_parses: self.failed_parses,
        }
    }
}


#[cfg(test)]
#[path = "mutation_segment.test.rs"]
mod tests;
