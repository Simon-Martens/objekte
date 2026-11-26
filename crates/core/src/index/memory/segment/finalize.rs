//! Apply final catalogue status and global inference policy to one segment.

use crate::index::traits::file_catalog::FileCatalogReader;
use crate::index::traits::segment::any_segment_file;
use crate::index::traits::transfer::{StorageError, StorageResult};
use crate::inventory::{FileEntry, FileStatus};
use crate::model::FileId;
use crate::repository::state::GlobalInferenceSummary;

use super::{IndexedCollectionSegment, SegmentBacking};
use crate::index::memory::posting::placement::InferredCoverage;

pub(crate) struct RelinkedFile {
    pub linked: bool,
    pub updated: Option<IndexedCollectionSegment>,
}

fn placed_coverage(decision: &GlobalInferenceSummary, path: &[String]) -> Option<InferredCoverage> {
    use crate::inference::InferenceIndexStatus;

    let field = decision
        .report
        .fields
        .iter()
        .find(|field| field.path == path);
    if field.is_none_or(|field| {
        matches!(
            field.index_status,
            InferenceIndexStatus::Redundant | InferenceIndexStatus::ExplicitOptOut
        )
    }) {
        return None;
    }
    let diagnostics = decision
        .diagnostics
        .iter()
        .find(|diagnostic| diagnostic.path == path);
    Some(InferredCoverage {
        general_values: diagnostics.is_some_and(|diagnostic| diagnostic.value_dictionary_complete),
        short_values: diagnostics.is_some_and(|diagnostic| diagnostic.short_string_candidates),
        length: diagnostics.is_some_and(|diagnostic| diagnostic.array_length_complete),
        empty_string: diagnostics.is_some_and(|diagnostic| diagnostic.empty_string_exact),
    })
}

impl IndexedCollectionSegment {
    pub(crate) fn reconcile_final_metadata(
        &self,
        catalog: &dyn FileCatalogReader,
        decision: Option<&GlobalInferenceSummary>,
    ) -> StorageResult<Option<Self>> {
        let needs_status_refresh = any_segment_file(self, |file| {
            Ok::<_, StorageError>(
                catalog
                    .file(file.id)?
                    .is_some_and(|current| current.status != file.status),
            )
        })?;
        let needs_inference_prune = decision.is_some_and(|decision| {
            self.inference.as_ref().is_some_and(|state| {
                state.indexes.iter().any(|(path, index)| {
                    let field = decision
                        .report
                        .fields
                        .iter()
                        .find(|field| field.path == *path);
                    if field.is_none_or(|field| {
                        field.index_status != crate::inference::InferenceIndexStatus::Built
                    }) {
                        return true;
                    }
                    let diagnostic = decision
                        .diagnostics
                        .iter()
                        .find(|diagnostic| diagnostic.path == *path);
                    index.value_complete
                        && !diagnostic
                            .is_some_and(|diagnostic| diagnostic.value_dictionary_complete)
                        || index.length_complete
                            && !diagnostic
                                .is_some_and(|diagnostic| diagnostic.array_length_complete)
                        || index.empty_string_complete
                            && !diagnostic.is_some_and(|diagnostic| diagnostic.empty_string_exact)
                }) || state.placed.paths().any(|path| {
                    let descriptor = state
                        .placed
                        .get(path, None)
                        .expect("placed posting path")
                        .reader
                        .descriptor();
                    placed_coverage(decision, path)
                        .is_none_or(|target| !target.compatible_with(descriptor.coverage))
                })
            })
        });
        if !needs_status_refresh && !needs_inference_prune {
            return Ok(None);
        }
        let mut index = self.clone();
        for file in &mut index.files {
            if let Some(current) = catalog.file(file.entry.id)? {
                file.entry.status = current.status;
                file.entry.diagnostics = current.diagnostics.clone();
                file.entry.omitted_diagnostics = current.omitted_diagnostics;
            }
        }
        let files = &index.files;
        let positions = &index.file_index;
        index
            .sorting
            .refresh_file_status_for_records(&index.records, |slot| {
                let file = index.backing.file_locator(slot)?.file;
                let status = positions
                    .get(&file)
                    .and_then(|&position| files.get(position))
                    .map_or(FileStatus::Valid, |entry| entry.entry.status);
                Some(status.code())
            })
            .map_err(|error| StorageError::invalid("refresh file status order", error))?;
        if let Some(decision) = decision {
            if let Some(inference) = index.inference.as_mut() {
                if !inference.placed.is_empty() {
                    let paths = inference
                        .placed
                        .paths()
                        .map(<[String]>::to_vec)
                        .collect::<Vec<_>>();
                    for path in paths {
                        let target = placed_coverage(decision, &path);
                        inference
                            .placed
                            .update_inferred(&path, index.records.len(), target)?;
                        if target.is_none_or(|coverage| !coverage.short_values) {
                            inference.short_string_coverage.remove(&path);
                        }
                    }
                }
                for field in &decision.report.fields {
                    if matches!(
                        field.index_status,
                        crate::inference::InferenceIndexStatus::Redundant
                            | crate::inference::InferenceIndexStatus::ExplicitOptOut
                    ) {
                        inference.indexes.remove(&field.path);
                        inference.short_string_coverage.remove(&field.path);
                        continue;
                    }
                    let diagnostics = decision
                        .diagnostics
                        .iter()
                        .find(|diagnostic| diagnostic.path == field.path);
                    let general_values =
                        diagnostics.is_some_and(|diagnostic| diagnostic.value_dictionary_complete);
                    let short_values =
                        diagnostics.is_some_and(|diagnostic| diagnostic.short_string_candidates);
                    let empty_string_exact =
                        diagnostics.is_some_and(|diagnostic| diagnostic.empty_string_exact);
                    if let Some(posting) = inference.indexes.get_mut(&field.path) {
                        let selected_null = posting
                            .values
                            .remove(&crate::index::memory::posting::ScalarKey::Null);
                        if !general_values && !short_values {
                            let empty = empty_string_exact.then(|| {
                                posting.values.remove(
                                        &crate::index::memory::posting::ScalarKey::String(
                                            String::new(),
                                        ),
                                    )
                            });
                            posting.values.clear();
                            if let Some(Some(slots)) = empty {
                                posting.values.insert(
                                    crate::index::memory::posting::ScalarKey::String(String::new()),
                                    slots,
                                );
                            }
                            posting.values_indexed = empty_string_exact;
                        }
                        if let Some(slots) = selected_null {
                            posting
                                .values
                                .insert(crate::index::memory::posting::ScalarKey::Null, slots);
                        }
                        posting.value_complete = general_values;
                        posting.length_complete =
                            diagnostics.is_some_and(|diagnostic| diagnostic.array_length_complete);
                        posting.empty_string_complete = empty_string_exact;
                        if !posting.length_complete {
                            posting.lengths.clear();
                        }
                        posting.finish_stats();
                    }
                    if !short_values {
                        inference.short_string_coverage.remove(&field.path);
                    }
                }
            }
        }
        Ok(Some(index))
    }
}

