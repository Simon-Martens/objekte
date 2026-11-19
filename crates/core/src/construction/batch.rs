//! Owned record-batch construction and admission contributions.

use std::collections::HashMap;
use std::ops::Range;
use std::path::Path;

use roaring::RoaringBitmap;
use serde::Serialize;

use crate::index::bigram::BigramAlphabet;
use crate::index::bigram::{BigramBuilder, BigramConstructionStats, BigramContribution};
use crate::index::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::index::relation::RelationContribution;
use crate::index::sorting::{SortBuilder, SortKey};
use crate::inference::{InferenceBuilder, InferenceConfig};
use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileInventory, FileStatus, RecordClass, RecordEntry,
    SharedPath, SourceDiagnostic, SourceRevision,
};
use crate::model::{ArrayValue, CollectionId, FieldPath, FileId, RecordId, RecordSlot, Value};
use crate::query::semantics::record::data_values;
use crate::record_preparation::finish_build_record;
use crate::repository::configuration::{CollectionDefinition, IdentityPolicy};
use crate::schema::{Schema, operation::BindingOperation};
use crate::source::access::read_record_range_for_build;
use crate::source::{
    DiagnosticCategory, ExtractedRecord, ReaderDiagnostic, SourceOutcome, TextDerivation,
    extraction::SourceBufferProfile, extraction::SourceBuildSample,
    extraction::extract_file_build_outcomes_hashed_with_buffer,
    extraction::trim_oversized_build_buffer, registry::PreparedSource,
    text::visit_logical_strings_and_empty,
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
    inventory: &FileInventory,
) -> Vec<&'a [FileId]> {
    const MAX_FILES: usize = 512;
    const MAX_BYTES: u64 = 8 * 1024 * 1024;
    let mut batches = Vec::new();
    let mut start = 0;
    while start < files.len() {
        let mut end = start;
        let mut bytes = 0u64;
        while end < files.len() && end - start < MAX_FILES {
            let size = inventory.files[files[end].0 as usize].revision.size;
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
    batches
}

/// All expensive per-record projection is complete before this crosses the
/// worker/coordinator boundary. Slots remain local until ordered admission.
pub(crate) struct BatchContribution {
    pub(super) files: Vec<BatchFile>,
    /// Complete physical-file digests captured during this batch's own reads.
    /// Framed chunks only report a digest once, after the whole read succeeds.
    pub(super) file_fingerprints: Vec<(FileId, [u8; 32])>,
    pub(super) records: Vec<RecordEntry>,
    pub(super) postings: PostingCollection,
    pub(super) inference: Option<InferenceBuilder>,
    pub(super) custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub(super) raw: BigramContribution,
    pub(super) fallback_text: BigramContribution,
    pub(super) custom_text: HashMap<(Vec<String>, String), BigramContribution>,
    pub(super) logical_records: RoaringBitmap,
    pub(super) empty_array_records: RoaringBitmap,
    pub(super) sort_keys: HashMap<FieldPath, Vec<SortKey>>,
    pub(super) relations: Vec<RelationContribution>,
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
        let mut stats = self.raw.storage_stats();
        stats.absorb(self.fallback_text.storage_stats());
        for contribution in self.custom_text.values() {
            stats.absorb(contribution.storage_stats());
        }
        stats
    }

    pub(crate) fn file_fingerprints(&self) -> &[(FileId, [u8; 32])] {
        &self.file_fingerprints
    }

    pub(crate) fn logical_records(&self) -> &RoaringBitmap {
        &self.logical_records
    }

    pub(crate) fn new_file_count(&self) -> usize {
        self.files.iter().filter(|file| file.first_range).count()
    }

    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }
}

pub(crate) struct BatchBuilder<'a> {
    root: &'a Path,
    definition: &'a CollectionDefinition,
    collection: CollectionId,
    source: &'a PreparedSource,
    pub(super) records: Vec<RecordEntry>,
    postings: PostingCollection,
    inference: Option<InferenceBuilder>,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramBuilder,
    fallback_text: BigramBuilder,
    custom_text: HashMap<(Vec<String>, String), BigramBuilder>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sorting: SortBuilder,
    relations: Vec<RelationContribution>,
    #[cfg(feature = "detailed-profiling")]
    profile_current_file: bool,
    #[cfg(feature = "detailed-profiling")]
    profile_record_index: usize,
    profile: BuildWorkProfile,
}

