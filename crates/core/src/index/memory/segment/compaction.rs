use std::collections::{HashMap, HashSet};

use super::projection::{
    BigramProjection, DomainProjection, PartitionRouting, PostingProjection, SlotMap,
    project_bigram, project_bitmap, project_inference, project_posting, project_sorting,
};
use super::{
    IndexedCollectionSegment, RetainedAddress, SegmentAccounting, SegmentBacking, SegmentFile,
};
use crate::index::memory::inference::InferenceState;
use crate::index::memory::posting::{CustomPostingIndex, PostingCollection};
use crate::index::memory::relation::RelationContribution;
use crate::index::memory::sorting::SortCollection;
use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::segment::maintenance::SegmentPartition;
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::inventory::{ActiveRecords, DiagnosticRef, FileEntry, FileStatus, RecordClass};
use crate::model::{FileId, RecordSlot};

/// A dense active-only shard plus a locator translation for callers that
/// retained an old local slot. Retired slots map to `None`.
pub(crate) struct CompactedSegment {
    pub index: IndexedCollectionSegment,
    #[cfg(test)]
    pub active: ActiveRecords,
    pub old_to_new: Vec<Option<RecordSlot>>,
}

impl IndexedCollectionSegment {
    pub fn compact_active(
        &self,
        active: &ActiveRecords,
        max_scanned_records: usize,
        retain_file: impl FnMut(&FileEntry) -> bool,
    ) -> Result<CompactedSegment, String> {
        self.compact_active_mapped(active, max_scanned_records, retain_file, None, None)
    }

    pub(crate) fn project_partitions(
        &self,
        selections: &[SegmentPartition],
        catalog: &dyn FileCatalogReader,
        cancelled: &mut dyn FnMut() -> bool,
    ) -> StorageResult<Vec<Self>> {
        let invalid = |detail| StorageError::invalid("project segment partitions", detail);
        let masks = selections
            .iter()
            .map(|partition| partition.slots.clone())
            .collect::<Vec<_>>();
        let routing = PartitionRouting::new(self, &masks).map_err(invalid)?;
        let projections = self
            .project_postings_for_partitions(&routing)
            .map_err(invalid)?;
        let bigram_projections = self
            .project_bigrams_for_partitions(&routing)
            .map_err(invalid)?;
        let relation_projections = self
            .project_relations_for_partitions(&routing)
            .map_err(invalid)?;
        let sorting_projections = self
            .project_sorting_for_partitions(&routing)
            .map_err(invalid)?;
        let domain_projections = self
            .project_domains_for_partitions(&routing)
            .map_err(invalid)?;
        let inference_projections = self
            .project_inference_for_partitions(&routing, &domain_projections)
            .map_err(invalid)?;
        let mut parts = Vec::with_capacity(selections.len());
        for ((((((partition, projection), bigrams), relations), sorting), domains), inference) in
            selections
                .iter()
                .zip(projections)
                .zip(bigram_projections)
                .zip(relation_projections)
                .zip(sorting_projections)
                .zip(domain_projections)
                .zip(inference_projections)
        {
            if cancelled() {
                return Err(StorageError::Cancelled);
            }
            let mut compacted = self
                .compact_partition_with_postings(
                    &routing,
                    parts.len(),
                    &partition.files,
                    projection,
                    bigrams,
                    relations,
                    sorting,
                    domains,
                    inference,
                )
                .map_err(invalid)?;
            for file in &mut compacted.files {
                if let Some(current) = catalog.file(file.entry.id)? {
                    file.entry.relative_path = current.relative_path.clone();
                }
            }
            parts.push(compacted);
        }
        Ok(parts)
    }

    /// Reorder a dense bounded shard without changing its physical slot count.
    /// Used to put a verified replacement into the old record's local slot.
    pub(crate) fn remap_dense(&self, new_slots: &[RecordSlot]) -> Result<Self, String> {
        if new_slots.len() != self.record_count() {
            return Err("slot remap coverage mismatch".into());
        }
        let active = ActiveRecords::full_selection(self.record_count())?;
        Ok(self
            .compact_active_mapped(
                &active,
                self.record_count(),
                |_| true,
                Some(new_slots),
                None,
            )?
            .index)
    }

