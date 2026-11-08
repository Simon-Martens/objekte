//! Bounded file-batch contributions and deterministic index admission.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Cursor, Read};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::time::Instant;

use rayon::ThreadPool;
use roaring::RoaringBitmap;
use sha2::{Digest, Sha256};

use crate::bigram::{BigramBuilder, BigramContribution};
use crate::inference::{InferenceBuilder, InferenceConfig};
use crate::inventory::{
    ByteRange, CollectionBuilder, DiagnosticRef, FileEntry, FileInventory, FileStatus, RecordClass,
    RecordEntry, SourceDiagnostic, SourceRevision, append_shifted_bitmap,
};
use crate::model::{CollectionId, FieldPath, FileId, RecordId, RecordSlot, Value};
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
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
    ReaderDiagnostic, ReaderFailure, SourceOutcome, TextDerivation, extract_file_build_outcomes,
    extract_framed_build_outcomes, frame_file_for_build, read_record_range_for_build,
    visit_logical_strings_and_empty,
};
use crate::workload::BigramAlphabet;


struct BatchFile {
    entry: FileEntry,
    records: Range<usize>,
    first_range: bool,
}


/// Immutable discovery metadata shared by all construction workers. Mutable
/// statuses and diagnostics stay in the coordinator-owned inventory.
#[derive(Clone)]
pub(crate) struct SourceFileInput {
    pub(crate) id: FileId,
    pub(crate) relative_path: PathBuf,
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
            relative_path: self.relative_path.clone(),
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
    files: Vec<BatchFile>,
    records: Vec<RecordEntry>,
    postings: PostingCollection,
    inference: Option<InferenceBuilder>,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramContribution,
    fallback_text: BigramContribution,
    custom_text: HashMap<(Vec<String>, String), BigramContribution>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sort_keys: HashMap<FieldPath, Vec<SortKey>>,
    relations: Vec<RelationContribution>,
    pub record_parses: u64,
    pub failed_parses: u64,
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
        self.postings.add(slot, &extracted.value)?;
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
        self.sorting.add(
            slot,
            &extracted.value,
            self.records.last().unwrap(),
            file,
            self.root,
            &self.definition.name,
        )?;
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
    for &file_id in file_ids {
        let original = &source_files[file_id.0 as usize];
        let path = root.join(&original.relative_path);
        let mut file = original.for_collection(collection);
        let start = builder.records.len();
        let stats =
            extract_file_build_outcomes(&path, source.reader(), file.revision, |outcome| {
                builder.outcome(&mut file, &path, outcome, None)
            })?;
        record_parses += stats.record_parses;
        failed_parses += stats.failed_parses;
        files.push(BatchFile {
            entry: file,
            records: start..builder.records.len(),
            first_range: true,
        });
    }
    builder.finish(files, record_parses, failed_parses)
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
    let started = Instant::now();
    admit(batch)?;
    Ok(BuildScheduleStats {
        weight_limit_bytes: 64 * 1024 * 1024,
        peak_weight_bytes: source_files[file_id.0 as usize].revision.size,
        admission_ns: started.elapsed().as_nanos() as u64,
        ..Default::default()
    })
}

