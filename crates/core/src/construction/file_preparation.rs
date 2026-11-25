//! File, chunk and supplied-byte preparation for construction.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::mpsc;

use rayon::ThreadPool;
use sha2::{Digest, Sha256};

use crate::index::bigram::BigramAlphabet;
use crate::inference::InferenceConfig;
use crate::inventory::{ByteRange, RecordClass, SourceRevision};
use crate::model::{CollectionId, FileId, RecordId};
use crate::repository::configuration::CollectionDefinition;
use crate::schema::Schema;
use crate::source::{
    ReaderDiagnostic, ReaderFailure, SourceOutcome, extraction::extract_file_build_outcomes,
    extraction::extract_framed_build_outcomes, extraction::frame_file_for_build,
    framing::BoundedBuildChunk, framing::BoundedJsonLines, framing::validate_reader_ranges,
    registry::PreparedSource,
};

use super::batch::{BatchBuilder, BatchContribution, BatchFile, SourceFileInput, process_batch};
use super::scheduling::{BuildScheduleStats, JobCounters, fits_allowance};

pub(crate) fn finish_build_record(
    schema: &Schema,
    record: &mut crate::source::ExtractedRecord,
) -> Result<(), String> {
    schema.coerce_record(&mut record.value)?;
    record.value.validate_finite_numbers()?;
    Ok(())
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
    /// Object observed through core's open handle. It is checked again after
    /// parsing and before a mutation hands its candidate to publication.
    pub identity: Option<FileObjectIdentity>,
    pub collections: Vec<PreparedCollectionBuild>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct FileObjectIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

impl FileObjectIdentity {
    fn from_metadata(metadata: &std::fs::Metadata) -> Option<Self> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Some(Self {
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
            None
        }
    }
}

pub(crate) fn created_ns(metadata: &std::fs::Metadata) -> Option<i128> {
    metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128)
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

pub(crate) fn verify_current_source(
    root: &Path,
    source: &SourceFileInput,
    identity: Option<FileObjectIdentity>,
    fingerprint: Option<[u8; 32]>,
) -> Result<(), String> {
    let path = root.join(&source.relative_path);
    let canonical = path.canonicalize().map_err(|error| error.to_string())?;
    if !canonical.starts_with(root) {
        return Err("source path escapes catalog root".into());
    }
    let metadata = std::fs::metadata(&canonical).map_err(|error| error.to_string())?;
    if SourceRevision::from_metadata(&metadata)? != source.revision {
        return Err("stale source revision before publication".into());
    }
    if source.created_ns.is_some() && created_ns(&metadata) != source.created_ns {
        return Err("source creation time changed before publication".into());
    }
    if let Some(identity) = identity {
        if FileObjectIdentity::from_metadata(&metadata) != Some(identity) {
            return Err("source object changed before publication".into());
        }
    } else if let Some(expected) = fingerprint {
        let mut digest = Sha256::new();
        let mut file = std::fs::File::open(&canonical).map_err(|error| error.to_string())?;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut chunk).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            digest.update(&chunk[..count]);
        }
        if <[u8; 32]>::from(digest.finalize()) != expected {
            return Err("source content changed before publication".into());
        }
    } else {
        return Err("source identity cannot be verified".into());
    }
    Ok(())
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
    file: &mut std::fs::File,
) -> Result<(Vec<BatchContribution>, Option<[u8; 32]>), String> {
    let path = root.join(&input.relative_path);
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    if SourceRevision::from_metadata(&file.metadata().map_err(|error| error.to_string())?)?
        != input.revision
    {
        return Err("stale source revision during preparation".into());
    }
    let mut digest = Sha256::new();
    let mut bytes_read = 0u64;
    let reader = HashingReader {
        inner: &mut *file,
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

fn prepare_custom_stream(
    root: &Path,
    input: &SourceFileInput,
    binding: &FilePreparationBinding<'_>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
    hash_bytes: bool,
    file: &mut std::fs::File,
) -> Option<Result<(Vec<BatchContribution>, Option<[u8; 32]>), String>> {
    if let Err(error) = file.seek(SeekFrom::Start(0)) {
        return Some(Err(error.to_string()));
    }
    let mut digest = Sha256::new();
    let mut bytes_read = 0u64;
    let mut input_reader = HashingReader {
        inner: &mut *file,
        digest: hash_bytes.then_some(&mut digest),
        bytes_read: &mut bytes_read,
    };
    let limits = BoundedBuildLimits::default();
    let mut contributions = Vec::new();
    let mut next_offset = 0u64;
    let mut preceding_lines = 0u64;
    let mut emit = |chunk: BoundedBuildChunk| -> Result<(), String> {
        if chunk.file_offset != next_offset || chunk.preceding_lines < preceding_lines {
            return Err("reader contract: streamed chunks are not consecutive".into());
        }
        if chunk.bytes.len() > limits.chunk_bytes && chunk.ranges.len() != 1 {
            return Err("reader contract: oversized chunk must contain one record".into());
        }
        validate_reader_ranges(&chunk.ranges, chunk.bytes.len())?;
        next_offset = next_offset
            .checked_add(chunk.bytes.len() as u64)
            .ok_or("source offset overflow")?;
        if next_offset > input.revision.size {
            return Err("reader contract: chunk exceeds observed source".into());
        }
        preceding_lines = chunk.preceding_lines;
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
        Ok(())
    };
    let result = binding.source.reader().stream_build_chunks(
        &mut input_reader,
        limits.chunk_bytes,
        limits.ranges,
        &mut emit,
    )?;
    drop(emit);
    drop(input_reader);
    Some(result.and_then(|()| {
        if bytes_read != input.revision.size || next_offset != bytes_read {
            return Err("reader contract: streamed chunks do not cover observed source".into());
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
        Ok((contributions, hash_bytes.then(|| digest.finalize().into())))
    }))
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
    let mut opened = std::fs::File::open(&path).ok();
    let identity = if let Some(file) = opened.as_ref() {
        let metadata = file.metadata().map_err(|error| error.to_string())?;
        if SourceRevision::from_metadata(&metadata)? != input.revision {
            return Err("stale source revision during preparation".into());
        }
        if input.created_ns.is_some() && created_ns(&metadata) != input.created_ns {
            return Err("source creation time changed during preparation".into());
        }
        FileObjectIdentity::from_metadata(&metadata)
    } else if let Ok(metadata) = std::fs::metadata(&path) {
        if SourceRevision::from_metadata(&metadata)? != input.revision {
            return Err("stale source revision during preparation".into());
        }
        if input.created_ns.is_some() && created_ns(&metadata) != input.created_ns {
            return Err("source creation time changed during preparation".into());
        }
        FileObjectIdentity::from_metadata(&metadata)
    } else {
        None
    };
    let mut fingerprint = None;
    let mut collections = Vec::with_capacity(bindings.len());
    for binding in bindings {
        if let Ok(revision) = source_revision_at(&path) {
            if revision != input.revision {
                return Err("stale source revision during preparation".into());
            }
        }
        let contributions = if let Some(result) = opened.as_mut().and_then(|file| {
            prepare_custom_stream(
                root,
                &input,
                binding,
                alphabet,
                inference,
                inference_jobs,
                fingerprint.is_none(),
                file,
            )
        }) {
            let (contributions, digest) = result?;
            fingerprint = fingerprint.or(digest);
            contributions
        } else if binding.source.reader().supports_bounded_json_lines_build() && opened.is_some() {
            let (contributions, digest) = prepare_bounded_json_lines(
                root,
                &input,
                binding,
                alphabet,
                inference,
                inference_jobs,
                fingerprint.is_none(),
                opened.as_mut().expect("checked above"),
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
    if let Some(file) = opened.as_ref() {
        let handle_metadata = file.metadata().map_err(|error| error.to_string())?;
        let path_metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
        if SourceRevision::from_metadata(&handle_metadata)? != input.revision
            || SourceRevision::from_metadata(&path_metadata)? != input.revision
            || (input.created_ns.is_some() && created_ns(&handle_metadata) != input.created_ns)
            || (input.created_ns.is_some() && created_ns(&path_metadata) != input.created_ns)
            || FileObjectIdentity::from_metadata(&handle_metadata) != identity
            || FileObjectIdentity::from_metadata(&path_metadata) != identity
        {
            return Err("stale source object during preparation".into());
        }
    }
    Ok(PreparedFileBuild {
        source: input,
        fingerprint,
        identity,
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
        identity: None,
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
    let started = crate::measurement::timer::DetailedInstant::now();
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
    if source.reader().supports_streaming_build() {
        let input = source_files[file_id.0 as usize].clone();
        let prepared = prepare_file_build(
            &root,
            input.clone(),
            &[FilePreparationBinding {
                collection,
                definition: &definition,
                source: &source,
            }],
            alphabet,
            inference,
            1,
        )?;
        let count = prepared.collections[0].contributions.len();
        let mut stats = BuildScheduleStats {
            source_reads: u64::from(prepared.fingerprint.is_some()),
            source_bytes_read: input.revision.size,
            file_fingerprint: prepared.fingerprint.map(|digest| (file_id, digest)),
            framing_chunks: count as u64,
            framing_bytes: input.revision.size,
            weight_limit_bytes: BoundedBuildLimits::default().admitted_bytes,
            peak_weight_bytes: BoundedBuildLimits::default().chunk_bytes as u64,
            ..Default::default()
        };
        for collection_build in prepared.collections {
            for contribution in collection_build.contributions {
                admit(contribution)?;
            }
        }
        stats.peak_active_jobs = 1;
        return Ok(stats);
    }
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
        let wait_started = crate::measurement::timer::DetailedInstant::now();
        let received = receiver.recv();
        stats.receive_wait_ns = stats
            .receive_wait_ns
            .saturating_add(wait_started.elapsed().as_nanos() as u64);
        let (index, result) = received.map_err(|error| error.to_string())?;
        completed += 1;
        ready.insert(index, result);
        while let Some(result) = ready.remove(&next) {
            admitted_bytes -= chunks[next].1;
            let admission_started = crate::measurement::timer::DetailedInstant::now();
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

#[cfg(test)]
mod tests;

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
pub(crate) struct BoundedBuildLimits {
    pub(crate) chunk_bytes: usize,
    pub(crate) ranges: usize,
    pub(crate) admitted_bytes: u64,
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

pub(crate) fn process_bounded_json_lines_reader<R: BufRead + Send>(
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
    let first_started = crate::measurement::timer::start_if_enabled(profile_build);
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
                let started = crate::measurement::timer::start_if_enabled(profile_build);
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
        let wait_started = crate::measurement::timer::DetailedInstant::now();
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
            let started = crate::measurement::timer::DetailedInstant::now();
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
