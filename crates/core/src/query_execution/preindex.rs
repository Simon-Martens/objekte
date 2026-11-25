//! Exact, bounded source search before a repository index is available.

use crate::records::RecordIdSlot;
use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::construction::file_preparation::finish_build_record;
use crate::inventory::discovery::discover;
use crate::inventory::{
    DiagnosticRef, FileEntry, FileInventory, RecordClass, RecordEntry, SourceDiagnostic,
    SourceRevision,
};
use crate::measurement::WorkCounters;
use crate::measurement::{SourceDiagnostic as QuerySourceDiagnostic, SourceUnavailableKind};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot, Value};
use crate::query::Query;
use crate::query::binding::filter::bind_filter;
use crate::query::binding::fts::bind_fts;
use crate::query::options::ReturnMode;
use crate::query::semantics::record::RecordContext;
use crate::query_execution::streaming::CancellationToken;
use crate::query_results::{Payload, SearchHit, SearchResult};
use crate::reference::{eval_unavailable_filter, filter::eval_filter};
use crate::reference::{fts::eval_bound_fts, raw_fallback_contains};
use crate::repository::configuration::{CollectionDefinition, IdentityPolicy};
use crate::source::access::read_record_range_for_search;
use crate::source::{
    ExtractedRecord, ReaderDiagnostic, SourceOutcome, extraction::extract_file_outcomes_for_search,
    registry::PreparedSource, registry::SourceRegistry, text::finalize_extracted_text,
};

struct ScanCollection {
    definition: CollectionDefinition,
    source: PreparedSource,
    files: Vec<FileId>,
}

/// A file inventory and schema prepared without parsing records or constructing
/// postings. Each request scans the selected source files directly.
pub struct SourceScan {
    files: FileInventory,
    collections: Vec<ScanCollection>,
}

