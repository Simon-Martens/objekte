//! Explicit conversion from native publication dictionaries to checked
//! in-memory archives.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use serde::Serialize;

use crate::archive_storage::take_construction_timing;
#[cfg(test)]
use crate::archive_storage::{ArchiveTestFaultControl, with_test_fault_control};
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedSegment};
use crate::repository::Repository;

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

    fn arrive(&self) {
        let mut state = self.state.lock().expect("archive job gate lock poisoned");
        state.arrived += 1;
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

fn archive_segment(
    published: &Arc<PublishedSegment>,
    active_jobs: &AtomicUsize,
    peak_jobs: &AtomicUsize,
    serializer_ns: &AtomicU64,
    validation_ns: &AtomicU64,
    #[cfg(test)] fault_control: Option<Arc<ArchiveTestFaultControl>>,
    #[cfg(test)] job_gate: Option<Arc<ArchiveJobTestGate>>,
) -> Result<Arc<PublishedSegment>, String> {
    if published.index.all_storage_archived() {
        return Ok(Arc::clone(published));
    }
    let active = active_jobs.fetch_add(1, Ordering::AcqRel) + 1;
    peak_jobs.fetch_max(active, Ordering::AcqRel);
    #[cfg(test)]
    if let Some(gate) = job_gate.as_ref() {
        gate.arrive();
    }
    let result = (|| {
        let _ = take_construction_timing();
        let mut index = (*published.index).clone();
        let mut archive_storage = || -> Result<(), String> {
            index.archive_storage().map_err(|error| {
                format!(
                    "segment {} in collection {}: {error}",
                    published.id, published.index.collection.0
                )
            })?;
            #[cfg(test)]
            if crate::archive_storage::injected_test_failure(
                crate::archive_storage::ArchiveFailureStage::SemanticValidation,
            ) {
                return Err("injected archive semantic-validation failure".into());
            }
            Ok(())
        };
        #[cfg(test)]
        if let Some(control) = fault_control.clone() {
            with_test_fault_control(control, archive_storage)?;
        } else {
            archive_storage()?;
        }
        #[cfg(not(test))]
        archive_storage()?;
        let timing = take_construction_timing();
        serializer_ns.fetch_add(timing.serializer_ns, Ordering::AcqRel);
        validation_ns.fetch_add(timing.validation_ns, Ordering::AcqRel);
        PublishedSegment::checked(published.id, Arc::new(index), published.active.clone())
            .map(Arc::new)
    })();
    active_jobs.fetch_sub(1, Ordering::AcqRel);
    result
}

fn archive_segment_groups(
    groups: &mut [Arc<Vec<Arc<PublishedSegment>>>],
    options: ArchiveOptions,
    converted: &AtomicUsize,
    peak_jobs: &AtomicUsize,
    serializer_ns: &AtomicU64,
    validation_ns: &AtomicU64,
) -> Result<(), String> {
    let active_jobs = AtomicUsize::new(0);
    #[cfg(test)]
    let fault_control = test_archive_fault_control();
    #[cfg(test)]
    let job_gate = TEST_ARCHIVE_JOB_GATE.with(|slot| slot.borrow().clone());
    let mut seen = std::collections::HashSet::new();
    let mut jobs = Vec::new();
    for (group_index, group) in groups.iter().enumerate() {
        for (segment_index, segment) in group.iter().enumerate() {
            if !segment.index.all_storage_archived() && seen.insert(Arc::as_ptr(segment) as usize) {
                jobs.push((group_index, segment_index, Arc::clone(segment)));
            }
        }
    }
    if jobs.is_empty() {
        return Ok(());
    }
    let convert = |(_, _, published): &(usize, usize, Arc<PublishedSegment>)| {
        archive_segment(
            published,
            &active_jobs,
            peak_jobs,
            serializer_ns,
            validation_ns,
            #[cfg(test)]
            fault_control.clone(),
            #[cfg(test)]
            job_gate.clone(),
        )
    };
    let results = if options.worker_limit() == 1 || jobs.len() == 1 {
        jobs.iter().map(convert).collect::<Result<Vec<_>, _>>()?
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
            pool.install(|| jobs.par_iter().map(convert).collect());
        drop(pool);
        crate::storage_usage::StorageWorkCounters::update(|counters| {
            counters.build_pool_destructions = counters.build_pool_destructions.saturating_add(1);
        });
        results?
    };
    let replacements = jobs
        .iter()
        .zip(results)
        .map(|((_, _, source), result)| (Arc::as_ptr(source) as usize, result))
        .collect::<Vec<_>>();
    converted.fetch_add(replacements.len(), Ordering::AcqRel);
    let replacements = replacements
        .into_iter()
        .collect::<std::collections::HashMap<_, _>>();
    for group in groups.iter_mut() {
        let group = Arc::make_mut(group);
        for segment in group {
            if let Some(replacement) = replacements.get(&(Arc::as_ptr(segment) as usize)) {
                *segment = Arc::clone(replacement);
            }
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

        let mut candidate = (*source).clone();
        let mut repository = source
            .repository
            .archive_candidate()
            .map_err(IndexError::InvalidPublication)?;
        let catalogs_aliased = source
            .repository
            .catalog
            .has_same_storage_owners(&source.catalog);
        let _ = take_construction_timing();
        repository.archive_collection_storage().map_err(|error| {
            IndexError::InvalidPublication(format!("base collection storage: {error}"))
        })?;
        repository.catalog.archive_storage().map_err(|error| {
            IndexError::InvalidPublication(format!("base catalog storage: {error}"))
        })?;
        repository.files = Arc::new(
            repository
                .files
                .with_published_paths(repository.catalog.path_directory()),
        );
        let base_timing = take_construction_timing();
        let mut serializer_ns = base_timing.serializer_ns;
        let mut validation_ns = base_timing.validation_ns;
        if catalogs_aliased {
            candidate.catalog = repository.catalog.clone();
        } else {
            candidate.catalog.archive_storage().map_err(|error| {
                IndexError::InvalidPublication(format!("current catalog storage: {error}"))
            })?;
        }
        let catalog_timing = take_construction_timing();
        serializer_ns = serializer_ns.saturating_add(catalog_timing.serializer_ns);
        validation_ns = validation_ns.saturating_add(catalog_timing.validation_ns);
        candidate.repository = Arc::new(repository);

        let converted_segments = AtomicUsize::new(0);
        let peak_jobs = AtomicUsize::new(0);
        let segment_serializer_ns = AtomicU64::new(0);
        let segment_validation_ns = AtomicU64::new(0);
        archive_segment_groups(
            &mut candidate.segments,
            options,
            &converted_segments,
            &peak_jobs,
            &segment_serializer_ns,
            &segment_validation_ns,
        )
        .map_err(IndexError::InvalidPublication)?;
        serializer_ns = serializer_ns.saturating_add(segment_serializer_ns.load(Ordering::Acquire));
        validation_ns = validation_ns.saturating_add(segment_validation_ns.load(Ordering::Acquire));

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
            peak_active_conversion_jobs: peak_jobs.load(Ordering::Acquire),
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
        let _ = take_construction_timing();
        let mut candidate = self
            .archive_candidate()
            .map_err(IndexError::InvalidPublication)?;
        candidate.archive_collection_storage().map_err(|error| {
            IndexError::InvalidPublication(format!("repository collection storage: {error}"))
        })?;
        candidate.catalog.archive_storage().map_err(|error| {
            IndexError::InvalidPublication(format!("repository catalog storage: {error}"))
        })?;
        candidate.files = Arc::new(
            candidate
                .files
                .with_published_paths(candidate.catalog.path_directory()),
        );
        let mut timing = take_construction_timing();
        let converted_segments = AtomicUsize::new(0);
        let peak_jobs = AtomicUsize::new(0);
        let segment_serializer_ns = AtomicU64::new(0);
        let segment_validation_ns = AtomicU64::new(0);
        archive_segment_groups(
            &mut candidate.segments,
            options,
            &converted_segments,
            &peak_jobs,
            &segment_serializer_ns,
            &segment_validation_ns,
        )
        .map_err(IndexError::InvalidPublication)?;
        timing.serializer_ns = timing
            .serializer_ns
            .saturating_add(segment_serializer_ns.load(Ordering::Acquire));
        timing.validation_ns = timing
            .validation_ns
            .saturating_add(segment_validation_ns.load(Ordering::Acquire));
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
            serializer_work_ns_sum: timing.serializer_ns,
            validation_work_ns_sum: timing.validation_ns,
            worker_limit: options.worker_limit(),
            peak_active_conversion_jobs: peak_jobs.load(Ordering::Acquire),
        })
    }
}