impl<'a> BatchBuilder<'a> {
    pub(super) fn new(
        root: &'a Path,
        definition: &'a CollectionDefinition,
        collection: CollectionId,
        source: &'a PreparedSource,
        alphabet: BigramAlphabet,
        inference: InferenceConfig,
        inference_jobs: usize,
    ) -> Result<Self, String> {
        let schema = &definition.schema;
        let custom_postings = schema
            .operations
            .iter()
            .filter(|(_, binding)| binding.operation.posting())
            .map(|(key, binding)| {
                (
                    key.clone(),
                    CustomPostingIndex {
                        index: PostingIndex::new(&schema.fields[&key.0]),
                        complete: binding.operation.complete_coverage(),
                    },
                )
            })
            .collect();
        let custom_text = schema
            .custom
            .keys()
            .chain(
                schema
                    .operations
                    .iter()
                    .filter(|(_, binding)| {
                        binding.operation.bigram() && binding.operation.complete_coverage()
                    })
                    .map(|(key, _)| key),
            )
            .map(|key| {
                (
                    canonical_custom_text_key(schema, &key.0, &key.1),
                    BigramBuilder::new(alphabet),
                )
            })
            .collect();
        Ok(Self {
            root,
            definition,
            collection,
            source,
            records: Vec::new(),
            postings: PostingCollection::new(schema),
            inference: InferenceBuilder::with_job_count(inference, true, inference_jobs),
            custom_postings,
            raw: BigramBuilder::new(alphabet),
            fallback_text: BigramBuilder::new(alphabet),
            custom_text,
            logical_records: RoaringBitmap::new(),
            empty_array_records: RoaringBitmap::new(),
            sorting: SortBuilder::new(
                &definition.sort_fields,
                schema,
                &definition.sort_locale,
                definition.sort_options,
                &definition.sort_overrides,
            )?,
            relations: RelationContribution::for_schema(schema),
            #[cfg(feature = "detailed-profiling")]
            profile_current_file: false,
            #[cfg(feature = "detailed-profiling")]
            profile_record_index: 0,
            profile: BuildWorkProfile::default(),
        })
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
        let id = RecordId::fallback(
            self.collection,
            self.source.id(),
            self.source.version(),
            &file.relative_path,
            file.id,
            file.revision.modified_ns,
            range.start,
            range.end,
        );
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
        self.raw.add_empty(slot)?;
        for relation in &mut self.relations {
            relation.add_unavailable(slot)?;
        }
        let text = String::from_utf8_lossy(bytes);
        self.fallback_text
            .add_record(slot, std::iter::once(text.as_ref()))?;
        for index in self.custom_text.values_mut() {
            index.add_empty(slot)?;
        }
        let empty = Value::Object(Default::default());
        self.sorting.add(
            slot,
            &empty,
            self.records.last().unwrap(),
            file,
            self.root,
            &self.definition.name,
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
                let bytes = read_record_range_for_build(path, range)?;
                self.fallback(file, range, &bytes, issue)
            };
        }
        if let Some(started) = stage_started {
            self.profile.validation_ns += started.elapsed().as_nanos() as u64;
        }
        let id = (|| -> Result<RecordId, String> {
            match &self.definition.identity {
                IdentityPolicy::Filename => {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .ok_or("non-UTF-8 filename identity")?;
                    RecordId::intrinsic(stem)
                }
                IdentityPolicy::RelativePath => {
                    if !matches!(
                        self.definition.format,
                        crate::source::SourceFormat::Json
                            | crate::source::SourceFormat::JsonDecodedScalars
                    ) {
                        return Err(
                            "relative-path identity requires a single-record JSON reader".into(),
                        );
                    }
                    let source_dir = self.root.join(&self.definition.source_path);
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
                    revision.modified_ns,
                    range.start,
                )),
                IdentityPolicy::Intrinsic(field) => {
                    let value = self
                        .source
                        .reader()
                        .intrinsic_id(&extracted, field)?
                        .ok_or_else(|| {
                            format!("missing intrinsic record ID at {}", field.join("."))
                        })?;
                    RecordId::intrinsic(value)
                }
            }
        })()
        .map_err(|error| format!("{}:{}..{}: {error}", path.display(), range.start, range.end))?;
        let slot = self.next_slot()?;
        self.records.push(RecordEntry {
            slot,
            id,
            file: file.id,
            range,
            revision,
            class: RecordClass::Logical,
            diagnostic_ref: DiagnosticRef(0),
        });
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        self.postings.add(slot, &extracted.value)?;
        if let Some(started) = stage_started {
            self.profile.postings_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        if let Some(inference) = &mut self.inference {
            inference.observe_record_with_schema(
                slot,
                &extracted.value,
                Some(&self.definition.schema),
            )?;
        }
        if let Some(started) = stage_started {
            self.profile.inference_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        for relation in &mut self.relations {
            relation.add_logical(slot, &extracted.value)?;
        }
        if let Some(started) = stage_started {
            self.profile.relations_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        for (key, custom) in &mut self.custom_postings {
            let binding = &self.definition.schema.operations[key];
            let selected = data_values(&extracted.value, &key.0);
            let mapped = selected.as_ref().map(|values| {
                values
                    .iter()
                    .map(|value| map_binding_value(binding.operation.as_ref(), value))
                    .collect::<Option<Vec<_>>>()
            });
            let mapped = match mapped {
                Some(Some(values)) => Some(values),
                Some(None) => {
                    custom.complete = false;
                    None
                }
                None => None,
            };
            let refs = mapped
                .as_ref()
                .map(|values| values.iter().collect::<Vec<_>>());
            custom.index.add_values(slot, refs.as_deref())?;
        }
        if let Some(started) = stage_started {
            self.profile.custom_postings_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        let has_empty_array;
        if self.source.reader().text_derivation() == TextDerivation::LogicalStringsAfterConversion {
            if let Some(binding) = &self.definition.schema.raw {
                let mut observed_empty = false;
                self.raw.add_record_with(slot, |emit| {
                    observed_empty =
                        visit_logical_strings_and_empty(&extracted.value, &mut |text| {
                            emit(&binding.operation.map_text(text));
                        });
                })?;
                has_empty_array = observed_empty;
            } else {
                let mut observed_empty = false;
                self.raw.add_record_with(slot, |emit| {
                    observed_empty = visit_logical_strings_and_empty(&extracted.value, emit);
                })?;
                has_empty_array = observed_empty;
            }
        } else {
            has_empty_array = crate::model::structure::contains_empty_array(&extracted.value);
            if let Some(binding) = &self.definition.schema.raw {
                self.raw.add_record(
                    slot,
                    extracted
                        .text
                        .iter()
                        .map(|segment| binding.operation.map_text(&segment.text)),
                )?;
            } else {
                self.raw.add_record(
                    slot,
                    extracted.text.iter().map(|segment| segment.text.as_str()),
                )?;
            }
        }
        self.fallback_text.add_empty(slot)?;
        self.logical_records.insert(slot.0);
        if has_empty_array {
            self.empty_array_records.insert(slot.0);
        }
        if let Some(started) = stage_started {
            self.profile.text_bigram_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        self.sorting.add(
            slot,
            &extracted.value,
            self.records.last().unwrap(),
            file,
            self.root,
            &self.definition.name,
        )?;
        if let Some(started) = stage_started {
            self.profile.sorting_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        for (key, index) in &mut self.custom_text {
            let values = data_values(&extracted.value, &key.0).unwrap_or_default();
            if let Some(binding) = self.definition.schema.operations.get(key) {
                index.add_record(
                    slot,
                    values
                        .into_iter()
                        .flat_map(string_values)
                        .map(|text| binding.operation.map_text(text)),
                )?;
            } else {
                index.add_record(slot, values.into_iter().flat_map(string_values))?;
            }
        }
        if let Some(started) = stage_started {
            self.profile.custom_text_ns += started.elapsed().as_nanos() as u64;
        }
        Ok(())
    }

    pub(super) fn outcome(
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

    pub(super) fn finish(
        self,
        files: Vec<BatchFile>,
        record_parses: u64,
        failed_parses: u64,
    ) -> Result<BatchContribution, String> {
        let custom_text = self
            .custom_text
            .into_iter()
            .map(|(key, index)| index.finish_blocks().map(|index| (key, index)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(BatchContribution {
            files,
            file_fingerprints: Vec::new(),
            records: self.records,
            postings: self.postings,
            inference: self.inference,
            custom_postings: self.custom_postings,
            raw: self.raw.finish_blocks()?,
            fallback_text: self.fallback_text.finish_blocks()?,
            custom_text,
            logical_records: self.logical_records,
            empty_array_records: self.empty_array_records,
            sort_keys: self.sorting.into_keys(),
            relations: self.relations,
            record_parses,
            failed_parses,
            source_reads: 0,
            source_bytes_read: 0,
            profile: self.profile,
        })
    }
}

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
    let mut builder = BatchBuilder::new(
        root,
        definition,
        collection,
        source,
        alphabet,
        inference,
        inference_jobs,
    )?;
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
