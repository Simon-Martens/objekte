//! File, chunk and supplied-byte preparation for construction.

use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Cursor, Read, Seek, SeekFrom};
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};

use rayon::ThreadPool;
use sha2::{Digest, Sha256};

use super::segment_admission::SegmentAdmission;
use crate::index::bigram::BigramAlphabet;
use crate::index::segment::{IndexedCollectionSegment, SegmentId};
use crate::inference::InferenceConfig;
use crate::inventory::{ByteRange, RecordClass, SourceRevision};
use crate::model::{CollectionId, FileId, RecordId};
use crate::repository::Repository;
use crate::repository::configuration::CollectionDefinition;
use crate::repository::statistics::StreamMemoryStats;
use crate::schema::Schema;
use crate::source::{
    ReaderDiagnostic, ReaderFailure, SourceOutcome, extraction::extract_framed_build_outcomes,
    framing::BoundedBuildChunk, framing::BoundedJsonLines, framing::validate_reader_ranges,
    registry::PreparedSource,
};

use super::batch::{BatchBuilder, BatchContribution, BatchFile, SourceFileInput};
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
    pub indexed: Vec<IndexedCollectionSegment>,
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
    _stream_memory: Option<StreamUse>,
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
            .map(|collection| {
                collection
                    .contributions
                    .iter()
                    .map(|batch| batch.record_count() as u64)
                    .sum::<u64>()
                    + collection
                        .indexed
                        .iter()
                        .map(|index| index.record_count() as u64)
                        .sum::<u64>()
            })
            .sum()
    }

    /// Record identity and framing reconstructed from one complete source.
    /// Verified removal compares this with active indexed ownership before
    /// using the old-byte hint for physical maintenance.
    #[cfg(test)]
    pub(crate) fn record_layout(&self) -> Vec<(CollectionId, RecordId, ByteRange, RecordClass)> {
        self.collections
            .iter()
            .flat_map(|collection| {
                collection
                    .contributions
                    .iter()
                    .flat_map(|contribution| {
                        contribution.records.iter().map(|record| {
                            (
                                collection.collection,
                                record.id.clone(),
                                record.range,
                                record.class,
                            )
                        })
                    })
                    .chain(collection.indexed.iter().flat_map(|index| {
                        index.records.iter().filter_map(|header| {
                            let record = index.file_record(header.slot)?;
                            Some((collection.collection, record.id, record.range, record.class))
                        })
                    }))
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
        crate::source::access::check_revision(&canonical, &file, source.revision)?;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let count = file.read(&mut chunk).map_err(|error| error.to_string())?;
            if count == 0 {
                break;
            }
            digest.update(&chunk[..count]);
        }
        crate::source::access::check_revision(&canonical, &file, source.revision)?;
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
    bytes_read: &'a AtomicU64,
    expected_bytes: Option<&'a [u8]>,
}

impl<R: Read> Read for HashingReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        let count = self.inner.read(bytes)?;
        if let Some(expected) = self.expected_bytes {
            let start = usize::try_from(self.bytes_read.load(Ordering::Relaxed))
                .map_err(std::io::Error::other)?;
            if expected.get(start..start.saturating_add(count)) != Some(&bytes[..count]) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "supplied bytes differ from core-observed file",
                ));
            }
        }
        if let Some(digest) = self.digest.as_mut() {
            digest.update(&bytes[..count]);
        }
        self.bytes_read.fetch_add(count as u64, Ordering::Relaxed);
        Ok(count)
    }
}

fn source_revision_at(path: &Path) -> Result<SourceRevision, String> {
    SourceRevision::from_metadata(
        &std::fs::metadata(path).map_err(|error| format!("{}: {error}", path.display()))?,
    )
}

