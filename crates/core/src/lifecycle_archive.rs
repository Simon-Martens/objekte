//! Explicit conversion from native publication dictionaries to checked
//! in-memory archives.

use std::num::NonZeroUsize;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::AtomicU8;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use serde::Serialize;

use crate::archive_storage::take_construction_timing;
#[cfg(test)]
use crate::archive_storage::{ArchiveTestFaultControl, with_test_fault_control};
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedSegment};
use crate::lifecycle_catalog::{CatalogShardJob, CatalogShardResult, FileCatalog};
use crate::native_segment::IndexedCollectionSegment;
#[cfg(test)]
use crate::native_segment::SegmentConversionMismatch;
use crate::repository::{CollectionState, Repository};
use crate::storage_usage::StorageWorkCounters;

#[cfg(test)]
pub(crate) struct ArchiveTestGate {
    state: std::sync::Mutex<ArchiveTestGateState>,
    changed: std::sync::Condvar,
}

#[cfg(test)]
pub(crate) struct ArchiveJobTestGate {
    state: std::sync::Mutex<ArchiveJobTestGateState>,
    changed: std::sync::Condvar,
}

#[cfg(test)]
#[derive(Default)]
struct ArchiveJobTestGateState {
    arrived: usize,
    collection_arrivals: usize,
    catalog_arrivals: usize,
    segment_arrivals: usize,
    released: bool,
}

#[cfg(test)]
impl ArchiveJobTestGate {
    pub(crate) fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(ArchiveJobTestGateState::default()),
            changed: std::sync::Condvar::new(),
        }
    }

    fn arrive(&self, collection: bool, catalog: bool, segment: bool) {
        let mut state = self.state.lock().expect("archive job gate lock poisoned");
        state.arrived += 1;
        state.collection_arrivals += usize::from(collection);
        state.catalog_arrivals += usize::from(catalog);
        state.segment_arrivals += usize::from(segment);
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .expect("archive job gate lock poisoned");
        }
    }

    pub(crate) fn wait_for_arrived(&self, count: usize) {
        let mut state = self.state.lock().expect("archive job gate lock poisoned");
        while state.arrived < count {
            state = self
                .changed
                .wait(state)
                .expect("archive job gate lock poisoned");
        }
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("archive job gate lock poisoned");
        state.released = true;
        self.changed.notify_all();
    }

    pub(crate) fn segment_arrivals(&self) -> usize {
        self.state
            .lock()
            .expect("archive job gate lock poisoned")
            .segment_arrivals
    }

    pub(crate) fn collection_arrivals(&self) -> usize {
        self.state
            .lock()
            .expect("archive job gate lock poisoned")
            .collection_arrivals
    }

    pub(crate) fn catalog_arrivals(&self) -> usize {
        self.state
            .lock()
            .expect("archive job gate lock poisoned")
            .catalog_arrivals
    }
}

#[cfg(test)]
#[derive(Default)]
struct ArchiveTestGateState {
    arrived: bool,
    released: bool,
}

#[cfg(test)]
impl ArchiveTestGate {
    pub(crate) fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(ArchiveTestGateState::default()),
            changed: std::sync::Condvar::new(),
        }
    }

    pub(crate) fn wait_for_arrival(&self) {
        let mut state = self.state.lock().expect("archive gate lock poisoned");
        while !state.arrived {
            state = self
                .changed
                .wait(state)
                .expect("archive gate lock poisoned");
        }
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("archive gate lock poisoned");
        state.released = true;
        self.changed.notify_all();
    }

    fn pause(&self) {
        let mut state = self.state.lock().expect("archive gate lock poisoned");
        state.arrived = true;
        self.changed.notify_all();
        while !state.released {
            state = self
                .changed
                .wait(state)
                .expect("archive gate lock poisoned");
        }
    }
}

