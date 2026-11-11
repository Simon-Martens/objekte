//! Exact, bounded source search before a repository index is available.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;

use crate::discovery::discover;
use crate::inventory::{
    DiagnosticRef, FileEntry, FileInventory, RecordClass, RecordEntry, SourceDiagnostic,
    SourceRevision,
};
use crate::measurement::{SourceDiagnostic as QuerySourceDiagnostic, SourceUnavailableKind};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot, Value};
use crate::query::Query;
use crate::record_preparation::finish_build_record;
use crate::reference::{
    RecordContext, bind_filter, bind_fts, eval_bound_fts, eval_filter, eval_unavailable_filter,
    raw_fallback_contains,
};
use crate::repository::{CollectionDefinition, IdentityPolicy, Payload, SearchHit, SearchResult};
use crate::source::{
    ExtractedRecord, PreparedSource, ReaderDiagnostic, SourceFormat, SourceOutcome, SourceRegistry,
    extract_file_outcomes, finalize_extracted_text, read_record_range,
};
use crate::streaming::CancellationToken;
use crate::workload::{ReturnMode, WorkCounters};

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
    pub fn open(root: &Path, definitions: Vec<CollectionDefinition>) -> Result<Self, String> {
        let root = root.canonicalize().map_err(|error| error.to_string())?;
        let sources = SourceRegistry::builtin();
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
        &self.files.root
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
                let path = self.files.root.join(&file.relative_path);
                counters.files_opened += 1;
                counters.distinct_files += 1;
                extract_file_outcomes(
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
                                        Some(read_record_range(&path, file.revision, range)?),
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
                                        file_path: file.relative_path.clone(),
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
                                &self.files.root,
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
                            bytes = Some(read_record_range(&path, file.revision, range)?);
                            counters.files_opened += 1;
                        }
                        let empty = Value::Object(Default::default());
                        let value = extracted.as_ref().map_or(&empty, |record| &record.value);
                        let ctx = RecordContext {
                            value,
                            source_bytes: bytes.as_deref(),
                            record: &entry,
                            file: &file,
                            collection_name: &collection.definition.name,
                            repository_root: &self.files.root,
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
                                    read_record_range(&path, file.revision, range)
                                })?;
                                Some(Payload::Bytes(raw))
                            }
                        };
                        let hit = SearchHit {
                            collection: collection.definition.name.clone(),
                            record_id: entry.id.0.clone(),
                            generation: 1,
                            file_id: file.id.0,
                            file_path: file.relative_path.clone(),
                            file_size: file.revision.size,
                            file_created_ns: file.created_ns,
                            byte_start: range.start,
                            byte_end: range.end,
                            modified_ns: file.revision.modified_ns,
                            matched_fields,
                            file_status: file.status,
                            record_class: class,
                            diagnostic_ref,
                            payload,
                        };
                        let key = (
                            hit.collection.clone(),
                            hit.record_id.clone(),
                            hit.file_id,
                            hit.byte_start,
                        );
                        winners.insert(key, hit);
                        if winners.len() > retain {
                            winners.pop_last();
                        }
                        Ok(())
                    },
                )?;
                counters.bytes_read += file.revision.size;
                for hit in winners.values_mut().filter(|hit| hit.file_id == file.id.0) {
                    hit.file_status = file.status;
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
            if !matches!(
                definition.format,
                SourceFormat::Json | SourceFormat::JsonDecodedScalars
            ) {
                return Err("relative-path identity requires a single-record JSON reader".into());
            }
            let relative = path
                .strip_prefix(root.join(&definition.source_path))
                .map_err(|error| error.to_string())?;
            RecordId::intrinsic(format!("path:{}", crate::path_repr::query_path(relative)))
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
mod tests {
    use super::*;
    use crate::model::LogicalType;
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery, QueryOptions, parse_filter};
    use crate::reference::FieldDefinition;
    use crate::repository::{Repository, RepositoryOptions, SnapshotMode};

    #[test]
    fn source_scan_matches_reference_search_before_an_index_exists() {
        let root = std::env::temp_dir().join(format!(
            "objekte-source-scan-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(
            root.join("data/a.json"),
            br#"{"title":"Alpha","meta":{"tag":"x"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("data/b.json"), br#"{"title":"Beta"}"#).unwrap();
        std::fs::write(root.join("data/c.json"), br#"{"title":"broken""#).unwrap();
        let mut definition = CollectionDefinition::folder("files", "data", SourceFormat::Json);
        definition
            .schema
            .register_field(FieldDefinition {
                path: vec!["meta".into(), "tag".into()],
                kind: LogicalType::String,
                required: false,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
        let scan = SourceScan::open(&root, vec![definition.clone()]).unwrap();
        let indexed = Repository::open_with_options(
            &root,
            vec![definition],
            RepositoryOptions {
                snapshot_mode: SnapshotMode::Disabled,
                ..RepositoryOptions::default()
            },
        )
        .unwrap();
        let mut query = Query {
            collections: vec!["files".into()],
            fts: None,
            filter: None,
            options: QueryOptions {
                sort: Vec::new(),
                limit: 1,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Bytes,
                evidence: true,
            },
        };
        let cancellation = CancellationToken::new();
        for (offset, fts, filter) in [
            (0, None, None),
            (1, None, None),
            (0, Some("Alpha"), None),
            (0, Some("broken"), None),
            (0, None, Some("meta.tag = 'x'")),
        ] {
            query.options.offset = offset;
            query.fts = fts.map(|literal| FtsQuery {
                case_sensitivity: CaseSensitivity::Sensitive,
                mode: FtsMode::Literal,
                literal: literal.into(),
                pattern_parameter: None,
                fields: Vec::new(),
            });
            query.filter = filter.map(|expression| parse_filter(expression).unwrap());
            let expected = indexed.search_reference(&query).unwrap();
            let actual = scan.search(&query, &cancellation).unwrap();
            assert_eq!(
                actual.count, expected.count,
                "offset={offset} fts={fts:?} filter={filter:?}"
            );
            assert_eq!(
                actual.hits, expected.hits,
                "offset={offset} fts={fts:?} filter={filter:?}"
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