impl SourceScan {
    pub fn open(
        root: &Path,
        definitions: Vec<CollectionDefinition>,
        sources: SourceRegistry,
    ) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let mut files = FileInventory::new(root.clone());
        let mut collections = Vec::with_capacity(definitions.len());
        for (number, definition) in definitions.into_iter().enumerate() {
            if collections
                .iter()
                .any(|existing: &ScanCollection| existing.definition.name == definition.name)
            {
                return Err(format!("duplicate collection: {}", definition.name));
            }
            let collection =
                CollectionId(u32::try_from(number).map_err(|_| "too many collections")?);
            let source = sources.resolve(&definition.format)?;
            let mut members = Vec::new();
            for path in discover(
                &root,
                &definition.source_path,
                &definition.discovery,
                &source,
            )? {
                let relative = path
                    .strip_prefix(&root)
                    .map_err(|error| error.to_string())?;
                let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
                let revision = SourceRevision::from_metadata(&metadata)?;
                let created_ns = metadata
                    .created()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
                    .map(|duration| duration.as_nanos() as i128);
                members.push(files.add(relative.to_owned(), revision, created_ns, collection)?);
            }
            collections.push(ScanCollection {
                definition,
                source,
                files: members,
            });
        }
        Ok(Self { files, collections })
    }

    pub fn root(&self) -> &Path {
        &self.files.required_root()
    }

    pub fn collections(&self) -> impl Iterator<Item = &str> {
        self.collections
            .iter()
            .map(|collection| collection.definition.name.as_str())
    }

    pub fn file_paths(&self) -> impl Iterator<Item = &Path> {
        self.files
            .files
            .iter()
            .map(|file| file.relative_path.as_path())
    }

    pub fn file_paths_for_collection(&self, name: &str) -> impl Iterator<Item = &Path> {
        self.collections
            .iter()
            .filter(move |collection| collection.definition.name == name)
            .flat_map(|collection| collection.files.iter())
            .map(|file| self.files.files[file.0 as usize].relative_path.as_path())
    }

    pub fn search(
        &self,
        query: &Query,
        cancellation: &CancellationToken,
    ) -> Result<SearchResult, String> {
        let resolved = query.resolve_current_clock()?;
        let query = resolved.as_ref();
        if !query.options.sort.is_empty() {
            return Err("SORT is available when the index finishes loading".into());
        }
        let retain = query.options.offset.saturating_add(query.options.limit);
        let mut winners = BTreeMap::<(String, String, u32, u64), SearchHit>::new();
        let mut counters = WorkCounters::default();
        let mut total = 0usize;
        let mut selected = 0usize;
        for (number, collection) in self.collections.iter().enumerate() {
            if !query.collections.is_empty()
                && !query.collections.contains(&collection.definition.name)
            {
                continue;
            }
            selected += 1;
            let filter = query
                .filter
                .as_ref()
                .map(|expression| bind_filter(expression, &collection.definition.schema))
                .transpose()?;
            let fts = query
                .fts
                .as_ref()
                .map(|expression| bind_fts(expression, &collection.definition.schema))
                .transpose()?;
            let collection_id = CollectionId(number as u32);
            let mut next_slot = 0u32;
            for &file_id in &collection.files {
                if cancellation.is_cancelled() {
                    return Err("source scan cancelled".into());
                }
                let mut file = self.files.files[file_id.0 as usize].clone();
                let path = self.files.required_root().join(&file.relative_path);
                counters.files_opened += 1;
                counters.distinct_files += 1;
                extract_file_outcomes_for_search(
                    &path,
                    collection.source.reader(),
                    file.revision,
                    |outcome| {
                        if cancellation.is_cancelled() {
                            return Err("source scan cancelled".into());
                        }
                        let (range, mut extracted, mut bytes, issue) = match outcome {
                            SourceOutcome::Logical { range, mut record } => {
                                match finish_build_record(
                                    &collection.definition.schema,
                                    &mut record,
                                ) {
                                    Ok(()) => {
                                        finalize_extracted_text(
                                            collection.source.reader(),
                                            &mut record,
                                            collection.definition.schema.has_converted_fields(),
                                        );
                                        (range, Some(record), None, None)
                                    }
                                    Err(message) => (
                                        range,
                                        None,
                                        Some(read_record_range_for_search(
                                            collection.source.reader(),
                                            &path,
                                            file.revision,
                                            range,
                                            None,
                                        )?),
                                        Some(ReaderDiagnostic::new(
                                            crate::source::DiagnosticCategory::LogicalValidation,
                                            "logical.validation",
                                            message,
                                        )),
                                    ),
                                }
                            }
                            SourceOutcome::Malformed {
                                range,
                                diagnostic,
                                bytes,
                            } => (range, None, Some(bytes), Some(diagnostic)),
                            SourceOutcome::Unreadable { diagnostic } => {
                                counters.source_unavailable_events += 1;
                                if counters.source_diagnostics.len() < 32 {
                                    counters.source_diagnostics.push(QuerySourceDiagnostic {
                                        file_id: file.id.0,
                                        file_path: file.relative_path.to_path_buf(),
                                        kind: SourceUnavailableKind::Unreadable,
                                        message: diagnostic.message,
                                    });
                                }
                                return Ok(());
                            }
                        };
                        let slot = RecordSlot(next_slot);
                        next_slot = next_slot.checked_add(1).ok_or("too many source records")?;
                        let class = if extracted.is_some() {
                            RecordClass::Logical
                        } else {
                            RecordClass::RawFallback
                        };
                        let diagnostic_ref = if let Some(issue) = issue {
                            file.record_diagnostic(SourceDiagnostic::from_reader(
                                file.id,
                                collection_id,
                                collection.source.id(),
                                collection.source.version(),
                                range,
                                Some(slot),
                                issue,
                            ))?
                        } else {
                            DiagnosticRef(0)
                        };
                        let id = if let Some(record) = &extracted {
                            record_id(
                                &self.files.required_root(),
                                &collection.definition,
                                &collection.source,
                                &path,
                                &file,
                                range.start,
                                record,
                            )?
                        } else {
                            RecordId::fallback(
                                collection_id,
                                collection.source.id(),
                                collection.source.version(),
                                &file.relative_path,
                                file.id,
                                file.revision.modified_ns,
                                range.start,
                                range.end,
                            )
                        };
                        let entry = RecordEntry {
                            slot,
                            id,
                            file: file.id,
                            range,
                            revision: file.revision,
                            class,
                            diagnostic_ref,
                        };
                        counters.candidates += 1;
                        counters.scanned_records += 1;
                        if extracted.is_some() {
                            counters.record_parses += 1;
                        }
                        if filter.is_some() && bytes.is_none() {
                            bytes = Some(read_record_range_for_search(
                                collection.source.reader(),
                                &path,
                                file.revision,
                                range,
                                None,
                            )?);
                            counters.files_opened += 1;
                        }
                        let empty = Value::Object(Default::default());
                        let value = extracted.as_ref().map_or(&empty, |record| &record.value);
                        let ctx = RecordContext {
                            value,
                            source_bytes: bytes.as_deref(),
                            record: &entry,
                            range: (&entry).record_range(),
                            file: Some(&file),
                            collection_name: &collection.definition.name,
                            repository_root: Some(&self.files.required_root()),
                        };
                        let mut evidence = HashSet::new();
                        let fts_match = match (&fts, &extracted) {
                            (Some(fts), Some(record)) => {
                                eval_bound_fts(fts, &record.text, &ctx, &mut evidence)?
                            }
                            (Some(fts), None) if fts.fields.is_empty() => {
                                let raw = bytes.as_deref().ok_or("missing raw fallback bytes")?;
                                let matched =
                                    raw_fallback_contains(fts, &String::from_utf8_lossy(raw));
                                if matched && query.options.evidence {
                                    evidence.insert("@record.raw_text".into());
                                }
                                matched
                            }
                            (Some(_), None) => false,
                            (None, _) => true,
                        };
                        let filter_match = match (&filter, class) {
                            (Some(filter), RecordClass::Logical) => {
                                eval_filter(filter, &ctx, &mut evidence)?
                            }
                            (Some(filter), RecordClass::RawFallback) => {
                                eval_unavailable_filter(filter, &ctx, &mut evidence)? == Some(true)
                            }
                            (None, _) => true,
                        };
                        if !(fts_match && filter_match) {
                            return Ok(());
                        }
                        total = total.checked_add(1).ok_or("too many matches")?;
                        if retain == 0 {
                            return Ok(());
                        }
                        let mut matched_fields = if query.options.evidence {
                            evidence.into_iter().collect::<Vec<_>>()
                        } else {
                            Vec::new()
                        };
                        matched_fields.sort();
                        let payload = match query.options.return_mode {
                            ReturnMode::Locator => None,
                            ReturnMode::Logical if let Some(record) = extracted.take() => {
                                Some(Payload::Logical(record.value))
                            }
                            ReturnMode::Logical => {
                                Some(Payload::LogicalUnavailable { diagnostic_ref })
                            }
                            ReturnMode::Bytes => {
                                let raw = bytes.take().map(Ok).unwrap_or_else(|| {
                                    read_record_range_for_search(
                                        collection.source.reader(),
                                        &path,
                                        file.revision,
                                        range,
                                        None,
                                    )
                                })?;
                                Some(Payload::Bytes(raw))
                            }
                        };
                        let hit = SearchHit {
                            collection: collection.definition.name.clone(),
                            record_id: entry.id.0.to_string(),
                            generation: 1,
                            file: Some(crate::files::RecordFileInfo::from_entry(
                                &file,
                                Some(range),
                            )),
                            matched_fields,
                            record_class: class,
                            diagnostic_ref,
                            payload,
                        };
                        let key = (
                            hit.collection.clone(),
                            hit.record_id.clone(),
                            hit.file.as_ref().expect("preindex file origin").file_id.0,
                            hit.file
                                .as_ref()
                                .and_then(|file| file.range)
                                .expect("preindex range")
                                .start,
                        );
                        winners.insert(key, hit);
                        if winners.len() > retain {
                            winners.pop_last();
                        }
                        Ok(())
                    },
                )?;
                counters.bytes_read += file.revision.size;
                for hit in winners.values_mut().filter(|hit| {
                    hit.file
                        .as_ref()
                        .is_some_and(|origin| origin.file_id == file.id)
                }) {
                    if let Some(origin) = &mut hit.file {
                        origin.status = file.status;
                    }
                }
            }
        }
        if selected == 0 {
            return Err("unknown or unloaded collection scope".into());
        }
        counters.total_matches = total as u64;
        counters.match_count_complete = counters.source_unavailable_events == 0;
        let hits = winners
            .into_values()
            .skip(query.options.offset)
            .collect::<Vec<_>>();
        counters.returned_records = hits.len() as u64;
        Ok(SearchResult {
            count: (query.options.count && counters.match_count_complete).then_some(total),
            hits,
            counters,
        })
    }
}

