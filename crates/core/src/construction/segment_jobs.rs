//! Bounded segment finalization and test-visible worker gates.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::lifecycle::IndexError;
use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};
use crate::repository::statistics::IncrementalSegmentProfile;

use crate::index::segment::{IndexedCollectionSegment, SegmentId};
use crate::index::storage::order::DeclaredOrderStore;
use crate::index::storage::segment::build::{SegmentBuilder, SegmentSeal};

pub(crate) struct SegmentJobGate {
    state: std::sync::Mutex<SegmentJobGateState>,
    changed: std::sync::Condvar,
    #[cfg(test)]
    private_drain: std::sync::Mutex<PrivateDrainTestState>,
    #[cfg(test)]
    private_drain_changed: std::sync::Condvar,
}

#[derive(Default)]
struct SegmentJobGateState {
    workers: usize,
    release_workers: bool,
    post_submission: usize,
    release_post_submission: bool,
    post_validation: usize,
    release_post_validation: bool,
}

#[cfg(test)]
#[derive(Default)]
pub(super) struct PrivateDrainTestState {
    next_job: usize,
    failing_job: Option<usize>,
    blocked: std::collections::HashSet<usize>,
    arrived: std::collections::HashSet<usize>,
    released: std::collections::HashSet<usize>,
    completions: usize,
    errors: usize,
    final_drain_entries: usize,
    final_outstanding: Option<usize>,
}

#[allow(dead_code)]
impl SegmentJobGate {
    pub(crate) fn new() -> Self {
        Self {
            state: std::sync::Mutex::new(SegmentJobGateState::default()),
            changed: std::sync::Condvar::new(),
            #[cfg(test)]
            private_drain: std::sync::Mutex::new(PrivateDrainTestState::default()),
            #[cfg(test)]
            private_drain_changed: std::sync::Condvar::new(),
        }
    }

