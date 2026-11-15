//! Bounded direct-source construction for the RocksDB prototype.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;
use std::sync::mpsc::{self, sync_channel};
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use roaring::RoaringBitmap;
use serde::Serialize;

use crate::build::{BuildWorkProfile, SourceFileInput, process_store_batch_profiled};
use crate::inference::InferenceConfig;
use crate::inventory::{FileEntry, FileStatus};
use crate::model::{CollectionId, RecordSlot};
use crate::repository::CollectionDefinition;
use crate::source::SourceRegistry;
use crate::workload::BigramAlphabet;

use super::finalize::{FinalizationReport, finalize};
use super::prepare::PreparedStoreBatch;
use super::schema::RuntimeSchema;
use super::write::{
    AdmissionBatch, AdmissionExecutionStats, PhysicalPreparedFile, admit_physical_batch, discover,
    initialise_schema, map_prepared_file,
};
use super::{RocksStore, StoreConfig, StoreError, StoreResult, value_codec};

const FIRST_JOB_FILES: usize = 1;
const MAX_JOB_FILES: usize = 256;
const MAX_JOB_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ADMISSION_FILES: usize = 256;
const MAX_ADMISSION_RECORDS: u64 = 2_048;
const MAX_ADMISSION_BYTES: u64 = 8 * 1024 * 1024;
const DEFAULT_RETAINED_TARGET_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct DirectBuildOptions {
    pub prepare_workers: Option<usize>,
    pub write_workers: Option<usize>,
    pub retained_target_bytes: usize,
    pub profile: bool,
    pub pipelined_writes: bool,
}

