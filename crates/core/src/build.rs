//! Bounded file-batch contributions and deterministic index admission.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Cursor, Read};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "detailed-profiling")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use rayon::ThreadPool;
use roaring::RoaringBitmap;
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::bigram::{
    BigramBuilder, BigramConstructionStats, BigramContribution,
};
use crate::inference::{InferenceBuilder, InferenceConfig};
use crate::inventory::{
    ByteRange, CollectionBuilder, DiagnosticRef, FileEntry, FileInventory, FileStatus, RecordClass,
    RecordEntry, SharedPath, SourceDiagnostic, SourceRevision, append_shifted_bitmap,
};
use crate::model::{CollectionId, FieldPath, FileId, RecordId, RecordSlot, Value};
use crate::posting::{
    CustomPostingIndex, PostingCollection, PostingIndex, ScalarKey, StructuralKey,
};
use crate::record_preparation::finish_build_record;
use crate::reference::data_values;
use crate::relation::RelationContribution;
use crate::repository::{
    CollectionDefinition, IdentityPolicy, canonical_custom_text_key, map_binding_value,
    string_values,
};
use crate::sorting::{SortBuilder, SortKey};
use crate::source::{
    BoundedBuildChunk, BoundedJsonLines, DiagnosticCategory, ExtractedRecord, PreparedSource,
    ReaderDiagnostic, ReaderFailure, SourceBufferProfile, SourceBuildSample, SourceOutcome,
    SourceFormat, TextDerivation, extract_file_build_outcomes,
    extract_file_build_outcomes_hashed_with_buffer, extract_framed_build_outcomes,
    finalize_extracted_text, frame_file_for_build, read_record_range_for_build,
    trim_oversized_build_buffer, visit_logical_strings_and_empty,
};
use crate::workload::BigramAlphabet;
use crate::index_store::store_build::prepare::{
    PreparedBinding, PreparedGramSet, PreparedLength, PreparedRelation, PreparedRelationState,
    PreparedSort, PreparedState, PreparedStoreBatch, PreparedValue,
};
use crate::index_store::value_codec::BindingIdentity;


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

    fn for_collection(&self, collection: CollectionId) -> FileEntry {
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
    pub(crate) files: Vec<BatchFile>,
    /// Complete physical-file digests captured during this batch's own reads.
    /// Framed chunks only report a digest once, after the whole read succeeds.
    pub(crate) file_fingerprints: Vec<(FileId, [u8; 32])>,
    pub(crate) records: Vec<RecordEntry>,
    pub(crate) postings: PostingCollection,
    pub(crate) inference: Option<InferenceBuilder>,
    pub(crate) custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub(crate) raw: BigramContribution,
    pub(crate) fallback_text: BigramContribution,
    pub(crate) custom_text: HashMap<(Vec<String>, String), BigramContribution>,
    pub(crate) logical_records: RoaringBitmap,
    pub(crate) empty_array_records: RoaringBitmap,
    pub(crate) sort_keys: HashMap<FieldPath, Vec<SortKey>>,
    pub(crate) relations: Vec<RelationContribution>,
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


/// The collection-wide identity and diagnostic observations needed while
/// local index families move directly into bounded fresh-build shards.
pub(crate) struct BatchMetadata {
    pub(crate) files: Vec<BatchFile>,
    pub(crate) records: Vec<RecordEntry>,
    pub(crate) inference: Option<InferenceBuilder>,
    pub(crate) logical_records: RoaringBitmap,
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

    pub(crate) fn metadata_copy(&self) -> BatchMetadata {
        BatchMetadata {
            files: self.files.clone(),
            records: self.records.clone(),
            inference: self.inference.clone(),
            logical_records: self.logical_records.clone(),
        }
    }

    /// Initial publication needs collection-wide inference decisions. Keep
    /// this copy separately measurable from coordinator bookkeeping.
    pub(crate) fn inference_copy(&self) -> Option<InferenceBuilder> {
        self.inference.clone()
    }

    pub(crate) fn logical_records(&self) -> &RoaringBitmap {
        &self.logical_records
    }

    pub(crate) fn new_file_count(&self) -> usize {
        self.files.iter().filter(|file| file.first_range).count()
    }

    pub(crate) fn new_file_bytes(&self) -> u64 {
        self.files
            .iter()
            .filter(|file| file.first_range)
            .fold(0u64, |total, file| {
                total.saturating_add(file.entry.revision.size)
            })
    }

    pub(crate) fn record_count(&self) -> usize {
        self.records.len()
    }
}


struct BatchBuilder<'a> {
    root: &'a Path,
    definition: &'a CollectionDefinition,
    collection: CollectionId,
    source: &'a PreparedSource,
    records: Vec<RecordEntry>,
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
    fn new(
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
        let stage_started = crate::detailed_timer::start_if_enabled(sample);
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
                    RecordId::intrinsic(format!("path:{}", crate::path_repr::query_path(relative)))
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
        let stage_started = crate::detailed_timer::start_if_enabled(sample);
        self.postings.add(slot, &extracted.value)?;
        if let Some(started) = stage_started {
            self.profile.postings_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::detailed_timer::start_if_enabled(sample);
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
        let stage_started = crate::detailed_timer::start_if_enabled(sample);
        for relation in &mut self.relations {
            relation.add_logical(slot, &extracted.value)?;
        }
        if let Some(started) = stage_started {
            self.profile.relations_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::detailed_timer::start_if_enabled(sample);
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
        let stage_started = crate::detailed_timer::start_if_enabled(sample);
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
            has_empty_array = crate::structural::contains_empty_array(&extracted.value);
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
        let stage_started = crate::detailed_timer::start_if_enabled(sample);
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
        let stage_started = crate::detailed_timer::start_if_enabled(sample);
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

struct DirectBindingRegistry {
    ids: HashMap<BindingIdentity, u32>,
    bindings: Vec<PreparedBinding>,
}

impl DirectBindingRegistry {
    fn new() -> Self {
        Self {
            ids: HashMap::new(),
            bindings: Vec::new(),
        }
    }

    fn id(
        &mut self,
        identity: BindingIdentity,
        role: crate::index_store::value_codec::BindingRole,
        kind: crate::model::LogicalType,
        multi: bool,
        alphabet: Option<BigramAlphabet>,
    ) -> u32 {
        if let Some(id) = self.ids.get(&identity) {
            return *id;
        }
        let id = self.bindings.len() as u32;
        self.ids.insert(identity.clone(), id);
        self.bindings.push(PreparedBinding {
            descriptor: crate::index_store::value_codec::BindingDescriptor {
                identity,
                role,
                kind,
                multi,
                text_alphabet: alphabet
                    .map(|alphabet| format!("{alphabet:?}").to_ascii_lowercase()),
                text_segment: None,
                fingerprint: [0; 32],
            },
        });
        id
    }
}

struct DirectStoreBatchBuilder<'a> {
    root: &'a Path,
    definition: &'a CollectionDefinition,
    collection: CollectionId,
    source: &'a PreparedSource,
    alphabet: BigramAlphabet,
    postings: PostingCollection,
    custom_postings: HashMap<(Vec<String>, String), PostingIndex>,
    custom_complete: HashMap<(Vec<String>, String), bool>,
    custom_text: Vec<(Vec<String>, String)>,
    records: Vec<RecordEntry>,
    files: Vec<BatchFile>,
    file_fingerprints: Vec<(FileId, [u8; 32])>,
    bindings: DirectBindingRegistry,
    fields: HashMap<Vec<String>, crate::index_store::value_codec::FieldDescriptor>,
    coverage: HashMap<u32, crate::index_store::value_codec::FieldCoverage>,
    values: Vec<PreparedValue>,
    lengths: Vec<PreparedLength>,
    states: Vec<PreparedState>,
    relations: Vec<RelationContribution>,
    grams: Vec<PreparedGramSet>,
    inference: Option<InferenceBuilder>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sorting: SortBuilder,
    record_parses: u64,
    failed_parses: u64,
    source_reads: u64,
    source_bytes_read: u64,
    profile: BuildWorkProfile,
}

impl<'a> DirectStoreBatchBuilder<'a> {
    #[allow(clippy::too_many_arguments)]
    fn new(
        root: &'a Path,
        definition: &'a CollectionDefinition,
        collection: CollectionId,
        source: &'a PreparedSource,
        alphabet: BigramAlphabet,
        inference: InferenceConfig,
        inference_jobs: usize,
    ) -> Result<Self, String> {
        let schema = &definition.schema;
        let postings = PostingCollection::new(schema);
        let mut bindings = DirectBindingRegistry::new();
        let mut fields = HashMap::new();
        let mut coverage = HashMap::new();
        let mut paths = postings.fields.keys().cloned().collect::<Vec<_>>();
        paths.sort_unstable();
        for path in paths {
            let posting = &postings.fields[&path];
            let binding = bindings.id(
                BindingIdentity::Declared { path: path.clone() },
                crate::index_store::value_codec::BindingRole::Structured,
                posting.kind.clone(),
                posting.multi,
                None,
            );
            fields.insert(
                path.clone(),
                crate::index_store::value_codec::FieldDescriptor {
                    path: FieldPath {
                        root: crate::model::PathRoot::Data,
                        segments: path.clone(),
                    },
                    kind: posting.kind.clone(),
                    multi: posting.multi,
                    indexed: posting.values_indexed,
                    values_indexed: posting.values_indexed,
                    value_complete: posting.value_complete,
                    length_complete: posting.length_complete,
                    empty_string_complete: posting.empty_string_complete,
                    covered_records: 0,
                },
            );
            coverage.insert(
                binding,
                crate::index_store::value_codec::FieldCoverage {
                    kind: posting.kind.clone(),
                    multi: posting.multi,
                    values_indexed: posting.values_indexed,
                    value_complete: posting.value_complete,
                    length_complete: posting.length_complete,
                    empty_string_complete: posting.empty_string_complete,
                    covered_records: 0,
                    inference_status:
                        crate::index_store::value_codec::INFERENCE_STATUS_NOT_APPLICABLE,
                    value_cardinality: 0,
                    length_cardinality: 0,
                },
            );
        }
        let mut custom_keys = schema
            .operations
            .iter()
            .filter(|(_, binding)| binding.operation.posting())
            .map(|(key, binding)| (key.clone(), binding.operation.complete_coverage()))
            .collect::<Vec<_>>();
        custom_keys.sort_by(|left, right| left.0.cmp(&right.0));
        let mut custom_postings = HashMap::new();
        let mut custom_complete = HashMap::new();
        for (key, complete) in custom_keys {
            let field = schema
                .fields
                .get(&key.0)
                .ok_or("custom posting field is unknown")?;
            let index = PostingIndex::new(field);
            let binding = bindings.id(
                BindingIdentity::Named {
                    path: key.0.clone(),
                    name: key.1.clone(),
                },
                crate::index_store::value_codec::BindingRole::Structured,
                index.kind.clone(),
                index.multi,
                None,
            );
            coverage.insert(
                binding,
                crate::index_store::value_codec::FieldCoverage {
                    kind: index.kind.clone(),
                    multi: index.multi,
                    values_indexed: index.values_indexed,
                    value_complete: complete && index.value_complete,
                    length_complete: complete && index.length_complete,
                    empty_string_complete: complete && index.empty_string_complete,
                    covered_records: 0,
                    inference_status:
                        crate::index_store::value_codec::INFERENCE_STATUS_NOT_APPLICABLE,
                    value_cardinality: 0,
                    length_cardinality: 0,
                },
            );
            custom_postings.insert(key.clone(), index);
            custom_complete.insert(key, complete);
        }
        bindings.id(
            BindingIdentity::RawText,
            crate::index_store::value_codec::BindingRole::Text,
            crate::model::LogicalType::String,
            false,
            Some(alphabet),
        );
        bindings.id(
            BindingIdentity::FallbackText,
            crate::index_store::value_codec::BindingRole::Text,
            crate::model::LogicalType::String,
            false,
            Some(alphabet),
        );
        let mut custom_text = schema
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
            .map(|key| canonical_custom_text_key(schema, &key.0, &key.1))
            .collect::<Vec<_>>();
        custom_text.sort_unstable();
        custom_text.dedup();
        for (path, name) in &custom_text {
            bindings.id(
                BindingIdentity::CustomText {
                    path: path.clone(),
                    name: name.clone(),
                },
                crate::index_store::value_codec::BindingRole::Text,
                crate::model::LogicalType::String,
                false,
                Some(alphabet),
            );
        }
        Ok(Self {
            root,
            definition,
            collection,
            source,
            alphabet,
            postings,
            custom_postings,
            custom_complete,
            custom_text,
            records: Vec::new(),
            files: Vec::new(),
            file_fingerprints: Vec::new(),
            bindings,
            fields,
            coverage,
            values: Vec::new(),
            lengths: Vec::new(),
            states: Vec::new(),
            relations: RelationContribution::for_schema(schema),
            grams: Vec::new(),
            inference: InferenceBuilder::with_job_count(inference, true, inference_jobs),
            logical_records: RoaringBitmap::new(),
            empty_array_records: RoaringBitmap::new(),
            sorting: SortBuilder::new(
                &definition.sort_fields,
                schema,
                &definition.sort_locale,
                definition.sort_options,
                &definition.sort_overrides,
            )?,
            record_parses: 0,
            failed_parses: 0,
            source_reads: 0,
            source_bytes_read: 0,
            profile: BuildWorkProfile::default(),
        })
    }

    fn binding_id(&self, identity: &BindingIdentity) -> u32 {
        self.bindings.ids[identity]
    }

    fn append_facts(
        &mut self,
        slot: RecordSlot,
        binding: u32,
        facts: crate::posting::RecordPostingFacts,
    ) {
        let mut mask = 0u16;
        if facts.presence {
            mask |= 1 << crate::index_store::value_codec::StateKind::PathPresence.tag();
        }
        if facts.empty_selection {
            mask |= 1 << crate::index_store::value_codec::StateKind::EmptySelection.tag();
        }
        if facts.null {
            mask |= 1 << crate::index_store::value_codec::StateKind::LegacyNullSet.tag();
        }
        for state in facts.structural {
            let state = match state {
                StructuralKey::AnyNull => crate::index_store::value_codec::StateKind::AnyNull,
                StructuralKey::NonEmpty => crate::index_store::value_codec::StateKind::NonEmpty,
                StructuralKey::String => crate::index_store::value_codec::StateKind::String,
                StructuralKey::Number => crate::index_store::value_codec::StateKind::Number,
                StructuralKey::Array => crate::index_store::value_codec::StateKind::Array,
                StructuralKey::Object => crate::index_store::value_codec::StateKind::Object,
                StructuralKey::EmptyObject => {
                    crate::index_store::value_codec::StateKind::EmptyObject
                }
            };
            mask |= 1 << state.tag();
        }
        if mask != 0 {
            self.states.push(PreparedState {
                slot: slot.0,
                binding,
                mask,
            });
        }
        self.values.extend(facts.values.into_iter().map(|value| PreparedValue {
            slot: slot.0,
            binding,
            value,
        }));
        self.lengths
            .extend(facts.lengths.into_iter().map(|length| PreparedLength {
                slot: slot.0,
                binding,
                length,
            }));
    }

    fn append_grams(
        &mut self,
        slot: RecordSlot,
        binding: u32,
        grams: (Vec<u64>, Vec<u64>),
    ) {
        for (kind, grams) in [
            (crate::index_store::value_codec::GramKind::Normal, grams.0),
            (crate::index_store::value_codec::GramKind::Skip, grams.1),
        ] {
            if !grams.is_empty() {
                self.grams.push(PreparedGramSet {
                    slot: slot.0,
                    binding,
                    kind,
                    grams,
                });
            }
        }
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
        let slot = RecordSlot(
            u32::try_from(self.records.len()).map_err(|_| "too many batch records")?,
        );
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
        for relation in &mut self.relations {
            relation.add_unavailable(slot)?;
        }
        let text = String::from_utf8_lossy(bytes);
        let binding = self.binding_id(&BindingIdentity::FallbackText);
        let grams = crate::bigram::collect_record_gram_facts(
            self.alphabet,
            std::iter::once(text.as_ref()),
        );
        self.append_grams(slot, binding, grams);
        let empty = Value::Object(Default::default());
        self.sorting.add(
            slot,
            &empty,
            self.records.last().expect("fallback record exists"),
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
        finalize_extracted_text(
            self.source.reader(),
            &mut extracted,
            self.definition.schema.has_converted_fields(),
        );
        let id = (|| -> Result<RecordId, String> {
            match &self.definition.identity {
                IdentityPolicy::Filename => Ok(RecordId::intrinsic(
                    path.file_stem()
                        .and_then(|stem| stem.to_str())
                        .ok_or("non-UTF-8 filename identity")?,
                )?),
                IdentityPolicy::RelativePath => {
                    if !matches!(
                        self.definition.format,
                        SourceFormat::Json | SourceFormat::JsonDecodedScalars
                    ) {
                        return Err(
                            "relative-path identity requires a single-record JSON reader".into(),
                        );
                    }
                    let source_dir = self.root.join(&self.definition.source_path);
                    let relative = path
                        .strip_prefix(&source_dir)
                        .map_err(|_| "source path is outside the collection directory")?;
                    Ok(RecordId::intrinsic(format!(
                        "path:{}",
                        crate::path_repr::query_path(relative)
                    ))?)
                }
                IdentityPolicy::Generated => {
                    Ok(RecordId::generated(file.id, revision.modified_ns, range.start))
                }
                IdentityPolicy::Intrinsic(field) => Ok(RecordId::intrinsic(
                    self.source
                        .reader()
                        .intrinsic_id(&extracted, field)?
                        .ok_or_else(|| {
                            format!("missing intrinsic record ID at {}", field.join("."))
                        })?,
                )?),
            }
        })()
        .map_err(|error: String| {
            format!("{}:{}..{}: {error}", path.display(), range.start, range.end)
        })?;
        let slot = RecordSlot(
            u32::try_from(self.records.len()).map_err(|_| "too many batch records")?,
        );
        self.records.push(RecordEntry {
            slot,
            id,
            file: file.id,
            range,
            revision,
            class: RecordClass::Logical,
            diagnostic_ref: DiagnosticRef(0),
        });
        let declared_facts = self
            .postings
            .fields
            .iter()
            .map(|(path, posting)| {
                (
                    path.clone(),
                    self.binding_id(&BindingIdentity::Declared { path: path.clone() }),
                    posting.record_facts(&extracted.value),
                )
            })
            .collect::<Vec<_>>();
        for (path, binding, facts) in declared_facts {
            self.append_facts(slot, binding, facts);
            let field = self.fields.get_mut(&path).expect("declared field exists");
            field.covered_records = field
                .covered_records
                .checked_add(1)
                .ok_or("field coverage overflow")?;
            let coverage = self.coverage.get_mut(&binding).expect("coverage exists");
            coverage.covered_records = coverage
                .covered_records
                .checked_add(1)
                .ok_or("coverage overflow")?;
        }
        if let Some(inference) = &mut self.inference {
            inference.observe_record_with_schema(
                slot,
                &extracted.value,
                Some(&self.definition.schema),
            )?;
        }
        for relation in &mut self.relations {
            relation.add_logical(slot, &extracted.value)?;
        }
        let custom_facts = self
            .custom_postings
            .iter()
            .map(|(key, index)| {
                let binding = self.binding_id(&BindingIdentity::Named {
                    path: key.0.clone(),
                    name: key.1.clone(),
                });
                let selected = data_values(&extracted.value, &key.0);
                let mapped = selected.as_ref().map(|values| {
                    values
                        .iter()
                        .map(|value| {
                            map_binding_value(
                                self.definition.schema.operations[key].operation.as_ref(),
                                value,
                            )
                        })
                        .collect::<Option<Vec<_>>>()
                });
                match mapped {
                    Some(Some(values)) => {
                        let refs = values.iter().collect::<Vec<_>>();
                        (key.clone(), binding, index.record_facts_selected(Some(refs.as_slice())), false)
                    }
                    Some(None) => (
                        key.clone(),
                        binding,
                        Default::default(),
                        true,
                    ),
                    None => (key.clone(), binding, Default::default(), false),
                }
            })
            .collect::<Vec<_>>();
        for (key, binding, facts, incomplete) in custom_facts {
            if incomplete {
                self.custom_complete.insert(key, false);
            }
            self.append_facts(slot, binding, facts);
        }
        let mut raw_text = Vec::new();
        let has_empty_array;
        if self.source.reader().text_derivation()
            == TextDerivation::LogicalStringsAfterConversion
        {
            let raw_binding = self.definition.schema.raw.as_ref();
            has_empty_array = visit_logical_strings_and_empty(&extracted.value, &mut |text| {
                raw_text.push(
                    raw_binding
                        .map(|binding| binding.operation.map_text(text))
                        .unwrap_or_else(|| text.to_owned()),
                );
            });
        } else {
            has_empty_array = crate::structural::contains_empty_array(&extracted.value);
            for segment in &extracted.text {
                raw_text.push(
                    self.definition
                        .schema
                        .raw
                        .as_ref()
                        .map(|binding| binding.operation.map_text(&segment.text))
                        .unwrap_or_else(|| segment.text.clone()),
                );
            }
        }
        let raw_binding = self.binding_id(&BindingIdentity::RawText);
        self.append_grams(
            slot,
            raw_binding,
            crate::bigram::collect_record_gram_facts(self.alphabet, raw_text),
        );
        self.logical_records.insert(slot.0);
        if has_empty_array {
            self.empty_array_records.insert(slot.0);
        }
        self.sorting.add(
            slot,
            &extracted.value,
            self.records.last().expect("logical record exists"),
            file,
            self.root,
            &self.definition.name,
        )?;
        let custom_text = self
            .custom_text
            .iter()
            .map(|(path, name)| {
                let key = (path.clone(), name.clone());
                let values = data_values(&extracted.value, path).unwrap_or_default();
                let texts = if let Some(binding) = self.definition.schema.operations.get(&key) {
                    values
                        .into_iter()
                        .flat_map(string_values)
                        .map(|text| binding.operation.map_text(text))
                        .collect::<Vec<_>>()
                } else {
                    values
                        .into_iter()
                        .flat_map(string_values)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                };
                (
                    self.binding_id(&BindingIdentity::CustomText {
                        path: path.clone(),
                        name: name.clone(),
                    }),
                    texts,
                )
            })
            .collect::<Vec<_>>();
        for (binding, texts) in custom_text {
            self.append_grams(
                slot,
                binding,
                crate::bigram::collect_record_gram_facts(self.alphabet, texts),
            );
        }
        Ok(())
    }

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

    fn finish(mut self) -> Result<PreparedStoreBatch, String> {
        let sort_keys = self.sorting.into_keys();
        let mut sorts = Vec::new();
        for (path, keys) in sort_keys {
            for (slot, key) in keys.into_iter().enumerate() {
                sorts.push(PreparedSort {
                    slot: slot as u32,
                    path: path.clone(),
                    key,
                });
            }
        }
        sorts.sort_by(|left, right| {
            left.slot
                .cmp(&right.slot)
                .then_with(|| left.path.segments.cmp(&right.path.segments))
        });
        let mut relations = Vec::new();
        let mut relation_states = Vec::new();
        for relation in &self.relations {
            let declaration = relation.declaration();
            for slot in 0..self.records.len() as u32 {
                for (occurrence_ordinal, (target_id, positions)) in relation
                    .references_for_source(RecordSlot(slot))
                    .enumerate()
                {
                    relations.push(PreparedRelation {
                        slot,
                        path: declaration.path.clone(),
                        occurrence_ordinal: occurrence_ordinal as u32,
                        target_collection: declaration.target_collection.clone(),
                        target_id: target_id.to_string(),
                        positions: positions.to_vec(),
                    });
                }
                for (state, slots) in [
                    (
                        crate::index_store::value_codec::RelationStateKind::Present,
                        relation.present(),
                    ),
                    (
                        crate::index_store::value_codec::RelationStateKind::Nulls,
                        relation.nulls(),
                    ),
                    (
                        crate::index_store::value_codec::RelationStateKind::EmptyArray,
                        relation.empty_arrays(),
                    ),
                    (
                        crate::index_store::value_codec::RelationStateKind::Unavailable,
                        relation.unavailable(),
                    ),
                ] {
                    if slots.contains(slot) {
                        relation_states.push(PreparedRelationState {
                            slot,
                            path: declaration.path.clone(),
                            state,
                        });
                    }
                }
            }
        }
        self.values.sort_by(|left, right| {
            left.slot
                .cmp(&right.slot)
                .then_with(|| left.binding.cmp(&right.binding))
                .then_with(|| left.value.cmp(&right.value))
        });
        self.values.dedup_by(|left, right| {
            left.slot == right.slot
                && left.binding == right.binding
                && left.value == right.value
        });
        self.lengths.sort_by(|left, right| {
            left.slot
                .cmp(&right.slot)
                .then_with(|| left.binding.cmp(&right.binding))
                .then_with(|| left.length.cmp(&right.length))
        });
        self.lengths.dedup_by(|left, right| {
            left.slot == right.slot
                && left.binding == right.binding
                && left.length == right.length
        });
        self.states.sort_by(|left, right| {
            left.slot
                .cmp(&right.slot)
                .then_with(|| left.binding.cmp(&right.binding))
                .then_with(|| left.mask.cmp(&right.mask))
        });
        for ((path, name), complete) in &self.custom_complete {
            if *complete {
                continue;
            }
            let binding = self.bindings.ids[&BindingIdentity::Named {
                path: path.clone(),
                name: name.clone(),
            }];
            if let Some(coverage) = self.coverage.get_mut(&binding) {
                coverage.value_complete = false;
                coverage.length_complete = false;
                coverage.empty_string_complete = false;
            }
        }
        for (binding, coverage) in &mut self.coverage {
            let mut values = 0usize;
            for fact in &self.values {
                if fact.binding == *binding && fact.slot == 0 {
                    values += 1;
                }
            }
            coverage.value_cardinality = values as u64;
        }
        let retained_capacity_bytes = {
            let base = self.values.len() * std::mem::size_of::<PreparedValue>()
                + self.lengths.len() * std::mem::size_of::<PreparedLength>()
                + self.states.len() * std::mem::size_of::<PreparedState>()
                + sorts.len() * std::mem::size_of::<PreparedSort>()
                + relations.len() * std::mem::size_of::<PreparedRelation>()
                + self.grams.len() * std::mem::size_of::<PreparedGramSet>();
            self.values
                .iter()
                .map(|value| match &value.value {
                    ScalarKey::String(value) | ScalarKey::Id(value) => value.len(),
                    ScalarKey::Number(crate::model::Number::ExactDecimal(value)) => value.digits.len(),
                    _ => 0,
                })
                .chain(
                    relations
                        .iter()
                        .map(|relation| {
                            relation.target_id.len()
                                + relation
                                    .path
                                    .iter()
                                    .map(String::len)
                                    .sum::<usize>()
                                + relation.positions.len() * std::mem::size_of::<u32>()
                        }),
                )
                .chain(
                    self.grams
                        .iter()
                        .map(|grams| grams.grams.len() * std::mem::size_of::<u64>()),
                )
                .fold(base, usize::saturating_add)
        };
        self.profile.batches = self.profile.batches.saturating_add(1);
        Ok(PreparedStoreBatch {
            files: self.files,
            file_fingerprints: self.file_fingerprints,
            bindings: self.bindings.bindings,
            records: self.records,
            values: self.values,
            lengths: self.lengths,
            states: self.states,
            sorts,
            relations,
            relation_states,
            grams: self.grams,
            fields: self.fields.into_iter().collect(),
            coverage: self.coverage.into_iter().collect(),
            inference: self.inference,
            logical_records: self.logical_records,
            empty_array_records: self.empty_array_records,
            record_parses: self.record_parses,
            failed_parses: self.failed_parses,
            source_reads: self.source_reads,
            source_bytes_read: self.source_bytes_read,
            profile: self.profile,
            retained_capacity_bytes,
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

fn process_batch_profiled(
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
                let started = crate::detailed_timer::start_if_enabled(sampled);
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
    let finish_started = crate::detailed_timer::start_if_enabled(profile_build);
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn process_store_batch_profiled(
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
) -> Result<PreparedStoreBatch, String> {
    let mut builder = DirectStoreBatchBuilder::new(
        root,
        definition,
        collection,
        source,
        alphabet,
        inference,
        inference_jobs,
    )?;
    let mut files = Vec::with_capacity(file_ids.len());
    let mut input_buffer = Vec::new();
    let mut input_buffer_profile = SourceBufferProfile::default();
    let mut file_fingerprints = Vec::with_capacity(file_ids.len());
    for &file_id in file_ids {
        let original = &source_files[file_id.0 as usize];
        let path = root.join(&original.relative_path);
        let mut file = original.for_collection(collection);
        let start = builder.records.len();
        let sampled = profile_build && file_id.0.is_multiple_of(64);
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
                let started = crate::detailed_timer::start_if_enabled(sampled);
                let result = builder.outcome(&mut file, &path, outcome, None);
                if let Some(started) = started {
                    callback_ns += started.elapsed().as_nanos() as u64;
                }
                result
            },
        )?;
        if trim_oversized_build_buffer(&mut input_buffer)
            && profile_build
        {
            builder.profile.input_buffer_oversized_drops += 1;
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
        builder.record_parses += stats.record_parses;
        builder.failed_parses += stats.failed_parses;
        files.push(BatchFile {
            entry: file,
            records: start..builder.records.len(),
            first_range: true,
        });
    }
    if profile_build {
        builder.profile.input_buffer_files_read += input_buffer_profile.files_read;
        builder.profile.input_buffer_growth_files += input_buffer_profile.growth_files;
        builder.profile.input_buffer_peak_capacity_bytes =
            input_buffer_profile.peak_capacity_bytes;
        builder.profile.input_buffer_batch_end_capacity_bytes += input_buffer.capacity() as u64;
    }
    builder.files = files;
    builder.file_fingerprints = file_fingerprints;
    builder.source_reads = input_buffer_profile.files_read;
    builder.source_bytes_read = input_buffer_profile.bytes_read;
    let finish_started = crate::detailed_timer::start_if_enabled(profile_build);
    let mut batch = builder.finish()?;
    if let Some(started) = finish_started {
        batch.profile.batch_finish_ns += started.elapsed().as_nanos() as u64;
    }
    Ok(batch)
}


/// A file's eligible collection bindings, resolved by discovery before I/O.
/// Preparation performs no publication and never changes source files.
#[derive(Clone, Copy)]
pub(crate) struct FilePreparationBinding<'a> {
    pub collection: CollectionId,
    pub definition: &'a CollectionDefinition,
    pub source: &'a PreparedSource,
}


pub(crate) struct PreparedCollectionBuild {
    pub collection: CollectionId,
    pub source_id: String,
    pub source_version: u32,
    pub contributions: Vec<BatchContribution>,
}


/// Private, complete per-file work. The coordinator must still validate the
/// file version, identities, and configuration before publishing any part.
pub(crate) struct PreparedFileBuild {
    pub source: SourceFileInput,
    /// Exact raw bytes from one preparation read, when that reader exposes it.
    pub fingerprint: Option<[u8; 32]>,
    pub collections: Vec<PreparedCollectionBuild>,
}


impl PreparedFileBuild {
    pub(crate) fn record_count(&self) -> u64 {
        self.collections
            .iter()
            .flat_map(|collection| &collection.contributions)
            .map(|contribution| contribution.records.len() as u64)
            .sum()
    }

    /// Record identity and framing reconstructed from one complete source.
    /// Verified removal compares this with active indexed ownership before
    /// using the old-byte hint for physical maintenance.
    pub(crate) fn record_layout(&self) -> Vec<(CollectionId, RecordId, ByteRange, RecordClass)> {
        self.collections
            .iter()
            .flat_map(|collection| {
                collection.contributions.iter().flat_map(|contribution| {
                    contribution.records.iter().map(|record| {
                        (
                            collection.collection,
                            record.id.clone(),
                            record.range,
                            record.class,
                        )
                    })
                })
            })
            .collect()
    }
}


/// Failure while preparing a complete replacement supplied by the caller.
/// A registered reader that cannot frame supplied bytes must be handled by
/// the path-based preparation route before publication.
#[derive(Debug, Eq, PartialEq)]
pub(crate) enum BytesPreparationError {
    RevisionSizeMismatch { declared: u64, actual: u64 },
    UnsupportedReader { source_id: String, reason: String },
    Build(String),
}


impl From<String> for BytesPreparationError {
    fn from(error: String) -> Self {
        Self::Build(error)
    }
}


struct HashingReader<'a, R> {
    inner: R,
    digest: Option<&'a mut Sha256>,
    bytes_read: &'a mut u64,
}


impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(bytes)?;
        if let Some(digest) = self.digest.as_mut() {
            digest.update(&bytes[..count]);
        }
        *self.bytes_read += count as u64;
        Ok(count)
    }
}


fn source_revision_at(path: &Path) -> Result<SourceRevision, String> {
    SourceRevision::from_metadata(
        &std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?,
    )
}

fn prepare_unframed_file(
    root: &Path,
    input: &SourceFileInput,
    binding: &FilePreparationBinding<'_>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
) -> Result<Vec<BatchContribution>, String> {
    let mut builder = BatchBuilder::new(
        root,
        binding.definition,
        binding.collection,
        binding.source,
        alphabet,
        inference,
        inference_jobs,
    )?;
    let path = root.join(&input.relative_path);
    let mut file = input.for_collection(binding.collection);
    let stats =
        extract_file_build_outcomes(&path, binding.source.reader(), input.revision, |outcome| {
            builder.outcome(&mut file, &path, outcome, None)
        })?;
    let count = builder.records.len();
    Ok(vec![builder.finish(
        vec![BatchFile {
            entry: file,
            records: 0..count,
            first_range: true,
        }],
        stats.record_parses,
        stats.failed_parses,
    )?])
}

fn prepare_bounded_json_lines(
    root: &Path,
    input: &SourceFileInput,
    binding: &FilePreparationBinding<'_>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
    hash_bytes: bool,
) -> Result<(Vec<BatchContribution>, Option<[u8; 32]>), String> {
    let path = root.join(&input.relative_path);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(_) => {
            return Ok((
                prepare_unframed_file(root, input, binding, alphabet, inference, inference_jobs)?,
                None,
            ));
        }
    };
    if SourceRevision::from_metadata(&file.metadata().map_err(|error| error.to_string())?)?
        != input.revision
    {
        return Err("stale source revision during preparation".into());
    }
    let mut digest = Sha256::new();
    let mut bytes_read = 0u64;
    let reader = HashingReader {
        inner: &file,
        digest: hash_bytes.then_some(&mut digest),
        bytes_read: &mut bytes_read,
    };
    let mut reader = BoundedJsonLines::from_reader(BufReader::new(reader));
    let limits = BoundedBuildLimits::default();
    let mut contributions = Vec::new();
    while let Some(chunk) = reader.next_chunk(limits.chunk_bytes, limits.ranges)? {
        contributions.push(process_framed_batch(
            root,
            input,
            binding,
            &chunk.bytes,
            &chunk.ranges,
            chunk.file_offset,
            chunk.preceding_lines,
            contributions.is_empty(),
            alphabet,
            inference,
            inference_jobs,
        )?);
    }
    drop(reader);
    if SourceRevision::from_metadata(&file.metadata().map_err(|error| error.to_string())?)?
        != input.revision
        || source_revision_at(&path)? != input.revision
        || bytes_read != input.revision.size
    {
        return Err("stale source revision during preparation".into());
    }
    if contributions.is_empty() {
        contributions.push(process_framed_batch(
            root,
            input,
            binding,
            &[],
            &[],
            0,
            0,
            true,
            alphabet,
            inference,
            inference_jobs,
        )?);
    }
    let fingerprint = hash_bytes.then(|| digest.finalize().into());
    Ok((contributions, fingerprint))
}

/// Prepare one physical file across all supplied eligible collections. JSONL
/// uses bounded chunks; other readers retain their existing whole-buffer
/// framing. A malformed frame falls back to ordinary build extraction.
pub(crate) fn prepare_file_build(
    root: &Path,
    input: SourceFileInput,
    bindings: &[FilePreparationBinding<'_>],
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
) -> Result<PreparedFileBuild, String> {
    let path = root.join(&input.relative_path);
    let mut fingerprint = None;
    let mut collections = Vec::with_capacity(bindings.len());
    for binding in bindings {
        if let Ok(revision) = source_revision_at(&path) {
            if revision != input.revision {
                return Err("stale source revision during preparation".into());
            }
        }
        let contributions = if binding.source.reader().supports_bounded_json_lines_build() {
            let (contributions, digest) = prepare_bounded_json_lines(
                root,
                &input,
                binding,
                alphabet,
                inference,
                inference_jobs,
                fingerprint.is_none(),
            )?;
            fingerprint = fingerprint.or(digest);
            contributions
        } else if let Some((bytes, ranges)) = frame_file_for_build(&path, binding.source.reader())?
        {
            if source_revision_at(&path)? != input.revision
                || bytes.len() as u64 != input.revision.size
            {
                return Err("stale source revision during preparation".into());
            }
            if fingerprint.is_none() {
                fingerprint = Some(Sha256::digest(&*bytes).into());
            }
            let mut contributions = Vec::new();
            let mut start = 0;
            while start < ranges.len() {
                let mut end = start;
                let mut weight = 0u64;
                while end < ranges.len() && end - start < 256 {
                    let size = ranges[end].end - ranges[end].start;
                    if end > start && weight.saturating_add(size) > 1024 * 1024 {
                        break;
                    }
                    weight = weight.saturating_add(size);
                    end += 1;
                }
                contributions.push(process_framed_batch(
                    root,
                    &input,
                    binding,
                    &bytes,
                    &ranges[start..end],
                    0,
                    0,
                    contributions.is_empty(),
                    alphabet,
                    inference,
                    inference_jobs,
                )?);
                start = end;
            }
            if contributions.is_empty() {
                contributions.push(process_framed_batch(
                    root,
                    &input,
                    binding,
                    &bytes,
                    &[],
                    0,
                    0,
                    true,
                    alphabet,
                    inference,
                    inference_jobs,
                )?);
            }
            contributions
        } else {
            prepare_unframed_file(root, &input, binding, alphabet, inference, inference_jobs)?
        };
        collections.push(PreparedCollectionBuild {
            collection: binding.collection,
            source_id: binding.source.id().into(),
            source_version: binding.source.version(),
            contributions,
        });
    }
    Ok(PreparedFileBuild {
        source: input,
        fingerprint,
        collections,
    })
}

/// Prepare a complete replacement without opening or writing its source path.
/// The caller owns `bytes` and must validate the declared revision and file
/// identity again before publishing the returned contributions. No raw payload
/// is kept in the result.
pub(crate) fn prepare_file_build_from_bytes(
    root: &Path,
    input: SourceFileInput,
    bytes: &[u8],
    bindings: &[FilePreparationBinding<'_>],
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
) -> Result<PreparedFileBuild, BytesPreparationError> {
    let actual = u64::try_from(bytes.len())
        .map_err(|_| BytesPreparationError::Build("source size overflow".into()))?;
    if input.revision.size != actual {
        return Err(BytesPreparationError::RevisionSizeMismatch {
            declared: input.revision.size,
            actual,
        });
    }
    let fingerprint = Some(Sha256::digest(bytes).into());
    let mut collections = Vec::with_capacity(bindings.len());
    for binding in bindings {
        let mut contributions = Vec::new();
        if binding.source.reader().supports_bounded_json_lines_build() {
            let mut reader = BoundedJsonLines::from_reader(BufReader::new(Cursor::new(bytes)));
            let limits = BoundedBuildLimits::default();
            while let Some(chunk) = reader
                .next_chunk(limits.chunk_bytes, limits.ranges)
                .map_err(BytesPreparationError::Build)?
            {
                contributions.push(process_framed_batch(
                    root,
                    &input,
                    binding,
                    &chunk.bytes,
                    &chunk.ranges,
                    chunk.file_offset,
                    chunk.preceding_lines,
                    contributions.is_empty(),
                    alphabet,
                    inference,
                    inference_jobs,
                )?);
            }
            if contributions.is_empty() {
                contributions.push(process_framed_batch(
                    root,
                    &input,
                    binding,
                    &[],
                    &[],
                    0,
                    0,
                    true,
                    alphabet,
                    inference,
                    inference_jobs,
                )?);
            }
        } else {
            match binding.source.reader().frame_outcome(bytes) {
                Ok(ranges) => {
                    validate_supplied_ranges(&ranges, actual)?;
                    let mut start = 0;
                    while start < ranges.len() {
                        let mut end = start;
                        let mut weight = 0u64;
                        while end < ranges.len() && end - start < 256 {
                            let size = ranges[end].end - ranges[end].start;
                            if end > start && weight.saturating_add(size) > 1024 * 1024 {
                                break;
                            }
                            weight = weight.saturating_add(size);
                            end += 1;
                        }
                        contributions.push(process_framed_batch(
                            root,
                            &input,
                            binding,
                            bytes,
                            &ranges[start..end],
                            0,
                            0,
                            contributions.is_empty(),
                            alphabet,
                            inference,
                            inference_jobs,
                        )?);
                        start = end;
                    }
                    if contributions.is_empty() {
                        contributions.push(process_framed_batch(
                            root,
                            &input,
                            binding,
                            bytes,
                            &[],
                            0,
                            0,
                            true,
                            alphabet,
                            inference,
                            inference_jobs,
                        )?);
                    }
                }
                Err(ReaderFailure::Malformed(diagnostic)) => {
                    contributions.push(prepare_malformed_supplied_file(
                        root,
                        &input,
                        binding,
                        bytes,
                        diagnostic,
                        alphabet,
                        inference,
                        inference_jobs,
                    )?);
                }
                Err(ReaderFailure::Fatal(reason)) => {
                    return Err(BytesPreparationError::UnsupportedReader {
                        source_id: binding.source.id().into(),
                        reason,
                    });
                }
            }
        }
        collections.push(PreparedCollectionBuild {
            collection: binding.collection,
            source_id: binding.source.id().into(),
            source_version: binding.source.version(),
            contributions,
        });
    }
    Ok(PreparedFileBuild {
        source: input,
        fingerprint,
        collections,
    })
}

fn validate_supplied_ranges(ranges: &[ByteRange], size: u64) -> Result<(), BytesPreparationError> {
    let mut previous_end = 0;
    for range in ranges {
        ByteRange::checked(range.start, range.end, size)
            .map_err(|error| BytesPreparationError::Build(format!("reader contract: {error}")))?;
        if range.start < previous_end {
            return Err(BytesPreparationError::Build(
                "reader contract: record ranges overlap or are out of order".into(),
            ));
        }
        previous_end = range.end;
    }
    Ok(())
}

fn prepare_malformed_supplied_file(
    root: &Path,
    input: &SourceFileInput,
    binding: &FilePreparationBinding<'_>,
    bytes: &[u8],
    diagnostic: ReaderDiagnostic,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
) -> Result<BatchContribution, BytesPreparationError> {
    let path = root.join(&input.relative_path);
    let mut file = input.for_collection(binding.collection);
    let mut builder = BatchBuilder::new(
        root,
        binding.definition,
        binding.collection,
        binding.source,
        alphabet,
        inference,
        inference_jobs,
    )?;
    builder.outcome(
        &mut file,
        &path,
        SourceOutcome::Malformed {
            range: ByteRange::checked(0, input.revision.size, input.revision.size)?,
            diagnostic,
            bytes: bytes.to_vec(),
        },
        None,
    )?;
    let count = builder.records.len();
    Ok(builder.finish(
        vec![BatchFile {
            entry: file,
            records: 0..count,
            first_range: true,
        }],
        0,
        0,
    )?)
}

fn process_framed_batch(
    root: &Path,
    original: &SourceFileInput,
    binding: &FilePreparationBinding<'_>,
    bytes: &[u8],
    ranges: &[ByteRange],
    file_offset: u64,
    preceding_lines: u64,
    first_range: bool,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
) -> Result<BatchContribution, String> {
    let path = root.join(&original.relative_path);
    let mut file = original.for_collection(binding.collection);
    let mut builder = BatchBuilder::new(
        root,
        binding.definition,
        binding.collection,
        binding.source,
        alphabet,
        inference,
        inference_jobs,
    )?;
    let stats =
        extract_framed_build_outcomes(&path, binding.source.reader(), bytes, ranges, |outcome| {
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

fn process_unframed_large_file(
    root: &Path,
    definition: &CollectionDefinition,
    collection: CollectionId,
    source: &PreparedSource,
    file_id: FileId,
    source_files: &[SourceFileInput],
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
    admit: &mut impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let batch = process_batch(
        root,
        definition,
        collection,
        source,
        &[file_id],
        source_files,
        alphabet,
        inference,
        inference_jobs,
    )?;
    let source_reads = batch.source_reads;
    let source_bytes_read = batch.source_bytes_read;
    let file_fingerprint = batch
        .file_fingerprints()
        .first()
        .map(|(_, digest)| (file_id, *digest));
    let started = crate::detailed_timer::DetailedInstant::now();
    admit(batch)?;
    Ok(BuildScheduleStats {
        file_fingerprint,
        source_reads,
        source_bytes_read,
        weight_limit_bytes: 64 * 1024 * 1024,
        peak_weight_bytes: source_files[file_id.0 as usize].revision.size,
        admission_ns: started.elapsed().as_nanos() as u64,
        ..Default::default()
    })
}

/// Built-in JSONL uses bounded build chunks. Other readers retain the
/// whole-buffer framing route, sharing one source buffer across range jobs.
/// Caller schedules oversized files one at a time.
#[cfg(test)]
pub(crate) fn process_framed_file(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    definition: Arc<CollectionDefinition>,
    collection: CollectionId,
    source: PreparedSource,
    file_id: FileId,
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    process_framed_file_profiled(
        pool,
        root,
        definition,
        collection,
        source,
        file_id,
        source_files,
        alphabet,
        inference,
        false,
        &mut admit,
    )
}

pub(crate) fn process_framed_file_profiled(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    definition: Arc<CollectionDefinition>,
    collection: CollectionId,
    source: PreparedSource,
    file_id: FileId,
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    profile_build: bool,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    if source.reader().supports_bounded_json_lines_build() {
        return process_bounded_json_lines_file(
            pool,
            root,
            definition,
            collection,
            source,
            file_id,
            source_files,
            alphabet,
            inference,
            profile_build,
            admit,
        );
    }
    const MAX_RANGE_BYTES: u64 = 1024 * 1024;
    const MAX_RANGES: usize = 256;
    const MAX_ADMITTED_BYTES: u64 = 64 * 1024 * 1024;
    let max_jobs = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    let mut stats = BuildScheduleStats {
        weight_limit_bytes: MAX_ADMITTED_BYTES,
        ..Default::default()
    };
    let input = &source_files[file_id.0 as usize];
    let path = root.join(&input.relative_path);
    let Some((bytes, ranges)) = frame_file_for_build(&path, source.reader())? else {
        return process_unframed_large_file(
            &root,
            &definition,
            collection,
            &source,
            file_id,
            &source_files,
            alphabet,
            inference,
            max_jobs,
            &mut admit,
        );
    };
    let file_fingerprint = (bytes.len() as u64 == input.revision.size)
        .then(|| (file_id, Sha256::digest(&*bytes).into()));
    let source_bytes = bytes.len() as u64;
    stats.source_reads = 1;
    stats.source_bytes_read = source_bytes;
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < ranges.len() {
        let mut end = start;
        let mut weight = 0u64;
        while end < ranges.len() && end - start < MAX_RANGES {
            let size = ranges[end].end - ranges[end].start;
            if end > start && weight.saturating_add(size) > MAX_RANGE_BYTES {
                break;
            }
            weight = weight.saturating_add(size);
            end += 1;
        }
        chunks.push((start..end, weight.max(64 * 1024)));
        start = end;
    }
    if chunks.is_empty() {
        chunks.push((0..0, 64 * 1024));
    }
    let ranges = Arc::new(ranges);
    let (sender, receiver) = mpsc::channel();
    let mut ready = BTreeMap::new();
    let mut dispatched = 0;
    let mut completed = 0;
    let mut next = 0;
    let mut admitted_bytes = 0u64;
    let mut peak_bytes = 0u64;
    let mut failure = None;
    let counters = Arc::new(JobCounters::default());
    while next < chunks.len() && failure.is_none() {
        while dispatched < chunks.len() && dispatched - next < max_jobs {
            let weight = chunks[dispatched].1;
            if !fits_allowance(admitted_bytes, weight, MAX_ADMITTED_BYTES) {
                break;
            }
            let index = dispatched;
            let chunk = chunks[index].0.clone();
            let sender = sender.clone();
            let root = Arc::clone(&root);
            let definition = Arc::clone(&definition);
            let source = source.clone();
            let source_files = Arc::clone(&source_files);
            let bytes = Arc::clone(&bytes);
            let ranges = Arc::clone(&ranges);
            let counters = Arc::clone(&counters);
            pool.spawn(move || {
                let result = counters.run(|| {
                    process_framed_batch(
                        &root,
                        &source_files[file_id.0 as usize],
                        &FilePreparationBinding {
                            collection,
                            definition: &definition,
                            source: &source,
                        },
                        &bytes,
                        &ranges[chunk],
                        0,
                        0,
                        index == 0,
                        alphabet,
                        inference,
                        max_jobs,
                    )
                });
                let _ = sender.send((index, result));
            });
            admitted_bytes = admitted_bytes.saturating_add(weight);
            peak_bytes = peak_bytes.max(admitted_bytes);
            dispatched += 1;
        }
        let wait_started = crate::detailed_timer::DetailedInstant::now();
        let received = receiver.recv();
        stats.receive_wait_ns = stats
            .receive_wait_ns
            .saturating_add(wait_started.elapsed().as_nanos() as u64);
        let (index, result) = received.map_err(|error| error.to_string())?;
        completed += 1;
        ready.insert(index, result);
        while let Some(result) = ready.remove(&next) {
            admitted_bytes -= chunks[next].1;
            let admission_started = crate::detailed_timer::DetailedInstant::now();
            let outcome = result.and_then(&mut admit);
            stats.admission_ns = stats
                .admission_ns
                .saturating_add(admission_started.elapsed().as_nanos() as u64);
            match outcome {
                Ok(()) => {
                    counters.admitted.fetch_add(1, Ordering::Relaxed);
                    next += 1;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
    }
    while completed < dispatched {
        let _ = receiver.recv().map_err(|error| error.to_string())?;
        completed += 1;
    }
    if let Some(error) = failure {
        return Err(error);
    }
    stats.peak_weight_bytes = source_bytes.saturating_add(peak_bytes);
    stats.file_fingerprint = file_fingerprint;
    counters.add_to(&mut stats);
    Ok(stats)
}

/// JSONL construction keeps only bounded line chunks and admitted worker
/// contributions alive. A single line larger than the normal chunk size is a
/// singleton job; memory therefore scales with the largest record, not the
/// whole file. Parsing remains parallel and admission remains file-order.
fn process_bounded_json_lines_file(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    definition: Arc<CollectionDefinition>,
    collection: CollectionId,
    source: PreparedSource,
    file_id: FileId,
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    profile_build: bool,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let max_jobs = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    let path = root.join(&source_files[file_id.0 as usize].relative_path);
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(_) => {
            return process_unframed_large_file(
                &root,
                &definition,
                collection,
                &source,
                file_id,
                &source_files,
                alphabet,
                inference,
                max_jobs,
                &mut admit,
            );
        }
    };
    let mut digest = Sha256::new();
    let mut bytes_read = 0u64;
    let expected_size = source_files[file_id.0 as usize].revision.size;
    let reader = BoundedJsonLines::from_reader(BufReader::new(HashingReader {
        inner: &file,
        digest: Some(&mut digest),
        bytes_read: &mut bytes_read,
    }));
    let mut stats = process_bounded_json_lines_reader(
        pool,
        root,
        definition,
        collection,
        source,
        file_id,
        source_files,
        alphabet,
        inference,
        profile_build,
        reader,
        BoundedBuildLimits::default(),
        admit,
    )?;
    // A fallback can already provide the complete normal-extraction digest.
    // Otherwise only a complete bounded read may identify the indexed bytes.
    if stats.file_fingerprint.is_none() && bytes_read == expected_size {
        stats.file_fingerprint = Some((file_id, digest.finalize().into()));
    }
    stats.source_reads = stats.source_reads.saturating_add(1);
    stats.source_bytes_read = stats.source_bytes_read.saturating_add(bytes_read);
    Ok(stats)
}


#[derive(Clone, Copy)]
struct BoundedBuildLimits {
    chunk_bytes: usize,
    ranges: usize,
    admitted_bytes: u64,
}


impl Default for BoundedBuildLimits {
    fn default() -> Self {
        Self {
            chunk_bytes: 1024 * 1024,
            ranges: 256,
            admitted_bytes: 64 * 1024 * 1024,
        }
    }
}


fn process_bounded_json_lines_reader<R: BufRead + Send>(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    definition: Arc<CollectionDefinition>,
    collection: CollectionId,
    source: PreparedSource,
    file_id: FileId,
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    profile_build: bool,
    mut reader: BoundedJsonLines<R>,
    limits: BoundedBuildLimits,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let max_jobs = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    let path = root.join(&source_files[file_id.0 as usize].relative_path);
    // A failure before any contribution can still use the ordinary unreadable
    // outcome. Once a chunk is admitted, an I/O failure aborts this private
    // generation instead of publishing a partially indexed file.
    let first_started = crate::detailed_timer::start_if_enabled(profile_build);
    let first = match reader.next_chunk(limits.chunk_bytes, limits.ranges) {
        Ok(Some(chunk)) => chunk,
        Ok(None) => BoundedBuildChunk {
            bytes: Vec::new(),
            ranges: Vec::new(),
            file_offset: 0,
            preceding_lines: 0,
        },
        Err(_) => {
            return process_unframed_large_file(
                &root,
                &definition,
                collection,
                &source,
                file_id,
                &source_files,
                alphabet,
                inference,
                max_jobs,
                &mut admit,
            );
        }
    };
    let mut pending = Some(first);
    let mut eof = false;
    let mut stats = BuildScheduleStats {
        weight_limit_bytes: limits.admitted_bytes,
        ..Default::default()
    };
    if let Some(started) = first_started {
        stats.framing_ns += started.elapsed().as_nanos() as u64;
        stats.framing_chunks += 1;
        stats.framing_bytes += pending.as_ref().map_or(0, |chunk| chunk.bytes.len()) as u64;
    }
    let (sender, receiver) = mpsc::channel();
    let counters = Arc::new(JobCounters::default());
    let mut ready = BTreeMap::new();
    let mut reservations = VecDeque::new();
    let (mut dispatched, mut completed, mut next) = (0usize, 0usize, 0usize);
    let (mut admitted_bytes, mut peak_bytes) = (0u64, 0u64);
    let mut failure = None;
    let mut read_failure = false;
    while failure.is_none() {
        while dispatched - next < max_jobs {
            if pending.is_none() && !eof {
                let started = crate::detailed_timer::start_if_enabled(profile_build);
                match reader.next_chunk(limits.chunk_bytes, limits.ranges) {
                    Ok(Some(chunk)) => {
                        if let Some(started) = started {
                            stats.framing_ns += started.elapsed().as_nanos() as u64;
                            stats.framing_chunks += 1;
                            stats.framing_bytes += chunk.bytes.len() as u64;
                        }
                        pending = Some(chunk);
                    }
                    Ok(None) => eof = true,
                    Err(error) => {
                        failure =
                            Some(format!("{}: bounded framing read: {error}", path.display()));
                        read_failure = true;
                        break;
                    }
                }
            }
            let Some(chunk) = pending.as_ref() else {
                break;
            };
            let weight = (chunk.bytes.capacity() as u64).max(64 * 1024);
            if !fits_allowance(admitted_bytes, weight, limits.admitted_bytes) {
                break;
            }
            let chunk = pending.take().expect("checked pending chunk");
            let index = dispatched;
            let sender = sender.clone();
            let root = Arc::clone(&root);
            let definition = Arc::clone(&definition);
            let source = source.clone();
            let source_files = Arc::clone(&source_files);
            let counters = Arc::clone(&counters);
            pool.spawn(move || {
                let result = counters.run(|| {
                    process_framed_batch(
                        &root,
                        &source_files[file_id.0 as usize],
                        &FilePreparationBinding {
                            collection,
                            definition: &definition,
                            source: &source,
                        },
                        &chunk.bytes,
                        &chunk.ranges,
                        chunk.file_offset,
                        chunk.preceding_lines,
                        index == 0,
                        alphabet,
                        inference,
                        max_jobs,
                    )
                });
                let _ = sender.send((index, result));
            });
            reservations.push_back(weight);
            admitted_bytes = admitted_bytes.saturating_add(weight);
            peak_bytes = peak_bytes.max(admitted_bytes);
            dispatched += 1;
        }
        if failure.is_some() {
            break;
        }
        if next == dispatched {
            if eof {
                break;
            }
            continue;
        }
        let wait_started = crate::detailed_timer::DetailedInstant::now();
        let (index, result) = receiver.recv().map_err(|error| error.to_string())?;
        stats.receive_wait_ns = stats
            .receive_wait_ns
            .saturating_add(wait_started.elapsed().as_nanos() as u64);
        completed += 1;
        ready.insert(index, result);
        while let Some(result) = ready.remove(&next) {
            let weight = reservations
                .pop_front()
                .ok_or("missing chunk reservation")?;
            admitted_bytes = admitted_bytes
                .checked_sub(weight)
                .ok_or("chunk reservation underflow")?;
            let started = crate::detailed_timer::DetailedInstant::now();
            let outcome = result.and_then(&mut admit);
            stats.admission_ns = stats
                .admission_ns
                .saturating_add(started.elapsed().as_nanos() as u64);
            match outcome {
                Ok(()) => {
                    counters.admitted.fetch_add(1, Ordering::Relaxed);
                    next += 1;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
    }
    while completed < dispatched {
        let _ = receiver.recv().map_err(|error| error.to_string())?;
        completed += 1;
    }
    if read_failure && next == 0 {
        return process_unframed_large_file(
            &root,
            &definition,
            collection,
            &source,
            file_id,
            &source_files,
            alphabet,
            inference,
            max_jobs,
            &mut admit,
        );
    }
    if let Some(error) = failure {
        return Err(error);
    }
    stats.peak_weight_bytes = peak_bytes;
    counters.add_to(&mut stats);
    Ok(stats)
}

/// Resolve a fresh build's worker policy independently of query workers.
/// Injecting detection makes the quota/failure cases deterministic in tests.
pub(crate) fn resolve_build_workers(
    requested: Option<NonZeroUsize>,
    detected: Option<NonZeroUsize>,
) -> Result<usize, String> {
    let available = detected.map_or(1, NonZeroUsize::get);
    if let Some(requested) = requested {
        if requested.get() > available {
            return Err(format!(
                "build_workers={} exceeds available_parallelism={available}",
                requested.get()
            ));
        }
        Ok(requested.get())
    } else {
        Ok(available)
    }
}

pub(crate) fn normal_batch_allowance(workers: usize) -> u64 {
    u64::try_from(workers)
        .unwrap_or(u64::MAX)
        .saturating_mul(16 * 1024 * 1024)
        .clamp(52 * 1024 * 1024, 512 * 1024 * 1024)
}

fn fits_allowance(current: u64, weight: u64, limit: u64) -> bool {
    current == 0 || (weight <= limit && current.saturating_add(weight) <= limit)
}


#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BuildScheduleStats {
    /// Digest of the one large file scheduled by this call, if readable.
    /// The repository coordinator must collect it before aggregate `absorb`.
    pub(crate) file_fingerprint: Option<(FileId, [u8; 32])>,
    pub source_reads: u64,
    pub source_bytes_read: u64,
    pub weight_limit_bytes: u64,
    pub peak_weight_bytes: u64,
    pub peak_active_jobs: usize,
    pub peak_ready_batches: usize,
    /// Lower bound for received results awaiting admission; active workers
    /// and completed messages still in the channel are excluded.
    pub peak_ready_bigram_capacity_bytes: usize,
    pub job_execution_ns: u64,
    pub receive_wait_ns: u64,
    pub admission_ns: u64,
    pub out_of_order_completed: u64,
    /// Sum of completed-batch time waiting for earlier ordinal admission.
    /// Overlaps across batches and is not an exclusive wall interval.
    pub ready_wait_sum_ns: u64,
    pub ready_wait_max_ns: u64,
    /// Bounded JSONL chunk read, frame, and inline fingerprint work.
    pub framing_ns: u64,
    pub framing_chunks: u64,
    pub framing_bytes: u64,
    pub admission_schedule: AdmissionScheduleProfile,
}


/// Opt-in aggregate intervals. Job intervals overlap across workers and are
/// not additive wall time; receive-call time includes channel processing.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct AdmissionScheduleProfile {
    pub dispatch_to_worker_start_ns: u64,
    pub worker_start_to_completion_ns: u64,
    pub completion_to_receipt_ns: u64,
    pub receipt_to_admission_start_ns: u64,
    pub coordinator_receive_call_ns: u64,
    pub possible_idle_during_admission_ns: u64,
    pub job_limit_blocks: u64,
    pub byte_limit_blocks: u64,
    pub prefetched_later_collection_jobs: u64,
}


impl AdmissionScheduleProfile {
    fn absorb(&mut self, other: Self) {
        self.dispatch_to_worker_start_ns += other.dispatch_to_worker_start_ns;
        self.worker_start_to_completion_ns += other.worker_start_to_completion_ns;
        self.completion_to_receipt_ns += other.completion_to_receipt_ns;
        self.receipt_to_admission_start_ns += other.receipt_to_admission_start_ns;
        self.coordinator_receive_call_ns += other.coordinator_receive_call_ns;
        self.possible_idle_during_admission_ns += other.possible_idle_during_admission_ns;
        self.job_limit_blocks += other.job_limit_blocks;
        self.byte_limit_blocks += other.byte_limit_blocks;
        self.prefetched_later_collection_jobs += other.prefetched_later_collection_jobs;
    }
}


impl BuildScheduleStats {
    pub fn absorb(&mut self, other: Self) {
        self.weight_limit_bytes = self.weight_limit_bytes.max(other.weight_limit_bytes);
        self.peak_weight_bytes = self.peak_weight_bytes.max(other.peak_weight_bytes);
        self.peak_active_jobs = self.peak_active_jobs.max(other.peak_active_jobs);
        self.peak_ready_batches = self.peak_ready_batches.max(other.peak_ready_batches);
        self.peak_ready_bigram_capacity_bytes = self
            .peak_ready_bigram_capacity_bytes
            .max(other.peak_ready_bigram_capacity_bytes);
        self.job_execution_ns = self.job_execution_ns.saturating_add(other.job_execution_ns);
        self.receive_wait_ns = self.receive_wait_ns.saturating_add(other.receive_wait_ns);
        self.admission_ns = self.admission_ns.saturating_add(other.admission_ns);
        self.out_of_order_completed += other.out_of_order_completed;
        self.ready_wait_sum_ns = self
            .ready_wait_sum_ns
            .saturating_add(other.ready_wait_sum_ns);
        self.ready_wait_max_ns = self.ready_wait_max_ns.max(other.ready_wait_max_ns);
        self.framing_ns = self.framing_ns.saturating_add(other.framing_ns);
        self.framing_chunks += other.framing_chunks;
        self.framing_bytes += other.framing_bytes;
        self.source_reads = self.source_reads.saturating_add(other.source_reads);
        self.source_bytes_read = self
            .source_bytes_read
            .saturating_add(other.source_bytes_read);
        self.admission_schedule.absorb(other.admission_schedule);
    }
}


#[derive(Default)]
struct JobCounters {
    active: AtomicUsize,
    peak_active: AtomicUsize,
    completed: AtomicUsize,
    admitted: AtomicUsize,
    peak_ready: AtomicUsize,
    #[cfg(feature = "detailed-profiling")]
    execution_ns: AtomicU64,
}


impl JobCounters {
    fn run<T>(&self, job: impl FnOnce() -> T) -> T {
        let active = self.active.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_active.fetch_max(active, Ordering::Relaxed);
        #[cfg(feature = "detailed-profiling")]
        let started = crate::detailed_timer::DetailedInstant::now();
        let result = job();
        #[cfg(feature = "detailed-profiling")]
        self.execution_ns
            .fetch_add(started.elapsed().as_nanos() as u64, Ordering::Relaxed);
        self.active.fetch_sub(1, Ordering::Relaxed);
        let completed = self.completed.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_ready.fetch_max(
            completed.saturating_sub(self.admitted.load(Ordering::Relaxed)),
            Ordering::Relaxed,
        );
        result
    }

    fn add_to(&self, stats: &mut BuildScheduleStats) {
        stats.peak_active_jobs = self.peak_active.load(Ordering::Relaxed);
        stats.peak_ready_batches = self.peak_ready.load(Ordering::Relaxed);
        #[cfg(feature = "detailed-profiling")]
        {
            stats.job_execution_ns = self.execution_ns.load(Ordering::Relaxed);
        }
    }
}


/// Ordered admission keeps the established slot and diagnostic order even if
/// later file batches finish first. The window includes completed batches
/// awaiting admission, so an early slow job cannot queue the whole corpus.
pub(crate) struct RepositoryFileJob {
    pub collection_index: usize,
    pub collection: CollectionId,
    pub definition: Arc<CollectionDefinition>,
    pub source: PreparedSource,
    pub files: Vec<FileId>,
}

pub(crate) trait ScheduledBatch: Send + 'static {
    fn retained_capacity_bytes(&self) -> usize;
}

impl ScheduledBatch for BatchContribution {
    fn retained_capacity_bytes(&self) -> usize {
        self.bigram_storage_stats().retained_capacity_bytes
    }
}


pub(crate) fn process_repository_file_batches(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    jobs: &[RepositoryFileJob],
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    mut admit: impl FnMut(usize, BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    process_repository_file_batches_profiled(
        pool,
        root,
        jobs,
        source_files,
        alphabet,
        inference,
        false,
        &mut admit,
    )
}

pub(crate) fn process_repository_file_batches_profiled(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    jobs: &[RepositoryFileJob],
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    profile_build: bool,
    mut admit: impl FnMut(usize, BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    process_repository_file_batches_prepared_profiled(
        pool,
        root,
        jobs,
        source_files,
        alphabet,
        inference,
        profile_build,
        None,
        |root: &Path,
         definition: &CollectionDefinition,
         collection,
         source: &PreparedSource,
         file_ids: &[FileId],
         source_files: &[SourceFileInput],
         alphabet,
         inference,
         inference_jobs,
         profile_build| {
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
                profile_build,
            )
        },
        &mut admit,
    )
}

pub(crate) fn process_repository_file_batches_prepared_profiled<T, P, A>(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    jobs: &[RepositoryFileJob],
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    profile_build: bool,
    retained_limit_bytes: Option<u64>,
    execute: P,
    mut admit: A,
) -> Result<BuildScheduleStats, String>
where
    T: ScheduledBatch,
    P: Fn(
            &Path,
            &CollectionDefinition,
            CollectionId,
            &PreparedSource,
            &[FileId],
            &[SourceFileInput],
            BigramAlphabet,
            InferenceConfig,
            usize,
            bool,
        ) -> Result<T, String>
        + Clone
        + Send
        + 'static,
    A: FnMut(usize, T) -> Result<(), String>,
{
    const MIN_FILE_BYTES: u64 = 16 * 1024;
    const PREPARED_OUTPUT_WEIGHT_FACTOR: u64 = 8;
    let allowance =
        retained_limit_bytes.unwrap_or_else(|| normal_batch_allowance(pool.current_num_threads()));
    let mut weights = jobs
        .iter()
        .map(|job| {
            job.files.iter().fold(0u64, |total, id| {
                let file = &source_files[id.0 as usize];
                total.saturating_add(file.revision.size.max(MIN_FILE_BYTES))
            })
            .saturating_mul(PREPARED_OUTPUT_WEIGHT_FACTOR)
        })
        .collect::<Vec<_>>();
    let max_jobs = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    let mut future_first_jobs = jobs
        .iter()
        .enumerate()
        .filter(|(index, job)| {
            *index > 0 && jobs[*index - 1].collection_index != job.collection_index
        })
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    future_first_jobs.sort_by_key(|index| (weights[*index], *index));
    let mut order = (0..jobs.len()).collect::<Vec<_>>();
    // Precompute a couple of small collections, leaving most of the byte
    // window for the first collection's ordered work. The next required job
    // is always scheduled inside the initial window to avoid head-of-line
    // deadlock on small pools.
    let mut prefetched = Vec::new();
    let mut prefetch_bytes = 0u64;
    for index in future_first_jobs {
        if pool.current_num_threads() == 1
            || prefetched.len() == 2
            || prefetched.len() + 2 > max_jobs
        {
            break;
        }
        let weight = weights[index];
        if prefetch_bytes.saturating_add(weight) > allowance / 4
            || prefetch_bytes
                .saturating_add(weight)
                .saturating_add(weights[0])
                > allowance
        {
            continue;
        }
        prefetch_bytes += weight;
        prefetched.push(index);
    }
    if !prefetched.is_empty() {
        order.clear();
        order.extend(prefetched.iter().copied());
        order.extend((0..jobs.len()).filter(|index| !prefetched.contains(index)));
    }
    let (sender, receiver) = mpsc::channel();
    let mut ready = BTreeMap::new();
    let mut ready_since = BTreeMap::new();
    let mut receipt_since = BTreeMap::new();
    let mut dispatched = 0;
    let mut completed = 0;
    let mut next = 0;
    let mut admitted_bytes = 0u64;
    let mut peak_bytes = 0u64;
    let mut failure = None;
    let counters = Arc::new(JobCounters::default());
    let mut stats = BuildScheduleStats {
        weight_limit_bytes: allowance,
        ..Default::default()
    };
    if profile_build {
        stats.admission_schedule.prefetched_later_collection_jobs = prefetched.len() as u64;
    }
    while next < jobs.len() && failure.is_none() {
        while dispatched < jobs.len() && dispatched - next < max_jobs {
            let index = order[dispatched];
            let weight = weights[index];
            if !fits_allowance(admitted_bytes, weight, allowance) {
                if profile_build {
                    stats.admission_schedule.byte_limit_blocks += 1;
                }
                break;
            }
            let file_ids = jobs[index].files.clone();
            let sender = sender.clone();
            let root = Arc::clone(&root);
            let definition = Arc::clone(&jobs[index].definition);
            let source = jobs[index].source.clone();
            let collection = jobs[index].collection;
            let source_files = Arc::clone(&source_files);
            let counters = Arc::clone(&counters);
            let execute = execute.clone();
            let dispatched_at = crate::detailed_timer::start_if_enabled(profile_build);
            pool.spawn(move || {
                let started_at = crate::detailed_timer::start_if_enabled(profile_build);
                let result = counters.run(|| {
                    execute(
                        &root,
                        &definition,
                        collection,
                        &source,
                        &file_ids,
                        &source_files,
                        alphabet,
                        inference,
                        max_jobs,
                        profile_build,
                    )
                });
                let completed_at = crate::detailed_timer::start_if_enabled(profile_build);
                let _ = sender.send((index, result, dispatched_at, started_at, completed_at));
            });
            admitted_bytes = admitted_bytes.saturating_add(weight);
            peak_bytes = peak_bytes.max(admitted_bytes);
            dispatched += 1;
        }
        if profile_build && dispatched < jobs.len() && dispatched - next >= max_jobs {
            stats.admission_schedule.job_limit_blocks += 1;
        }
        let wait_started = crate::detailed_timer::DetailedInstant::now();
        let received = receiver.recv();
        let received_at = crate::detailed_timer::DetailedInstant::now();
        let receive_ns = received_at.duration_since(wait_started).as_nanos() as u64;
        stats.receive_wait_ns = stats.receive_wait_ns.saturating_add(receive_ns);
        let (index, result, dispatched_at, started_at, completed_at) =
            received.map_err(|error| error.to_string())?;
        completed += 1;
        if profile_build {
            stats.admission_schedule.coordinator_receive_call_ns += receive_ns;
            if let (Some(dispatched_at), Some(started_at), Some(completed_at)) =
                (dispatched_at, started_at, completed_at)
            {
                stats.admission_schedule.dispatch_to_worker_start_ns +=
                    started_at.duration_since(dispatched_at).as_nanos() as u64;
                stats.admission_schedule.worker_start_to_completion_ns +=
                    completed_at.duration_since(started_at).as_nanos() as u64;
                stats.admission_schedule.completion_to_receipt_ns +=
                    received_at.duration_since_instant(completed_at).as_nanos() as u64;
            }
            receipt_since.insert(index, received_at);
            if index != next {
                stats.out_of_order_completed += 1;
                ready_since.insert(index, received_at);
            }
        }
        let actual_retained = result
            .as_ref()
            .ok()
            .map_or(weights[index], |batch| {
                batch.retained_capacity_bytes() as u64
            });
        if actual_retained > weights[index] {
            admitted_bytes = admitted_bytes.saturating_add(actual_retained - weights[index]);
            peak_bytes = peak_bytes.max(admitted_bytes);
        } else {
            admitted_bytes = admitted_bytes.saturating_sub(weights[index] - actual_retained);
        }
        weights[index] = actual_retained;
        ready.insert(index, result);
        if profile_build {
            let ready_bytes = ready
                .values()
                .filter_map(|result| result.as_ref().ok())
                .fold(0usize, |bytes, batch| {
                    bytes.saturating_add(batch.retained_capacity_bytes())
                });
            stats.peak_ready_bigram_capacity_bytes =
                stats.peak_ready_bigram_capacity_bytes.max(ready_bytes);
        }
        while let Some(result) = ready.remove(&next) {
            if let Some(received_at) = receipt_since.remove(&next) {
                stats.admission_schedule.receipt_to_admission_start_ns +=
                    received_at.elapsed().as_nanos() as u64;
            }
            if let Some(started) = ready_since.remove(&next) {
                let wait = started.elapsed().as_nanos() as u64;
                stats.ready_wait_sum_ns = stats.ready_wait_sum_ns.saturating_add(wait);
                stats.ready_wait_max_ns = stats.ready_wait_max_ns.max(wait);
            }
            admitted_bytes -= weights[next];
            let admission_started = crate::detailed_timer::DetailedInstant::now();
            let possible_idle = profile_build
                && dispatched < jobs.len()
                && dispatched.saturating_sub(completed) < pool.current_num_threads();
            let outcome = result.and_then(|batch| admit(jobs[next].collection_index, batch));
            let admission_ns = admission_started.elapsed().as_nanos() as u64;
            stats.admission_ns = stats.admission_ns.saturating_add(admission_ns);
            if possible_idle {
                stats.admission_schedule.possible_idle_during_admission_ns += admission_ns;
            }
            match outcome {
                Ok(()) => {
                    counters.admitted.fetch_add(1, Ordering::Relaxed);
                    next += 1;
                }
                Err(error) => {
                    failure = Some(error);
                    break;
                }
            }
        }
    }
    // Rayon jobs own only immutable inputs. Drain them before the pool is
    // dropped, even on an admission or parser error.
    while completed < dispatched {
        let _ = receiver.recv().map_err(|error| error.to_string())?;
        completed += 1;
    }
    if let Some(error) = failure {
        return Err(error);
    }
    stats.peak_weight_bytes = peak_bytes;
    counters.add_to(&mut stats);
    Ok(stats)
}

#[cfg(test)]
pub(crate) fn process_file_batches(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    definition: Arc<CollectionDefinition>,
    collection: CollectionId,
    source: PreparedSource,
    jobs: &[&[FileId]],
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let jobs = jobs
        .iter()
        .map(|files| RepositoryFileJob {
            collection_index: 0,
            collection,
            definition: Arc::clone(&definition),
            source: source.clone(),
            files: files.to_vec(),
        })
        .collect::<Vec<_>>();
    process_repository_file_batches(
        pool,
        root,
        &jobs,
        source_files,
        alphabet,
        InferenceConfig::default(),
        |_, batch| admit(batch),
    )
}


pub(crate) struct CollectionAdmission<'a> {
    pub inventory: &'a mut FileInventory,
    pub builder: &'a mut CollectionBuilder,
    pub postings: &'a mut PostingCollection,
    pub inference: &'a mut Option<InferenceBuilder>,
    pub custom_postings: &'a mut HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: &'a mut BigramContribution,
    pub fallback_text: &'a mut BigramContribution,
    pub custom_text: &'a mut HashMap<(Vec<String>, String), BigramContribution>,
    pub logical_records: &'a mut RoaringBitmap,
    pub empty_array_records: &'a mut RoaringBitmap,
    pub sorting: &'a mut SortBuilder,
    pub relations: &'a mut Vec<RelationContribution>,
}


/// Optional aggregate timing of the ordered merge. Values from separate
/// batches are summed; `remainder_ns` includes timer and drop boundaries.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct AdmissionBreakdown {
    pub batches: u64,
    pub files: u64,
    pub records: u64,
    pub callback_ns: u64,
    pub total_ns: u64,
    /// Incremental coordinator accounting and capacity checks.
    pub coordinator_ns: u64,
    /// Collection-wide inference copy and merge outside segment admission.
    pub inference_observation_ns: u64,
    pub record_inventory_ns: u64,
    pub postings_ns: u64,
    pub inference_ns: u64,
    pub relations_ns: u64,
    pub custom_postings_ns: u64,
    pub bigrams_ns: u64,
    /// Incremental bigram merges; `bigrams_ns` is their nonadditive parent.
    pub raw_bigrams_ns: u64,
    pub fallback_bigrams_ns: u64,
    pub custom_bigrams_ns: u64,
    pub structural_facts_ns: u64,
    pub sorting_ns: u64,
    pub remainder_ns: u64,
}


impl AdmissionBreakdown {
    pub(crate) fn absorb(&mut self, other: Self) {
        self.batches += other.batches;
        self.files += other.files;
        self.records += other.records;
        self.callback_ns += other.callback_ns;
        self.total_ns += other.total_ns;
        self.coordinator_ns += other.coordinator_ns;
        self.inference_observation_ns += other.inference_observation_ns;
        self.record_inventory_ns += other.record_inventory_ns;
        self.postings_ns += other.postings_ns;
        self.inference_ns += other.inference_ns;
        self.relations_ns += other.relations_ns;
        self.custom_postings_ns += other.custom_postings_ns;
        self.bigrams_ns += other.bigrams_ns;
        self.raw_bigrams_ns += other.raw_bigrams_ns;
        self.fallback_bigrams_ns += other.fallback_bigrams_ns;
        self.custom_bigrams_ns += other.custom_bigrams_ns;
        self.structural_facts_ns += other.structural_facts_ns;
        self.sorting_ns += other.sorting_ns;
        self.remainder_ns += other.remainder_ns;
    }

    /// Exclusive incremental phases; omit the aggregate `bigrams_ns` parent.
    pub(crate) fn incremental_component_ns(&self) -> u64 {
        self.coordinator_ns
            + self.inference_observation_ns
            + self.record_inventory_ns
            + self.postings_ns
            + self.inference_ns
            + self.relations_ns
            + self.custom_postings_ns
            + self.raw_bigrams_ns
            + self.fallback_bigrams_ns
            + self.custom_bigrams_ns
            + self.structural_facts_ns
            + self.sorting_ns
    }
}


impl CollectionAdmission<'_> {
    pub(crate) fn admit_profiled(
        &mut self,
        batch: BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<(), String> {
        if let Some(profile) = profile.as_deref_mut() {
            profile.batches += 1;
            profile.files += batch.files.len() as u64;
            profile.records += batch.records.len() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        let base = self.builder.next_slot()?.0;
        let count = u32::try_from(batch.records.len()).map_err(|_| "too many batch records")?;
        base.checked_add(count).ok_or("record slot overflow")?;
        let mut records = batch.records.into_iter();
        for batch_file in batch.files {
            let file_id = batch_file.entry.id;
            let path = self.inventory.root.join(&batch_file.entry.relative_path);
            if batch_file.first_range {
                self.builder.include_file(file_id)?;
            } else if !self.builder.contains_file(file_id) {
                return Err("later record range precedes first file range".into());
            }
            let mut diagnostic_refs = Vec::with_capacity(batch_file.entry.diagnostics.len());
            for mut diagnostic in batch_file.entry.diagnostics {
                if let Some(local_slot) = diagnostic.record {
                    diagnostic.record = Some(RecordSlot(
                        base.checked_add(local_slot.0)
                            .ok_or("diagnostic record slot overflow")?,
                    ));
                }
                diagnostic_refs.push(self.inventory.record_diagnostic(file_id, diagnostic)?);
            }
            let file = &mut self.inventory.files[file_id.0 as usize];
            file.status = file.status.max(batch_file.entry.status);
            file.omitted_diagnostics = file
                .omitted_diagnostics
                .saturating_add(batch_file.entry.omitted_diagnostics);
            for expected_slot in batch_file.records {
                let local = records.next().ok_or("batch record range out of bounds")?;
                if local.slot.0 as usize != expected_slot {
                    return Err("batch record order mismatch".into());
                }
                if local.file != file_id {
                    return Err("batch record file ownership mismatch".into());
                }
                let actual = if local.class == RecordClass::Logical {
                    self.builder
                        .add_record(file, local.id, local.range.start, local.range.end)
                        .map_err(|error| {
                            format!(
                                "{}:{}..{}: {error}",
                                path.display(),
                                local.range.start,
                                local.range.end,
                            )
                        })?
                } else {
                    let reference = match local.diagnostic_ref.stored_index() {
                        Some(index) => *diagnostic_refs
                            .get(index)
                            .ok_or("batch diagnostic reference out of bounds")?,
                        None => DiagnosticRef(u32::MAX),
                    };
                    self.builder
                        .add_raw_fallback(file, local.id, local.range, reference)?
                };
                if actual.0 != base + local.slot.0 {
                    return Err("batch record slot mapping mismatch".into());
                }
            }
        }
        if records.next().is_some() {
            return Err("unadmitted batch records".into());
        }
        if self.builder.next_slot()?.0 != base + count {
            return Err("incomplete batch record admission".into());
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.record_inventory_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        self.postings.merge_shifted(batch.postings, base)?;
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.postings_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        match (self.inference.as_mut(), batch.inference) {
            (Some(global), Some(local)) => global.merge_shifted(local, base)?,
            (None, None) => {}
            _ => return Err("inference batch mode mismatch".into()),
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.inference_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        if self.relations.len() != batch.relations.len() {
            return Err("relation batch coverage mismatch".into());
        }
        for (global, local) in self.relations.iter_mut().zip(batch.relations) {
            global.merge_shifted(local, base, count)?;
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.relations_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        if self.custom_postings.len() != batch.custom_postings.len() {
            return Err("custom posting batch coverage mismatch".into());
        }
        for (key, local) in batch.custom_postings {
            let global = self
                .custom_postings
                .get_mut(&key)
                .ok_or("custom posting batch missing")?;
            global.index.merge_shifted(local.index, base)?;
            global.complete &= local.complete;
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.custom_postings_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        self.raw.merge_shifted(batch.raw, base)?;
        self.fallback_text
            .merge_shifted(batch.fallback_text, base)?;
        if self.custom_text.len() != batch.custom_text.len() {
            return Err("custom bigram batch coverage mismatch".into());
        }
        for (key, index) in batch.custom_text {
            self.custom_text
                .get_mut(&key)
                .ok_or("custom bigram batch missing")?
                .merge_shifted(index, base)?;
        }
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.bigrams_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        append_shifted_bitmap(self.logical_records, &batch.logical_records, base)?;
        append_shifted_bitmap(self.empty_array_records, &batch.empty_array_records, base)?;
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.structural_facts_ns += started.elapsed().as_nanos() as u64;
        }
        let started = crate::detailed_timer::start_if_enabled(profile.is_some());
        self.sorting.append_keys(batch.sort_keys, base, count)?;
        if let (Some(started), Some(profile)) = (started, profile.as_deref_mut()) {
            profile.sorting_ns += started.elapsed().as_nanos() as u64;
        }
        Ok(())
    }
}


#[cfg(test)]
#[path = "build.test.rs"]
mod tests;

#[path = "mutation_segment.rs"]
mod mutation_segment;
pub(crate) use mutation_segment::PartitionRouting;

pub(crate) use mutation_segment::{
    CollectionSegment, IndexedCollectionSegment, SegmentAdmission, SegmentFile, SegmentLimits,
};