impl IndexedCollectionSegment {
    /// Keep path-only links on a live file and retain byte ranges only while
    /// they refer to the same source revision.
    pub(crate) fn relink_file(
        &self,
        file: FileId,
        replacement: Option<&FileEntry>,
        root: Option<&std::path::Path>,
        collection_name: &str,
    ) -> StorageResult<Option<RelinkedFile>> {
        const OP: &str = "refresh segment file links";
        let Some(&file_index) = self.file_index.get(&file) else {
            return Ok(None);
        };
        let links = match &self.backing {
            SegmentBacking::Retained { links, .. } | SegmentBacking::Provider { links, .. } => {
                links
            }
            SegmentBacking::FileRange { .. } => return Ok(None),
        };
        if replacement.is_some_and(|entry| entry.id != file) {
            return Err(StorageError::invalid(OP, "replacement file ID differs"));
        }
        let previous = self
            .files
            .get(file_index)
            .ok_or_else(|| StorageError::invalid(OP, "file route points outside the segment"))?;
        let mut kept = Vec::new();
        for &slot in &previous.records {
            let locator = links
                .get(slot.0 as usize)
                .and_then(Option::as_ref)
                .ok_or_else(|| StorageError::invalid(OP, "file slot has no existing link"))?;
            if locator.file != file {
                return Err(StorageError::invalid(
                    OP,
                    "file slot belongs to another file",
                ));
            }
            if replacement
                .is_some_and(|entry| locator.range.is_none() || locator.revision == entry.revision)
            {
                kept.push(slot);
            }
        }
        let linked = !kept.is_empty();
        if replacement == Some(&previous.entry) && kept.len() == previous.records.len() {
            return Ok(Some(RelinkedFile {
                linked,
                updated: None,
            }));
        }

        let mut index = self.clone();
        let links = match &mut index.backing {
            SegmentBacking::Retained { links, .. } | SegmentBacking::Provider { links, .. } => {
                links
            }
            SegmentBacking::FileRange { .. } => unreachable!("checked direct backing"),
        };
        let old_slots = std::mem::take(&mut index.files[file_index].records);
        for &slot in &old_slots {
            let link = &mut links[slot.0 as usize];
            if let (Some(entry), Some(locator)) = (replacement, link)
                && (locator.range.is_none() || locator.revision == entry.revision)
            {
                locator.revision = entry.revision;
            } else {
                links[slot.0 as usize] = None;
            }
        }
        if linked {
            index.files[file_index].entry =
                replacement.expect("kept links have replacement").clone();
            index.files[file_index].records = kept;
        } else {
            index.files.remove(file_index);
            index.file_index.clear();
            for (position, file) in index.files.iter().enumerate() {
                index.file_index.insert(file.entry.id, position);
            }
        }
        index
            .refresh_metadata_sort_slots(old_slots, root, collection_name)
            .map_err(|error| StorageError::invalid(OP, error))?;
        index.recompute_accounting();
        Ok(Some(RelinkedFile {
            linked,
            updated: Some(index),
        }))
    }
}
