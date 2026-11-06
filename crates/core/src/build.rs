//! Bounded file-batch contributions and deterministic index admission.

use std::collections::{BTreeMap, HashMap};
use std::num::NonZeroUsize;
use std::ops::Range;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::mpsc;

use rayon::ThreadPool;
use roaring::RoaringBitmap;

use crate::bigram::{BigramBuilder, BigramIndex};
use crate::inventory::{
    ByteRange, CollectionBuilder, DiagnosticRef, FileEntry, FileInventory, FileStatus, RecordClass,
    RecordEntry, SourceDiagnostic, SourceRevision, append_shifted_bitmap,
};
use crate::model::{CollectionId, FieldPath, FileId, RecordId, RecordSlot, Value};
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::record_preparation::finish_build_record;
use crate::reference::data_values;
use crate::repository::{
    CollectionDefinition, IdentityPolicy, canonical_custom_text_key, map_binding_value,
    string_values,
};
use crate::sorting::{SortBuilder, SortKey};
use crate::source::{
    DiagnosticCategory, ExtractedRecord, PreparedSource, ReaderDiagnostic, SourceOutcome,
    extract_file_outcomes, extract_framed_outcomes, frame_file_for_build, read_record_range,
};
use crate::workload::BigramAlphabet;

struct BatchFile {
    entry: FileEntry,
    records: Range<usize>,
    first_range: bool,
}

/// Immutable discovery metadata shared by all construction workers. Mutable
/// statuses and diagnostics stay in the coordinator-owned inventory.
pub(crate) struct SourceFileInput {
    id: FileId,
    relative_path: PathBuf,
    revision: SourceRevision,
    created_ns: Option<i128>,
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
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramIndex,
    fallback_text: BigramIndex,
    custom_text: HashMap<(Vec<String>, String), BigramIndex>,
    logical_records: RoaringBitmap,
    sort_keys: HashMap<FieldPath, Vec<SortKey>>,
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
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramBuilder,
    fallback_text: BigramBuilder,
    custom_text: HashMap<(Vec<String>, String), BigramBuilder>,
    logical_records: RoaringBitmap,
    sorting: SortBuilder,
}

