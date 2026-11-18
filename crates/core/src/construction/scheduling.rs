//! Source-job dispatch, worker reservations and ordered draining.

use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "detailed-profiling")]
use std::sync::atomic::AtomicU64;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

use rayon::ThreadPool;
use serde::Serialize;

use crate::execution_options::BigramAlphabet;
use crate::index::bigram::storage_diagnostic_enabled;
use crate::inference::InferenceConfig;
use crate::model::{CollectionId, FileId};
use crate::repository::CollectionDefinition;
use crate::source::registry::PreparedSource;

use super::batch::{BatchContribution, SourceFileInput, process_batch_profiled};

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

pub(crate) fn fits_allowance(current: u64, weight: u64, limit: u64) -> bool {
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
pub(crate) struct JobCounters {
    active: AtomicUsize,
    peak_active: AtomicUsize,
    completed: AtomicUsize,
    pub(super) admitted: AtomicUsize,
    peak_ready: AtomicUsize,
    #[cfg(feature = "detailed-profiling")]
    execution_ns: AtomicU64,
}

impl JobCounters {
    pub(super) fn run<T>(&self, job: impl FnOnce() -> T) -> T {
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

    pub(super) fn add_to(&self, stats: &mut BuildScheduleStats) {
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

#[cfg(test)]
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
            let dispatched_at = crate::detailed_timer::start_if_enabled(profile_build);
            pool.spawn(move || {
                let started_at = crate::detailed_timer::start_if_enabled(profile_build);
                let result = counters.run(|| {
                    process_batch_profiled(
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
        ready.insert(index, result);
        if profile_build && storage_diagnostic_enabled() {
            let ready_bytes = ready
                .values()
                .filter_map(|result| result.as_ref().ok())
                .fold(0usize, |bytes, batch| {
                    bytes.saturating_add(batch.bigram_storage_stats().retained_capacity_bytes)
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