fn prepare_unreadable_file(
    root: &Path,
    input: &SourceFileInput,
    binding: &FilePreparationBinding<'_>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
    code: &str,
    error: &std::io::Error,
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
    builder.outcome(
        &mut file,
        &path,
        SourceOutcome::Unreadable {
            diagnostic: ReaderDiagnostic::new(
                crate::source::DiagnosticCategory::Io,
                code,
                error.to_string(),
            ),
        },
        None,
    )?;
    let count = builder.records.len();
    Ok(vec![builder.finish(
        vec![BatchFile {
            entry: file,
            records: 0..count,
            first_range: true,
        }],
        0,
        0,
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
    expected_bytes: Option<&[u8]>,
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
    let bytes_read = AtomicU64::new(0);
    let reader = HashingReader {
        inner: &mut *file,
        digest: hash_bytes.then_some(&mut digest),
        bytes_read: &bytes_read,
        expected_bytes,
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
        || bytes_read.load(Ordering::Relaxed) != input.revision.size
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
    expected_bytes: Option<&[u8]>,
) -> Option<Result<(Vec<BatchContribution>, Option<[u8; 32]>), String>> {
    if let Err(error) = file.seek(SeekFrom::Start(0)) {
        return Some(Err(error.to_string()));
    }
    let mut digest = Sha256::new();
    let bytes_read = AtomicU64::new(0);
    let mut input_reader = HashingReader {
        inner: &mut *file,
        digest: hash_bytes.then_some(&mut digest),
        bytes_read: &bytes_read,
        expected_bytes,
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
        if chunk.ranges.len() > limits.ranges {
            return Err("reader contract: too many ranges in a streamed chunk".into());
        }
        if next_offset > bytes_read.load(Ordering::Relaxed) || next_offset > input.revision.size {
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
        if bytes_read.load(Ordering::Relaxed) != input.revision.size
            || next_offset != input.revision.size
        {
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

/// Shared admission across every stream in a repository or initial build.
/// One framer may hold scratch and one pending input in addition to its
/// decoder reservations. An extension still controls its callback allocation.
#[derive(Clone, Default)]
pub(crate) struct StreamBudget(Arc<StreamBudgetInner>);

#[derive(Default)]
struct StreamBudgetInner {
    usage: Mutex<StreamUsage>,
    available: Condvar,
}

#[derive(Default)]
struct StreamUsage {
    jobs: usize,
    current: StreamMemoryStats,
    peak: StreamMemoryStats,
    frame_limit: Option<usize>,
    byte_limit: Option<u64>,
}

impl StreamUsage {
    fn observe(&mut self) {
        self.current.pending_and_admitted_bytes =
            self.current.pending_input_bytes + self.current.admitted_decoder_bytes;
        self.peak.absorb(self.current);
    }
}

struct StreamCredit {
    budget: StreamBudget,
    bytes: u64,
}

#[derive(Clone, Copy)]
pub(super) enum StreamActivity {
    Framer,
    Callback,
    PendingInput,
    Decoder,
    Completed,
    PrivateSegments,
}

pub(super) struct StreamUse {
    budget: StreamBudget,
    activity: StreamActivity,
    count: u64,
    bytes: u64,
}

impl StreamBudget {
    pub(crate) fn peak(&self) -> StreamMemoryStats {
        self.0
            .usage
            .lock()
            .expect("stream budget mutex poisoned")
            .peak
    }

    #[cfg(test)]
    pub(crate) fn current(&self) -> StreamMemoryStats {
        self.0
            .usage
            .lock()
            .expect("stream budget mutex poisoned")
            .current
    }

    #[cfg(test)]
    pub(crate) fn set_test_limits(&self, framers: usize, bytes: u64) {
        self.update(|usage| {
            assert!(framers > 0 && bytes > 0);
            usage.frame_limit = Some(framers);
            usage.byte_limit = Some(bytes);
        });
    }

    fn update(&self, operation: impl FnOnce(&mut StreamUsage)) {
        let mut usage = self.0.usage.lock().expect("stream budget mutex poisoned");
        operation(&mut usage);
        usage.observe();
    }

    fn enter_framing(&self, pool: &ThreadPool) -> StreamUse {
        loop {
            let mut usage = self.0.usage.lock().expect("stream budget mutex poisoned");
            let limit = usage
                .frame_limit
                .unwrap_or_else(|| pool.current_num_threads().min(4));
            if usage.current.active_framers < limit as u64 {
                usage.current.active_framers += 1;
                usage.observe();
                return StreamUse {
                    budget: self.clone(),
                    activity: StreamActivity::Framer,
                    count: 1,
                    bytes: 0,
                };
            }
            if pool.current_thread_index().is_some() {
                drop(usage);
                pool.yield_now();
                let usage = self.0.usage.lock().expect("stream budget mutex poisoned");
                if usage.current.active_framers >= limit as u64 {
                    drop(
                        self.0
                            .available
                            .wait_timeout(usage, std::time::Duration::from_millis(2))
                            .expect("stream budget mutex poisoned"),
                    );
                }
            } else {
                drop(
                    self.0
                        .available
                        .wait(usage)
                        .expect("stream budget mutex poisoned"),
                );
            }
        }
    }

    pub(super) fn activity(&self, activity: StreamActivity, count: u64, bytes: u64) -> StreamUse {
        let mut ticket = StreamUse {
            budget: self.clone(),
            activity,
            count: 0,
            bytes: 0,
        };
        ticket.set(count, bytes);
        ticket
    }

    fn try_reserve(
        &self,
        bytes: u64,
        jobs: usize,
        limit: u64,
        pending: &mut Option<StreamUse>,
    ) -> Option<StreamCredit> {
        let mut usage = self.0.usage.lock().expect("stream budget mutex poisoned");
        let limit = limit.min(usage.byte_limit.unwrap_or(limit));
        if usage.jobs >= jobs || !fits_allowance(usage.current.admitted_decoder_bytes, bytes, limit)
        {
            return None;
        }
        if let Some(pending) = pending.as_mut() {
            usage.current.pending_input_bytes -= pending.bytes;
            pending.bytes = 0;
        }
        usage.jobs += 1;
        usage.current.admitted_decoder_bytes += bytes;
        usage.observe();
        Some(StreamCredit {
            budget: self.clone(),
            bytes,
        })
    }

    fn reserve(
        &self,
        bytes: u64,
        jobs: usize,
        limit: u64,
        pool: &ThreadPool,
        pending: &mut Option<StreamUse>,
    ) -> StreamCredit {
        loop {
            if let Some(credit) = self.try_reserve(bytes, jobs, limit, pending) {
                return credit;
            }
            if pool.current_thread_index().is_some() {
                pool.yield_now();
            }
            let usage = self.0.usage.lock().expect("stream budget mutex poisoned");
            let limit = limit.min(usage.byte_limit.unwrap_or(limit));
            if usage.jobs >= jobs
                || !fits_allowance(usage.current.admitted_decoder_bytes, bytes, limit)
            {
                drop(
                    self.0
                        .available
                        .wait_timeout(usage, std::time::Duration::from_millis(2))
                        .expect("stream budget mutex poisoned"),
                );
            }
        }
    }
}

impl StreamUse {
    pub(super) fn add_private_segment(&mut self, segment: &IndexedCollectionSegment) {
        self.set(
            self.count + 1,
            self.bytes + segment.accounting.index_family_bytes as u64,
        );
    }

    pub(super) fn set(&mut self, count: u64, bytes: u64) {
        self.budget.update(|usage| {
            let current = &mut usage.current;
            let apply = |value: &mut u64| *value = *value - self.count + count;
            match self.activity {
                StreamActivity::Framer => apply(&mut current.active_framers),
                StreamActivity::Callback => apply(&mut current.active_callbacks),
                StreamActivity::PendingInput => {
                    current.pending_input_bytes = current.pending_input_bytes - self.bytes + bytes
                }
                StreamActivity::Decoder => apply(&mut current.active_decoders),
                StreamActivity::Completed => apply(&mut current.completed_chunks),
                StreamActivity::PrivateSegments => {
                    apply(&mut current.private_segments);
                    current.private_index_family_bytes =
                        current.private_index_family_bytes - self.bytes + bytes;
                }
            }
        });
        if matches!(self.activity, StreamActivity::Framer) && count < self.count {
            self.budget.0.available.notify_all();
        }
        self.count = count;
        self.bytes = bytes;
    }
}

impl Drop for StreamUse {
    fn drop(&mut self) {
        if self.count != 0 || self.bytes != 0 {
            self.set(0, 0);
        }
    }
}

impl Drop for StreamCredit {
    fn drop(&mut self) {
        self.budget.update(|usage| {
            usage.jobs -= 1;
            usage.current.admitted_decoder_bytes -= self.bytes;
        });
        self.budget.0.available.notify_all();
    }
}

fn chunk_capacity(chunk: &BoundedBuildChunk) -> u64 {
    chunk.bytes.capacity() as u64
        + (chunk.ranges.capacity() * std::mem::size_of::<ByteRange>()) as u64
}

/// Process a registered stream through an ordered, bounded window of owned
/// chunks. The callback contributes to private segment admission as it runs.
fn process_custom_stream_profiled(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    definition: Arc<CollectionDefinition>,
    collection: CollectionId,
    source: PreparedSource,
    input: SourceFileInput,
    file: &mut std::fs::File,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    expected_bytes: Option<&[u8]>,
    hash: bool,
    limits: BoundedBuildLimits,
    record_target: usize,
    shared_budget: Option<&StreamBudget>,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<(BuildScheduleStats, Option<[u8; 32]>), String> {
    file.seek(SeekFrom::Start(0))
        .map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    let bytes_read = AtomicU64::new(0);
    let mut reader = HashingReader {
        inner: file,
        digest: hash.then_some(&mut digest),
        bytes_read: &bytes_read,
        expected_bytes,
    };
    let input = Arc::new(input);
    let inline = pool.current_num_threads() == 1 || pool.current_thread_index().is_some();
    let window = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    let counters = Arc::new(JobCounters::default());
    type ChunkResult = (
        Result<(BatchContribution, Option<BatchContribution>), String>,
        Option<StreamUse>,
    );
    type Waiting = (u64, mpsc::Receiver<ChunkResult>, Option<StreamCredit>);
    let mut waiting = VecDeque::<Waiting>::new();
    let mut stats = BuildScheduleStats {
        weight_limit_bytes: limits.admitted_bytes,
        source_reads: 1,
        ..Default::default()
    };
    let mut buffered = 0u64;
    let mut next_offset = 0u64;
    let mut previous_lines = 0;
    let mut emitted = 0usize;
    let mut ordinal = 0usize;
    let mut remaining = record_target;

    let mut drain = |pending: Waiting, ready: Option<ChunkResult>| {
        let started = crate::measurement::timer::DetailedInstant::now();
        let result = match ready {
            Some(result) => result,
            None => pending
                .1
                .recv()
                .map_err(|error| format!("stream decoder stopped: {error}"))?,
        };
        let wait = started.elapsed().as_nanos() as u64;
        let (result, _completed) = result;
        let (part, continuation) = result?;
        admit(part)?;
        if let Some(continuation) = continuation {
            admit(continuation)?;
        }
        counters.admitted.fetch_add(1, Ordering::Relaxed);
        Ok::<_, String>(wait)
    };
    let mut schedule = |chunk: BoundedBuildChunk| -> Result<(), String> {
        let weight = chunk_capacity(&chunk);
        let mut pending_input =
            shared_budget.map(|budget| budget.activity(StreamActivity::PendingInput, 0, weight));
        if weight > 128 * 1024 * 1024 {
            return Err("registered build chunk exceeds 128 MiB of owned input capacity".into());
        }
        while waiting.len() >= window || !fits_allowance(buffered, weight, limits.admitted_bytes) {
            let pending = waiting.pop_front().expect("a pending job owns credit");
            buffered -= pending.0;
            stats.receive_wait_ns += drain(pending, None)?;
        }
        let credit = if let Some(shared) = shared_budget {
            Some(loop {
                if let Some(credit) =
                    shared.try_reserve(weight, window, limits.admitted_bytes, &mut pending_input)
                {
                    break credit;
                }
                if let Some(pending) = waiting.pop_front() {
                    buffered -= pending.0;
                    stats.receive_wait_ns += drain(pending, None)?;
                } else {
                    break shared.reserve(
                        weight,
                        window,
                        limits.admitted_bytes,
                        pool,
                        &mut pending_input,
                    );
                }
            })
        } else {
            None
        };
        stats.peak_weight_bytes = stats.peak_weight_bytes.max(buffered.saturating_add(weight));
        let worker_budget = shared_budget.cloned();
        let (send, receive) = mpsc::sync_channel(1);
        let root = Arc::clone(&root);
        let definition = Arc::clone(&definition);
        let input = Arc::clone(&input);
        let source = source.clone();
        let counters = Arc::clone(&counters);
        let index = ordinal;
        ordinal += 1;
        let split = remaining.min(chunk.ranges.len());
        remaining -= split;
        if remaining == 0 {
            remaining = record_target - (chunk.ranges.len() - split);
        }
        let execute = move || {
            let active = worker_budget
                .as_ref()
                .map(|budget| budget.activity(StreamActivity::Decoder, 1, 0));
            let result = counters.run(|| {
                let binding = FilePreparationBinding {
                    collection,
                    definition: &definition,
                    source: &source,
                };
                let process = |ranges, first| {
                    process_framed_batch(
                        &root,
                        &input,
                        &binding,
                        &chunk.bytes,
                        ranges,
                        chunk.file_offset,
                        chunk.preceding_lines,
                        first,
                        alphabet,
                        inference,
                        window,
                    )
                };
                // A reader chunk can straddle a segment boundary. Both
                // parts borrow the same buffer and decode disjoint ranges.
                let part = process(&chunk.ranges[..split], index == 0)?;
                let continuation = if split < chunk.ranges.len() {
                    Some(process(&chunk.ranges[split..], false)?)
                } else {
                    None
                };
                Ok((part, continuation))
            });
            drop(chunk);
            drop(active);
            let completed = worker_budget
                .as_ref()
                .map(|budget| budget.activity(StreamActivity::Completed, 1, 0));
            let _ = send.send((result, completed));
        };
        if inline {
            if pool.current_thread_index().is_some() {
                execute();
            } else {
                pool.install(execute);
            }
            stats.receive_wait_ns += drain((weight, receive, credit), None)?;
        } else {
            buffered += weight;
            waiting.push_back((weight, receive, credit));
            pool.spawn(execute);
            // Feed completed contributions before allowing the reader to
            // advance, without waiting for a younger ordinal.
            while let Some((_, first, _)) = waiting.front() {
                match first.try_recv() {
                    Ok(result) => {
                        let pending = waiting.pop_front().expect("front is ready");
                        buffered -= pending.0;
                        stats.receive_wait_ns += drain(pending, Some(result))?;
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err("stream decoder stopped".into());
                    }
                }
            }
        }
        Ok(())
    };

    let mut emit = |chunk: BoundedBuildChunk| -> Result<(), String> {
        if chunk.file_offset != next_offset || chunk.preceding_lines < previous_lines {
            return Err("reader contract: streamed chunks are not consecutive".into());
        }
        if chunk.ranges.len() > limits.ranges
            || (chunk.bytes.len() > limits.chunk_bytes && chunk.ranges.len() != 1)
        {
            return Err("reader contract: streamed chunk exceeds its range or byte budget".into());
        }
        validate_reader_ranges(&chunk.ranges, chunk.bytes.len())?;
        next_offset = next_offset
            .checked_add(chunk.bytes.len() as u64)
            .ok_or("source offset overflow")?;
        if next_offset > input.revision.size || next_offset > bytes_read.load(Ordering::Relaxed) {
            return Err("reader contract: chunk exceeds observed input".into());
        }
        previous_lines = chunk.preceding_lines;
        emitted += 1;
        schedule(chunk)
    };
    let _framing = shared_budget.map(|budget| budget.enter_framing(pool));
    let result = {
        let _callback = shared_budget.map(|budget| budget.activity(StreamActivity::Callback, 1, 0));
        source
            .reader()
            .stream_build_chunks(&mut reader, limits.chunk_bytes, limits.ranges, &mut emit)
            .unwrap_or_else(|| {
                Err("reader advertised streaming but did not provide a stream".into())
            })
    };
    drop(emit);
    drop(reader);
    let result = result.and_then(|()| {
        if next_offset != input.revision.size
            || bytes_read.load(Ordering::Relaxed) != input.revision.size
        {
            return Err("reader contract: streamed chunks do not cover observed source".into());
        }
        if emitted == 0 {
            schedule(BoundedBuildChunk {
                bytes: Vec::new(),
                ranges: Vec::new(),
                file_offset: 0,
                preceding_lines: 0,
            })?;
        }
        Ok(())
    });
    drop(schedule);
    let mut outcome = result;
    for pending in waiting {
        if outcome.is_ok() {
            match drain(pending, None) {
                Ok(wait) => stats.receive_wait_ns += wait,
                Err(error) => outcome = Err(error),
            }
        } else {
            let _ = pending.1.recv();
        }
    }
    outcome?;
    stats.framing_chunks = ordinal as u64;
    stats.framing_bytes = next_offset;
    stats.source_bytes_read = bytes_read.load(Ordering::Relaxed);
    counters.add_to(&mut stats);
    Ok((stats, hash.then(|| digest.finalize().into())))
}

/// Prepare one physical file across all supplied eligible collections. JSONL
/// uses bounded chunks; other readers retain their existing whole-buffer
/// framing. A malformed frame falls back to ordinary build extraction.
#[cfg(test)]
pub(crate) fn prepare_file_build(
    root: &Path,
    input: SourceFileInput,
    bindings: &[FilePreparationBinding<'_>],
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
) -> Result<PreparedFileBuild, String> {
    let opened = std::fs::File::open(root.join(&input.relative_path));
    prepare_observed_file_build(
        root,
        input,
        bindings,
        alphabet,
        inference,
        inference_jobs,
        opened,
        None,
        None,
    )
}

/// Use exactly the handle whose observation selected this candidate. A
/// whole-document buffer is shared by all compatible whole-document bindings.
pub(crate) fn prepare_observed_file_build(
    root: &Path,
    input: SourceFileInput,
    bindings: &[FilePreparationBinding<'_>],
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
    opened: std::io::Result<std::fs::File>,
    expected_bytes: Option<&[u8]>,
    streaming_repository: Option<&Repository>,
) -> Result<PreparedFileBuild, String> {
    let path = root.join(&input.relative_path);
    if expected_bytes.is_some_and(|bytes| bytes.len() as u64 != input.revision.size) {
        return Err("supplied bytes differ from core-observed file".into());
    }
    let (mut opened, open_error) = match opened {
        Ok(file) => (Some(file), None),
        Err(error) => (None, Some(error)),
    };
    let observed_metadata = match &opened {
        Some(file) => Some(file.metadata().map_err(|error| error.to_string())?),
        None => std::fs::metadata(&path).ok(),
    };
    let identity = if let Some(metadata) = observed_metadata {
        if SourceRevision::from_metadata(&metadata)? != input.revision
            || (input.created_ns.is_some() && created_ns(&metadata) != input.created_ns)
        {
            return Err("stale source revision during preparation".into());
        }
        FileObjectIdentity::from_metadata(&metadata)
    } else {
        None
    };
    let mut fingerprint = None;
    let mut private = streaming_repository.map(|repository| {
        repository
            .stream_budget
            .activity(StreamActivity::PrivateSegments, 0, 0)
    });
    let mut collections = Vec::with_capacity(bindings.len());
    let mut whole = None;
    for binding in bindings {
        let reader = binding.source.reader();
        let compare = expected_bytes.filter(|_| fingerprint.is_none());
        if let (Some(repository), Some(file)) = (streaming_repository, opened.as_mut())
            && (reader.supports_streaming_build() || reader.supports_bounded_json_lines_build())
        {
            let (indexed, digest) = prepare_managed_stream(
                repository,
                root,
                &input,
                binding,
                file,
                compare,
                fingerprint.is_none(),
                private.as_mut().expect("managed stream ticket"),
            )?;
            fingerprint = fingerprint.or(digest);
            collections.push(PreparedCollectionBuild {
                collection: binding.collection,
                source_id: binding.source.id().into(),
                source_version: binding.source.version(),
                contributions: Vec::new(),
                indexed,
            });
            continue;
        }
        let contributions = match &mut opened {
            None => prepare_unreadable_file(
                root,
                &input,
                binding,
                alphabet,
                inference,
                inference_jobs,
                "source.open",
                open_error.as_ref().expect("unopened source"),
            )?,
            Some(file) => {
                if let Some(result) = reader
                    .supports_streaming_build()
                    .then(|| {
                        prepare_custom_stream(
                            root,
                            &input,
                            binding,
                            alphabet,
                            inference,
                            inference_jobs,
                            fingerprint.is_none(),
                            file,
                            compare,
                        )
                    })
                    .flatten()
                {
                    let (contributions, digest) = result?;
                    fingerprint = fingerprint.or(digest);
                    contributions
                } else if reader.supports_bounded_json_lines_build() {
                    let (contributions, digest) = prepare_bounded_json_lines(
                        root,
                        &input,
                        binding,
                        alphabet,
                        inference,
                        inference_jobs,
                        fingerprint.is_none(),
                        file,
                        compare,
                    )?;
                    fingerprint = fingerprint.or(digest);
                    contributions
                } else {
                    let bytes = whole.get_or_insert_with(|| {
                        file.seek(SeekFrom::Start(0))?;
                        let mut bytes = Vec::new();
                        file.read_to_end(&mut bytes)?;
                        Ok::<_, std::io::Error>(bytes)
                    });
                    match bytes {
                        Ok(bytes) => {
                            if bytes.len() as u64 != input.revision.size {
                                return Err("stale source length during preparation".into());
                            }
                            if compare.is_some_and(|expected| expected != bytes.as_slice()) {
                                return Err("supplied bytes differ from core-observed file".into());
                            }
                            if fingerprint.is_none() {
                                fingerprint = Some(Sha256::digest(&*bytes).into());
                            }
                            let mut prepared = prepare_file_build_with_fingerprint(
                                root,
                                input.clone(),
                                bytes,
                                std::slice::from_ref(binding),
                                alphabet,
                                inference,
                                inference_jobs,
                                streaming_repository.map_or(usize::MAX, |repository| {
                                    repository.options().segment_policy.target_records.get()
                                }),
                                fingerprint,
                            )
                            .map_err(|error| match error {
                                BytesPreparationError::Build(message) => message,
                                BytesPreparationError::UnsupportedReader { reason, .. } => {
                                    format!("{}: reader fatal: {reason}", path.display())
                                }
                                error => format!("{error:?}"),
                            })?;
                            prepared
                                .collections
                                .pop()
                                .expect("one binding")
                                .contributions
                        }
                        Err(error) if compare.is_none() => prepare_unreadable_file(
                            root,
                            &input,
                            binding,
                            alphabet,
                            inference,
                            inference_jobs,
                            "source.read",
                            error,
                        )?,
                        Err(error) => {
                            return Err(format!("supplied byte comparison failed: {error}"));
                        }
                    }
                }
            }
        };
        collections.push(PreparedCollectionBuild {
            collection: binding.collection,
            source_id: binding.source.id().into(),
            source_version: binding.source.version(),
            contributions,
            indexed: Vec::new(),
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
        _stream_memory: private,
    })
}

fn prepare_managed_stream(
    repository: &Repository,
    root: &Path,
    input: &SourceFileInput,
    binding: &FilePreparationBinding<'_>,
    file: &mut std::fs::File,
    expected_bytes: Option<&[u8]>,
    hash: bool,
    private: &mut StreamUse,
) -> Result<(Vec<IndexedCollectionSegment>, Option<[u8; 32]>), String> {
    let options = repository.options();
    let pool = &repository.workers;
    let policy = options.segment_policy;
    let fresh = || {
        SegmentAdmission::with_limits_and_id(
            binding.collection,
            binding.source,
            policy,
            SegmentId::new(2),
        )
    };
    let mut admission = fresh();
    let mut indexed = Vec::new();
    let mut seal = |draft: SegmentAdmission| -> Result<(), String> {
        let segment = draft.finish().into_indexed(
            binding.definition,
            options.bigram_alphabet,
            options.inference,
            repository.collections().len(),
            pool,
        )?;
        private.add_private_segment(&segment);
        indexed.push(segment);
        Ok(())
    };
    let mut consume = |batch: BatchContribution| -> Result<(), String> {
        if !admission.can_admit_batch(&batch) {
            seal(std::mem::replace(&mut admission, fresh()))?;
        }
        admission.admit_batch(batch)?;
        if admission.at_capacity() {
            seal(std::mem::replace(&mut admission, fresh()))?;
        }
        Ok(())
    };
    let root = Arc::new(root.to_path_buf());
    let definition = Arc::new(binding.definition.clone());
    let mut limits = BoundedBuildLimits::default();
    limits.ranges = limits.ranges.min(policy.target_records.get());
    let digest = if binding.source.reader().supports_streaming_build() {
        let (_, digest) = process_custom_stream_profiled(
            pool,
            root,
            definition,
            binding.collection,
            binding.source.clone(),
            input.clone(),
            file,
            options.bigram_alphabet,
            options.inference,
            expected_bytes,
            hash,
            limits,
            policy.target_records.get(),
            Some(&repository.stream_budget),
            &mut consume,
        )?;
        digest
    } else {
        let _framing = repository.stream_budget.enter_framing(pool);
        file.seek(SeekFrom::Start(0))
            .map_err(|error| error.to_string())?;
        let bytes_read = AtomicU64::new(0);
        let mut digest = Sha256::new();
        let reader = BoundedJsonLines::with_buffered_reader(BufReader::new(HashingReader {
            inner: file,
            digest: hash.then_some(&mut digest),
            bytes_read: &bytes_read,
            expected_bytes,
        }));
        process_bounded_json_lines_input(
            pool,
            root,
            definition,
            binding.collection,
            binding.source.clone(),
            Arc::new(input.clone()),
            options.bigram_alphabet,
            options.inference,
            options.detailed_profiling_enabled(),
            reader,
            limits,
            policy.target_records.get(),
            Some(&repository.stream_budget),
            &mut consume,
        )?;
        if bytes_read.load(Ordering::Relaxed) != input.revision.size {
            return Err("stale source length during managed streaming".into());
        }
        hash.then(|| digest.finalize().into())
    };
    drop(consume);
    if admission.file_count() != 0 {
        seal(admission)?;
    }
    Ok((indexed, digest))
}

/// Prepare a complete replacement without opening or writing its source path.
/// The caller owns `bytes` and must validate the declared revision and file
/// identity again before publishing the returned contributions. No raw payload
/// is kept in the result.
#[cfg(test)]
pub(crate) fn prepare_file_build_from_bytes(
    root: &Path,
    input: SourceFileInput,
    bytes: &[u8],
    bindings: &[FilePreparationBinding<'_>],
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
) -> Result<PreparedFileBuild, BytesPreparationError> {
    prepare_file_build_with_fingerprint(
        root,
        input,
        bytes,
        bindings,
        alphabet,
        inference,
        inference_jobs,
        usize::MAX,
        Some(Sha256::digest(bytes).into()),
    )
}

fn prepare_file_build_with_fingerprint(
    root: &Path,
    input: SourceFileInput,
    bytes: &[u8],
    bindings: &[FilePreparationBinding<'_>],
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
    record_target: usize,
    fingerprint: Option<[u8; 32]>,
) -> Result<PreparedFileBuild, BytesPreparationError> {
    let actual = u64::try_from(bytes.len())
        .map_err(|_| BytesPreparationError::Build("source size overflow".into()))?;
    if input.revision.size != actual {
        return Err(BytesPreparationError::RevisionSizeMismatch {
            declared: input.revision.size,
            actual,
        });
    }
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
                    for (chunk, _) in whole_document_chunks(&ranges, record_target) {
                        contributions.push(process_framed_batch(
                            root,
                            &input,
                            binding,
                            bytes,
                            &ranges[chunk],
                            0,
                            0,
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
            indexed: Vec::new(),
        });
    }
    Ok(PreparedFileBuild {
        source: input,
        fingerprint,
        identity: None,
        collections,
        _stream_memory: None,
    })
}

pub(crate) type FileRecordLayout = Vec<(CollectionId, RecordId, ByteRange, RecordClass)>;

/// Reconstruct only the fields used by verified maintenance.
pub(crate) fn reconstruct_file_layout(
    root: &Path,
    input: &SourceFileInput,
    bytes: &[u8],
    bindings: &[FilePreparationBinding<'_>],
) -> Result<FileRecordLayout, String> {
    use super::batch::{build_record_id, fallback_build_id};

    if input.revision.size != bytes.len() as u64 {
        return Err("old source size changed".into());
    }
    let path = root.join(&input.relative_path);
    let mut layout = Vec::new();
    for binding in bindings {
        let file = input.for_collection(binding.collection);
        let fallback = |range| {
            (
                binding.collection,
                fallback_build_id(binding.collection, binding.source, &file, range),
                range,
                RecordClass::RawFallback,
            )
        };
        let reader = binding.source.reader();
        let ranges = match reader.frame_outcome(bytes) {
            Ok(ranges) => ranges,
            Err(ReaderFailure::Malformed(_)) => {
                layout.push(fallback(ByteRange {
                    start: 0,
                    end: input.revision.size,
                }));
                continue;
            }
            Err(ReaderFailure::Fatal(error)) => return Err(error),
        };
        validate_reader_ranges(&ranges, bytes.len())?;
        for range in ranges {
            match reader.parse_for_build_outcome(&bytes[range.start as usize..range.end as usize]) {
                Ok(mut record) => {
                    if finish_build_record(&binding.definition.schema, &mut record).is_ok() {
                        let id = build_record_id(
                            root,
                            binding.definition,
                            binding.source,
                            &file,
                            &path,
                            range,
                            &record,
                        )?;
                        layout.push((binding.collection, id, range, RecordClass::Logical));
                    } else {
                        layout.push(fallback(range));
                    }
                }
                Err(ReaderFailure::Malformed(_)) => layout.push(fallback(range)),
                Err(ReaderFailure::Fatal(error)) => return Err(error),
            }
        }
    }
    Ok(layout)
}

fn whole_document_chunks(
    ranges: &[ByteRange],
    record_target: usize,
) -> Vec<(std::ops::Range<usize>, u64)> {
    const MAX_RANGE_BYTES: u64 = 1024 * 1024;
    const MAX_RANGES: usize = 256;
    let mut chunks = Vec::new();
    let mut start = 0;
    while start < ranges.len() {
        let remaining = record_target - start % record_target;
        let end_limit = ranges
            .len()
            .min(start.saturating_add(remaining.min(MAX_RANGES)));
        let mut end = start;
        let mut weight = 0u64;
        while end < end_limit {
            let size = ranges[end].len();
            if end > start && weight.saturating_add(size) > MAX_RANGE_BYTES {
                break;
            }
            weight = weight.saturating_add(size);
            end += 1;
        }
        chunks.push((start..end, weight.max(64 * 1024)));
        start = end;
    }
    chunks
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
    mut diagnostic: ReaderDiagnostic,
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
    let range = ByteRange::checked(0, input.revision.size, input.revision.size)?;
    crate::source::extraction::file_coordinates(
        bytes,
        range,
        &mut diagnostic,
        &mut crate::source::extraction::FileCoordinateCursor::default(),
    );
    builder.outcome(
        &mut file,
        &path,
        SourceOutcome::Malformed {
            range,
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

fn admit_large_unreadable(
    root: &Path,
    input: &SourceFileInput,
    binding: &FilePreparationBinding<'_>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    inference_jobs: usize,
    code: &str,
    error: &std::io::Error,
    admit: &mut impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    for batch in prepare_unreadable_file(
        root,
        input,
        binding,
        alphabet,
        inference,
        inference_jobs,
        code,
        error,
    )? {
        admit(batch)?;
    }
    Ok(BuildScheduleStats::default())
}

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
        usize::MAX,
        &mut admit,
    )
}

#[cfg(test)]
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
    record_target: usize,
    admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    process_framed_file_with_budget(
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
        record_target,
        None,
        admit,
    )
}

pub(super) fn process_framed_file_with_budget(
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
    record_target: usize,
    shared_budget: Option<&StreamBudget>,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let mut limits = BoundedBuildLimits::default();
    limits.ranges = limits.ranges.min(record_target);
    if source.reader().supports_streaming_build() {
        let input = source_files[file_id.0 as usize].clone();
        let path = root.join(&input.relative_path);
        let mut file = std::fs::File::open(&path).map_err(|error| error.to_string())?;
        if SourceRevision::from_metadata(&file.metadata().map_err(|error| error.to_string())?)?
            != input.revision
        {
            return Err("stale source revision before registered streaming".into());
        }
        let (mut stats, digest) = process_custom_stream_profiled(
            pool,
            root,
            definition,
            collection,
            source,
            input.clone(),
            &mut file,
            alphabet,
            inference,
            None,
            true,
            limits,
            record_target,
            shared_budget,
            admit,
        )?;
        crate::source::access::check_revision(&path, &file, input.revision)?;
        stats.file_fingerprint = digest.map(|digest| (file_id, digest));
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
            record_target,
            shared_budget,
            admit,
        );
    }
    const MAX_ADMITTED_BYTES: u64 = 64 * 1024 * 1024;
    let max_jobs = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    let mut stats = BuildScheduleStats {
        weight_limit_bytes: MAX_ADMITTED_BYTES,
        ..Default::default()
    };
    let input = &source_files[file_id.0 as usize];
    let path = root.join(&input.relative_path);
    let binding = FilePreparationBinding {
        collection,
        definition: &definition,
        source: &source,
    };
    let mut file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            return admit_large_unreadable(
                &root,
                input,
                &binding,
                alphabet,
                inference,
                max_jobs,
                "source.open",
                &error,
                &mut admit,
            );
        }
    };
    if SourceRevision::from_metadata(&file.metadata().map_err(|error| error.to_string())?)?
        != input.revision
    {
        return Err("stale source revision before large-file extraction".into());
    }
    let mut bytes = Vec::new();
    if let Err(error) = file.read_to_end(&mut bytes) {
        let read_bytes = bytes.len() as u64;
        let mut stats = admit_large_unreadable(
            &root,
            input,
            &binding,
            alphabet,
            inference,
            max_jobs,
            "source.read",
            &error,
            &mut admit,
        )?;
        stats.source_reads = 1;
        stats.source_bytes_read = read_bytes;
        return Ok(stats);
    }
    if bytes.len() as u64 != input.revision.size {
        return Err("stale source length during large-file extraction".into());
    }
    let bytes = Arc::new(bytes);
    let ranges = match source.reader().frame_outcome(&bytes) {
        Ok(ranges) => ranges,
        Err(ReaderFailure::Malformed(diagnostic)) => {
            let fingerprint = Sha256::digest(&**bytes).into();
            let batch = prepare_malformed_supplied_file(
                &root, input, &binding, &bytes, diagnostic, alphabet, inference, max_jobs,
            )
            .map_err(|error| format!("{error:?}"))?;
            crate::source::access::check_revision(&path, &file, input.revision)?;
            admit(batch)?;
            return Ok(BuildScheduleStats {
                source_reads: 1,
                source_bytes_read: bytes.len() as u64,
                file_fingerprint: Some((file_id, fingerprint)),
                ..Default::default()
            });
        }
        Err(ReaderFailure::Fatal(error)) => {
            return Err(format!("{}: reader fatal: {error}", path.display()));
        }
    };
    validate_reader_ranges(&ranges, bytes.len())?;
    let file_fingerprint = (bytes.len() as u64 == input.revision.size)
        .then(|| (file_id, Sha256::digest(&*bytes).into()));
    let source_bytes = bytes.len() as u64;
    stats.source_reads = 1;
    stats.source_bytes_read = source_bytes;
    let mut chunks = whole_document_chunks(&ranges, record_target);
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
            let execute = move || {
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
            };
            if pool.current_thread_index().is_some() {
                execute();
            } else {
                pool.spawn(execute);
            }
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
    crate::source::access::check_revision(&path, &file, input.revision)?;
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
    record_target: usize,
    shared_budget: Option<&StreamBudget>,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let max_jobs = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    let path = root.join(&source_files[file_id.0 as usize].relative_path);
    let input = source_files[file_id.0 as usize].clone();
    let binding = FilePreparationBinding {
        collection,
        definition: &definition,
        source: &source,
    };
    let file = match std::fs::File::open(&path) {
        Ok(file) => file,
        Err(error) => {
            return admit_large_unreadable(
                &root,
                &input,
                &binding,
                alphabet,
                inference,
                max_jobs,
                "source.open",
                &error,
                &mut admit,
            );
        }
    };
    if SourceRevision::from_metadata(&file.metadata().map_err(|error| error.to_string())?)?
        != input.revision
    {
        return Err("stale source revision before bounded extraction".into());
    }
    let mut digest = Sha256::new();
    let bytes_read = AtomicU64::new(0);
    let expected_size = source_files[file_id.0 as usize].revision.size;
    let _framing = shared_budget.map(|budget| budget.enter_framing(pool));
    let reader = BoundedJsonLines::with_buffered_reader(BufReader::new(HashingReader {
        inner: &file,
        digest: Some(&mut digest),
        bytes_read: &bytes_read,
        expected_bytes: None,
    }));
    let mut stats = process_bounded_json_lines_input(
        pool,
        root,
        definition,
        collection,
        source,
        Arc::new(input.clone()),
        alphabet,
        inference,
        profile_build,
        reader,
        BoundedBuildLimits::default(),
        record_target,
        shared_budget,
        admit,
    )?;
    crate::source::access::check_revision(&path, &file, input.revision)?;
    if bytes_read.load(Ordering::Relaxed) != expected_size {
        return Err("stale source length during bounded extraction".into());
    }
    stats.file_fingerprint = Some((file_id, digest.finalize().into()));
    stats.source_reads = stats.source_reads.saturating_add(1);
    stats.source_bytes_read = stats
        .source_bytes_read
        .saturating_add(bytes_read.load(Ordering::Relaxed));
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

#[cfg(test)]
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
    reader: BoundedJsonLines<R>,
    limits: BoundedBuildLimits,
    admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let input = Arc::new(source_files[file_id.0 as usize].clone());
    process_bounded_json_lines_input(
        pool,
        root,
        definition,
        collection,
        source,
        input,
        alphabet,
        inference,
        profile_build,
        reader,
        limits,
        usize::MAX,
        None,
        admit,
    )
}

fn process_bounded_json_lines_input<R: BufRead + Send>(
    pool: &ThreadPool,
    root: Arc<PathBuf>,
    definition: Arc<CollectionDefinition>,
    collection: CollectionId,
    source: PreparedSource,
    input: Arc<SourceFileInput>,
    alphabet: BigramAlphabet,
    inference: InferenceConfig,
    profile_build: bool,
    mut reader: BoundedJsonLines<R>,
    limits: BoundedBuildLimits,
    record_target: usize,
    shared_budget: Option<&StreamBudget>,
    mut admit: impl FnMut(BatchContribution) -> Result<(), String>,
) -> Result<BuildScheduleStats, String> {
    let path = root.join(&input.relative_path);
    let max_jobs = inference.max_build_jobs(pool.current_num_threads().saturating_mul(2).max(1));
    if shared_budget.is_some() {
        reader.limit_record_bytes(128 * 1024 * 1024);
    }
    let mut scratch = shared_budget.map(|budget| {
        budget.activity(
            StreamActivity::PendingInput,
            0,
            reader.retained_line_capacity() as u64,
        )
    });
    // Failed input aborts private work. Reopening the pathname here could
    // read a different file from the one being observed.
    let mut remaining = record_target;
    let mut next_chunk = || {
        let chunk = reader.next_chunk(limits.chunk_bytes, limits.ranges.min(remaining))?;
        if let Some(scratch) = scratch.as_mut() {
            scratch.set(0, reader.retained_line_capacity() as u64);
        }
        if let Some(chunk) = &chunk {
            remaining -= chunk.ranges.len();
            if remaining == 0 {
                remaining = record_target;
            }
        }
        Ok::<_, String>(chunk)
    };
    let first_started = crate::measurement::timer::start_if_enabled(profile_build);
    let first = match next_chunk() {
        Ok(Some(chunk)) => chunk,
        Ok(None) => BoundedBuildChunk {
            bytes: Vec::new(),
            ranges: Vec::new(),
            file_offset: 0,
            preceding_lines: 0,
        },
        Err(error) => return Err(error),
    };
    let mut pending_input = shared_budget
        .map(|budget| budget.activity(StreamActivity::PendingInput, 0, chunk_capacity(&first)));
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
    while failure.is_none() {
        while dispatched - next < max_jobs {
            if pending.is_none() && !eof {
                let started = crate::measurement::timer::start_if_enabled(profile_build);
                match next_chunk() {
                    Ok(Some(chunk)) => {
                        if let Some(started) = started {
                            stats.framing_ns += started.elapsed().as_nanos() as u64;
                            stats.framing_chunks += 1;
                            stats.framing_bytes += chunk.bytes.len() as u64;
                        }
                        if let Some(pending_input) = pending_input.as_mut() {
                            pending_input.set(0, chunk_capacity(&chunk));
                        }
                        pending = Some(chunk);
                    }
                    Ok(None) => eof = true,
                    Err(error) => {
                        failure =
                            Some(format!("{}: bounded framing read: {error}", path.display()));
                        break;
                    }
                }
            }
            let Some(chunk) = pending.as_ref() else {
                break;
            };
            let weight = chunk_capacity(chunk).max(64 * 1024);
            if !fits_allowance(admitted_bytes, weight, limits.admitted_bytes) {
                break;
            }
            let credit = if let Some(shared) = shared_budget {
                match shared.try_reserve(
                    weight,
                    max_jobs,
                    limits.admitted_bytes,
                    &mut pending_input,
                ) {
                    Some(credit) => Some(credit),
                    None if next < dispatched => break,
                    None => Some(shared.reserve(
                        weight,
                        max_jobs,
                        limits.admitted_bytes,
                        pool,
                        &mut pending_input,
                    )),
                }
            } else {
                None
            };
            let chunk = pending.take().expect("checked pending chunk");
            let index = dispatched;
            let sender = sender.clone();
            let root = Arc::clone(&root);
            let definition = Arc::clone(&definition);
            let source = source.clone();
            let input = Arc::clone(&input);
            let counters = Arc::clone(&counters);
            let worker_budget = shared_budget.cloned();
            let execute = move || {
                let active = worker_budget
                    .as_ref()
                    .map(|budget| budget.activity(StreamActivity::Decoder, 1, 0));
                let result = counters.run(|| {
                    process_framed_batch(
                        &root,
                        &input,
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
                drop(chunk);
                drop(active);
                let completed = worker_budget
                    .as_ref()
                    .map(|budget| budget.activity(StreamActivity::Completed, 1, 0));
                let _ = sender.send((index, result, completed));
            };
            if pool.current_thread_index().is_some() {
                execute();
            } else if pool.current_num_threads() == 1 {
                pool.install(execute);
            } else {
                pool.spawn(execute);
            }
            reservations.push_back((weight, credit));
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
        let (index, result, completed_ticket) =
            receiver.recv().map_err(|error| error.to_string())?;
        stats.receive_wait_ns = stats
            .receive_wait_ns
            .saturating_add(wait_started.elapsed().as_nanos() as u64);
        completed += 1;
        ready.insert(index, (result, completed_ticket));
        while let Some((result, _completed)) = ready.remove(&next) {
            let (weight, _credit) = reservations
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
    if let Some(error) = failure {
        return Err(error);
    }
    stats.peak_weight_bytes = peak_bytes;
    counters.add_to(&mut stats);
    Ok(stats)
}