#[cfg(test)]
thread_local! {
    static TEST_ARCHIVE_GATE: std::cell::RefCell<Option<Arc<ArchiveTestGate>>> =
        const { std::cell::RefCell::new(None) };
    static TEST_ARCHIVE_JOB_GATE: std::cell::RefCell<Option<Arc<ArchiveJobTestGate>>> =
        const { std::cell::RefCell::new(None) };
    static TEST_ARCHIVE_SEGMENT_MISMATCH: std::cell::RefCell<Option<Arc<AtomicU8>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn install_test_archive_gate() -> Arc<ArchiveTestGate> {
    let gate = Arc::new(ArchiveTestGate::new());
    TEST_ARCHIVE_GATE.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&gate)));
    gate
}

#[cfg(test)]
pub(crate) fn clear_test_archive_gate() {
    TEST_ARCHIVE_GATE.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(test)]
pub(crate) fn install_test_archive_job_gate() -> Arc<ArchiveJobTestGate> {
    let gate = Arc::new(ArchiveJobTestGate::new());
    TEST_ARCHIVE_JOB_GATE.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&gate)));
    gate
}

#[cfg(test)]
pub(crate) fn clear_test_archive_job_gate() {
    TEST_ARCHIVE_JOB_GATE.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(test)]
pub(crate) fn install_test_archive_segment_mismatch(
    mismatch: SegmentConversionMismatch,
) -> Arc<AtomicU8> {
    let control = Arc::new(AtomicU8::new(match mismatch {
        SegmentConversionMismatch::FileLookup => 1,
        SegmentConversionMismatch::IdLookup => 2,
    }));
    TEST_ARCHIVE_SEGMENT_MISMATCH.with(|slot| *slot.borrow_mut() = Some(Arc::clone(&control)));
    control
}

#[cfg(test)]
pub(crate) fn clear_test_archive_segment_mismatch() {
    TEST_ARCHIVE_SEGMENT_MISMATCH.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(test)]
fn test_archive_segment_mismatch() -> Option<Arc<AtomicU8>> {
    TEST_ARCHIVE_SEGMENT_MISMATCH.with(|slot| slot.borrow().clone())
}

#[cfg(test)]
fn maybe_pause_archive() {
    let gate = TEST_ARCHIVE_GATE.with(|slot| slot.borrow_mut().take());
    if let Some(gate) = gate {
        gate.pause();
    }
}

#[cfg(not(test))]
fn maybe_pause_archive() {}

