//! Owned record-batch construction and admission contributions.

use std::ops::Range;
use std::path::Path;

use serde::Serialize;

use crate::construction::file_preparation::finish_build_record;
use crate::index::bigram::BigramAlphabet;
use crate::index::traits::StorageFactory;
use crate::index::traits::transfer::{StorageError, StorageResult};
#[cfg(test)]
use crate::index::traits::worker::DirectRecordInput;
use crate::index::traits::worker::{
    DirectRecordBatch, FileWorkerBatch, FileWorkerInput, PreparedDirectBatch, PreparedIndexes,
    WorkerWriter,
};
use crate::inference::InferenceConfig;
use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SharedPath,
    SourceDiagnostic, SourceRevision,
};
use crate::model::{ArrayValue, CollectionId, FileId, RecordId, RecordSlot, Value};
use crate::query_access::membership::MembershipView;
use crate::repository::configuration::{CollectionDefinition, IdentityPolicy};
use crate::repository::statistics::BigramConstructionStats;
use crate::schema::{Schema, operation::BindingOperation};
use crate::source::access::read_record_range_for_build;
use crate::source::{
    DiagnosticCategory, ExtractedRecord, ReaderDiagnostic, SourceOutcome, TextDerivation,
    extraction::SourceBufferProfile, extraction::SourceBuildSample,
    extraction::extract_file_build_outcomes_hashed_with_buffer,
    extraction::extract_framed_build_outcomes, extraction::trim_oversized_build_buffer,
    registry::PreparedSource,
};

pub(crate) fn map_binding_value(operation: &dyn BindingOperation, value: &Value) -> Option<Value> {
    if let Value::Array(array) = value {
        let values = array
            .values
            .iter()
            .map(|item| map_binding_value(operation, item))
            .collect::<Option<Vec<_>>>()?;
        return Some(Value::Array(ArrayValue::from_unrestricted(values)));
    }
    operation.map_value(value)
}

pub(crate) fn string_values(value: &Value) -> impl Iterator<Item = &str> {
    let single = match value {
        Value::String(text) | Value::Id(text) => Some(text.as_str()),
        _ => None,
    };
    let array = match value {
        Value::Array(array) => Some(array),
        _ => None,
    };
    single.into_iter().chain(
        array
            .into_iter()
            .flat_map(|array| array.values.iter())
            .filter_map(|value| match value {
                Value::String(text) | Value::Id(text) => Some(text.as_str()),
                _ => None,
            }),
    )
}

/// Built-in identity and case-fold registrations for the same logical field
/// have identical candidate grams after mandatory index folding.
pub(crate) fn canonical_custom_text_key(
    schema: &Schema,
    path: &[String],
    name: &str,
) -> (Vec<String>, String) {
    let canonical = if schema
        .custom
        .contains_key(&(path.to_vec(), name.to_owned()))
    {
        schema
            .custom
            .keys()
            .filter(|(candidate_path, _)| candidate_path == path)
            .map(|(_, candidate_name)| candidate_name.as_str())
            .min()
            .unwrap_or(name)
    } else {
        name
    };
    (path.to_vec(), canonical.to_owned())
}

#[derive(Clone)]
pub(crate) struct BatchFile {
    pub(crate) entry: FileEntry,
    pub(crate) records: Range<usize>,
    pub(crate) first_range: bool,
}

/// Immutable discovery metadata shared by all construction workers. Mutable
/// statuses and diagnostics stay in the coordinator-owned inventory.
#[derive(Clone)]
pub(crate) struct SourceFileInput {
    pub(crate) id: FileId,
    pub(crate) relative_path: SharedPath,
    pub(crate) revision: SourceRevision,
    pub(crate) created_ns: Option<i128>,
}

impl SourceFileInput {
    pub(crate) fn from_entry(entry: &FileEntry) -> Self {
        Self {
            id: entry.id,
            relative_path: entry.relative_path.clone(),
            revision: entry.revision,
            created_ns: entry.created_ns,
        }
    }