impl<'a> BatchBuilder<'a> {
    fn new(
        root: &'a Path,
        definition: &'a CollectionDefinition,
        collection: CollectionId,
        source: &'a PreparedSource,
        alphabet: BigramAlphabet,
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
            custom_postings,
            raw: BigramBuilder::new(alphabet),
            fallback_text: BigramBuilder::new(alphabet),
            custom_text,
            logical_records: RoaringBitmap::new(),
            sorting: SortBuilder::new(
                &definition.sort_fields,
                schema,
                &definition.sort_locale,
                definition.sort_options,
                &definition.sort_overrides,
            )?,
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
        if let Err(error) =
            finish_build_record(self.source, &self.definition.schema, &mut extracted)
        {
            let issue = ReaderDiagnostic::new(
                DiagnosticCategory::LogicalValidation,
                "logical.validation",
                error,
            );
            return if let Some(bytes) = source_bytes {
                self.fallback(file, range, bytes, issue)
            } else {
                let bytes = read_record_range(path, revision, range)?;
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
        self.fallback_text.add_empty(slot)?;
        self.logical_records.insert(slot.0);
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
            .map(|(key, index)| index.finish().map(|index| (key, index)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(BatchContribution {
            files,
            records: self.records,
            postings: self.postings,
            custom_postings: self.custom_postings,
            raw: self.raw.finish()?,
            fallback_text: self.fallback_text.finish()?,
            custom_text,
            logical_records: self.logical_records,
            sort_keys: self.sorting.into_keys(),
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
) -> Result<BatchContribution, String> {
    let mut builder = BatchBuilder::new(root, definition, collection, source, alphabet)?;
    let mut files = Vec::with_capacity(file_ids.len());
    let (mut record_parses, mut failed_parses) = (0, 0);
    for &file_id in file_ids {
        let original = &source_files[file_id.0 as usize];
        let path = root.join(&original.relative_path);
        let mut file = original.for_collection(collection);
        let start = builder.records.len();
        let stats = extract_file_outcomes(&path, source.reader(), file.revision, |outcome| {
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

fn process_framed_batch(
    root: &Path,
    definition: &CollectionDefinition,
    collection: CollectionId,
    source: &PreparedSource,
    file_id: FileId,
    source_files: &[SourceFileInput],
    bytes: &[u8],
    ranges: &[ByteRange],
    first_range: bool,
    alphabet: BigramAlphabet,
) -> Result<BatchContribution, String> {
    let original = &source_files[file_id.0 as usize];
    let path = root.join(&original.relative_path);
    let mut file = original.for_collection(collection);
    let mut builder = BatchBuilder::new(root, definition, collection, source, alphabet)?;
    let stats = extract_framed_outcomes(&path, source.reader(), bytes, ranges, |outcome| {
        let source_part = match &outcome {
            SourceOutcome::Logical { range, .. } => {
                Some(&bytes[range.start as usize..range.end as usize])
            }
            _ => None,
        };
        builder.outcome(&mut file, &path, outcome, source_part)
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

/// One oversized source buffer is shared by disjoint range jobs. Caller
/// schedules oversized files one at a time; no worker copies or reopens the
/// complete source. A framing failure follows the ordinary source outcome
/// path so unreadable/malformed file semantics stay unchanged.
pub(crate) fn process_framed_file(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    definition: Arc<CollectionDefinition>,
    collection: CollectionId,
    source: PreparedSource,
    file_id: FileId,
    source_files: Arc<Vec<SourceFileInput>>,
    alphabet: BigramAlphabet,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<u64, String> {
    const MAX_RANGE_BYTES: u64 = 1024 * 1024;
    const MAX_RANGES: usize = 256;
    const MAX_ADMITTED_BYTES: u64 = 64 * 1024 * 1024;
    let input = &source_files[file_id.0 as usize];
    let path = root.join(&input.relative_path);
    let Some((bytes, ranges)) = frame_file_for_build(&path, source.reader(), input.revision)?
    else {
        let batch = process_batch(
            &root,
            &definition,
            collection,
            &source,
            &[file_id],
            &source_files,
            alphabet,
        )?;
        admit(batch)?;
        return Ok(input.revision.size);
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
    let max_jobs = pool.current_num_threads().saturating_mul(2).max(1);
    while next < chunks.len() && failure.is_none() {
        while dispatched < chunks.len() && dispatched - next < max_jobs {
            let weight = chunks[dispatched].1;
            if admitted_bytes > 0
                && (weight > MAX_ADMITTED_BYTES
                    || admitted_bytes.saturating_add(weight) > MAX_ADMITTED_BYTES)
            {
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
            pool.spawn(move || {
                let result = process_framed_batch(
                    &root,
                    &definition,
                    collection,
                    &source,
                    file_id,
                    &source_files,
                    &bytes,
                    &ranges[chunk],
                    index == 0,
                    alphabet,
                );
                let _ = sender.send((index, result));
            });
            admitted_bytes = admitted_bytes.saturating_add(weight);
            peak_bytes = peak_bytes.max(admitted_bytes);
            dispatched += 1;
        }
        let (index, result) = receiver.recv().map_err(|error| error.to_string())?;
        completed += 1;
        ready.insert(index, result);
        while let Some(result) = ready.remove(&next) {
            admitted_bytes -= chunks[next].1;
            match result.and_then(&mut admit) {
                Ok(()) => next += 1,
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
    if SourceRevision::from_metadata(
        &std::fs::metadata(&path).map_err(|error| format!("{}: {error}", path.display()))?,
    )? != input.revision
    {
        return Err("stale source revision after framed extraction".into());
    }
    Ok(source_bytes.saturating_add(peak_bytes))
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
    mut admit: impl FnMut(usize, BatchContribution) -> Result<(), String>,
) -> Result<u64, String> {
    const MAX_ADMITTED_BYTES: u64 = 52 * 1024 * 1024;
    const MIN_FILE_BYTES: u64 = 16 * 1024;
    let weights = jobs
        .iter()
        .map(|job| {
            job.files.iter().fold(0u64, |total, id| {
                let file = &source_files[id.0 as usize];
                total.saturating_add(file.revision.size.max(MIN_FILE_BYTES))
            })
        })
        .collect::<Vec<_>>();
    let max_jobs = pool.current_num_threads().saturating_mul(2).max(1);
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
        if prefetch_bytes.saturating_add(weight) > MAX_ADMITTED_BYTES / 4
            || prefetch_bytes
                .saturating_add(weight)
                .saturating_add(weights[0])
                > MAX_ADMITTED_BYTES
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
    while next < jobs.len() && failure.is_none() {
        while dispatched < jobs.len() && dispatched - next < max_jobs {
            let index = order[dispatched];
            let weight = weights[index];
            if admitted_bytes > 0
                && (weight > MAX_ADMITTED_BYTES
                    || admitted_bytes.saturating_add(weight) > MAX_ADMITTED_BYTES)
            {
                break;
            }
            let file_ids = jobs[index].files.clone();
            let sender = sender.clone();
            let root = Arc::clone(&root);
            let definition = Arc::clone(&jobs[index].definition);
            let source = jobs[index].source.clone();
            let collection = jobs[index].collection;
            let source_files = Arc::clone(&source_files);
            pool.spawn(move || {
                let result = process_batch(
                    &root,
                    &definition,
                    collection,
                    &source,
                    &file_ids,
                    &source_files,
                    alphabet,
                );
                let _ = sender.send((index, result));
            });
            admitted_bytes = admitted_bytes.saturating_add(weight);
            peak_bytes = peak_bytes.max(admitted_bytes);
            dispatched += 1;
        }
        let (index, result) = receiver.recv().map_err(|error| error.to_string())?;
        completed += 1;
        ready.insert(index, result);
        while let Some(result) = ready.remove(&next) {
            admitted_bytes -= weights[next];
            match result.and_then(|batch| admit(jobs[next].collection_index, batch)) {
                Ok(()) => next += 1,
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
    Ok(peak_bytes)
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
) -> Result<u64, String> {
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
    process_repository_file_batches(pool, root, &jobs, source_files, alphabet, |_, batch| {
        admit(batch)
    })
}

pub(crate) struct CollectionAdmission<'a> {
    pub inventory: &'a mut FileInventory,
    pub builder: &'a mut CollectionBuilder,
    pub postings: &'a mut PostingCollection,
    pub custom_postings: &'a mut HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub raw: &'a mut BigramIndex,
    pub fallback_text: &'a mut BigramIndex,
    pub custom_text: &'a mut HashMap<(Vec<String>, String), BigramIndex>,
    pub logical_records: &'a mut RoaringBitmap,
    pub sorting: &'a mut SortBuilder,
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
        self.sorting.append_keys(batch.sort_keys, base, count)?;
        Ok(())
    }
}

#[cfg(test)]
#[path = "build.test.rs"]
mod tests;