#[cfg(test)]
thread_local! {
    static TEST_ARCHIVE_FAULT_CONTROL:
        std::cell::RefCell<Option<Arc<ArchiveTestFaultControl>>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
pub(crate) fn install_test_archive_fault_control(control: Arc<ArchiveTestFaultControl>) {
    TEST_ARCHIVE_FAULT_CONTROL.with(|slot| *slot.borrow_mut() = Some(control));
}

#[cfg(test)]
pub(crate) fn clear_test_archive_fault_control() {
    TEST_ARCHIVE_FAULT_CONTROL.with(|slot| {
        slot.borrow_mut().take();
    });
}

#[cfg(test)]
fn test_archive_fault_control() -> Option<Arc<ArchiveTestFaultControl>> {
    TEST_ARCHIVE_FAULT_CONTROL.with(|slot| slot.borrow().clone())
}

#[cfg(test)]
fn operation_injected_failure(stage: crate::archive_storage::ArchiveFailureStage) -> bool {
    test_archive_fault_control().is_some_and(|control| {
        with_test_fault_control(control, || {
            crate::archive_storage::injected_test_failure(stage)
        })
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum ArchiveRepresentation {
    Native,
    Mixed,
    Archived,
}

impl ArchiveRepresentation {
    fn from_counts(native: usize, archived: usize) -> Self {
        match (native, archived) {
            (_, 0) => Self::Native,
            (0, _) => Self::Archived,
            _ => Self::Mixed,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArchiveOptions {
    worker_limit: NonZeroUsize,
}

impl ArchiveOptions {
    pub fn new(worker_limit: usize) -> Result<Self, IndexError> {
        let worker_limit = NonZeroUsize::new(worker_limit).ok_or_else(|| {
            IndexError::ResourceLimit("archive worker limit must be nonzero".into())
        })?;
        Ok(Self { worker_limit })
    }

    pub fn worker_limit(self) -> usize {
        self.worker_limit.get()
    }
}

impl Default for ArchiveOptions {
    fn default() -> Self {
        Self {
            worker_limit: NonZeroUsize::new(1).expect("one is nonzero"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ArchiveOptions;

    #[test]
    fn archive_options_reject_zero_and_default_to_one_worker() {
        assert!(ArchiveOptions::new(0).is_err());
        assert_eq!(ArchiveOptions::default().worker_limit(), 1);
        assert_eq!(ArchiveOptions::new(8).unwrap().worker_limit(), 8);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ArchiveStatus {
    pub representation: ArchiveRepresentation,
    pub representation_generation: u64,
    pub native_components: usize,
    pub archived_components: usize,
    pub native_storage_owners: usize,
    pub archived_storage_owners: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct ArchiveReport {
    pub index_revision: IndexRevision,
    pub instance_id: u64,
    pub representation_before: ArchiveRepresentation,
    pub representation_after: ArchiveRepresentation,
    pub representation_generation_before: u64,
    pub representation_generation_after: u64,
    pub converted_components: usize,
    pub reused_components: usize,
    pub converted_storage_owners: usize,
    pub reused_storage_owners: usize,
    pub native_components: usize,
    pub archived_components: usize,
    pub native_storage_owners: usize,
    pub archived_storage_owners: usize,
    pub newly_serialized_bytes: u64,
    pub newly_capacity_bytes: u64,
    pub current_serialized_bytes: u64,
    pub current_capacity_bytes: u64,
    /// Reachable payloads intentionally retained in native form by this task.
    pub residual_native_bigram_bytes: u64,
    pub residual_native_sorting_bytes: u64,
    pub residual_native_relation_bytes: u64,
    pub residual_native_control_metadata_bytes: u64,
    pub total_ns: u64,
    pub transition_wait_ns: u64,
    pub preparation_ns: u64,
    pub installation_ns: u64,
    pub retirement_ns: u64,
    pub serializer_work_ns_sum: u64,
    pub validation_work_ns_sum: u64,
    pub worker_limit: usize,
    pub peak_active_conversion_jobs: usize,
}

fn status_from_counts(
    generation: u64,
    (native, archived): (usize, usize),
    archived_storage_owners: usize,
) -> ArchiveStatus {
    ArchiveStatus {
        representation: ArchiveRepresentation::from_counts(native, archived),
        representation_generation: generation,
        native_components: native,
        archived_components: archived,
        // Logical native components are the only native query-owner registry
        // retained by this prototype; archived owners are deduplicated by the
        // checked-owner accounting traversal.
        native_storage_owners: native,
        archived_storage_owners,
    }
}

enum PreparationJob<'a> {
    Collection(usize, &'a CollectionState),
    Catalog(CatalogShardJob<'a>),
    Segment(usize, Arc<IndexedCollectionSegment>),
}

enum PreparationResult {
    Collection(usize, CollectionState),
    Catalog(CatalogShardResult),
    Segment(usize, Arc<IndexedCollectionSegment>),
}

struct PreparationOutput {
    collections: Vec<CollectionState>,
    catalogs: std::collections::HashMap<usize, CatalogShardResult>,
    segments: std::collections::HashMap<usize, Arc<IndexedCollectionSegment>>,
    serializer_ns: u64,
    validation_ns: u64,
    peak_jobs: usize,
}

fn execute_preparation_job(
    job: &PreparationJob<'_>,
    active_jobs: &AtomicUsize,
    peak_jobs: &AtomicUsize,
    #[cfg(test)] fault_control: Option<Arc<ArchiveTestFaultControl>>,
    #[cfg(test)] job_gate: Option<Arc<ArchiveJobTestGate>>,
    #[cfg(test)] segment_mismatch: Option<Arc<AtomicU8>>,
) -> Result<
    (
        PreparationResult,
        crate::archive_storage::ArchiveConstructionTiming,
        StorageWorkCounters,
    ),
    String,
> {
    let prior_work = StorageWorkCounters::take();
    let active = active_jobs.fetch_add(1, Ordering::AcqRel) + 1;
    peak_jobs.fetch_max(active, Ordering::AcqRel);
    #[cfg(test)]
    if let Some(gate) = job_gate.as_ref() {
        gate.arrive(
            matches!(job, PreparationJob::Collection(_, _)),
            matches!(job, PreparationJob::Catalog(_)),
            matches!(job, PreparationJob::Segment(_, _)),
        );
    }
    let _ = take_construction_timing();
    let convert = || -> Result<PreparationResult, String> {
        Ok(match job {
            PreparationJob::Collection(index, collection) => {
                PreparationResult::Collection(*index, collection.archived_candidate()?)
            }
            PreparationJob::Catalog(job) => PreparationResult::Catalog(job.convert()?),
            PreparationJob::Segment(owner, index) => {
                #[cfg(test)]
                let mismatch = segment_mismatch.as_ref().map(|mismatch| {
                    match mismatch.load(Ordering::Acquire) {
                        1 => SegmentConversionMismatch::FileLookup,
                        2 => SegmentConversionMismatch::IdLookup,
                        _ => {
                            panic!("invalid test segment mismatch control")
                        }
                    }
                });
                #[cfg(test)]
                let candidate = index.archived_candidate_with_mismatch(mismatch);
                #[cfg(not(test))]
                let candidate = index.archived_candidate();
                let index = candidate.map_err(|error| {
                    format!(
                        "segment {} in collection {}: {error}",
                        index.id, index.collection.0
                    )
                })?;
                #[cfg(test)]
                if crate::archive_storage::injected_test_failure(
                    crate::archive_storage::ArchiveFailureStage::SemanticValidation,
                ) {
                    return Err("injected archive semantic-validation failure".into());
                }
                PreparationResult::Segment(*owner, Arc::new(index))
            }
        })
    };
    #[cfg(test)]
    let result = if let Some(control) = fault_control {
        with_test_fault_control(control, convert)
    } else {
        convert()
    };
    #[cfg(not(test))]
    let result = convert();
    let timing = take_construction_timing();
    let work = StorageWorkCounters::take();
    StorageWorkCounters::commit_merge(prior_work);
    active_jobs.fetch_sub(1, Ordering::AcqRel);
    result.map(|result| (result, timing, work))
}

fn prepare_archive_jobs(
    collections: &[CollectionState],
    catalogs: &[&FileCatalog],
    segment_groups: &[Arc<Vec<Arc<PublishedSegment>>>],
    options: ArchiveOptions,
) -> Result<PreparationOutput, String> {
    let mut jobs = collections
        .iter()
        .enumerate()
        .map(|(index, collection)| PreparationJob::Collection(index, collection))
        .collect::<Vec<_>>();
    let mut catalog_owners = std::collections::HashSet::new();
    for catalog in catalogs {
        for job in catalog.native_archive_jobs() {
            if catalog_owners.insert(job.owner_id()) {
                jobs.push(PreparationJob::Catalog(job));
            }
        }
    }
    let mut segment_owners = std::collections::HashSet::new();
    for segment in segment_groups.iter().flat_map(|group| group.iter()) {
        let owner = Arc::as_ptr(&segment.index) as usize;
        if segment_owners.insert(owner) && !segment.index.all_storage_archived() {
            jobs.push(PreparationJob::Segment(owner, Arc::clone(&segment.index)));
        }
    }

    let active_jobs = AtomicUsize::new(0);
    let peak_jobs = AtomicUsize::new(0);
    #[cfg(test)]
    let fault_control = test_archive_fault_control();
    #[cfg(test)]
    let job_gate = TEST_ARCHIVE_JOB_GATE.with(|slot| slot.borrow().clone());
    #[cfg(test)]
    let segment_mismatch = test_archive_segment_mismatch();
    let execute = |job: &PreparationJob<'_>| {
        execute_preparation_job(
            job,
            &active_jobs,
            &peak_jobs,
            #[cfg(test)]
            fault_control.clone(),
            #[cfg(test)]
            job_gate.clone(),
            #[cfg(test)]
            segment_mismatch.clone(),
        )
    };
    let results = if jobs.len() <= 1 || options.worker_limit() == 1 {
        jobs.iter().map(execute).collect::<Result<Vec<_>, _>>()?
    } else {
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(options.worker_limit().min(jobs.len()))
            .thread_name(|index| format!("objekte-archive-{index}"))
            .build()
            .map_err(|error| format!("archive worker pool creation failed: {error}"))?;
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.build_pool_creations = counters.build_pool_creations.saturating_add(1);
        });
        let results: Result<Vec<_>, String> =
            pool.install(|| jobs.par_iter().map(execute).collect());
        drop(pool);
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.build_pool_destructions = counters.build_pool_destructions.saturating_add(1);
        });
        results?
    };
    let mut collection_results = vec![None; collections.len()];
    let mut catalog_results = std::collections::HashMap::new();
    let mut segment_results = std::collections::HashMap::new();
    let mut serializer_ns = 0u64;
    let mut validation_ns = 0u64;
    let mut work = StorageWorkCounters::default();
    for (result, timing, job_work) in results {
        serializer_ns = serializer_ns.saturating_add(timing.serializer_ns);
        validation_ns = validation_ns.saturating_add(timing.validation_ns);
        work.merge(job_work);
        match result {
            PreparationResult::Collection(index, collection) => {
                collection_results[index] = Some(collection);
            }
            PreparationResult::Catalog(result) => {
                catalog_results.insert(result.owner_id(), result);
            }
            PreparationResult::Segment(owner, index) => {
                segment_results.insert(owner, index);
            }
        }
    }
    StorageWorkCounters::commit_merge(work);
    Ok(PreparationOutput {
        collections: collection_results
            .into_iter()
            .map(|collection| collection.expect("every collection has one archive result"))
            .collect(),
        catalogs: catalog_results,
        segments: segment_results,
        serializer_ns,
        validation_ns,
        peak_jobs: peak_jobs.load(Ordering::Acquire),
    })
}

fn install_segment_results(
    groups: &mut [Arc<Vec<Arc<PublishedSegment>>>],
    results: &std::collections::HashMap<usize, Arc<IndexedCollectionSegment>>,
) -> Result<(), String> {
    for group in groups.iter_mut() {
        if !group
            .iter()
            .any(|segment| results.contains_key(&(Arc::as_ptr(&segment.index) as usize)))
        {
            continue;
        }
        let group = Arc::make_mut(group);
        for segment in group.iter_mut() {
            let Some(index) = results.get(&(Arc::as_ptr(&segment.index) as usize)) else {
                continue;
            };
            *segment = Arc::new(PublishedSegment::checked(
                segment.id,
                Arc::clone(index),
                segment.active.clone(),
            )?);
        }
    }
    Ok(())
}

impl Index {
    pub fn archive(&self) -> Result<ArchiveReport, IndexError> {
        self.archive_with_options(ArchiveOptions::default())
    }

    pub fn archive_status(&self) -> Result<ArchiveStatus, IndexError> {
        let view = self.view()?;
        Ok(status_from_counts(
            view.0.representation_generation,
            view.0.storage_representation_counts(),
            view.0.storage_usage().archive.owners,
        ))
    }

    pub fn archive_with_options(
        &self,
        options: ArchiveOptions,
    ) -> Result<ArchiveReport, IndexError> {
        let precheck = self.view()?;
        if !precheck.initial_construction_complete() {
            return Err(IndexError::IncompleteBuild);
        }
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(IndexError::Cancelled);
        }

        let started = Instant::now();
        let transition_started = Instant::now();
        let _transition = self
            .0
            .transition
            .write()
            .map_err(|_| IndexError::LockPoisoned)?;
        let transition_wait_ns = transition_started.elapsed().as_nanos() as u64;
        if self.0.quiescing.load(Ordering::Acquire) {
            return Err(IndexError::Cancelled);
        }
        let view = self.view()?;
        if !view.initial_construction_complete() {
            return Err(IndexError::IncompleteBuild);
        }
        let source = Arc::clone(&view.0);
        let before_generation = source.representation_generation;
        let before_counts = source.storage_representation_counts();
        let before_usage = source.storage_usage();
        if before_counts.0 == 0 {
            return Ok(ArchiveReport {
                index_revision: source.revision,
                instance_id: source.instance_id,
                representation_before: ArchiveRepresentation::Archived,
                representation_after: ArchiveRepresentation::Archived,
                representation_generation_before: before_generation,
                representation_generation_after: before_generation,
                converted_components: 0,
                reused_components: before_counts.1,
                converted_storage_owners: 0,
                reused_storage_owners: before_usage.archive.owners,
                native_components: 0,
                archived_components: before_counts.1,
                native_storage_owners: 0,
                archived_storage_owners: before_usage.archive.owners,
                newly_serialized_bytes: 0,
                newly_capacity_bytes: 0,
                current_serialized_bytes: before_usage.archive.serialized_bytes,
                current_capacity_bytes: before_usage.archive.allocated_bytes,
                residual_native_bigram_bytes: before_usage.bigram_payload_bytes,
                residual_native_sorting_bytes: before_usage.sorting_payload_bytes,
                residual_native_relation_bytes: before_usage.relation_payload_bytes,
                residual_native_control_metadata_bytes: before_usage
                    .active_and_control_metadata_bytes,
                total_ns: started.elapsed().as_nanos() as u64,
                transition_wait_ns,
                preparation_ns: 0,
                installation_ns: 0,
                retirement_ns: 0,
                serializer_work_ns_sum: 0,
                validation_work_ns_sum: 0,
                worker_limit: options.worker_limit(),
                peak_active_conversion_jobs: 0,
            });
        }
        maybe_pause_archive();
        let preparation_started = Instant::now();

        let preparation = prepare_archive_jobs(
            &source.repository.collections,
            &[&source.repository.catalog, &source.catalog],
            &source.segments,
            options,
        )
        .map_err(IndexError::InvalidPublication)?;
        let base_catalog = source
            .repository
            .catalog
            .with_converted_shards(&preparation.catalogs)
            .map_err(IndexError::InvalidPublication)?;
        let current_catalog = source
            .catalog
            .with_converted_shards(&preparation.catalogs)
            .map_err(IndexError::InvalidPublication)?;
        let mut repository = source
            .repository
            .archive_shell(preparation.collections, base_catalog);
        repository.files = Arc::new(
            repository
                .files
                .with_published_paths(repository.catalog.path_directory()),
        );
        let mut candidate = (*source).clone();
        candidate.catalog = current_catalog;
        candidate.repository = Arc::new(repository);
        install_segment_results(&mut candidate.segments, &preparation.segments)
            .map_err(IndexError::InvalidPublication)?;
        let serializer_ns = preparation.serializer_ns;
        let validation_ns = preparation.validation_ns;
        let peak_jobs = preparation.peak_jobs;

        candidate.representation_generation = before_generation
            .checked_add(1)
            .ok_or(IndexError::RevisionOverflow)?;
        #[cfg(test)]
        if operation_injected_failure(
            crate::archive_storage::ArchiveFailureStage::FinalCandidateValidation,
        ) {
            return Err(IndexError::InvalidPublication(
                "injected final candidate-validation failure".into(),
            ));
        }
        let after_counts = candidate.storage_representation_counts();
        if after_counts.0 != 0 {
            return Err(IndexError::InvalidPublication(format!(
                "archive candidate retained {} native eligible components",
                after_counts.0
            )));
        }
        let preparation_ns = preparation_started.elapsed().as_nanos() as u64;

        #[cfg(test)]
        if operation_injected_failure(crate::archive_storage::ArchiveFailureStage::PreInstall) {
            return Err(IndexError::InvalidPublication(
                "injected pre-installation failure".into(),
            ));
        }
        let installation_started = Instant::now();
        let retired = {
            let mut published = self
                .0
                .published
                .write()
                .map_err(|_| IndexError::LockPoisoned)?;
            if self.0.quiescing.load(Ordering::Acquire) {
                return Err(IndexError::Cancelled);
            }
            if published.revision != source.revision
                || published.representation_generation != before_generation
            {
                return Err(IndexError::InvalidPublication(
                    "archive source publication changed".into(),
                ));
            }
            std::mem::replace(&mut *published, Arc::new(candidate))
        };
        let retirement_started = Instant::now();
        drop(retired);
        let retirement_ns = retirement_started.elapsed().as_nanos() as u64;
        let installation_ns = installation_started.elapsed().as_nanos() as u64;
        let after_usage = self.view()?.storage_usage();
        let converted_components = after_counts.1.saturating_sub(before_counts.1);
        Ok(ArchiveReport {
            index_revision: source.revision,
            instance_id: source.instance_id,
            representation_before: ArchiveRepresentation::from_counts(
                before_counts.0,
                before_counts.1,
            ),
            representation_after: ArchiveRepresentation::from_counts(
                after_counts.0,
                after_counts.1,
            ),
            representation_generation_before: before_generation,
            representation_generation_after: candidate_generation(before_generation)?,
            converted_components,
            reused_components: before_counts.1,
            converted_storage_owners: converted_components,
            reused_storage_owners: before_usage.archive.owners,
            native_components: after_counts.0,
            archived_components: after_counts.1,
            native_storage_owners: after_counts.0,
            archived_storage_owners: after_usage.archive.owners,
            newly_serialized_bytes: after_usage
                .archive
                .serialized_bytes
                .saturating_sub(before_usage.archive.serialized_bytes),
            newly_capacity_bytes: after_usage
                .archive
                .allocated_bytes
                .saturating_sub(before_usage.archive.allocated_bytes),
            current_serialized_bytes: after_usage.archive.serialized_bytes,
            current_capacity_bytes: after_usage.archive.allocated_bytes,
            residual_native_bigram_bytes: after_usage.bigram_payload_bytes,
            residual_native_sorting_bytes: after_usage.sorting_payload_bytes,
            residual_native_relation_bytes: after_usage.relation_payload_bytes,
            residual_native_control_metadata_bytes: after_usage.active_and_control_metadata_bytes,
            total_ns: started.elapsed().as_nanos() as u64,
            transition_wait_ns,
            preparation_ns,
            installation_ns,
            retirement_ns,
            serializer_work_ns_sum: serializer_ns,
            validation_work_ns_sum: validation_ns,
            worker_limit: options.worker_limit(),
            peak_active_conversion_jobs: peak_jobs,
        })
    }
}

fn candidate_generation(generation: u64) -> Result<u64, IndexError> {
    generation
        .checked_add(1)
        .ok_or(IndexError::RevisionOverflow)
}

impl Repository {
    pub fn archive(&mut self) -> Result<ArchiveReport, IndexError> {
        self.archive_with_options(ArchiveOptions::default())
    }

    pub fn archive_status(&self) -> ArchiveStatus {
        status_from_counts(
            self.representation_generation,
            self.storage_representation_counts(),
            self.storage_usage().archive.owners,
        )
    }

    pub fn archive_with_options(
        &mut self,
        options: ArchiveOptions,
    ) -> Result<ArchiveReport, IndexError> {
        let started = Instant::now();
        let before_generation = self.representation_generation;
        let before_counts = self.storage_representation_counts();
        let before_usage = self.storage_usage();
        if before_counts.0 == 0 {
            return Ok(ArchiveReport {
                index_revision: self.revision,
                instance_id: self.instance_id,
                representation_before: ArchiveRepresentation::Archived,
                representation_after: ArchiveRepresentation::Archived,
                representation_generation_before: before_generation,
                representation_generation_after: before_generation,
                converted_components: 0,
                reused_components: before_counts.1,
                converted_storage_owners: 0,
                reused_storage_owners: before_usage.archive.owners,
                native_components: 0,
                archived_components: before_counts.1,
                native_storage_owners: 0,
                archived_storage_owners: before_usage.archive.owners,
                newly_serialized_bytes: 0,
                newly_capacity_bytes: 0,
                current_serialized_bytes: before_usage.archive.serialized_bytes,
                current_capacity_bytes: before_usage.archive.allocated_bytes,
                residual_native_bigram_bytes: before_usage.bigram_payload_bytes,
                residual_native_sorting_bytes: before_usage.sorting_payload_bytes,
                residual_native_relation_bytes: before_usage.relation_payload_bytes,
                residual_native_control_metadata_bytes: before_usage
                    .active_and_control_metadata_bytes,
                total_ns: started.elapsed().as_nanos() as u64,
                transition_wait_ns: 0,
                preparation_ns: 0,
                installation_ns: 0,
                retirement_ns: 0,
                serializer_work_ns_sum: 0,
                validation_work_ns_sum: 0,
                worker_limit: options.worker_limit(),
                peak_active_conversion_jobs: 0,
            });
        }
        let preparation_started = Instant::now();
        let preparation =
            prepare_archive_jobs(&self.collections, &[&self.catalog], &self.segments, options)
                .map_err(IndexError::InvalidPublication)?;
        let catalog = self
            .catalog
            .with_converted_shards(&preparation.catalogs)
            .map_err(IndexError::InvalidPublication)?;
        let mut candidate = self.archive_shell(preparation.collections, catalog);
        candidate.files = Arc::new(
            candidate
                .files
                .with_published_paths(candidate.catalog.path_directory()),
        );
        install_segment_results(&mut candidate.segments, &preparation.segments)
            .map_err(IndexError::InvalidPublication)?;
        candidate.representation_generation = before_generation
            .checked_add(1)
            .ok_or(IndexError::RevisionOverflow)?;
        #[cfg(test)]
        if operation_injected_failure(
            crate::archive_storage::ArchiveFailureStage::FinalCandidateValidation,
        ) {
            return Err(IndexError::InvalidPublication(
                "injected final candidate-validation failure".into(),
            ));
        }
        let after_counts = candidate.storage_representation_counts();
        if after_counts.0 != 0 {
            return Err(IndexError::InvalidPublication(
                "repository archive retained native eligible components".into(),
            ));
        }
        let preparation_ns = preparation_started.elapsed().as_nanos() as u64;
        let after_usage = candidate.storage_usage();
        #[cfg(test)]
        if operation_injected_failure(crate::archive_storage::ArchiveFailureStage::PreInstall) {
            return Err(IndexError::InvalidPublication(
                "injected pre-installation failure".into(),
            ));
        }
        let revision = candidate.revision;
        let instance_id = candidate.instance_id;
        let representation_generation = candidate.representation_generation;
        *self = candidate;
        Ok(ArchiveReport {
            index_revision: revision,
            instance_id,
            representation_before: ArchiveRepresentation::from_counts(
                before_counts.0,
                before_counts.1,
            ),
            representation_after: ArchiveRepresentation::from_counts(
                after_counts.0,
                after_counts.1,
            ),
            representation_generation_before: before_generation,
            representation_generation_after: representation_generation,
            converted_components: after_counts.1.saturating_sub(before_counts.1),
            reused_components: before_counts.1,
            converted_storage_owners: after_counts.1.saturating_sub(before_counts.1),
            reused_storage_owners: before_usage.archive.owners,
            native_components: after_counts.0,
            archived_components: after_counts.1,
            native_storage_owners: after_counts.0,
            archived_storage_owners: after_usage.archive.owners,
            newly_serialized_bytes: after_usage
                .archive
                .serialized_bytes
                .saturating_sub(before_usage.archive.serialized_bytes),
            newly_capacity_bytes: after_usage
                .archive
                .allocated_bytes
                .saturating_sub(before_usage.archive.allocated_bytes),
            current_serialized_bytes: after_usage.archive.serialized_bytes,
            current_capacity_bytes: after_usage.archive.allocated_bytes,
            residual_native_bigram_bytes: after_usage.bigram_payload_bytes,
            residual_native_sorting_bytes: after_usage.sorting_payload_bytes,
            residual_native_relation_bytes: after_usage.relation_payload_bytes,
            residual_native_control_metadata_bytes: after_usage.active_and_control_metadata_bytes,
            total_ns: started.elapsed().as_nanos() as u64,
            transition_wait_ns: 0,
            preparation_ns,
            installation_ns: 0,
            retirement_ns: 0,
            serializer_work_ns_sum: preparation.serializer_ns,
            validation_work_ns_sum: preparation.validation_ns,
            worker_limit: options.worker_limit(),
            peak_active_conversion_jobs: preparation.peak_jobs,
        })
    }
}