    pub(super) fn for_collection(&self, collection: CollectionId) -> FileEntry {
        FileEntry {
            id: self.id,
            relative_path: self.relative_path.clone().into(),
            revision: self.revision,
            created_ns: self.created_ns,
            collections: vec![collection],
            status: FileStatus::Valid,
            diagnostics: Vec::new(),
            omitted_diagnostics: 0,
        }
    }
}

/// Bound each job by both file count and discovered source bytes. A single
/// oversized source is admitted alone; its framing contract may need that
/// entire file in memory.
pub(crate) fn file_batches<'a>(
    files: &'a [FileId],
    size_of: impl Fn(FileId) -> Result<u64, String>,
) -> Result<Vec<&'a [FileId]>, String> {
    const MAX_FILES: usize = 512;
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    let mut batches = Vec::new();
    let mut start = 0;
    while start < files.len() {
        let mut end = start;
        let mut bytes = 0u64;
        while end < files.len() && end - start < MAX_FILES {
            let size = size_of(files[end])?;
            if end > start && bytes.saturating_add(size) > MAX_BYTES {
                break;
            }
            bytes = bytes.saturating_add(size);
            end += 1;
            if bytes >= MAX_BYTES {
                break;
            }
        }
        batches.push(&files[start..end]);
        start = end;
    }
    Ok(batches)
}

/// All expensive per-record projection is complete before this crosses the
/// worker/coordinator boundary. Slots remain local until ordered admission.
pub(crate) struct BatchContribution {
    pub(crate) files: Vec<BatchFile>,
    /// Complete physical-file digests captured during this batch's own reads.
    /// Framed chunks only report a digest once, after the whole read succeeds.
    pub(crate) file_fingerprints: Vec<(FileId, [u8; 32])>,
    pub(crate) records: Vec<RecordEntry>,
    pub(crate) indexes: Box<dyn PreparedIndexes>,
    pub record_parses: u64,
    pub failed_parses: u64,
    pub(crate) source_reads: u64,
    pub(crate) source_bytes_read: u64,
    pub(crate) profile: BuildWorkProfile,
}

/// Optional sampled work attribution. The source fields cover every 64th
/// physical file; component fields cover up to one in 64 records of those
/// files. They are not exclusive wall-clock construction intervals.
#[derive(Clone, Debug, Default, Serialize)]
pub struct BuildWorkProfile {
    pub sampled_files: u64,
    pub sampled_file_bytes: u64,
    pub sampled_read_calls: u64,
    pub sampled_source_records: u64,
    pub sampled_callback_ns: u64,
    pub source_open_ns: u64,
    pub source_read_ns: u64,
    pub source_hash_ns: u64,
    pub source_frame_ns: u64,
    pub source_extraction_ns: u64,
    pub sampled_component_records: u64,
    pub validation_ns: u64,
    pub postings_ns: u64,
    pub inference_ns: u64,
    pub relations_ns: u64,
    pub custom_postings_ns: u64,
    pub text_bigram_ns: u64,
    pub sorting_ns: u64,
    pub custom_text_ns: u64,
    pub batch_finish_ns: u64,
    pub batches: u64,
    pub input_buffer_files_read: u64,
    pub input_buffer_growth_files: u64,
    pub input_buffer_peak_capacity_bytes: u64,
    pub input_buffer_oversized_drops: u64,
    pub input_buffer_batch_end_capacity_bytes: u64,
}