impl Default for DirectBuildOptions {
    fn default() -> Self {
        Self {
            prepare_workers: None,
            write_workers: None,
            retained_target_bytes: DEFAULT_RETAINED_TARGET_BYTES,
            profile: false,
            pipelined_writes: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct DirectBuildTiming {
    pub open_ns: u64,
    pub schema_ns: u64,
    pub discovery_ns: u64,
    pub preparation_wall_ns: u64,
    pub preparation_worker_ns: u64,
    pub coordinator_mapping_ns: u64,
    pub writer_queue_wait_ns: u64,
    pub admission_wall_ns: u64,
    pub admission_worker_ns: u64,
    pub finalization_ns: u64,
    pub ready_ns: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DirectBuildCounts {
    pub physical_files: u64,
    pub memberships: u64,
    pub records: u64,
    pub record_parses: u64,
    pub batches: u64,
    pub failed_parses: u64,
    pub source_reads: u64,
    pub source_bytes_read: u64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DirectBuildSchedule {
    pub available_parallelism: usize,
    pub background_jobs: usize,
    pub requested_prepare_workers: Option<usize>,
    pub resolved_prepare_workers: usize,
    pub requested_write_workers: Option<usize>,
    pub resolved_write_workers: usize,
    pub retained_target_bytes: usize,
    pub peak_ready_physical_files: usize,
    pub peak_ready_bytes: u64,
    pub peak_reservation_estimate_bytes: u64,
    pub job_count: usize,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct DirectBuildReport {
    pub timing: DirectBuildTiming,
    pub counts: DirectBuildCounts,
    pub schedule: DirectBuildSchedule,
    pub profile: BuildWorkProfile,
    pub admission: AdmissionExecutionStats,
    pub discovery_metadata: super::metadata::MetadataWriteStats,
    pub finalization: FinalizationReport,
}

pub struct BuiltStore {
    pub store: RocksStore,
    pub report: DirectBuildReport,
}

struct PreparationJob {
    index: usize,
    files: Vec<FileEntry>,
    source_bytes: u64,
}

struct PreparedPhysical {
    file: FileEntry,
    collections: Vec<(CollectionId, PreparedStoreBatch)>,
}

struct PreparedChunk {
    index: usize,
    files: Vec<PreparedPhysical>,
    preparation_ns: u64,
    profile: BuildWorkProfile,
    records: u64,
    record_parses: u64,
    failed_parses: u64,
    source_reads: u64,
    source_bytes_read: u64,
}

pub fn build_from_source(
    config: StoreConfig,
    root: &Path,
    definitions: &[CollectionDefinition],
    registry: &SourceRegistry,
    alphabet: BigramAlphabet,
    options: DirectBuildOptions,
) -> StoreResult<BuiltStore> {
    let trace = std::env::var_os("OBJEKTE_ROCKSDB_TRACE").is_some();
    if options.pipelined_writes {
        return Err(StoreError::State(
            "pipelined writes are reserved for the post-control experiment".into(),
        ));
    }
    let ready_started = Instant::now();
    let available = std::thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    let background_jobs = available.min(2).max(1);
    let default_writers = available.div_euclid(4).clamp(1, 4);
    let writers = resolve_requested_workers(
        "write-workers",
        options.write_workers,
        default_writers,
        available,
    )?;
    let default_prepare = available.saturating_sub(writers + background_jobs).max(1);
    let prepare = resolve_requested_workers(
        "prepare-workers",
        options.prepare_workers,
        default_prepare,
        available,
    )?;
    if prepare.saturating_add(writers) > available {
        return Err(StoreError::State(format!(
            "prepare-workers {prepare} plus write-workers {writers} exceeds available_parallelism {available}"
        )));
    }
    if options.retained_target_bytes == 0 {
        return Err(StoreError::State(
            "retained preparation target must be positive".into(),
        ));
    }

    let open_started = Instant::now();
    let store = RocksStore::create(config)?;
    let open_ns = elapsed_ns(open_started);

    let schema_started = Instant::now();
    let schema = Arc::new(RuntimeSchema::new(definitions, registry, alphabet)?);
    initialise_schema(&store, &schema)?;
    let schema_ns = elapsed_ns(schema_started);

    let discovery_started = Instant::now();
    let discovery = discover(&store, &schema, root)?;
    let mut files = discovery.inventory.clone();
    files.sort_by_key(|file| file.id.0);
    let source_files = Arc::new(
        files
            .iter()
            .map(SourceFileInput::from_entry)
            .collect::<Vec<_>>(),
    );
    let jobs = preparation_jobs(&files);
    let job_count = jobs.len();
    if trace {
        eprintln!(
            "rocksdb-build: discovered files={} jobs={job_count}",
            files.len()
        );
    }
    let discovery_ns = elapsed_ns(discovery_started);

    let max_in_flight = (prepare.saturating_mul(2).saturating_add(writers))
        .min(16)
        .max(1);
    let peak_reservation_estimate_bytes = jobs
        .iter()
        .take(max_in_flight)
        .map(|job| job.source_bytes.saturating_mul(8).max(1024 * 1024))
        .fold(0u64, u64::saturating_add);
    let preparation_concurrency = prepare.min(max_in_flight);
    let queue_budget = max_in_flight.saturating_sub(preparation_concurrency);
    let prepared_queue_capacity = queue_budget.div_ceil(2).max(1);
    let writer_queue_capacity = queue_budget.saturating_sub(prepared_queue_capacity).max(1);
    if trace {
        eprintln!(
            "rocksdb-build: pools prepare={prepare} writers={writers} in_flight={max_in_flight}"
        );
    }

    let (job_sender, job_receiver) = mpsc::channel::<PreparationJob>();
    for job in jobs {
        job_sender
            .send(job)
            .map_err(|_| StoreError::State("preparation job queue closed".into()))?;
    }
    drop(job_sender);
    if trace {
        eprintln!("rocksdb-build: jobs queued");
    }

    let (prepared_sender, prepared_receiver) =
        sync_channel::<StoreResult<PreparedChunk>>(prepared_queue_capacity);
    let (file_sender, file_receiver) = sync_channel::<AdmissionBatch>(writer_queue_capacity);
    let root = Arc::new(root.to_path_buf());
    let shared_job_receiver = Arc::new(std::sync::Mutex::new(job_receiver));
    let schema_for_workers = Arc::clone(&schema);
    let source_files_for_workers = Arc::clone(&source_files);
    let preparation_started = Instant::now();
    let admission_started = Arc::new(OnceLock::<Instant>::new());

    let (
        preparation_failure,
        mapping_ns,
        queue_wait_ns,
        preparation_worker_ns,
        peak_ready_files,
        peak_ready_bytes,
        build_profile,
        prepared_counts,
        admission_totals,
        writer_failure,
        writer_time,
    ) = std::thread::scope(|scope| -> StoreResult<_> {
        if trace {
            eprintln!("rocksdb-build: starting workers");
        }
        let mut writer_handles = Vec::with_capacity(writers);
        let shared_file_receiver = Arc::new(std::sync::Mutex::new(file_receiver));
        for index in 0..writers {
            let receiver = Arc::clone(&shared_file_receiver);
            let store = &store;
            let schema = Arc::clone(&schema);
            let admission_started = Arc::clone(&admission_started);
            writer_handles.push(scope.spawn(move || {
                if trace {
                    eprintln!("rocksdb-build: writer {index} started");
                }
                let mut outcomes = Vec::new();
                let mut scratch =
                    crate::rocksdb_store::contribution_codec::ContributionScratch::default();
                loop {
                    let admission = match receiver.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(_) => break,
                    };
                    let Ok(admission) = admission else {
                        break;
                    };
                    let _ = admission_started.get_or_init(Instant::now);
                    let started = Instant::now();
                    let batch_id = admission.id;
                    let outcome =
                        admit_physical_batch_with_retry(store, &schema, &admission, &mut scratch);
                    outcomes.push((batch_id, started.elapsed().as_nanos() as u64, outcome));
                }
                let _ = index;
                outcomes
            }));
        }

        let mut preparation_handles = Vec::with_capacity(preparation_concurrency);
        let profile_build = options.profile;
        for index in 0..preparation_concurrency {
            let receiver = Arc::clone(&shared_job_receiver);
            let sender = prepared_sender.clone();
            let root = Arc::clone(&root);
            let schema = Arc::clone(&schema_for_workers);
            let source_files = Arc::clone(&source_files_for_workers);
            preparation_handles.push(scope.spawn(move || {
                if trace {
                    eprintln!("rocksdb-build: preparation worker {index} started");
                }
                loop {
                    if trace {
                        eprintln!("rocksdb-build: preparation task waiting");
                    }
                    let job = match receiver.lock() {
                        Ok(receiver) => receiver.recv(),
                        Err(_) => return,
                    };
                    let Ok(job) = job else {
                        if trace {
                            eprintln!("rocksdb-build: preparation queue closed");
                        }
                        return;
                    };
                    if trace {
                        eprintln!("rocksdb-build: preparing job {}", job.index);
                    }
                    let result = prepare_job(
                        &root,
                        &schema,
                        &job,
                        &source_files,
                        alphabet,
                        profile_build,
                        prepare,
                    )
                    .map_err(StoreError::State);
                    if trace {
                        eprintln!(
                            "rocksdb-build: prepared job {}: {}",
                            job.index,
                            result.as_ref().map_or_else(
                                |error| error.to_string(),
                                |chunk| format!("{} files", chunk.files.len())
                            )
                        );
                    }
                    if sender.send(result).is_err() {
                        return;
                    }
                }
            }));
        }
        drop(prepared_sender);

        let mut ready = BTreeMap::new();
        let mut next_job = 0usize;
        let mut next_admission_id = 1u64;
        let mut sent_admissions = 0u64;
        let mut base_slots = BTreeMap::<u32, u32>::new();
        let mut failure = None;
        let mut mapping_ns = 0u64;
        let mut queue_wait_ns = 0u64;
        let mut peak_ready_files = 0usize;
        let mut peak_ready_bytes = 0u64;
        let mut ready_bytes = 0u64;
        let mut preparation_worker_ns = 0u64;
        let mut profile = BuildWorkProfile::default();
        let mut counts = DirectBuildCounts::default();

        for _ in 0..job_count {
            let result = prepared_receiver
                .recv()
                .map_err(|_| StoreError::State("preparation result queue closed".into()));
            let Ok(result) = result else {
                failure = result.err();
                break;
            };
            if trace {
                eprintln!(
                    "rocksdb-build: received preparation result: {}",
                    result.as_ref().map_or_else(
                        |error| error.to_string(),
                        |chunk| format!("job {}", chunk.index)
                    )
                );
            }
            match result {
                Ok(chunk) => {
                    preparation_worker_ns =
                        preparation_worker_ns.saturating_add(chunk.preparation_ns);
                    profile.absorb(&chunk.profile);
                    counts.records = counts.records.saturating_add(chunk.records);
                    counts.record_parses = counts.record_parses.saturating_add(chunk.record_parses);
                    counts.failed_parses = counts.failed_parses.saturating_add(chunk.failed_parses);
                    counts.source_reads = counts.source_reads.saturating_add(chunk.source_reads);
                    counts.source_bytes_read = counts
                        .source_bytes_read
                        .saturating_add(chunk.source_bytes_read);
                    let bytes = chunk
                        .files
                        .iter()
                        .flat_map(|file| &file.collections)
                        .map(|(_, batch)| batch.retained_capacity_bytes as u64)
                        .fold(0u64, u64::saturating_add);
                    ready_bytes = ready_bytes.saturating_add(bytes);
                    peak_ready_bytes = peak_ready_bytes.max(ready_bytes);
                    peak_ready_files = peak_ready_files.max(ready.len().saturating_add(1));
                    ready.insert(chunk.index, (chunk, bytes));
                }
                Err(error) => {
                    ready.insert(
                        usize::MAX,
                        (
                            PreparedChunk {
                                index: usize::MAX,
                                files: Vec::new(),
                                preparation_ns: 0,
                                profile: BuildWorkProfile::default(),
                                records: 0,
                                record_parses: 0,
                                failed_parses: 0,
                                source_reads: 0,
                                source_bytes_read: 0,
                            },
                            0,
                        ),
                    );
                    failure = Some(error);
                }
            }

            while failure.is_none() {
                let Some((chunk, bytes)) = ready.remove(&next_job) else {
                    break;
                };
                ready_bytes = ready_bytes.saturating_sub(bytes);
                let map_started = Instant::now();
                let mut admission_batch = Vec::<PhysicalPreparedFile>::new();
                let mut admission_records = 0u64;
                let mut admission_bytes = 0u64;
                for prepared in chunk.files {
                    let physical_file_id = prepared.file.id.0;
                    let merged = merge_file_entry(&prepared.file, &prepared.collections);
                    let mapped = map_prepared_file(
                        &schema,
                        prepared.file.id,
                        prepared.collections,
                        &mut base_slots,
                    )?;
                    let records = mapped
                        .collections
                        .iter()
                        .map(|mapped| mapped.batch.records.len() as u64)
                        .sum::<u64>();
                    counts.physical_files = counts.physical_files.saturating_add(1);
                    counts.memberships = counts
                        .memberships
                        .saturating_add(mapped.collections.len() as u64);
                    let mut mapped = mapped;
                    mapped.file = merged;
                    let file_bytes = mapped.file.revision.size;
                    debug_assert_eq!(
                        records,
                        mapped
                            .collections
                            .iter()
                            .map(|mapped| mapped.batch.records.len() as u64)
                            .sum::<u64>()
                    );
                    if !admission_batch.is_empty()
                        && (admission_batch.len() >= MAX_ADMISSION_FILES
                            || admission_records.saturating_add(records) > MAX_ADMISSION_RECORDS
                            || admission_bytes.saturating_add(file_bytes) > MAX_ADMISSION_BYTES)
                    {
                        let send_started = Instant::now();
                        let id = next_admission_id;
                        next_admission_id = next_admission_id.checked_add(1).ok_or_else(|| {
                            StoreError::State("admission batch ID overflow".into())
                        })?;
                        sent_admissions += 1;
                        file_sender
                            .send(AdmissionBatch {
                                id,
                                files: std::mem::take(&mut admission_batch),
                            })
                            .map_err(|_| {
                                StoreError::State("admission worker queue closed".into())
                            })?;
                        queue_wait_ns = queue_wait_ns.saturating_add(elapsed_ns(send_started));
                        admission_records = 0;
                        admission_bytes = 0;
                    }
                    admission_records = admission_records.saturating_add(records);
                    admission_bytes = admission_bytes.saturating_add(file_bytes);
                    admission_batch.push(mapped);
                    if sent_admissions == 0 {
                        let send_started = Instant::now();
                        let id = next_admission_id;
                        next_admission_id = next_admission_id.checked_add(1).ok_or_else(|| {
                            StoreError::State("admission batch ID overflow".into())
                        })?;
                        sent_admissions += 1;
                        file_sender
                            .send(AdmissionBatch {
                                id,
                                files: std::mem::take(&mut admission_batch),
                            })
                            .map_err(|_| {
                                StoreError::State("admission worker queue closed".into())
                            })?;
                        queue_wait_ns = queue_wait_ns.saturating_add(elapsed_ns(send_started));
                        admission_records = 0;
                        admission_bytes = 0;
                    }
                    if trace {
                        eprintln!(
                            "rocksdb-build: prepared physical file {physical_file_id} for admission"
                        );
                    }
                }
                if !admission_batch.is_empty() {
                    if trace {
                        eprintln!(
                            "rocksdb-build: queued {} physical files",
                            admission_batch.len()
                        );
                    }
                    let send_started = Instant::now();
                    let id = next_admission_id;
                    next_admission_id = next_admission_id
                        .checked_add(1)
                        .ok_or_else(|| StoreError::State("admission batch ID overflow".into()))?;
                    sent_admissions += 1;
                    file_sender
                        .send(AdmissionBatch {
                            id,
                            files: admission_batch,
                        })
                        .map_err(|_| StoreError::State("admission worker queue closed".into()))?;
                    queue_wait_ns = queue_wait_ns.saturating_add(elapsed_ns(send_started));
                }
                mapping_ns = mapping_ns.saturating_add(elapsed_ns(map_started));
                next_job += 1;
            }
            if failure.is_some() {
                break;
            }
        }
        drop(file_sender);
        drop(prepared_receiver);
        for handle in preparation_handles {
            handle
                .join()
                .map_err(|_| StoreError::State("RocksDB preparation worker panicked".into()))?;
        }

        let mut writer_failure = None;
        let mut writer_time = 0u64;
        let mut admission_totals = AdmissionExecutionStats::default();
        let mut writer_commits = Vec::new();
        for handle in writer_handles {
            let outcomes = handle
                .join()
                .map_err(|_| StoreError::State("RocksDB admission worker panicked".into()))?;
            for (batch_id, elapsed, outcome) in outcomes {
                writer_time = writer_time.saturating_add(elapsed);
                if let Ok(stats) = &outcome {
                    admission_totals.planning_ns = admission_totals
                        .planning_ns
                        .saturating_add(stats.planning_ns);
                    admission_totals.database_ns = admission_totals
                        .database_ns
                        .saturating_add(stats.database_ns);
                    admission_totals.commit_ns =
                        admission_totals.commit_ns.saturating_add(stats.commit_ns);
                    admission_totals.keys = admission_totals.keys.saturating_add(stats.keys);
                    admission_totals.encoded_bytes = admission_totals
                        .encoded_bytes
                        .saturating_add(stats.encoded_bytes);
                    admission_totals.block_reads = admission_totals
                        .block_reads
                        .saturating_add(stats.block_reads);
                    admission_totals.attempts =
                        admission_totals.attempts.saturating_add(stats.attempts);
                }
                writer_commits.push((batch_id, outcome));
            }
        }
        // Deterministic error selection independent of writer scheduling.
        writer_commits.sort_by_key(|(batch_id, _)| *batch_id);
        counts.batches = writer_commits.len() as u64;
        for (_, outcome) in writer_commits {
            if let Err(error) = outcome {
                writer_failure.get_or_insert(error);
            }
        }
        Ok::<_, StoreError>((
            failure,
            mapping_ns,
            queue_wait_ns,
            preparation_worker_ns,
            peak_ready_files,
            peak_ready_bytes,
            profile,
            counts,
            admission_totals,
            writer_failure,
            writer_time,
        ))
    })?;

    let admission_wall_ns = admission_started
        .get()
        .map(|started| elapsed_ns(*started))
        .unwrap_or(0);
    if let Some(error) = writer_failure.or(preparation_failure) {
        return Err(error);
    }

    let finalization_started = Instant::now();
    if trace {
        eprintln!("rocksdb-build: finalizing");
    }
    let finalization = finalize(&store)?;
    let finalization_ns = elapsed_ns(finalization_started);
    let mut counts = prepared_counts;
    counts.physical_files = discovery.files;
    counts.memberships = discovery.memberships;

    Ok(BuiltStore {
        store,
        report: DirectBuildReport {
            timing: DirectBuildTiming {
                open_ns,
                schema_ns,
                discovery_ns,
                preparation_wall_ns: elapsed_ns(preparation_started),
                preparation_worker_ns,
                coordinator_mapping_ns: mapping_ns.saturating_sub(queue_wait_ns),
                writer_queue_wait_ns: queue_wait_ns,
                admission_wall_ns,
                admission_worker_ns: writer_time,
                finalization_ns,
                ready_ns: elapsed_ns(ready_started),
            },
            counts,
            schedule: DirectBuildSchedule {
                available_parallelism: available,
                background_jobs,
                requested_prepare_workers: options.prepare_workers,
                resolved_prepare_workers: prepare,
                requested_write_workers: options.write_workers,
                resolved_write_workers: writers,
                retained_target_bytes: options.retained_target_bytes,
                peak_ready_physical_files: peak_ready_files,
                peak_ready_bytes,
                peak_reservation_estimate_bytes,
                job_count,
            },
            profile: build_profile,
            admission: admission_totals,
            discovery_metadata: discovery.metadata,
            finalization,
        },
    })
}

fn admit_physical_batch_with_retry(
    store: &RocksStore,
    schema: &RuntimeSchema,
    admission: &AdmissionBatch,
    scratch: &mut crate::rocksdb_store::contribution_codec::ContributionScratch,
) -> StoreResult<super::write::AdmissionExecutionStats> {
    for attempt in 0..4 {
        if attempt > 0 {
            let delay_ms = 1u64 << (attempt - 1);
            std::thread::sleep(std::time::Duration::from_millis(delay_ms));
        }
        match admit_physical_batch(store, schema, admission, scratch) {
            Ok(mut stats) => {
                stats.attempts = attempt + 1;
                return Ok(stats);
            }
            Err(error) if error.is_contention() && attempt < 3 => continue,
            Err(error) => return Err(error),
        }
    }
    unreachable!("bounded admission retry loop always returns")
}

fn resolve_requested_workers(
    name: &str,
    requested: Option<usize>,
    default: usize,
    available: usize,
) -> StoreResult<usize> {
    let resolved = requested.unwrap_or(default);
    if resolved == 0 {
        return Err(StoreError::State(format!("{name} must be positive")));
    }
    if resolved > available {
        return Err(StoreError::State(format!(
            "{name}={resolved} exceeds available_parallelism={available}"
        )));
    }
    Ok(resolved)
}

fn elapsed_ns(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX)
}

fn read_discovered_files(store: &RocksStore) -> StoreResult<Vec<FileEntry>> {
    let view = store.view()?;
    let mut iterator = view.namespace_prefix(super::namespace::Namespace::Files, b"")?;
    iterator.seek_to_first();
    let mut files = Vec::new();
    while iterator.valid() {
        let bytes = iterator
            .value()
            .ok_or_else(|| StoreError::CorruptIndex("missing file metadata value".into()))?;
        let metadata: value_codec::FileMetadata = value_codec::decode(bytes, "file metadata")?;
        files.push(metadata.entry);
        iterator.next();
    }
    iterator.status()?;
    Ok(files)
}

fn preparation_jobs(files: &[FileEntry]) -> Vec<PreparationJob> {
    let mut jobs = Vec::new();
    let mut start = 0usize;
    while start < files.len() {
        let minimum = if start == 0 { FIRST_JOB_FILES } else { 1 };
        let mut end = start;
        let mut bytes = 0u64;
        while end < files.len() && end - start < MAX_JOB_FILES {
            let size = files[end].revision.size;
            if end > start && bytes.saturating_add(size) > MAX_JOB_BYTES {
                break;
            }
            bytes = bytes.saturating_add(size);
            end += 1;
            if end - start >= minimum && bytes >= MAX_JOB_BYTES {
                break;
            }
        }
        jobs.push(PreparationJob {
            index: jobs.len(),
            files: files[start..end].to_vec(),
            source_bytes: bytes,
        });
        start = end;
    }
    jobs
}

fn prepare_job(
    root: &Path,
    schema: &RuntimeSchema,
    job: &PreparationJob,
    source_files: &[SourceFileInput],
    alphabet: BigramAlphabet,
    profile: bool,
    prepare_workers: usize,
) -> Result<PreparedChunk, String> {
    let started = Instant::now();
    let mut by_file = BTreeMap::<u32, Vec<(CollectionId, PreparedStoreBatch)>>::new();
    for file in &job.files {
        by_file.entry(file.id.0).or_default();
    }
    let mut work_profile = BuildWorkProfile::default();
    let mut records = 0u64;
    let mut record_parses = 0u64;
    let mut failed_parses = 0u64;
    let mut source_reads = 0u64;
    let mut source_bytes_read = 0u64;
    for collection in &schema.collections {
        let selected = job
            .files
            .iter()
            .filter(|file| file.collections.contains(&collection.id))
            .map(|file| file.id)
            .collect::<Vec<_>>();
        if selected.is_empty() {
            continue;
        }
        let batch = process_store_batch_profiled(
            root,
            &collection.definition,
            collection.id,
            &collection.source,
            &selected,
            source_files,
            alphabet,
            InferenceConfig::default(),
            prepare_workers,
            profile,
        )?;
        records = records.saturating_add(batch.records.len() as u64);
        record_parses = record_parses.saturating_add(batch.record_parses);
        failed_parses = failed_parses.saturating_add(batch.failed_parses);
        source_reads = source_reads.saturating_add(batch.source_reads);
        source_bytes_read = source_bytes_read.saturating_add(batch.source_bytes_read);
        work_profile.absorb(&batch.profile);
        for (file, batch) in split_prepared_batch(batch)? {
            by_file
                .get_mut(&file)
                .ok_or_else(|| "prepared batch contains an unselected file".to_string())?
                .push((collection.id, batch));
        }
    }
    let mut files = Vec::with_capacity(job.files.len());
    for file in &job.files {
        let collections = by_file
            .remove(&file.id.0)
            .ok_or_else(|| format!("prepared file {} has no collection batch", file.id.0))?;
        for collection in &file.collections {
            if !collections.iter().any(|(actual, _)| actual == collection) {
                return Err(format!(
                    "prepared file {} is missing collection {}",
                    file.id.0, collection.0
                ));
            }
        }
        files.push(PreparedPhysical {
            file: file.clone(),
            collections,
        });
    }
    Ok(PreparedChunk {
        index: job.index,
        files,
        preparation_ns: elapsed_ns(started),
        profile: work_profile,
        records,
        record_parses,
        failed_parses,
        source_reads,
        source_bytes_read,
    })
}

fn split_prepared_batch(
    batch: PreparedStoreBatch,
) -> Result<Vec<(u32, PreparedStoreBatch)>, String> {
    if batch.inference.is_some() {
        return Err("inference is unsupported by the RocksDB prototype".into());
    }
    let count = batch.files.len();
    if count == 0 {
        return Ok(Vec::new());
    }
    let mut file_ranges = HashMap::<u32, (usize, usize, usize)>::new();
    for (index, file) in batch.files.iter().enumerate() {
        let entry = (index, file.records.start, file.records.end);
        if file_ranges.insert(file.entry.id.0, entry).is_some() {
            return Err("duplicate physical file in prepared batch".into());
        }
    }
    let mut slot_to_file = vec![usize::MAX; batch.records.len()];
    for (output, file) in batch.files.iter().enumerate() {
        if file.records.end > batch.records.len() || file.records.start > file.records.end {
            return Err("prepared file record range is invalid".into());
        }
        for slot in file.records.clone() {
            if slot_to_file[slot] != usize::MAX {
                return Err("overlapping prepared file record ranges".into());
            }
            slot_to_file[slot] = output;
        }
    }

    let descriptors = std::sync::Arc::clone(&batch.descriptors);
    let mut records = (0..count).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut values = (0..count).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut lengths = (0..count).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut states = (0..count).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut sorts = (0..count).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut relations = (0..count).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut relation_states = (0..count).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut grams = (0..count).map(|_| Vec::new()).collect::<Vec<_>>();
    let mut logical = (0..count).map(|_| RoaringBitmap::new()).collect::<Vec<_>>();
    let mut empty_arrays = (0..count).map(|_| RoaringBitmap::new()).collect::<Vec<_>>();

    for mut record in batch.records {
        let old = record.slot.0 as usize;
        let output = *slot_to_file
            .get(old)
            .ok_or_else(|| "record slot outside prepared batch".to_string())?;
        if output == usize::MAX {
            return Err("record is not assigned to a prepared file".into());
        }
        let start = batch.files[output].records.start;
        record.slot = RecordSlot(u32::try_from(old - start).map_err(|_| "record slot overflow")?);
        records[output].push(record);
    }
    for mut value in batch.values {
        let output = slot_to_file[value.slot as usize];
        value.slot -= batch.files[output].records.start as u32;
        values[output].push(value);
    }
    for mut length in batch.lengths {
        let output = slot_to_file[length.slot as usize];
        length.slot -= batch.files[output].records.start as u32;
        lengths[output].push(length);
    }
    for mut state in batch.states {
        let output = slot_to_file[state.slot as usize];
        state.slot -= batch.files[output].records.start as u32;
        states[output].push(state);
    }
    for mut sort in batch.sorts {
        let output = slot_to_file[sort.slot as usize];
        sort.slot -= batch.files[output].records.start as u32;
        sorts[output].push(sort);
    }
    for mut relation in batch.relations {
        let output = slot_to_file[relation.slot as usize];
        relation.slot -= batch.files[output].records.start as u32;
        relations[output].push(relation);
    }
    for mut state in batch.relation_states {
        let output = slot_to_file[state.slot as usize];
        state.slot -= batch.files[output].records.start as u32;
        relation_states[output].push(state);
    }
    for mut gram in batch.grams {
        let output = slot_to_file[gram.slot as usize];
        gram.slot -= batch.files[output].records.start as u32;
        grams[output].push(gram);
    }
    for slot in batch.logical_records.iter() {
        let output = slot_to_file[slot as usize];
        logical[output].insert(slot - batch.files[output].records.start as u32);
    }
    for slot in batch.empty_array_records.iter() {
        let output = slot_to_file[slot as usize];
        empty_arrays[output].insert(slot - batch.files[output].records.start as u32);
    }

    let mut outputs = Vec::with_capacity(count);
    for (index, file) in batch.files.into_iter().enumerate() {
        let file_id = file.entry.id;
        let fingerprint = batch
            .file_fingerprints
            .iter()
            .find(|(id, _)| *id == file.entry.id)
            .map(|(_, fingerprint)| *fingerprint);
        let retained_capacity_bytes = estimate_retained(
            &records[index],
            &values[index],
            &lengths[index],
            &states[index],
            &sorts[index],
            &relations[index],
            &relation_states[index],
            &grams[index],
        );
        outputs.push((
            file.entry.id.0,
            PreparedStoreBatch {
                files: vec![crate::build::BatchFile {
                    entry: file.entry,
                    records: 0..records[index].len(),
                    first_range: true,
                }],
                file_fingerprints: fingerprint
                    .map(|fingerprint| vec![(file_id, fingerprint)])
                    .unwrap_or_default(),
                descriptors: std::sync::Arc::clone(&descriptors),
                records: std::mem::take(&mut records[index]),
                values: std::mem::take(&mut values[index]),
                lengths: std::mem::take(&mut lengths[index]),
                states: std::mem::take(&mut states[index]),
                sorts: std::mem::take(&mut sorts[index]),
                relations: std::mem::take(&mut relations[index]),
                relation_states: std::mem::take(&mut relation_states[index]),
                grams: std::mem::take(&mut grams[index]),
                inference: None,
                logical_records: std::mem::take(&mut logical[index]),
                empty_array_records: std::mem::take(&mut empty_arrays[index]),
                record_parses: 0,
                failed_parses: 0,
                source_reads: 0,
                source_bytes_read: 0,
                profile: if index == 0 {
                    batch.profile.clone()
                } else {
                    BuildWorkProfile::default()
                },
                retained_capacity_bytes,
            },
        ));
    }
    Ok(outputs)
}

fn estimate_retained(
    records: &[crate::inventory::RecordEntry],
    values: &[super::prepare::PreparedValue],
    lengths: &[super::prepare::PreparedLength],
    states: &[super::prepare::PreparedState],
    sorts: &[super::prepare::PreparedSort],
    relations: &[super::prepare::PreparedRelation],
    relation_states: &[super::prepare::PreparedRelationState],
    grams: &[super::prepare::PreparedGramSet],
) -> usize {
    records
        .len()
        .saturating_mul(std::mem::size_of::<crate::inventory::RecordEntry>())
        .saturating_add(
            values
                .len()
                .saturating_mul(std::mem::size_of::<super::prepare::PreparedValue>()),
        )
        .saturating_add(
            lengths
                .len()
                .saturating_mul(std::mem::size_of::<super::prepare::PreparedLength>()),
        )
        .saturating_add(
            states
                .len()
                .saturating_mul(std::mem::size_of::<super::prepare::PreparedState>()),
        )
        .saturating_add(
            sorts
                .len()
                .saturating_mul(std::mem::size_of::<super::prepare::PreparedSort>()),
        )
        .saturating_add(
            relations
                .len()
                .saturating_mul(std::mem::size_of::<super::prepare::PreparedRelation>()),
        )
        .saturating_add(
            relation_states
                .len()
                .saturating_mul(std::mem::size_of::<super::prepare::PreparedRelationState>()),
        )
        .saturating_add(
            grams
                .len()
                .saturating_mul(std::mem::size_of::<super::prepare::PreparedGramSet>()),
        )
        .saturating_add(
            grams
                .iter()
                .map(|grams| grams.grams.len() * std::mem::size_of::<u64>())
                .sum::<usize>(),
        )
}

fn merge_file_entry(
    original: &FileEntry,
    collections: &[(CollectionId, PreparedStoreBatch)],
) -> FileEntry {
    let mut merged = original.clone();
    merged.status = FileStatus::Valid;
    merged.diagnostics.clear();
    merged.omitted_diagnostics = 0;
    for (_, batch) in collections {
        for file in &batch.files {
            for diagnostic in &file.entry.diagnostics {
                if !merged
                    .diagnostics
                    .iter()
                    .any(|existing| existing == diagnostic)
                {
                    merged.diagnostics.push(diagnostic.clone());
                }
            }
            merged.status = merged.status.max(file.entry.status);
            merged.omitted_diagnostics = merged
                .omitted_diagnostics
                .saturating_add(file.entry.omitted_diagnostics);
        }
    }
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{FieldPath, LogicalType};
    use crate::reference::FieldDefinition;
    use crate::repository::CollectionDefinition;
    use crate::rocksdb_store::keys;
    use crate::rocksdb_store::value_codec::CollectionMetadata;
    use crate::source::{SourceFormat, SourceRegistry};
    use crate::workload::BigramAlphabet;

    #[test]
    fn direct_build_admits_overlapping_collections_and_completes() {
        let root = std::env::temp_dir().join(format!(
            "objekte-rocksdb-build-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(
            root.join("data/a.jsonl"),
            b"{\"title\":\"alpha\",\"count\":1,\"tags\":[\"a\",\"b\"]}\n{\"title\":\"beta\",\"count\":2,\"tags\":[]}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("data/b.jsonl"),
            b"{\"title\":\"gamma\",\"count\":3,\"tags\":[\"c\"]}\n",
        )
        .unwrap();
        std::fs::write(
            root.join("data/c.jsonl"),
            b"{\"title\":\"delta\",\"count\":4,\"tags\":[\"d\"]}\n",
        )
        .unwrap();
        let mut definitions = vec![
            CollectionDefinition::folder("first", "data", SourceFormat::JsonLines),
            CollectionDefinition::folder("second", "data", SourceFormat::JsonLines),
        ];
        for definition in &mut definitions {
            for (name, kind, multi) in [
                ("title", LogicalType::String, false),
                ("count", LogicalType::Number, false),
                (
                    "tags",
                    LogicalType::Array(Box::new(LogicalType::String)),
                    true,
                ),
            ] {
                definition
                    .schema
                    .register_field(FieldDefinition {
                        path: vec![name.into()],
                        kind,
                        required: false,
                        nullable: true,
                        indexed: true,
                        multi,
                        element_nullable: Vec::new(),
                    })
                    .unwrap();
            }
            definition.sort_fields = vec![FieldPath::parse("title").unwrap()];
        }
        let config = StoreConfig {
            location: super::super::StoreLocation::Memory,
            configuration_digest: [7; 32],
            alphabet: "ascii".into(),
            source_root: crate::path_repr::native_bytes(&root),
            build_identity: [8; 32],
        };
        let built = build_from_source(
            config,
            &root,
            &definitions,
            &SourceRegistry::builtin(),
            BigramAlphabet::Ascii,
            DirectBuildOptions {
                prepare_workers: Some(1),
                write_workers: Some(2),
                ..DirectBuildOptions::default()
            },
        )
        .unwrap();
        let view = built.store.view().unwrap();
        assert_eq!(
            view.header().phase,
            super::super::value_codec::BuildPhase::Complete
        );
        assert_eq!(built.report.counts.physical_files, 3);
        assert_eq!(built.report.counts.memberships, 6);
        assert_eq!(built.report.counts.records, 8);
        assert_eq!(built.report.finalization.admission_receipts, 2);
        assert_eq!(built.report.finalization.coverage_delta_rows, 12);
        assert_eq!(built.report.finalization.coverage_rows, 6);
        for collection in 0..2 {
            let metadata: CollectionMetadata = value_codec::decode(
                &view
                    .get(
                        super::super::namespace::Namespace::Collections,
                        &keys::collection(collection),
                    )
                    .unwrap()
                    .unwrap(),
                "collection metadata",
            )
            .unwrap();
            assert_eq!(metadata.live_records, 4);
            assert_eq!(metadata.discovered_files, 3);
            assert_eq!(metadata.indexed_files, 3);
            assert_eq!(metadata.pending_files, 0);
        }
        let mut pending = view
            .namespace_prefix(super::super::namespace::Namespace::PendingFiles, b"")
            .unwrap();
        pending.seek_to_first();
        assert!(!pending.valid());
        pending.status().unwrap();
        let validation = super::super::validation::validate(&built.store).unwrap();
        assert!(validation.is_valid(), "{:?}", validation.errors);
        assert_eq!(validation.records, 8);
        assert_eq!(validation.receipts, 2);
        assert!(validation.gram_memberships > 0);
        let native = crate::repository::Repository::open_with_options(
            &root,
            definitions.clone(),
            crate::repository::RepositoryOptions {
                bigram_alphabet: BigramAlphabet::Ascii,
                build_workers: std::num::NonZeroUsize::new(1),
                ..crate::repository::RepositoryOptions::default()
            },
        )
        .unwrap();
        let native_facts = super::super::reference::native_reference(&native).unwrap();
        let store_facts =
            super::super::reference::store_reference(&built.store, &definitions).unwrap();
        let comparison = super::super::reference::compare_reference(&native_facts, &store_facts);
        assert!(comparison.equal, "{comparison:?}");
        drop(pending);
        drop(view);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_build_handles_empty_and_malformed_jsonl_files() {
        let root = std::env::temp_dir().join(format!(
            "objekte-rocksdb-malformed-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(root.join("data/empty.jsonl"), b"").unwrap();
        std::fs::write(root.join("data/bad.jsonl"), b"{not-json}\n").unwrap();
        let definitions = vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::JsonLines,
        )];
        let built = build_from_source(
            StoreConfig {
                location: super::super::StoreLocation::Memory,
                configuration_digest: [9; 32],
                alphabet: "ascii".into(),
                source_root: crate::path_repr::native_bytes(&root),
                build_identity: [10; 32],
            },
            &root,
            &definitions,
            &SourceRegistry::builtin(),
            BigramAlphabet::Ascii,
            DirectBuildOptions {
                prepare_workers: Some(1),
                write_workers: Some(1),
                ..DirectBuildOptions::default()
            },
        )
        .unwrap();
        assert_eq!(built.report.counts.physical_files, 2);
        assert_eq!(built.report.counts.records, 1);
        let validation = super::super::validation::validate(&built.store).unwrap();
        assert!(validation.is_valid(), "{:?}", validation.errors);
        let native = crate::repository::Repository::open_with_options(
            &root,
            definitions.clone(),
            crate::repository::RepositoryOptions {
                bigram_alphabet: BigramAlphabet::Ascii,
                build_workers: std::num::NonZeroUsize::new(1),
                ..crate::repository::RepositoryOptions::default()
            },
        )
        .unwrap();
        let native_facts = super::super::reference::native_reference(&native).unwrap();
        let store_facts =
            super::super::reference::store_reference(&built.store, &definitions).unwrap();
        let comparison = super::super::reference::compare_reference(&native_facts, &store_facts);
        assert!(comparison.equal, "{comparison:?}");
        drop(native);
        drop(built);
        std::fs::remove_dir_all(root).unwrap();
    }
}