fn record_id(
    root: &Path,
    definition: &CollectionDefinition,
    source: &PreparedSource,
    path: &Path,
    file: &FileEntry,
    start: u64,
    record: &ExtractedRecord,
) -> Result<RecordId, String> {
    match &definition.identity {
        IdentityPolicy::Filename => RecordId::intrinsic(
            path.file_stem()
                .and_then(|stem| stem.to_str())
                .ok_or("non-UTF-8 filename identity")?,
        ),
        IdentityPolicy::RelativePath => {
            if !source.reader().supports_relative_path_identity() {
                return Err("relative-path identity requires a reader that supports it".into());
            }
            let relative = path
                .strip_prefix(root.join(&definition.source_path))
                .map_err(|error| error.to_string())?;
            RecordId::intrinsic(format!(
                "path:{}",
                crate::model::path_representation::query_path(relative)
            ))
        }
        IdentityPolicy::Generated => Ok(RecordId::generated(
            file.id,
            file.revision.modified_ns,
            start,
        )),
        IdentityPolicy::Intrinsic(field) => RecordId::intrinsic(
            source
                .reader()
                .intrinsic_id(record, field)?
                .ok_or_else(|| format!("missing intrinsic record ID at {}", field.join(".")))?,
        ),
    }
}

#[cfg(test)]
mod tests;