impl BuildWorkProfile {
    pub(crate) fn absorb(&mut self, other: &Self) {
        self.sampled_files += other.sampled_files;
        self.sampled_file_bytes += other.sampled_file_bytes;
        self.sampled_read_calls += other.sampled_read_calls;
        self.sampled_source_records += other.sampled_source_records;
        self.sampled_callback_ns += other.sampled_callback_ns;
        self.source_open_ns += other.source_open_ns;
        self.source_read_ns += other.source_read_ns;
        self.source_hash_ns += other.source_hash_ns;
        self.source_frame_ns += other.source_frame_ns;
        self.source_extraction_ns += other.source_extraction_ns;
        self.sampled_component_records += other.sampled_component_records;
        self.validation_ns += other.validation_ns;
        self.postings_ns += other.postings_ns;
        self.inference_ns += other.inference_ns;
        self.relations_ns += other.relations_ns;
        self.custom_postings_ns += other.custom_postings_ns;
        self.text_bigram_ns += other.text_bigram_ns;
        self.sorting_ns += other.sorting_ns;
        self.custom_text_ns += other.custom_text_ns;
        self.batch_finish_ns += other.batch_finish_ns;
        self.batches += other.batches;
        self.input_buffer_files_read += other.input_buffer_files_read;
        self.input_buffer_growth_files += other.input_buffer_growth_files;
        self.input_buffer_peak_capacity_bytes = self
            .input_buffer_peak_capacity_bytes
            .max(other.input_buffer_peak_capacity_bytes);
        self.input_buffer_oversized_drops += other.input_buffer_oversized_drops;
        self.input_buffer_batch_end_capacity_bytes += other.input_buffer_batch_end_capacity_bytes;
    }
}

impl BatchContribution {
    pub(crate) fn bigram_storage_stats(&self) -> BigramConstructionStats {
        self.indexes.bigram_storage_stats()
    }

    pub(crate) fn file_fingerprints(&self) -> &[(FileId, [u8; 32])] {
        &self.file_fingerprints
    }

    pub(crate) fn logical_slots(&self) -> MembershipView<'_> {
        self.indexes.logical_slots()
    }

    pub(crate) fn new_file_count(&self) -> usize {
        self.files.iter().filter(|file| file.first_range).count()
    }

    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }
}

struct BatchBuilder<'a, W: WorkerWriter> {
    root: &'a Path,
    definition: &'a CollectionDefinition,
    collection: CollectionId,
    source: &'a PreparedSource,
    records: Vec<RecordEntry>,
    worker: W,
    #[cfg(feature = "detailed-profiling")]
    profile_current_file: bool,
    #[cfg(feature = "detailed-profiling")]
    profile_record_index: usize,
    profile: BuildWorkProfile,
}

impl<'a, W: WorkerWriter> BatchBuilder<'a, W> {
    fn new(batch: &FileWorkerBatch<'a>, worker: W) -> Self {
        Self {
            root: batch.root,
            definition: batch.definition,
            collection: batch.collection,
            source: batch.source,
            records: Vec::new(),
            worker,
            #[cfg(feature = "detailed-profiling")]
            profile_current_file: false,
            #[cfg(feature = "detailed-profiling")]
            profile_record_index: 0,
            profile: BuildWorkProfile::default(),
        }
    }

    fn next_slot(&self) -> Result<RecordSlot, String> {
        Ok(RecordSlot(
            u32::try_from(self.records.len()).map_err(|_| "too many batch records")?,
        ))
    }

    fn fallback(
        &mut self,
        file: &mut FileEntry,
        range: ByteRange,
        bytes: &[u8],
        issue: ReaderDiagnostic,
    ) -> Result<(), String> {
        let diagnostic = SourceDiagnostic::from_reader(
            file.id,
            self.collection,
            self.source.id(),
            self.source.version(),
            range,
            None,
            issue,
        );
        let reference = file.record_diagnostic(diagnostic)?;
        let id = fallback_build_id(self.collection, self.source, file, range);
        let slot = self.next_slot()?;
        self.records.push(RecordEntry {
            slot,
            id,
            file: file.id,
            range,
            revision: file.revision,
            class: RecordClass::RawFallback,
            diagnostic_ref: reference,
        });
        if let Some(index) = reference.stored_index() {
            file.diagnostics[index].record = Some(slot);
        }
        self.worker.add_fallback(
            slot,
            &self.records.last().unwrap().id,
            bytes,
            file,
            range,
            self.root,
        )?;
        if std::str::from_utf8(bytes).is_err() {
            file.record_diagnostic(SourceDiagnostic::from_reader(
                file.id,
                self.collection,
                self.source.id(),
                self.source.version(),
                range,
                Some(slot),
                ReaderDiagnostic::new(
                    DiagnosticCategory::Encoding,
                    "source.utf8_lossy",
                    "invalid UTF-8 replaced in raw search text",
                ),
            ))?;
        }
        Ok(())
    }