/// Built-in JSONL uses bounded build chunks. Other readers retain the
/// whole-buffer framing route, sharing one source buffer across range jobs.
/// Caller schedules oversized files one at a time.
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
    let source_bytes = bytes.len() as u64;
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
        let wait_started = Instant::now();
        let received = receiver.recv();
        stats.receive_wait_ns = stats
            .receive_wait_ns
            .saturating_add(wait_started.elapsed().as_nanos() as u64);
        let (index, result) = received.map_err(|error| error.to_string())?;
        completed += 1;
        ready.insert(index, result);
        while let Some(result) = ready.remove(&next) {
            admitted_bytes -= chunks[next].1;
            let admission_started = Instant::now();
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
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let max_jobs = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    let path = root.join(&source_files[file_id.0 as usize].relative_path);
    let reader = match BoundedJsonLines::open(&path) {
        Ok(reader) => reader,
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
    process_bounded_json_lines_reader(
        pool,
        root,
        definition,
        collection,
        source,
        file_id,
        source_files,
        alphabet,
        inference,
        reader,
        BoundedBuildLimits::default(),
        admit,
    )
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
    mut reader: BoundedJsonLines<R>,
    limits: BoundedBuildLimits,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let max_jobs = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    let path = root.join(&source_files[file_id.0 as usize].relative_path);
    // A failure before any contribution can still use the ordinary unreadable
    // outcome. Once a chunk is admitted, an I/O failure aborts this private
    // generation instead of publishing a partially indexed file.
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
                match reader.next_chunk(limits.chunk_bytes, limits.ranges) {
                    Ok(Some(chunk)) => pending = Some(chunk),
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
        let wait_started = Instant::now();
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
            let started = Instant::now();
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
    pub weight_limit_bytes: u64,
    pub peak_weight_bytes: u64,
    pub peak_active_jobs: usize,
    pub peak_ready_batches: usize,
    pub job_execution_ns: u64,
    pub receive_wait_ns: u64,
    pub admission_ns: u64,
}


impl BuildScheduleStats {
    pub fn absorb(&mut self, other: Self) {
        self.weight_limit_bytes = self.weight_limit_bytes.max(other.weight_limit_bytes);
        self.peak_weight_bytes = self.peak_weight_bytes.max(other.peak_weight_bytes);
        self.peak_active_jobs = self.peak_active_jobs.max(other.peak_active_jobs);
        self.peak_ready_batches = self.peak_ready_batches.max(other.peak_ready_batches);
        self.job_execution_ns = self.job_execution_ns.saturating_add(other.job_execution_ns);
        self.receive_wait_ns = self.receive_wait_ns.saturating_add(other.receive_wait_ns);
        self.admission_ns = self.admission_ns.saturating_add(other.admission_ns);
    }
}


#[derive(Default)]
struct JobCounters {
    active: AtomicUsize,
    peak_active: AtomicUsize,
    completed: AtomicUsize,
    admitted: AtomicUsize,
    peak_ready: AtomicUsize,
    execution_ns: AtomicU64,
}


impl JobCounters {
    fn run<T>(&self, job: impl FnOnce() -> T) -> T {
        let active = self.active.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_active.fetch_max(active, Ordering::Relaxed);
        let started = Instant::now();
        let result = job();
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
        stats.job_execution_ns = self.execution_ns.load(Ordering::Relaxed);
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


pub(crate) fn process_repository_file_batches(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    jobs: &[RepositoryFileJob],
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    mut admit: impl FnMut(usize, BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    const MIN_FILE_BYTES: u64 = 16 * 1024;
    let allowance = normal_batch_allowance(pool.current_num_threads());
    let weights = jobs
        .iter()
        .map(|job| {
            job.files.iter().fold(0u64, |total, id| {
                let file = &source_files[id.0 as usize];
                total.saturating_add(file.revision.size.max(MIN_FILE_BYTES))
            })
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
    while next < jobs.len() && failure.is_none() {
        while dispatched < jobs.len() && dispatched - next < max_jobs {
            let index = order[dispatched];
            let weight = weights[index];
            if !fits_allowance(admitted_bytes, weight, allowance) {
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
            pool.spawn(move || {
                let result = counters.run(|| {
                    process_batch(
                        &root,
                        &definition,
                        collection,
                        &source,
                        &file_ids,
                        &source_files,
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
        let wait_started = Instant::now();
        let received = receiver.recv();
        stats.receive_wait_ns = stats
            .receive_wait_ns
            .saturating_add(wait_started.elapsed().as_nanos() as u64);
        let (index, result) = received.map_err(|error| error.to_string())?;
        completed += 1;
        ready.insert(index, result);
        while let Some(result) = ready.remove(&next) {
            admitted_bytes -= weights[next];
            let admission_started = Instant::now();
            let outcome = result.and_then(|batch| admit(jobs[next].collection_index, batch));
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


impl CollectionAdmission<'_> {
    pub fn admit(&mut self, batch: BatchContribution) -> Result<(), String> {
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
        self.postings.merge_shifted(batch.postings, base)?;
        match (self.inference.as_mut(), batch.inference) {
            (Some(global), Some(local)) => global.merge_shifted(local, base)?,
            (None, None) => {}
            _ => return Err("inference batch mode mismatch".into()),
        }
        if self.relations.len() != batch.relations.len() {
            return Err("relation batch coverage mismatch".into());
        }
        for (global, local) in self.relations.iter_mut().zip(batch.relations) {
            global.merge_shifted(local, base, count)?;
        }
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
        append_shifted_bitmap(self.logical_records, &batch.logical_records, base)?;
        append_shifted_bitmap(self.empty_array_records, &batch.empty_array_records, base)?;
        self.sorting.append_keys(batch.sort_keys, base, count)?;
        Ok(())
    }
}


#[cfg(test)]
#[path = "build.test.rs"]
mod tests;

#[path = "mutation_segment.rs"]
mod mutation_segment;

pub(crate) use mutation_segment::{
    CollectionSegment, CompactedSegment, ConsolidatedSegment, IndexedCollectionSegment,
    SegmentAdmission, SegmentFile, SegmentLimits,
};