    pub(super) fn wait(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.workers += 1;
        self.changed.notify_all();
        while !state.release_workers {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn wait_for_workers(&self, count: usize) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        while state.workers < count {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn release(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.release_workers = true;
        self.changed.notify_all();
    }

    pub(super) fn wait_after_validation(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.post_validation += 1;
        self.changed.notify_all();
        while !state.release_post_validation {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(super) fn wait_after_submission(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.post_submission += 1;
        self.changed.notify_all();
        while !state.release_post_submission {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn wait_for_post_submission(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        while state.post_submission == 0 {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn release_after_submission(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.release_post_submission = true;
        self.changed.notify_all();
    }

    pub(crate) fn wait_for_post_validation(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        while state.post_validation == 0 {
            state = self
                .changed
                .wait(state)
                .expect("segment gate lock poisoned");
        }
    }

    pub(crate) fn release_after_validation(&self) {
        let mut state = self.state.lock().expect("segment gate lock poisoned");
        state.release_post_validation = true;
        self.changed.notify_all();
    }

    #[cfg(test)]
    pub(crate) fn configure_private_final_drain_failure(
        &self,
        failing_job: usize,
        blocked_jobs: impl IntoIterator<Item = usize>,
    ) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.failing_job = Some(failing_job);
        state.blocked = blocked_jobs.into_iter().collect();
        state.released.clear();
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(super) fn allocate_private_job(&self) -> usize {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        let job = state.next_job;
        state.next_job += 1;
        job
    }

    #[cfg(test)]
    pub(super) fn wait_before_private_completion(&self, job: usize) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.arrived.insert(job);
        self.private_drain_changed.notify_all();
        while state.blocked.contains(&job) && !state.released.contains(&job) {
            state = self
                .private_drain_changed
                .wait(state)
                .expect("private drain gate lock poisoned");
        }
    }

    #[cfg(test)]
    pub(super) fn private_job_fails(&self, job: usize) -> bool {
        self.private_drain
            .lock()
            .expect("private drain gate lock poisoned")
            .failing_job
            == Some(job)
    }

    #[cfg(test)]
    pub(super) fn record_private_completion(&self, error: bool) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.completions += 1;
        state.errors += usize::from(error);
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(super) fn record_private_final_drain(&self) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.final_drain_entries += 1;
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(super) fn record_private_drain_complete(&self, outstanding: usize) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.final_outstanding = Some(outstanding);
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(crate) fn release_private_job(&self, job: usize) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.released.insert(job);
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(crate) fn release_all_private_jobs(&self) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        let blocked = state.blocked.clone();
        state.released.extend(blocked);
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    pub(crate) fn wait_for_private_job(&self, job: usize, timeout: std::time::Duration) -> bool {
        self.wait_for_private_state(timeout, |state| state.arrived.contains(&job))
    }

    #[cfg(test)]
    pub(crate) fn wait_for_private_final_drain(&self, timeout: std::time::Duration) -> bool {
        self.wait_for_private_state(timeout, |state| state.final_drain_entries != 0)
    }

    #[cfg(test)]
    pub(crate) fn wait_for_private_error(&self, timeout: std::time::Duration) -> bool {
        self.wait_for_private_state(timeout, |state| state.errors != 0)
    }

    #[cfg(test)]
    pub(super) fn wait_for_private_state(
        &self,
        timeout: std::time::Duration,
        ready: impl Fn(&PrivateDrainTestState) -> bool,
    ) -> bool {
        let started = std::time::Instant::now();
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        while !ready(&state) {
            let Some(remaining) = timeout.checked_sub(started.elapsed()) else {
                return false;
            };
            let (next, timed_out) = self
                .private_drain_changed
                .wait_timeout(state, remaining)
                .expect("private drain gate lock poisoned");
            state = next;
            if timed_out.timed_out() && !ready(&state) {
                return false;
            }
        }
        true
    }

    #[cfg(test)]
    pub(crate) fn private_drain_snapshot(&self) -> (usize, usize, Option<usize>) {
        let state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        (state.completions, state.errors, state.final_outstanding)
    }
}
pub(super) struct IndexedSegmentBuild {
    pub index: IndexedCollectionSegment,
    pub observation: Option<crate::index::storage::inference::InferenceObservationHandle>,
    pub profile: Option<IncrementalSegmentProfile>,
    pub build_ns: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SegmentBuildDestination {
    Normal,
    Private,
}

struct SegmentBuildMessage {
    collection_index: usize,
    segment_id: u64,
    destination: SegmentBuildDestination,
    result: Result<IndexedSegmentBuild, String>,
}

pub(super) struct SegmentBuildScheduler {
    sender: std::sync::mpsc::Sender<SegmentBuildMessage>,
    receiver: std::sync::mpsc::Receiver<SegmentBuildMessage>,
    pub(super) buffered:
        std::collections::BTreeMap<(usize, u64), Result<IndexedSegmentBuild, String>>,
    private_buffered: std::collections::BTreeMap<u64, (usize, Result<IndexedSegmentBuild, String>)>,
    pub(super) planned_ids: Vec<Vec<u64>>,
    pub(super) next_publish: Vec<usize>,
    next_plan_id: u64,
    pub(super) outstanding: usize,
    pub(super) peak_outstanding: usize,
    pub(super) limit: usize,
    running: Arc<AtomicUsize>,
    pub(super) peak_running: Arc<AtomicUsize>,
    declared_orders: Option<Arc<dyn DeclaredOrderStore>>,
    pub(super) gate: Option<Arc<SegmentJobGate>>,
}

impl SegmentBuildScheduler {
    pub(super) fn new(
        workers: usize,
        collections: usize,
        declared_orders: Option<Arc<dyn DeclaredOrderStore>>,
        gate: Option<Arc<SegmentJobGate>>,
    ) -> Self {
        let (sender, receiver) = std::sync::mpsc::channel();
        Self {
            sender,
            receiver,
            buffered: std::collections::BTreeMap::new(),
            private_buffered: std::collections::BTreeMap::new(),
            planned_ids: vec![Vec::new(); collections],
            next_publish: vec![0; collections],
            next_plan_id: 1,
            outstanding: 0,
            peak_outstanding: 0,
            limit: workers.saturating_mul(2).max(1),
            running: Arc::new(AtomicUsize::new(0)),
            peak_running: Arc::new(AtomicUsize::new(0)),
            declared_orders,
            gate,
        }
    }

    fn submit(
        &mut self,
        collection_index: usize,
        segment: Box<dyn SegmentBuilder>,
        definition: Arc<CollectionDefinition>,
        options: RepositoryOptions,
        collection_count: usize,
        pool: &Arc<rayon::ThreadPool>,
        destination: SegmentBuildDestination,
    ) -> Result<(), String> {
        let planned = self
            .planned_ids
            .get_mut(collection_index)
            .ok_or("segment collection out of bounds")?;
        let segment_id = self.next_plan_id;
        self.next_plan_id = self
            .next_plan_id
            .checked_add(1)
            .ok_or("segment plan overflow")?;
        #[cfg(test)]
        let private_job = (destination == SegmentBuildDestination::Private)
            .then(|| self.gate.as_ref().map(|gate| gate.allocate_private_job()))
            .flatten();
        planned.push(segment_id);
        self.outstanding = self
            .outstanding
            .checked_add(1)
            .ok_or("segment scheduler overflow")?;
        self.peak_outstanding = self.peak_outstanding.max(self.outstanding);
        let collection = crate::model::CollectionId(collection_index as u32);
        let sender = self.sender.clone();
        let profile_build = options.detailed_profiling_enabled();
        let running = Arc::clone(&self.running);
        let peak_running = Arc::clone(&self.peak_running);
        let gate = self.gate.clone();
        let worker_pool = Arc::clone(pool);
        let declared_orders = self.declared_orders.clone();
        pool.spawn(move || {
            let active = running.fetch_add(1, Ordering::Relaxed) + 1;
            peak_running.fetch_max(active, Ordering::Relaxed);
            if let Some(gate) = gate.as_ref() {
                gate.wait();
            }
            let phase = crate::measurement::timer::DetailedInstant::now();
            let mut profile = profile_build.then(|| IncrementalSegmentProfile {
                collection: definition.name.clone(),
                files: segment.file_count(),
                records: segment.record_count(),
                source_bytes: segment.source_bytes(),
                ..Default::default()
            });
            #[allow(unused_mut)]
            let mut result = if let Some(error) =
                injected_private_finalization_failure(destination, &definition.name)
            {
                Err(error)
            } else {
                segment
                    .seal(SegmentSeal {
                        id: SegmentId::new(segment_id),
                        definition: definition.as_ref(),
                        options,
                        collection_count,
                        pool: &worker_pool,
                        profile: profile.as_mut(),
                        observe_inference: true,
                        declared_orders,
                    })
                    .map_err(String::from)
                    .map(|built| IndexedSegmentBuild {
                        index: built.index,
                        observation: built.observation,
                        profile,
                        build_ns: phase.elapsed().as_nanos() as u64,
                    })
            };
            #[cfg(test)]
            if let (Some(gate), Some(private_job)) = (gate.as_ref(), private_job) {
                gate.wait_before_private_completion(private_job);
                if gate.private_job_fails(private_job) {
                    result = Err("injected final-drain private segment failure".into());
                }
            }
            debug_assert!(match result.as_ref() {
                Ok(built) => built.index.collection == collection,
                Err(_) => true,
            });
            running.fetch_sub(1, Ordering::Relaxed);
            let _ = sender.send(SegmentBuildMessage {
                collection_index,
                segment_id,
                destination,
                result,
            });
        });
        Ok(())
    }

    pub(super) fn mark_plan_published(
        &mut self,
        collection_index: usize,
        count: usize,
    ) -> Result<(), IndexError> {
        let cursor = self
            .next_publish
            .get_mut(collection_index)
            .ok_or_else(|| IndexError::InvalidPublication("segment collection missing".into()))?;
        *cursor = cursor.checked_add(count).ok_or_else(|| {
            IndexError::ResourceLimit("segment publication cursor overflow".into())
        })?;
        Ok(())
    }

    pub(super) fn receive(&mut self, block: bool) -> Result<bool, String> {
        let message = if block {
            self.receiver.recv().map_err(|error| error.to_string())?
        } else {
            match self.receiver.try_recv() {
                Ok(message) => message,
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(false),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err("segment build channel disconnected".into());
                }
            }
        };
        match message.destination {
            SegmentBuildDestination::Normal => {
                self.buffered.insert(
                    (message.collection_index, message.segment_id),
                    message.result,
                );
            }
            SegmentBuildDestination::Private => {
                self.private_buffered.insert(
                    message.segment_id,
                    (message.collection_index, message.result),
                );
            }
        }
        Ok(true)
    }
}

fn injected_private_finalization_failure(
    _destination: SegmentBuildDestination,
    _collection_name: &str,
) -> Option<String> {
    #[cfg(test)]
    if _destination == SegmentBuildDestination::Private
        && _collection_name == "__test_private_segment_failure__"
    {
        return Some("injected private segment finalization failure".into());
    }
    None
}

pub(super) struct PrivateSegmentGroup {
    collection_index: usize,
    memory: super::file_preparation::StreamUse,
    pub(super) indexed: std::collections::BTreeMap<u64, IndexedSegmentBuild>,
}

impl PrivateSegmentGroup {
    pub(super) fn new(
        collection_index: usize,
        budget: &super::file_preparation::StreamBudget,
    ) -> Self {
        Self {
            collection_index,
            memory: budget.activity(
                super::file_preparation::StreamActivity::PrivateSegments,
                0,
                0,
            ),
            indexed: std::collections::BTreeMap::new(),
        }
    }

    pub(super) fn consume_ready(
        &mut self,
        scheduler: &mut SegmentBuildScheduler,
        block: bool,
    ) -> Result<bool, String> {
        if !Self::receive_ready(scheduler, block)? {
            return Ok(false);
        }
        let mut first_error = None;
        self.process_ready(scheduler, true, &mut first_error);
        first_error.map_or(Ok(true), Err)
    }

    /// Receiving and processing are separate so a worker's `Err` can be
    /// retained while all normally submitted completions are still drained. A
    /// channel failure remains terminal and is returned directly.
    fn receive_ready(scheduler: &mut SegmentBuildScheduler, block: bool) -> Result<bool, String> {
        if scheduler.outstanding == 0 {
            return Ok(false);
        }
        while scheduler.private_buffered.is_empty() {
            if !scheduler.receive(block)? || !block {
                return Ok(!scheduler.private_buffered.is_empty());
            }
        }
        Ok(true)
    }

    fn process_ready(
        &mut self,
        scheduler: &mut SegmentBuildScheduler,
        retain_successes: bool,
        first_error: &mut Option<String>,
    ) {
        let completed = std::mem::take(&mut scheduler.private_buffered);
        for (segment_id, (collection_index, result)) in completed {
            scheduler.outstanding = scheduler.outstanding.saturating_sub(1);
            #[cfg(test)]
            let worker_error = result.is_err();
            #[cfg(test)]
            if let Some(gate) = scheduler.gate.as_ref() {
                gate.record_private_completion(worker_error);
            }
            if collection_index != self.collection_index {
                first_error
                    .get_or_insert_with(|| "private segment completion changed collection".into());
                self.indexed.clear();
                self.memory.set(0, 0);
                continue;
            }
            let built = match result {
                Ok(built) => built,
                Err(error) => {
                    first_error.get_or_insert(error);
                    self.indexed.clear();
                    self.memory.set(0, 0);
                    continue;
                }
            };
            if built.index.id.get() != segment_id {
                first_error
                    .get_or_insert_with(|| "private segment completion identity mismatch".into());
                self.indexed.clear();
                self.memory.set(0, 0);
                continue;
            }
            if !retain_successes || first_error.is_some() {
                continue;
            }
            match self.indexed.entry(segment_id) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    self.memory.add_private_segment(&built.index);
                    entry.insert(built);
                }
                std::collections::btree_map::Entry::Occupied(_) => {
                    first_error
                        .get_or_insert_with(|| "duplicate private segment completion".into());
                    self.indexed.clear();
                    self.memory.set(0, 0);
                }
            }
        }
    }

    pub(super) fn drain(&mut self, scheduler: &mut SegmentBuildScheduler) -> Result<(), String> {
        self.drain_completions(scheduler, true)
    }

    fn drain_completions(
        &mut self,
        scheduler: &mut SegmentBuildScheduler,
        retain_successes: bool,
    ) -> Result<(), String> {
        let mut first_error = None;
        while scheduler.outstanding != 0 {
            if !Self::receive_ready(scheduler, true)? {
                return Err("private segment completion accounting stalled".into());
            }
            self.process_ready(scheduler, retain_successes, &mut first_error);
        }
        #[cfg(test)]
        if let Some(gate) = scheduler.gate.as_ref() {
            gate.record_private_drain_complete(scheduler.outstanding);
        }
        if first_error.is_some() {
            self.indexed.clear();
        }
        first_error.map_or(Ok(()), Err)
    }

    pub(super) fn discard(mut self, scheduler: &mut SegmentBuildScheduler) -> Result<(), String> {
        let result = self.drain_completions(scheduler, false);
        self.indexed.clear();
        result
    }

    pub(super) fn into_ordered(self) -> Vec<IndexedSegmentBuild> {
        self.indexed.into_values().collect()
    }
}

pub(super) fn schedule_segment(
    scheduler: &mut SegmentBuildScheduler,
    collection_index: usize,
    segment: Box<dyn SegmentBuilder>,
    definition: Arc<CollectionDefinition>,
    options: RepositoryOptions,
    collection_count: usize,
    pool: &Arc<rayon::ThreadPool>,
    cancelled: &impl Fn() -> bool,
    publish_ready: &mut impl FnMut(&mut SegmentBuildScheduler) -> Result<(), IndexError>,
) -> Result<(), IndexError> {
    while scheduler.outstanding >= scheduler.limit {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        scheduler.receive(true).map_err(IndexError::Build)?;
        publish_ready(scheduler)?;
    }
    scheduler
        .submit(
            collection_index,
            segment,
            definition,
            options,
            collection_count,
            pool,
            SegmentBuildDestination::Normal,
        )
        .map_err(IndexError::Build)?;
    while scheduler.receive(false).map_err(IndexError::Build)? {}
    publish_ready(scheduler)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn schedule_private_segment(
    scheduler: &mut SegmentBuildScheduler,
    private_group: &mut PrivateSegmentGroup,
    collection_index: usize,
    segment: Box<dyn SegmentBuilder>,
    definition: Arc<CollectionDefinition>,
    options: RepositoryOptions,
    collection_count: usize,
    pool: &Arc<rayon::ThreadPool>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), IndexError> {
    #[cfg(test)]
    let collection_name = definition.name.clone();
    while scheduler.outstanding >= scheduler.limit {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        private_group
            .consume_ready(scheduler, true)
            .map_err(IndexError::Build)?;
    }
    scheduler
        .submit(
            collection_index,
            segment,
            definition,
            options,
            collection_count,
            pool,
            SegmentBuildDestination::Private,
        )
        .map_err(IndexError::Build)?;
    #[cfg(test)]
    if collection_name == "__test_cancel_while_outstanding__"
        && let Some(gate) = &scheduler.gate
    {
        gate.wait_after_submission();
    }
    private_group
        .consume_ready(scheduler, false)
        .map_err(IndexError::Build)?;
    Ok(())
}