    fn logical(
        &mut self,
        file: &mut FileEntry,
        path: &Path,
        revision: SourceRevision,
        range: ByteRange,
        mut extracted: ExtractedRecord,
        source_bytes: Option<&[u8]>,
    ) -> Result<(), String> {
        #[cfg(feature = "detailed-profiling")]
        let sample = self.profile_current_file && self.profile_record_index.is_multiple_of(64);
        #[cfg(feature = "detailed-profiling")]
        {
            self.profile_record_index += 1;
        }

        #[cfg(not(feature = "detailed-profiling"))]
        let sample = false;
        if sample {
            self.profile.sampled_component_records += 1;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        if let Err(error) = finish_build_record(&self.definition.schema, &mut extracted) {
            let issue = ReaderDiagnostic::new(
                DiagnosticCategory::LogicalValidation,
                "logical.validation",
                error,
            );
            return if let Some(bytes) = source_bytes {
                self.fallback(file, range, bytes, issue)
            } else {
                let bytes = read_record_range_for_build(self.source.reader(), path, range)?;
                self.fallback(file, range, &bytes, issue)
            };
        }
        if let Some(started) = stage_started {
            self.profile.validation_ns += started.elapsed().as_nanos() as u64;
        }
        let id = build_record_id(
            self.root,
            self.definition,
            self.source,
            file,
            path,
            range,
            &extracted,
        )?;
        let slot = self.next_slot()?;
        let text_derivation = self.source.reader().text_derivation();
        self.worker.add_logical_indexes(
            slot,
            &id,
            &extracted,
            Some((file, Some(range))),
            Some(self.root),
            text_derivation,
            sample,
            &mut self.profile,
        )?;
        self.records.push(RecordEntry {
            slot,
            id,
            file: file.id,
            range,
            revision,
            class: RecordClass::Logical,
            diagnostic_ref: DiagnosticRef(0),
        });
        Ok(())
    }
}

/// Prepare typed records through the same logical index admission as file
/// records. The returned bodies move into immutable segment storage.
#[cfg(test)]
pub(crate) fn build_retained_contribution(
    mut inputs: Vec<DirectRecordInput>,
    declaration: &crate::repository::configuration::CollectionDeclaration,
    options: &crate::repository::configuration::RepositoryOptions,
) -> Result<
    (
        Vec<crate::records::RecordHeader>,
        Vec<crate::index::segment::RetainedBody>,
        BatchContribution,
    ),
    String,
> {
    let batch =
        crate::index::traits::default_factory().prepare_direct_records(DirectRecordBatch {
            inputs: &mut inputs,
            declaration,
            options: *options,
            retain_bodies: true,
            completed_records: &mut 0,
        })?;
    Ok((batch.headers, batch.bodies, batch.contribution))
}

pub(crate) fn prepare_direct_records(
    batch: DirectRecordBatch<'_>,
    mut worker: impl WorkerWriter,
) -> StorageResult<PreparedDirectBatch> {
    let DirectRecordBatch {
        inputs,
        declaration,
        retain_bodies,
        completed_records,
        ..
    } = batch;
    let schema = &declaration.schema;
    let mut profile = BuildWorkProfile::default();
    let mut headers = Vec::with_capacity(inputs.len());
    *completed_records = 0;
    for (id, value, text) in inputs.iter_mut() {
        let slot = RecordSlot(
            u32::try_from(headers.len())
                .map_err(|_| StorageError::ResourceLimit("too many batch records".into()))?,
        );
        let text_derivation = if text.is_empty() {
            TextDerivation::LogicalStringsAfterConversion
        } else {
            TextDerivation::ReaderSegments
        };
        let mut extracted = ExtractedRecord {
            value: std::mem::replace(value, Value::Null),
            text: std::mem::take(text),
        };
        let result = finish_build_record(schema, &mut extracted)
            .map_err(|error| StorageError::invalid("validate direct record", error))
            .and_then(|()| {
                worker.add_logical_indexes(
                    slot,
                    id,
                    &extracted,
                    None,
                    None,
                    text_derivation,
                    false,
                    &mut profile,
                )
            });
        // Preserve earlier member input if this attempt has to publish a
        // valid prefix. Restoring ownership does not copy the payload.
        *value = extracted.value;
        *text = extracted.text;
        result?;
        headers.push(crate::records::RecordHeader {
            slot,
            id: id.clone(),
            class: RecordClass::Logical,
            diagnostic_ref: DiagnosticRef(0),
        });
        *completed_records += 1;
    }
    let contribution = BatchContribution {
        files: Vec::new(),
        file_fingerprints: Vec::new(),
        records: Vec::new(),
        indexes: worker.finish()?,
        record_parses: 0,
        failed_parses: 0,
        source_reads: 0,
        source_bytes_read: 0,
        profile,
    };
    let bodies = if retain_bodies {
        inputs
            .iter_mut()
            .map(|(_, value, text)| crate::records::RetainedBody {
                value: std::mem::replace(value, Value::Null),
                text: std::mem::take(text),
            })
            .collect()
    } else {
        Vec::new()
    };
    Ok(PreparedDirectBatch {
        headers,
        bodies,
        contribution,
    })
}

#[cfg(test)]
mod retained_contribution_tests {
    use super::*;