    fn compact_active_mapped(
        &self,
        active: &ActiveRecords,
        max_scanned_records: usize,
        retain_file: impl FnMut(&FileEntry) -> bool,
        new_slots: Option<&[RecordSlot]>,
        posting_projection: Option<PostingProjection>,
    ) -> Result<CompactedSegment, String> {
        if active.slot_count() as usize != self.record_count() {
            return Err("compaction active-mask coverage mismatch".into());
        }
        if self.record_count() > max_scanned_records {
            return Err("compaction record budget exceeded".into());
        }
        if new_slots.is_some_and(|slots| slots.len() != self.record_count()) {
            return Err("slot remap coverage mismatch".into());
        }
        let mut old_to_new = Vec::with_capacity(self.record_count());
        let mut next = 0u32;
        for slot in 0..self.record_count() {
            let record = self
                .record(RecordSlot(slot as u32))
                .ok_or("compaction record row missing")?;
            if record.slot.0 as usize != old_to_new.len() {
                return Err("compaction record slots out of order".into());
            }
            if active.contains(record.slot)? {
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
        let index = self.compact_with_mapping(
            &SlotMap::Dense(&old_to_new),
            next,
            retain_file,
            posting_projection,
            None,
            None,
            None,
            None,
            None,
            None,
        )?;
        Ok(CompactedSegment {
            #[cfg(test)]
            active: ActiveRecords::full_selection(index.record_count())?,
            index,
            old_to_new,
        })
    }

    pub(crate) fn compact_partition_with_postings(
        &self,
        routing: &PartitionRouting,
        part: usize,
        selected_files: &HashSet<FileId>,
        projection: PostingProjection,
        bigrams: BigramProjection,
        relations: Vec<RelationContribution>,
        sorting: SortCollection,
        domains: DomainProjection,
        inference: Option<InferenceState>,
    ) -> Result<IndexedCollectionSegment, String> {
        let selected = routing
            .selected
            .get(part)
            .ok_or("partition routing part missing")?;
        let next = u32::try_from(selected.len()).map_err(|_| "partition slot overflow")?;
        let mut file_indices = selected_files
            .iter()
            .map(|id| {
                self.file_index
                    .get(id)
                    .copied()
                    .ok_or_else(|| "partition file missing".to_owned())
            })
            .collect::<Result<Vec<_>, _>>()?;
        file_indices.sort_unstable();
        self.compact_with_mapping(
            &SlotMap::Partition { routing, part },
            next,
            |entry| selected_files.contains(&entry.id),
            Some(projection),
            Some(bigrams),
            Some(relations),
            Some(sorting),
            Some(domains),
            Some(inference),
            Some(&file_indices),
        )
    }

    fn compact_with_mapping(
        &self,
        slot_map: &SlotMap<'_>,
        next: u32,
        mut retain_file: impl FnMut(&FileEntry) -> bool,
        posting_projection: Option<PostingProjection>,
        bigram_projection: Option<BigramProjection>,
        relation_projection: Option<Vec<RelationContribution>>,
        sorting_projection: Option<SortCollection>,
        domain_projection: Option<DomainProjection>,
        inference_projection: Option<Option<InferenceState>>,
        selected_files: Option<&[usize]>,
    ) -> Result<IndexedCollectionSegment, String> {
        let mut files = Vec::new();
        let mut file_index = HashMap::new();
        let mut diagnostic_refs: HashMap<FileId, Vec<Option<DiagnosticRef>>> = HashMap::new();
        let mut selected_by_file: HashMap<FileId, Vec<RecordSlot>> = HashMap::new();
        slot_map.for_each_selected(|old, slot| {
            let record = self
                .record(RecordSlot(old as u32))
                .ok_or("compaction record slot out of bounds")?;
            if record.slot.0 as usize != old {
                return Err("compaction record slots out of order".into());
            }
            if let Some(locator) = self.backing.file_locator(RecordSlot(old as u32)) {
                selected_by_file.entry(locator.file).or_default().push(slot);
            }
            Ok(())
        })?;
        let all_files = 0..self.file_count();
        let file_indices: Box<dyn Iterator<Item = usize> + '_> = match selected_files {
            Some(selected) => Box::new(selected.iter().copied()),
            None => Box::new(all_files),
        };
        for index in file_indices {
            let file = self
                .files
                .get(index)
                .ok_or("compaction file index out of bounds")?;
            let retained = retain_file(&file.entry);
            let mut owned = selected_by_file.remove(&file.entry.id).unwrap_or_default();
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
                    let new = slot_map
                        .get(old.0 as usize)
                        .ok_or("diagnostic slot out of bounds")?;
                    diagnostic.record = new;
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
        if !selected_by_file.is_empty() {
            return Err("active record file missing".into());
        }
        let mut records = vec![None; next as usize];
        let mut id_lookup = HashMap::new();
        slot_map.for_each_selected(|old, slot| {
            let record = self
                .record(RecordSlot(old as u32))
                .ok_or("compaction record slot out of bounds")?;
            if record.slot.0 as usize != old {
                return Err("compaction record slots out of order".into());
            }
            let locator = self.backing.file_locator(RecordSlot(old as u32));
            if let Some(locator) = locator {
                if !file_index.contains_key(&locator.file) {
                    return Err("active record file missing".into());
                }
            }
            let mut compacted = record.clone();
            compacted.slot = slot;
            if compacted.class == RecordClass::RawFallback {
                compacted.diagnostic_ref = match compacted.diagnostic_ref.stored_index() {
                    Some(old) => diagnostic_refs
                        .get(&locator.ok_or("fallback record has no file locator")?.file)
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
            Ok(())
        })?;
        let records = records
            .into_iter()
            .collect::<Option<Vec<_>>>()
            .ok_or("compacted record target missing")?;
        let bounds = slot_map.bounds()?;
        let (logical_records, empty_array_records) = if let Some(domains) = domain_projection {
            (domains.logical, domains.empty_array)
        } else {
            (
                project_bitmap(&self.logical_records, slot_map)?,
                project_bitmap(&self.empty_array_records, slot_map)?,
            )
        };
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
                        project_posting(index, slot_map, logical_count, bounds)
                            .map(|projected| (path.clone(), projected))
                    })
                    .collect::<Result<HashMap<_, _>, _>>()?,
            };
            let custom_postings = self
                .custom_postings
                .iter()
                .map(|(key, custom)| {
                    project_posting(&custom.index, slot_map, logical_count, bounds).map(|index| {
                        (
                            key.clone(),
                            CustomPostingIndex {
                                index,
                                complete: custom.complete,
                            },
                        )
                    })
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
            (postings, custom_postings)
        };
        let (raw, fallback_text, custom_text) = if let Some(projected) = bigram_projection {
            (projected.raw, projected.fallback, projected.custom)
        } else {
            let raw = project_bigram(&*self.raw, slot_map, next, bounds)?;
            let fallback = project_bigram(&*self.fallback_text, slot_map, next, bounds)?;
            let custom = self
                .custom_text
                .iter()
                .map(|(key, index)| {
                    project_bigram(index, slot_map, next, bounds).map(|index| (key.clone(), index))
                })
                .collect::<Result<HashMap<_, _>, _>>()?;
            (raw, fallback, custom)
        };
        let sorting = if let Some(sorting) = sorting_projection {
            sorting
        } else {
            project_sorting(&self.sorting, slot_map, &records)?
        };
        let inference = if let Some(inference) = inference_projection {
            inference
        } else {
            project_inference(
                &self.inference,
                slot_map,
                &logical_records,
                records.len(),
                bounds,
            )?
        };
        let relations = if let Some(projected) = relation_projection {
            if projected.len() != self.relations.len()
                || projected
                    .iter()
                    .any(|relation| relation.record_count() != next)
            {
                return Err("partition relation projection mismatch".into());
            }
            projected
        } else {
            self.relations
                .iter()
                .map(|relation| relation.project_slots_with(next, |old| slot_map.get(old as usize)))
                .collect::<Result<Vec<_>, _>>()?
        };
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
        let backing = self.project_backing(slot_map, next)?;
        let mut index = IndexedCollectionSegment {
            id: self.id,
            collection: self.collection,
            source_id: self.source_id.clone(),
            source_version: self.source_version,
            files,
            file_index,
            records,
            backing,
            write_versions: self.write_versions.project(slot_map, next as usize)?,
            raw_covers_logical_fields: self.raw_covers_logical_fields,
            id_lookup,
            identity_order,
            postings: postings.into(),
            inference,
            custom_postings,
            raw: raw.into(),
            fallback_text: fallback_text.into(),
            custom_text,
            logical_records,
            empty_array_records,
            sorting: sorting.into(),
            relations,
            record_parses: self.record_parses,
            failed_parses: self.failed_parses,
            accounting: SegmentAccounting::default(),
        };
        index.recompute_accounting();
        Ok(index)
    }

    fn project_backing(&self, slot_map: &SlotMap<'_>, next: u32) -> Result<SegmentBacking, String> {
        let len = next as usize;
        match &self.backing {
            SegmentBacking::FileRange { locators } => {
                let mut projected = vec![None; len];
                slot_map.for_each_selected(|old, new| {
                    let locator = *locators.get(old).ok_or("file locator slot missing")?;
                    let target = projected
                        .get_mut(new.0 as usize)
                        .ok_or("file locator target missing")?;
                    if target.replace(locator).is_some() {
                        return Err("duplicate file locator target".into());
                    }
                    Ok(())
                })?;
                Ok(SegmentBacking::FileRange {
                    locators: projected
                        .into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or("file locator target missing")?,
                })
            }
            SegmentBacking::Retained {
                blocks,
                addresses,
                links,
                registration,
            } => {
                let mut projected_addresses = vec![None; len];
                let mut projected_links = vec![None; len];
                let mut selected_blocks = Vec::new();
                let mut block_map = HashMap::new();
                slot_map.for_each_selected(|old, new| {
                    let address = *addresses.get(old).ok_or("retained address slot missing")?;
                    let replacement = if let Some(replacement) = block_map.get(&address.block) {
                        *replacement
                    } else {
                        let replacement = u32::try_from(selected_blocks.len())
                            .map_err(|_| "too many retained blocks")?;
                        selected_blocks.push(
                            blocks
                                .get(address.block as usize)
                                .ok_or("retained block missing")?
                                .clone(),
                        );
                        block_map.insert(address.block, replacement);
                        replacement
                    };
                    let target = projected_addresses
                        .get_mut(new.0 as usize)
                        .ok_or("retained address target missing")?;
                    if target
                        .replace(RetainedAddress {
                            block: replacement,
                            offset: address.offset,
                        })
                        .is_some()
                    {
                        return Err("duplicate retained address target".into());
                    }
                    projected_links[new.0 as usize] =
                        *links.get(old).ok_or("retained link slot missing")?;
                    Ok(())
                })?;
                Ok(SegmentBacking::Retained {
                    blocks: selected_blocks,
                    addresses: projected_addresses
                        .into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or("retained address target missing")?,
                    links: projected_links,
                    registration: *registration,
                })
            }
            SegmentBacking::Provider {
                versions,
                links,
                registration,
            } => {
                let mut projected_versions = vec![None; len];
                let mut projected_links = vec![None; len];
                slot_map.for_each_selected(|old, new| {
                    let version = *versions.get(old).ok_or("provider version slot missing")?;
                    let target = projected_versions
                        .get_mut(new.0 as usize)
                        .ok_or("provider version target missing")?;
                    if target.replace(version).is_some() {
                        return Err("duplicate provider version target".into());
                    }
                    projected_links[new.0 as usize] =
                        *links.get(old).ok_or("provider link slot missing")?;
                    Ok(())
                })?;
                Ok(SegmentBacking::Provider {
                    versions: projected_versions
                        .into_iter()
                        .collect::<Option<Vec<_>>>()
                        .ok_or("provider version target missing")?,
                    links: projected_links,
                    registration: *registration,
                })
            }
        }
    }
}
