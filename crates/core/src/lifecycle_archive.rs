//! Explicit conversion from native publication dictionaries to checked
//! in-memory archives.

use std::num::NonZeroUsize;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use serde::Serialize;

use crate::archive_storage::take_construction_timing;
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedSegment};
use crate::repository::Repository;

#[cfg(test)]
pub(crate) struct ArchiveTestGate {
    state: std::sync::Mutex<ArchiveTestGateState>,
    changed: std::sync::Condvar,
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
fn maybe_pause_archive() {
    let gate = TEST_ARCHIVE_GATE.with(|slot| slot.borrow_mut().take());
    if let Some(gate) = gate {
        gate.pause();
    }
}

#[cfg(not(test))]
fn maybe_pause_archive() {}

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
    pub native_components: usize,
    pub archived_components: usize,
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
    pub serializer_work_ns_sum: u64,
    pub validation_work_ns_sum: u64,
    pub worker_limit: usize,
    pub peak_active_conversion_jobs: usize,
}

fn status_from_counts(generation: u64, (native, archived): (usize, usize)) -> ArchiveStatus {
    ArchiveStatus {
        representation: ArchiveRepresentation::from_counts(native, archived),
        representation_generation: generation,
        native_components: native,
        archived_components: archived,
    }
}

fn archive_segment(
    published: &Arc<PublishedSegment>,
    active_jobs: &AtomicUsize,
    peak_jobs: &AtomicUsize,
    serializer_ns: &AtomicU64,
    validation_ns: &AtomicU64,
) -> Result<Arc<PublishedSegment>, String> {
    if published.index.all_storage_archived() {
        return Ok(Arc::clone(published));
    }
    let active = active_jobs.fetch_add(1, Ordering::AcqRel) + 1;
    peak_jobs.fetch_max(active, Ordering::AcqRel);
    let result = (|| {
        let _ = take_construction_timing();
        let mut index = (*published.index).clone();
        index.archive_storage()?;
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
    let convert_group = |group: &mut Arc<Vec<Arc<PublishedSegment>>>| {
        let group = Arc::make_mut(group);
        for published in group.iter_mut() {
            if published.index.all_storage_archived() {
                continue;
            }
            *published = archive_segment(
                published,
                &active_jobs,
                peak_jobs,
                serializer_ns,
                validation_ns,
            )?;
            converted.fetch_add(1, Ordering::AcqRel);
        }
        Ok::<_, String>(())
    };
    if options.worker_limit() == 1 {
        groups.iter_mut().try_for_each(convert_group)?;
        return Ok(());
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(options.worker_limit())
        .thread_name(|index| format!("objekte-archive-{index}"))
        .build()
        .map_err(|error| format!("archive worker pool creation failed: {error}"))?;
    crate::storage_usage::StorageWorkCounters::update(|counters| {
        counters.build_pool_creations = counters.build_pool_creations.saturating_add(1);
    });
    let result = pool.install(|| {
        groups
            .par_iter_mut()
            .try_for_each(|group| convert_group(group))
    });
    drop(pool);
    crate::storage_usage::StorageWorkCounters::update(|counters| {
        counters.build_pool_destructions = counters.build_pool_destructions.saturating_add(1);
    });
    result
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
                native_components: 0,
                archived_components: before_counts.1,
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
        let _ = take_construction_timing();
        repository
            .archive_collection_storage()
            .map_err(IndexError::InvalidPublication)?;
        let base_timing = take_construction_timing();
        let mut serializer_ns = base_timing.serializer_ns;
        let mut validation_ns = base_timing.validation_ns;
        candidate
            .catalog
            .archive_storage()
            .map_err(IndexError::InvalidPublication)?;
        let catalog_timing = take_construction_timing();
        serializer_ns = serializer_ns.saturating_add(catalog_timing.serializer_ns);
        validation_ns = validation_ns.saturating_add(catalog_timing.validation_ns);
        repository.catalog = candidate.catalog.clone();
        candidate.catalog = repository.catalog.clone();
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
        let after_counts = candidate.storage_representation_counts();
        if after_counts.0 != 0 {
            return Err(IndexError::InvalidPublication(
                "archive candidate retained native eligible components".into(),
            ));
        }
        let preparation_ns = preparation_started.elapsed().as_nanos() as u64;

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
        drop(retired);
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
            native_components: after_counts.0,
            archived_components: after_counts.1,
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
                native_components: 0,
                archived_components: before_counts.1,
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
                serializer_work_ns_sum: 0,
                validation_work_ns_sum: 0,
                worker_limit: options.worker_limit(),
                peak_active_conversion_jobs: 0,
            });
        }
        let preparation_started = Instant::now();
        let _ = take_construction_timing();
        self.archive_collection_storage()
            .map_err(IndexError::InvalidPublication)?;
        self.catalog
            .archive_storage()
            .map_err(IndexError::InvalidPublication)?;
        let mut timing = take_construction_timing();
        let converted_segments = AtomicUsize::new(0);
        let peak_jobs = AtomicUsize::new(0);
        let segment_serializer_ns = AtomicU64::new(0);
        let segment_validation_ns = AtomicU64::new(0);
        archive_segment_groups(
            &mut self.segments,
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
        self.representation_generation = before_generation
            .checked_add(1)
            .ok_or(IndexError::RevisionOverflow)?;
        let after_counts = self.storage_representation_counts();
        if after_counts.0 != 0 {
            return Err(IndexError::InvalidPublication(
                "repository archive retained native eligible components".into(),
            ));
        }
        let preparation_ns = preparation_started.elapsed().as_nanos() as u64;
        let after_usage = self.storage_usage();
        Ok(ArchiveReport {
            index_revision: self.revision,
            instance_id: self.instance_id,
            representation_before: ArchiveRepresentation::from_counts(
                before_counts.0,
                before_counts.1,
            ),
            representation_after: ArchiveRepresentation::from_counts(
                after_counts.0,
                after_counts.1,
            ),
            representation_generation_before: before_generation,
            representation_generation_after: self.representation_generation,
            converted_components: after_counts.1.saturating_sub(before_counts.1),
            reused_components: before_counts.1,
            native_components: after_counts.0,
            archived_components: after_counts.1,
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
            serializer_work_ns_sum: timing.serializer_ns,
            validation_work_ns_sum: timing.validation_ns,
            worker_limit: options.worker_limit(),
            peak_active_conversion_jobs: peak_jobs.load(Ordering::Acquire),
        })
    }
}