    #[test]
    fn typed_record_uses_fileless_native_segment_and_shared_logical_indexes() {
        let mut schema = Schema::default();
        schema
            .register_field(crate::schema::FieldDefinition {
                path: vec!["year".into()],
                kind: crate::model::LogicalType::Number,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
        let declaration =
            crate::repository::configuration::CollectionDeclaration::new("entries", schema);
        let options = crate::repository::configuration::RepositoryOptions::default();
        let id = RecordId::intrinsic("entry-47").unwrap();
        let value = Value::Object(std::collections::BTreeMap::from([
            (
                "year".into(),
                Value::Number(crate::model::Number::integer(1808)),
            ),
            ("title".into(), Value::String("Almanach".into())),
        ]));
        let (headers, bodies, contribution) = build_retained_contribution(
            vec![(id.clone(), value.clone(), Vec::new())],
            &declaration,
            &options,
        )
        .unwrap();
        let segment = crate::index::segment::admission::CollectionSegment::from_retained(
            crate::index::segment::SegmentId::new(2),
            CollectionId(0),
            "application.entries".into(),
            1,
            0,
            true,
            headers,
            crate::index::traits::test_factory()
                .retain_record_bodies(bodies)
                .unwrap(),
            contribution,
        )
        .unwrap();
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(1)
            .build()
            .unwrap();
        let indexed = segment
            .into_indexed(
                &declaration,
                options.bigram_alphabet,
                options.inference,
                1,
                &pool,
            )
            .unwrap();
        assert_eq!(indexed.record_count(), 1);
        assert_eq!(indexed.file_count(), 0);
        assert_eq!(indexed.record(RecordSlot(0)).unwrap().id, id);
        assert_eq!(
            indexed
                .backing
                .retained_body(RecordSlot(0))
                .unwrap()
                .unwrap()
                .value,
            value
        );
        assert_eq!(indexed.logical_records.len(), 1);
        assert!(indexed.postings.fields.contains_key(&vec!["year".into()]));
    }
}

pub(super) fn fallback_build_id(
    collection: CollectionId,
    source: &PreparedSource,
    file: &FileEntry,
    range: ByteRange,
) -> RecordId {
    RecordId::fallback(
        collection,
        source.id(),
        source.version(),
        &file.relative_path,
        file.id,
        file.revision.modified_ns,
        range.start,
        range.end,
    )
}

pub(super) fn build_record_id(
    root: &Path,
    definition: &CollectionDefinition,
    source: &PreparedSource,
    file: &FileEntry,
    path: &Path,
    range: ByteRange,
    extracted: &ExtractedRecord,
) -> Result<RecordId, String> {
    (|| -> Result<RecordId, String> {
        match &definition.identity {
            IdentityPolicy::Filename => {
                let stem = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .ok_or("non-UTF-8 filename identity")?;
                RecordId::intrinsic(stem)
            }
            IdentityPolicy::RelativePath => {
                if !source.reader().supports_relative_path_identity() {
                    return Err("relative-path identity requires a reader that supports it".into());
                }
                let source_dir = root.join(&definition.source_path);
                let relative = path
                    .strip_prefix(&source_dir)
                    .map_err(|_| "source path is outside the collection directory")?;
                RecordId::intrinsic(format!(
                    "path:{}",
                    crate::model::path_representation::query_path(relative)
                ))
            }
            IdentityPolicy::Generated => Ok(RecordId::generated(
                file.id,
                file.revision.modified_ns,
                range.start,
            )),
            IdentityPolicy::Intrinsic(field) => {
                let value = source
                    .reader()
                    .intrinsic_id(extracted, field)?
                    .ok_or_else(|| format!("missing intrinsic record ID at {}", field.join(".")))?;
                RecordId::intrinsic(value)
            }
        }
    })()
    .map_err(|error| format!("{}:{}..{}: {error}", path.display(), range.start, range.end))
}

impl<W: WorkerWriter> BatchBuilder<'_, W> {
    fn outcome(
        &mut self,
        file: &mut FileEntry,
        path: &Path,
        outcome: SourceOutcome,
        source_bytes: Option<&[u8]>,
    ) -> Result<(), String> {
        match outcome {
            SourceOutcome::Logical { range, record } => {
                self.logical(file, path, file.revision, range, record, source_bytes)
            }
            SourceOutcome::Malformed {
                range,
                diagnostic,
                bytes,
            } => self.fallback(file, range, &bytes, diagnostic),
            SourceOutcome::Unreadable { diagnostic } => {
                file.record_diagnostic(SourceDiagnostic {
                    file: file.id,
                    collection: self.collection,
                    source_id: self.source.id().into(),
                    source_version: self.source.version(),
                    record: None,
                    range: None,
                    category: diagnostic.category,
                    code: diagnostic.code,
                    field_path: diagnostic.field_path,
                    byte_offset: None,
                    line: diagnostic.line,
                    column: diagnostic.column,
                    message: diagnostic.message,
                })?;
                Ok(())
            }
        }
    }

    fn finish(
        self,
        files: Vec<BatchFile>,
        record_parses: u64,
        failed_parses: u64,
    ) -> Result<BatchContribution, String> {
        Ok(BatchContribution {
            files,
            file_fingerprints: Vec::new(),
            records: self.records,
            indexes: self.worker.finish()?,
            record_parses,
            failed_parses,
            source_reads: 0,
            source_bytes_read: 0,
            profile: self.profile,
        })
    }
}

#[cfg(test)]
pub(crate) fn process_batch(
    root: &Path,
    definition: &CollectionDefinition,
    collection: CollectionId,
    source: &PreparedSource,
    file_ids: &[FileId],
    source_files: &[SourceFileInput],
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
) -> Result<BatchContribution, String> {
    process_batch_profiled(
        crate::index::traits::default_factory().as_ref(),
        root,
        definition,
        collection,
        source,
        file_ids,
        source_files,
        alphabet,
        inference,
        inference_jobs,
        false,
    )
}

pub(crate) fn process_batch_profiled(
    factory: &dyn StorageFactory,
    root: &Path,
    definition: &CollectionDefinition,
    collection: CollectionId,
    source: &PreparedSource,
    file_ids: &[FileId],
    source_files: &[SourceFileInput],
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
    profile_build: bool,
) -> Result<BatchContribution, String> {
    factory
        .prepare_file_records(FileWorkerBatch {
            root,
            definition,
            collection,
            source,
            alphabet,
            inference,
            inference_jobs,
            input: FileWorkerInput::WholeFiles {
                ids: file_ids,
                descriptors: source_files,
                profile: profile_build,
            },
        })
        .map_err(String::from)
}

pub(crate) fn prepare_file_records(
    batch: FileWorkerBatch<'_>,
    worker: impl WorkerWriter,
) -> StorageResult<BatchContribution> {
    let mut builder = BatchBuilder::new(&batch, worker);
    let result = match batch.input {
        FileWorkerInput::WholeFiles {
            ids,
            descriptors,
            profile,
        } => prepare_whole_files(builder, ids, descriptors, profile),
        FileWorkerInput::Framed {
            input,
            bytes,
            ranges,
            file_offset,
            preceding_lines,
            first_range,
        } => prepare_framed_records(
            builder,
            input,
            bytes,
            ranges,
            file_offset,
            preceding_lines,
            first_range,
        ),
        FileWorkerInput::Outcome { input, outcome } => {
            let path = batch.root.join(&input.relative_path);
            let mut file = input.for_collection(batch.collection);
            builder
                .outcome(&mut file, &path, outcome, None)
                .map_err(|message| StorageError::invalid("prepare file outcome", message))?;
            let count = builder.records.len();
            builder.finish(
                vec![BatchFile {
                    entry: file,
                    records: 0..count,
                    first_range: true,
                }],
                0,
                0,
            )
        }
    };
    result.map_err(|message| StorageError::invalid("prepare file records", message))
}

fn prepare_whole_files(
    mut builder: BatchBuilder<'_, impl WorkerWriter>,
    file_ids: &[FileId],
    source_files: &[SourceFileInput],
    profile_build: bool,
) -> Result<BatchContribution, String> {
    let root = builder.root;
    let source = builder.source;
    let collection = builder.collection;
    let mut files = Vec::with_capacity(file_ids.len());
    let (mut record_parses, mut failed_parses) = (0, 0);
    let mut file_fingerprints = Vec::with_capacity(file_ids.len());
    let mut input_buffer = Vec::new();
    let mut input_buffer_profile = SourceBufferProfile::default();
    for &file_id in file_ids {
        let original = &source_files[file_id.0 as usize];
        let path = root.join(&original.relative_path);
        let mut file = original.for_collection(collection);
        let start = builder.records.len();
        let sampled = profile_build && file_id.0.is_multiple_of(64);
        #[cfg(feature = "detailed-profiling")]
        {
            builder.profile_current_file = sampled;
            builder.profile_record_index = 0;
        }
        let mut source_sample = SourceBuildSample::default();
        let mut callback_ns = 0u64;
        let (stats, fingerprint) = extract_file_build_outcomes_hashed_with_buffer(
            &path,
            source.reader(),
            file.revision,
            &mut input_buffer,
            sampled.then_some(&mut source_sample),
            Some(&mut input_buffer_profile),
            |outcome| {
                let started = crate::measurement::timer::start_if_enabled(sampled);
                let result = builder.outcome(&mut file, &path, outcome, None);
                if let Some(started) = started {
                    callback_ns += started.elapsed().as_nanos() as u64;
                }
                result
            },
        )?;
        if trim_oversized_build_buffer(&mut input_buffer) {
            if profile_build {
                builder.profile.input_buffer_oversized_drops += 1;
            }
        }
        if sampled {
            builder.profile.sampled_files += 1;
            builder.profile.sampled_file_bytes += source_sample.bytes_read;
            builder.profile.sampled_read_calls += source_sample.read_calls;
            builder.profile.sampled_source_records += stats.framed_records;
            builder.profile.sampled_callback_ns += callback_ns;
            builder.profile.source_open_ns += source_sample.open_ns;
            builder.profile.source_read_ns += source_sample.read_ns;
            builder.profile.source_hash_ns += source_sample.hash_ns;
            builder.profile.source_frame_ns += source_sample.frame_ns;
            builder.profile.source_extraction_ns += source_sample.extraction_ns;
        }
        if let Some(fingerprint) = fingerprint {
            file_fingerprints.push((file_id, fingerprint));
        }
        record_parses += stats.record_parses;
        failed_parses += stats.failed_parses;
        files.push(BatchFile {
            entry: file,
            records: start..builder.records.len(),
            first_range: true,
        });
    }
    if profile_build {
        builder.profile.input_buffer_files_read += input_buffer_profile.files_read;
        builder.profile.input_buffer_growth_files += input_buffer_profile.growth_files;
        builder.profile.input_buffer_peak_capacity_bytes = input_buffer_profile.peak_capacity_bytes;
        builder.profile.input_buffer_batch_end_capacity_bytes += input_buffer.capacity() as u64;
    }
    let finish_started = crate::measurement::timer::start_if_enabled(profile_build);
    let mut batch = builder.finish(files, record_parses, failed_parses)?;
    if let Some(started) = finish_started {
        batch.profile.batch_finish_ns += started.elapsed().as_nanos() as u64;
        batch.profile.batches += 1;
    }
    batch.file_fingerprints = file_fingerprints;
    batch.source_reads = input_buffer_profile.files_read;
    batch.source_bytes_read = input_buffer_profile.bytes_read;
    Ok(batch)
}

fn prepare_framed_records(
    mut builder: BatchBuilder<'_, impl WorkerWriter>,
    original: &SourceFileInput,
    bytes: &[u8],
    ranges: &[ByteRange],
    file_offset: u64,
    preceding_lines: u64,
    first_range: bool,
) -> Result<BatchContribution, String> {
    let path = builder.root.join(&original.relative_path);
    let mut file = original.for_collection(builder.collection);
    let stats =
        extract_framed_build_outcomes(&path, builder.source.reader(), bytes, ranges, |outcome| {
            let revision_size = file.revision.size;
            let absolute = |range: ByteRange| -> Result<ByteRange, String> {
                let start = file_offset
                    .checked_add(range.start)
                    .ok_or("record range offset overflow")?;
                let end = file_offset
                    .checked_add(range.end)
                    .ok_or("record range offset overflow")?;
                ByteRange::checked(start, end, revision_size)
            };
            match outcome {
                SourceOutcome::Logical { range, record } => {
                    let source_part = &bytes[range.start as usize..range.end as usize];
                    let range = absolute(range)?;
                    builder.outcome(
                        &mut file,
                        &path,
                        SourceOutcome::Logical { range, record },
                        Some(source_part),
                    )
                }
                SourceOutcome::Malformed {
                    range,
                    mut diagnostic,
                    bytes,
                } => {
                    diagnostic.line = diagnostic
                        .line
                        .and_then(|line| line.checked_add(preceding_lines));
                    let range = absolute(range)?;
                    builder.outcome(
                        &mut file,
                        &path,
                        SourceOutcome::Malformed {
                            range,
                            diagnostic,
                            bytes,
                        },
                        None,
                    )
                }
                SourceOutcome::Unreadable { diagnostic } => builder.outcome(
                    &mut file,
                    &path,
                    SourceOutcome::Unreadable { diagnostic },
                    None,
                ),
            }
        })?;
    let count = builder.records.len();
    builder.finish(
        vec![BatchFile {
            entry: file,
            records: 0..count,
            first_range,
        }],
        stats.record_parses,
        stats.failed_parses,
    )
}
