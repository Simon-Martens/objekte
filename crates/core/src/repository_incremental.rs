//! Single-pass initial construction into immutable lifecycle segments.
//! Discovery supplies one shared path inventory. Completed work units move
//! into segments; the final metadata repository shares that same inventory.

use super::*;

use crate::build::{
    AdmissionBreakdown, BatchContribution, IndexedCollectionSegment, RepositoryFileJob,
    SegmentAdmission, SegmentId, SourceFileInput,
};
use crate::inventory::append_shifted_bitmap;
use crate::lifecycle::{Index, IndexError, IndexRevision, PublishedIndex};
use crate::model::FileId;

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
struct PrivateDrainTestState {
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

    fn wait(&self) {
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

    fn wait_after_validation(&self) {
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

    fn wait_after_submission(&self) {
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
    fn allocate_private_job(&self) -> usize {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        let job = state.next_job;
        state.next_job += 1;
        job
    }

    #[cfg(test)]
    fn wait_before_private_completion(&self, job: usize) {
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
    fn private_job_fails(&self, job: usize) -> bool {
        self.private_drain
            .lock()
            .expect("private drain gate lock poisoned")
            .failing_job
            == Some(job)
    }

    #[cfg(test)]
    fn record_private_completion(&self, error: bool) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.completions += 1;
        state.errors += usize::from(error);
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    fn record_private_final_drain(&self) {
        let mut state = self
            .private_drain
            .lock()
            .expect("private drain gate lock poisoned");
        state.final_drain_entries += 1;
        self.private_drain_changed.notify_all();
    }

    #[cfg(test)]
    fn record_private_drain_complete(&self, outstanding: usize) {
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
    fn wait_for_private_state(
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

struct IncrementalCollection {
    definition: Arc<CollectionDefinition>,
    source: PreparedSource,
    id: CollectionId,
    member_files: Vec<FileId>,
    admission: SegmentAdmission,
    records: u32,
    observed_batches: u64,
    logical_records: RoaringBitmap,
    options: RepositoryOptions,
}

impl IncrementalCollection {
    fn new(
        definition: CollectionDefinition,
        source: PreparedSource,
        id: CollectionId,
        member_files: Vec<FileId>,
        options: RepositoryOptions,
        _collection_count: usize,
    ) -> Self {
        let definition = Arc::new(definition);
        let admission = SegmentAdmission::with_limits_and_id(
            id,
            &source,
            options.segment_policy,
            SegmentId::new(0),
        );
        Self {
            definition,
            source,
            id,
            member_files,
            admission,
            records: 0,
            observed_batches: 0,
            logical_records: RoaringBitmap::new(),
            options,
        }
    }

    fn observe(
        &mut self,
        batch: &BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<(), String> {
        let base = self.records;
        let count = u32::try_from(batch.record_count()).map_err(|_| "too many batch records")?;
        self.records = base
            .checked_add(count)
            .ok_or("collection record slot overflow")?;
        self.observed_batches = self
            .observed_batches
            .checked_add(1)
            .ok_or("inference batch count overflow")?;
        let coordinator = crate::detailed_timer::start_if_enabled(profile.is_some());
        append_shifted_bitmap(&mut self.logical_records, batch.logical_records(), base)?;
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        Ok(())
    }

    /// Admit one complete ordinary batch, sealing the preceding group if the
    /// next batch would exceed its bounded segment target.
    fn admit_profiled(
        &mut self,
        batch: BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<Option<crate::build::CollectionSegment>, String> {
        self.observe(&batch, profile.as_deref_mut())?;
        let coordinator = crate::detailed_timer::start_if_enabled(profile.is_some());
        let sealed = if !self.admission.can_admit_batch(&batch) {
            self.take_segment()
        } else {
            None
        };
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        self.admission.admit_batch_profiled(batch, profile)?;
        Ok(sealed)
    }

    /// A large framed file is never split between publications. Its chunks
    /// remain one logical file contribution until framing/validation finish.
    fn admit_large_chunk_profiled(
        &mut self,
        batch: BatchContribution,
        mut profile: Option<&mut AdmissionBreakdown>,
    ) -> Result<Option<crate::build::CollectionSegment>, String> {
        self.observe(&batch, profile.as_deref_mut())?;
        let coordinator = crate::detailed_timer::start_if_enabled(profile.is_some());
        let sealed = if self.admission.file_count() != 0 && !self.admission.can_admit_batch(&batch)
        {
            self.take_segment()
        } else {
            None
        };
        if let (Some(started), Some(profile)) = (coordinator, profile.as_mut()) {
            profile.coordinator_ns += started.elapsed().as_nanos() as u64;
        }
        self.admission.admit_batch_profiled(batch, profile)?;
        Ok(sealed.or_else(|| {
            self.admission
                .at_capacity()
                .then(|| self.take_segment())
                .flatten()
        }))
    }

    fn take_segment(&mut self) -> Option<crate::build::CollectionSegment> {
        if self.admission.file_count() == 0 {
            return None;
        }
        let replacement = SegmentAdmission::with_limits_and_id(
            self.id,
            &self.source,
            self.options.segment_policy,
            SegmentId::new(0),
        );
        Some(std::mem::replace(&mut self.admission, replacement).finish())
    }

    fn finish_summary(
        &mut self,
    ) -> Result<(Option<GlobalInferenceSummary>, InferenceBuildStats), String> {
        Ok((
            None,
            if self.options.inference.mode == crate::inference::InferenceMode::Disabled {
                InferenceBuildStats::default()
            } else {
                InferenceBuildStats {
                    observed_records: self.records as u64,
                    merged_batches: self.observed_batches,
                    ..Default::default()
                }
            },
        ))
    }
}

struct IndexedSegmentBuild {
    index: IndexedCollectionSegment,
    profile: Option<IncrementalSegmentProfile>,
    build_ns: u64,
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

struct SegmentBuildScheduler {
    sender: std::sync::mpsc::Sender<SegmentBuildMessage>,
    receiver: std::sync::mpsc::Receiver<SegmentBuildMessage>,
    buffered: std::collections::BTreeMap<(usize, u64), Result<IndexedSegmentBuild, String>>,
    private_buffered: std::collections::BTreeMap<u64, (usize, Result<IndexedSegmentBuild, String>)>,
    planned_ids: Vec<Vec<u64>>,
    next_publish: Vec<usize>,
    next_plan_id: u64,
    outstanding: usize,
    peak_outstanding: usize,
    limit: usize,
    running: Arc<AtomicUsize>,
    peak_running: Arc<AtomicUsize>,
    gate: Option<Arc<SegmentJobGate>>,
}

impl SegmentBuildScheduler {
    fn new(workers: usize, collections: usize, gate: Option<Arc<SegmentJobGate>>) -> Self {
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
            gate,
        }
    }

    fn submit(
        &mut self,
        collection_index: usize,
        mut segment: crate::build::CollectionSegment,
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
        segment.id = SegmentId::new(segment_id);
        planned.push(segment_id);
        self.outstanding = self
            .outstanding
            .checked_add(1)
            .ok_or("segment scheduler overflow")?;
        self.peak_outstanding = self.peak_outstanding.max(self.outstanding);
        let collection = segment.collection;
        let sender = self.sender.clone();
        let profile_build = options.detailed_profiling_enabled();
        let running = Arc::clone(&self.running);
        let peak_running = Arc::clone(&self.peak_running);
        let gate = self.gate.clone();
        let worker_pool = Arc::clone(pool);
        pool.spawn(move || {
            let active = running.fetch_add(1, Ordering::Relaxed) + 1;
            peak_running.fetch_max(active, Ordering::Relaxed);
            if let Some(gate) = gate.as_ref() {
                gate.wait();
            }
            let phase = crate::detailed_timer::DetailedInstant::now();
            let mut profile = profile_build.then(|| IncrementalSegmentProfile {
                collection: definition.name.clone(),
                files: segment.files.len(),
                records: segment.records.len(),
                source_bytes: segment
                    .files
                    .iter()
                    .map(|file| file.entry.revision.size)
                    .sum(),
                ..Default::default()
            });
            #[allow(unused_mut)]
            let mut result = if let Some(error) =
                injected_private_finalization_failure(destination, &definition.name)
            {
                Err(error)
            } else {
                segment
                    .into_indexed_profiled(
                        &definition,
                        options.bigram_alphabet,
                        options.inference,
                        collection_count,
                        &worker_pool,
                        profile.as_mut(),
                    )
                    .map(|index| IndexedSegmentBuild {
                        index,
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

    fn mark_plan_published(
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

    fn receive(&mut self, block: bool) -> Result<bool, String> {
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

    fn publish_ready(
        &mut self,
        index: &Index,
        expected: &mut IndexRevision,
        pending: &[IncrementalCollection],
        on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
        publication_ns: &mut u64,
        profile: &mut Option<BuildProfileStats>,
    ) -> Result<(), IndexError> {
        for collection_index in 0..self.next_publish.len() {
            loop {
                let position = self.next_publish[collection_index];
                let Some(&id) = self.planned_ids[collection_index].get(position) else {
                    break;
                };
                let Some(result) = self.buffered.remove(&(collection_index, id)) else {
                    break;
                };
                let built = result.map_err(IndexError::Build)?;
                publish_indexed_segment(
                    index,
                    expected,
                    built,
                    pending
                        .get(collection_index)
                        .ok_or_else(|| IndexError::Build("segment collection missing".into()))?,
                    on_publication,
                    publication_ns,
                    profile,
                )?;
                self.outstanding = self.outstanding.saturating_sub(1);
                self.next_publish[collection_index] = position.checked_add(1).ok_or_else(|| {
                    IndexError::ResourceLimit("segment publication cursor overflow".into())
                })?;
            }
        }
        Ok(())
    }

    fn drain(
        &mut self,
        index: &Index,
        expected: &mut IndexRevision,
        pending: &[IncrementalCollection],
        on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
        publication_ns: &mut u64,
        profile: &mut Option<BuildProfileStats>,
    ) -> Result<(), IndexError> {
        while self.outstanding != 0 {
            self.receive(true).map_err(IndexError::Build)?;
            self.publish_ready(
                index,
                expected,
                pending,
                on_publication,
                publication_ns,
                profile,
            )?;
            if self.buffered.len() > self.limit {
                return Err(IndexError::ResourceLimit(
                    "segment completion window exceeded".into(),
                ));
            }
        }
        Ok(())
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

struct PrivateSegmentGroup {
    collection_index: usize,
    indexed: std::collections::BTreeMap<u64, IndexedSegmentBuild>,
}

impl PrivateSegmentGroup {
    fn new(collection_index: usize) -> Self {
        Self {
            collection_index,
            indexed: std::collections::BTreeMap::new(),
        }
    }

    fn consume_ready(
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
                continue;
            }
            let built = match result {
                Ok(built) => built,
                Err(error) => {
                    first_error.get_or_insert(error);
                    self.indexed.clear();
                    continue;
                }
            };
            if built.index.id.get() != segment_id {
                first_error
                    .get_or_insert_with(|| "private segment completion identity mismatch".into());
                self.indexed.clear();
                continue;
            }
            if !retain_successes || first_error.is_some() {
                continue;
            }
            match self.indexed.entry(segment_id) {
                std::collections::btree_map::Entry::Vacant(entry) => {
                    entry.insert(built);
                }
                std::collections::btree_map::Entry::Occupied(_) => {
                    first_error
                        .get_or_insert_with(|| "duplicate private segment completion".into());
                    self.indexed.clear();
                }
            }
        }
    }

    fn drain(&mut self, scheduler: &mut SegmentBuildScheduler) -> Result<(), String> {
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

    fn discard(mut self, scheduler: &mut SegmentBuildScheduler) -> Result<(), String> {
        let result = self.drain_completions(scheduler, false);
        self.indexed.clear();
        result
    }

    fn into_ordered(self) -> Vec<IndexedSegmentBuild> {
        self.indexed.into_values().collect()
    }
}

fn publish_indexed_segment(
    index: &Index,
    expected: &mut IndexRevision,
    built: IndexedSegmentBuild,
    collection: &IncrementalCollection,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
) -> Result<(), IndexError> {
    let started = crate::detailed_timer::DetailedInstant::now();
    let IndexedSegmentBuild {
        index: indexed,
        profile: mut segment_profile,
        build_ns,
    } = built;
    if let (Some(profile), Some(segment)) = (profile.as_mut(), segment_profile.as_mut()) {
        profile.publication_index_ns = profile.publication_index_ns.saturating_add(build_ns);
        segment.index_ns = build_ns;
    }
    let _ = collection;
    let (revision, phases) =
        index.publish_initial_segment(*expected, indexed, profile.is_some())?;
    *expected = revision;
    if let Some(profile) = profile.as_mut() {
        profile.publication_prepare_ns = profile
            .publication_prepare_ns
            .saturating_add(phases.preparation_ns);
        profile.publication_install_ns = profile
            .publication_install_ns
            .saturating_add(phases.install_ns);
        let segment = segment_profile.as_mut().unwrap();
        segment.prepare_ns = phases.preparation_ns;
        segment.install_ns = phases.install_ns;
    }
    let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
    on_publication(index)?;
    if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
        let ns = started.elapsed().as_nanos() as u64;
        profile.publication_callback_ns = profile.publication_callback_ns.saturating_add(ns);
        segment_profile.as_mut().unwrap().callback_ns = ns;
    }
    *publication_ns = publication_ns.saturating_add(started.elapsed().as_nanos() as u64);
    if let (Some(profile), Some(segment)) = (profile.as_mut(), segment_profile) {
        profile.segments.push(segment);
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn publish_private_segment_group(
    scheduler: &mut SegmentBuildScheduler,
    index: &Index,
    expected: &mut IndexRevision,
    mut private_group: PrivateSegmentGroup,
    collection_index: usize,
    pending: &[IncrementalCollection],
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), IndexError> {
    #[cfg(test)]
    if let Some(gate) = scheduler.gate.as_ref() {
        gate.record_private_final_drain();
    }
    private_group.drain(scheduler).map_err(IndexError::Build)?;
    let mut builds = private_group.into_ordered();
    let total = builds.len();
    if total == 0 {
        scheduler.mark_plan_published(collection_index, 0)?;
        return Ok(());
    }
    let _ = pending
        .get(collection_index)
        .ok_or_else(|| IndexError::Build("segment collection missing".into()))?;
    if cancelled() {
        return Err(IndexError::Cancelled);
    }
    let mut index_build_ns = 0u64;
    for build in &mut builds {
        index_build_ns = index_build_ns.saturating_add(build.build_ns);
        if let (Some(profile), Some(segment)) = (profile.as_mut(), build.profile.as_mut()) {
            segment.index_ns = build.build_ns;
            profile.segments.push(segment.clone());
        }
    }
    if let Some(profile) = profile.as_mut() {
        profile.publication_index_ns = profile.publication_index_ns.saturating_add(index_build_ns);
    }
    let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
    let (revision, phases) = index.publish_initial_segments(
        *expected,
        builds
            .into_iter()
            .map(|build| build.index)
            .collect::<Vec<_>>(),
        profile.is_some(),
    )?;
    *expected = revision;
    if let (Some(profile), phases) = (profile.as_mut(), phases) {
        profile.publication_prepare_ns = profile
            .publication_prepare_ns
            .saturating_add(phases.preparation_ns);
        profile.publication_install_ns = profile
            .publication_install_ns
            .saturating_add(phases.install_ns);
    }
    scheduler.mark_plan_published(collection_index, total)?;
    on_publication(index)?;
    *publication_ns = publication_ns
        .saturating_add(phase.map_or(0, |started| started.elapsed().as_nanos() as u64));
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn schedule_segment(
    scheduler: &mut SegmentBuildScheduler,
    index: &Index,
    expected: &mut IndexRevision,
    pending: &[IncrementalCollection],
    collection_index: usize,
    segment: crate::build::CollectionSegment,
    options: RepositoryOptions,
    pool: &Arc<rayon::ThreadPool>,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), IndexError> {
    while scheduler.outstanding >= scheduler.limit {
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        scheduler.receive(true).map_err(IndexError::Build)?;
        scheduler.publish_ready(
            index,
            expected,
            pending,
            on_publication,
            publication_ns,
            profile,
        )?;
    }
    let collection = pending
        .get(collection_index)
        .ok_or_else(|| IndexError::Build("segment collection missing".into()))?;
    scheduler
        .submit(
            collection_index,
            segment,
            Arc::clone(&collection.definition),
            options,
            pending.len(),
            pool,
            SegmentBuildDestination::Normal,
        )
        .map_err(IndexError::Build)?;
    while scheduler.receive(false).map_err(IndexError::Build)? {}
    scheduler.publish_ready(
        index,
        expected,
        pending,
        on_publication,
        publication_ns,
        profile,
    )
}

#[allow(clippy::too_many_arguments)]
fn schedule_private_segment(
    scheduler: &mut SegmentBuildScheduler,
    private_group: &mut PrivateSegmentGroup,
    collection_index: usize,
    segment: crate::build::CollectionSegment,
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

fn finish_admission_profile(
    profile: &mut Option<BuildProfileStats>,
    collection: &str,
    started: Option<Instant>,
    detail: Option<AdmissionBreakdown>,
) -> Result<(), String> {
    // The outer interval ends before profile aggregation and before any
    // segment publication. Consumed contribution temporaries are dropped by
    // the inner admission calls before this boundary.
    if let (Some(started), Some(mut detail), Some(profile)) = (started, detail, profile.as_mut()) {
        detail.total_ns = started.elapsed().as_nanos() as u64;
        detail.remainder_ns = detail
            .total_ns
            .checked_sub(detail.incremental_component_ns())
            .ok_or("incremental admission components overlap")?;
        profile.admission.absorb(detail);
        profile
            .admission_by_collection
            .entry(collection.to_owned())
            .or_default()
            .absorb(detail);
    }
    Ok(())
}

struct SegmentMetadata {
    generation: CollectionGeneration,
    id_lookup: HashMap<RecordId, RecordSlot>,
    identity_order: Vec<RecordSlot>,
    stats: CollectionStats,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
}

fn segment_metadata_generation(
    collection: CollectionId,
    name: &str,
    segments: &[Arc<crate::native_segment::PublishedSegment>],
    files: &mut FileInventory,
    relations: &mut [RelationContribution],
) -> Result<SegmentMetadata, String> {
    let mut builder = CollectionBuilder::new(collection, name.to_owned(), 1, files.files.len());
    let mut file_set = HashSet::new();
    let mut record_count = 0usize;
    let mut logical_records = RoaringBitmap::new();
    let mut empty_array_records = RoaringBitmap::new();
    for segment in segments {
        let base = u32::try_from(record_count).map_err(|_| "too many segment records")?;
        let segment_count =
            u32::try_from(segment.index.records.len()).map_err(|_| "too many segment records")?;
        if relations.len() != segment.index.relations.len() {
            return Err("segment relation coverage mismatch".into());
        }
        for (global, local) in relations
            .iter_mut()
            .zip(segment.index.relations.iter().cloned())
        {
            global.merge_shifted(local, base, segment_count)?;
        }
        for slot in &segment.index.logical_records {
            logical_records.insert(
                base.checked_add(slot)
                    .ok_or("logical record slot overflow")?,
            );
        }
        for slot in &segment.index.empty_array_records {
            empty_array_records.insert(
                base.checked_add(slot)
                    .ok_or("empty-array record slot overflow")?,
            );
        }
        for segment_file in &segment.index.files {
            if file_set.insert(segment_file.entry.id) {
                builder.include_file(segment_file.entry.id)?;
            }
        }
        for record in &segment.index.records {
            let file = files
                .files
                .get_mut(record.file.0 as usize)
                .ok_or("segment metadata file missing")?;
            if record.class == RecordClass::Logical {
                builder.add_record(
                    file,
                    record.id.clone(),
                    record.range.start,
                    record.range.end,
                )
                .map_err(|error| {
                    format!(
                        "{}:{}..{}: {error}",
                        file.relative_path.as_path().display(),
                        record.range.start,
                        record.range.end,
                    )
                })?;
            } else {
                let segment_file = segment
                    .index
                    .file(record.file)
                    .ok_or("fallback segment file missing")?;
                let Some(local) = record.diagnostic_ref.stored_index() else {
                    builder.add_raw_fallback(
                        file,
                        record.id.clone(),
                        record.range,
                        record.diagnostic_ref,
                    )?;
                    record_count += 1;
                    continue;
                };
                let diagnostic = segment_file
                    .entry
                    .diagnostics
                    .get(local)
                    .ok_or("fallback diagnostic missing")?
                    .clone();
                let existing = file.diagnostics.iter().position(|candidate| {
                    let mut candidate = candidate.clone();
                    candidate.record = None;
                    let mut diagnostic = diagnostic.clone();
                    diagnostic.record = None;
                    candidate == diagnostic
                });
                if existing.is_none() && file.omitted_diagnostics != 0 {
                    builder.add_raw_fallback(
                        file,
                        record.id.clone(),
                        record.range,
                        crate::inventory::DiagnosticRef(u32::MAX),
                    )?;
                    record_count += 1;
                    continue;
                }
                let global = existing.unwrap_or_else(|| {
                    let index = file.diagnostics.len();
                    file.diagnostics.push(diagnostic);
                    index
                });
                let reference = crate::inventory::DiagnosticRef(
                    u32::try_from(global + 1).map_err(|_| "diagnostic index overflow")?,
                );
                let slot =
                    builder.add_raw_fallback(file, record.id.clone(), record.range, reference)?;
                file.diagnostics[global].record = Some(slot);
            }
            record_count += 1;
        }
    }
    let generation = builder.finish(files)?;
    let id_lookup = generation
        .records
        .iter()
        .map(|record| (record.id.clone(), record.slot))
        .collect::<HashMap<_, _>>();
    let mut identity_order = generation
        .records
        .iter()
        .map(|record| record.slot)
        .collect::<Vec<_>>();
    identity_order.sort_by(|left, right| {
        generation.records[left.0 as usize]
            .id
            .cmp(&generation.records[right.0 as usize].id)
    });
    let stats = CollectionStats {
        file_count: file_set.len(),
        record_count,
        total_file_bytes: file_set
            .iter()
            .map(|file| {
                files
                    .files
                    .get(file.0 as usize)
                    .map_or(0, |entry| entry.revision.size)
            })
            .sum(),
    };
    Ok(SegmentMetadata {
        generation,
        id_lookup,
        identity_order,
        stats,
        logical_records,
        empty_array_records,
    })
}

#[allow(clippy::too_many_arguments)]
fn admit_normal(
    collection_index: usize,
    batch: BatchContribution,
    pending: &mut [IncrementalCollection],
    digest_bindings: &mut [Vec<(CollectionId, [u8; 32])>],
    build_parses: &mut u64,
    failed_parses: &mut u64,
    source_reads: &mut u64,
    source_bytes_read: &mut u64,
    index: &Index,
    expected: &mut IndexRevision,
    scheduler: &mut SegmentBuildScheduler,
    options: RepositoryOptions,
    pool: &Arc<rayon::ThreadPool>,
    on_publication: &mut impl FnMut(&Index) -> Result<(), IndexError>,
    publication_ns: &mut u64,
    profile: &mut Option<BuildProfileStats>,
    cancelled: &impl Fn() -> bool,
) -> Result<(), String> {
    if cancelled() {
        return Err("initial construction cancelled".into());
    }
    let admission_started = crate::detailed_timer::start_if_enabled(profile.is_some());
    let mut detail = profile.as_ref().map(|_| AdmissionBreakdown {
        batches: 1,
        files: batch.new_file_count() as u64,
        records: batch.record_count() as u64,
        ..Default::default()
    });
    let collection_id = pending[collection_index].id;
    for &(file, digest) in batch.file_fingerprints() {
        digest_bindings
            .get_mut(file.0 as usize)
            .ok_or("digest file ID out of bounds")?
            .push((collection_id, digest));
    }
    *build_parses = build_parses.saturating_add(batch.record_parses);
    *failed_parses = failed_parses.saturating_add(batch.failed_parses);
    *source_reads = source_reads.saturating_add(batch.source_reads);
    *source_bytes_read = source_bytes_read.saturating_add(batch.source_bytes_read);
    if let Some(profile) = profile.as_mut() {
        profile.work.absorb(&batch.profile);
        profile
            .work_by_collection
            .entry(pending[collection_index].definition.name.clone())
            .or_default()
            .absorb(&batch.profile);
    }
    if let (Some(started), Some(detail)) = (admission_started, detail.as_mut()) {
        detail.coordinator_ns += started.elapsed().as_nanos() as u64;
    }
    let sealed = pending[collection_index].admit_profiled(batch, detail.as_mut())?;
    finish_admission_profile(
        profile,
        &pending[collection_index].definition.name,
        admission_started,
        detail,
    )?;
    if let Some(sealed) = sealed {
        schedule_segment(
            scheduler,
            index,
            expected,
            pending,
            collection_index,
            sealed,
            options,
            pool,
            on_publication,
            publication_ns,
            profile,
            cancelled,
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

impl Repository {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_incremental_with_sources(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        on_catalogue: impl FnOnce(&Index) -> Result<(), IndexError>,
        on_publication: impl FnMut(&Index) -> Result<(), IndexError>,
        cancelled: impl Fn() -> bool,
    ) -> Result<Index, IndexError> {
        Self::build_incremental_with_sources_inner(
            root,
            definitions,
            options,
            sources,
            on_catalogue,
            on_publication,
            cancelled,
            None,
        )
    }

    #[cfg(test)]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn build_incremental_with_sources_gated(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        on_catalogue: impl FnOnce(&Index) -> Result<(), IndexError>,
        on_publication: impl FnMut(&Index) -> Result<(), IndexError>,
        cancelled: impl Fn() -> bool,
        gate: Arc<SegmentJobGate>,
    ) -> Result<Index, IndexError> {
        Self::build_incremental_with_sources_inner(
            root,
            definitions,
            options,
            sources,
            on_catalogue,
            on_publication,
            cancelled,
            Some(gate),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn build_incremental_with_sources_inner(
        root: PathBuf,
        definitions: Vec<CollectionDefinition>,
        options: RepositoryOptions,
        sources: SourceRegistry,
        on_catalogue: impl FnOnce(&Index) -> Result<(), IndexError>,
        mut on_publication: impl FnMut(&Index) -> Result<(), IndexError>,
        cancelled: impl Fn() -> bool,
        gate: Option<Arc<SegmentJobGate>>,
    ) -> Result<Index, IndexError> {
        let started = Instant::now();
        let mut profile = options
            .detailed_profiling_enabled()
            .then(BuildProfileStats::default);
        let root = root
            .canonicalize()
            .map_err(|error| IndexError::Io(error.to_string()))?;
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let build_workers = crate::build::resolve_build_workers(
            options.build_workers,
            std::thread::available_parallelism().ok(),
        )
        .map_err(IndexError::Build)?;
        let pool = Arc::new(
            rayon::ThreadPoolBuilder::new()
                .num_threads(build_workers)
                .thread_name(|number| format!("objekte-build-{number}"))
                .build()
                .map_err(|error| IndexError::Build(error.to_string()))?,
        );
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.pool_init_ns = started.elapsed().as_nanos() as u64;
        }
        let discovery_started = Instant::now();
        let mut files = FileInventory::new(root.clone());
        let mut selected = Vec::new();
        let metadata_window = build_workers.saturating_mul(64).max(1);
        let metadata_peak_active = AtomicUsize::new(0);
        for (number, definition) in definitions.iter().cloned().enumerate() {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            if selected.iter().any(
                |(existing, _, _, _): &(
                    CollectionDefinition,
                    CollectionId,
                    PreparedSource,
                    Vec<FileId>,
                )| existing.name == definition.name,
            ) {
                return Err(IndexError::Build(format!(
                    "duplicate collection: {}",
                    definition.name
                )));
            }
            let id = CollectionId(
                u32::try_from(number)
                    .map_err(|_| IndexError::ResourceLimit("too many collections".into()))?,
            );
            let source = sources
                .resolve(&definition.format)
                .map_err(IndexError::Build)?;
            let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
            let paths = discover(
                &root,
                &definition.source_path,
                &definition.discovery,
                &source,
            )
            .map_err(IndexError::Build)?;
            if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
                profile.discovery_enumeration_ns = profile
                    .discovery_enumeration_ns
                    .saturating_add(started.elapsed().as_nanos() as u64);
            }
            let mut members = Vec::with_capacity(paths.len());
            let mut paths = paths.into_iter();
            loop {
                if cancelled() {
                    return Err(IndexError::Cancelled);
                }
                let window = paths.by_ref().take(metadata_window).collect::<Vec<_>>();
                if window.is_empty() {
                    break;
                }
                let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
                let prepared = if build_workers == 1 {
                    window
                        .into_iter()
                        .map(|path| prepare_discovered_file(&root, path))
                        .collect::<Vec<_>>()
                } else {
                    prepare_discovered_window(&pool, &root, window, false, &metadata_peak_active)
                };
                if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
                    profile.discovery_metadata_prepare_ns = profile
                        .discovery_metadata_prepare_ns
                        .saturating_add(started.elapsed().as_nanos() as u64);
                }
                let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
                for entry in prepared {
                    let entry = entry.map_err(IndexError::Build)?;
                    members.push(
                        files
                            .add(entry.relative, entry.revision, entry.created_ns, id)
                            .map_err(IndexError::Build)?,
                    );
                }
                if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
                    profile.discovery_ordered_insert_ns = profile
                        .discovery_ordered_insert_ns
                        .saturating_add(started.elapsed().as_nanos() as u64);
                }
            }
            selected.push((definition, id, source, members));
        }
        files.validate_diagnostics().map_err(IndexError::Build)?;
        let discovery_ns = discovery_started.elapsed().as_nanos() as u64;
        if let Some(profile) = profile.as_mut() {
            profile.discovery_catalogue_ns = discovery_ns;
            profile.discovery_metadata_peak_active = metadata_peak_active.load(Ordering::Relaxed);
            profile.discovery_rss_bytes = build_profile_rss_bytes();
        }
        let setup_started = crate::detailed_timer::start_if_enabled(profile.is_some());
        let discovered = Arc::new(files);
        let source_files = Arc::new(
            discovered
                .files
                .iter()
                .map(SourceFileInput::from_entry)
                .collect::<Vec<_>>(),
        );
        let collection_count = selected.len();
        let mut pending = selected
            .into_iter()
            .map(|(definition, id, source, members)| {
                IncrementalCollection::new(
                    definition,
                    source,
                    id,
                    members,
                    options,
                    collection_count,
                )
            })
            .collect::<Vec<_>>();
        let mut repository =
            Self::build_fresh(root.clone(), definitions.clone(), options, &sources, false)
                .map_err(IndexError::Build)?;
        repository.files = Arc::clone(&discovered);
        repository.base_fingerprints = vec![None; discovered.files.len()];
        for (collection, state) in repository.collections.iter_mut().zip(&pending) {
            collection.generation = CollectionBuilder::new(
                state.id,
                state.definition.name.clone(),
                1,
                discovered.files.len(),
            )
            .finish(&discovered)
            .map_err(IndexError::Build)?;
            collection.stats.file_count = state.member_files.len();
            collection.stats.total_file_bytes = state
                .member_files
                .iter()
                .map(|file| discovered.files[file.0 as usize].revision.size)
                .sum();
        }
        repository.startup.build_discovery_ns = discovery_ns;
        repository.startup.build_workers = build_workers;
        let mut published = PublishedIndex::from_repository_owned(
            next_repository_instance(),
            IndexRevision::INITIAL,
            repository,
        )?;
        published.begin_initial_construction();
        let index = Index::from_published(published);
        on_catalogue(&index)?;
        if let (Some(started), Some(profile)) = (setup_started, profile.as_mut()) {
            profile.builder_setup_ns = started.elapsed().as_nanos() as u64;
        }
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        let mut expected = IndexRevision::INITIAL;
        let mut scheduler =
            SegmentBuildScheduler::new(pool.current_num_threads(), pending.len(), gate);
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let mut jobs = Vec::new();
        for (collection_index, state) in pending.iter().enumerate() {
            for batch in crate::build::file_batches(&state.member_files, &discovered) {
                if batch.len() == 1
                    && (discovered.files[batch[0].0 as usize].revision.size > 8 * 1024 * 1024
                        || matches!(state.definition.format, crate::source::SourceFormat::JsonLines))
                {
                    jobs.push(ScheduledBuildJob::Large {
                        collection_index,
                        collection: state.id,
                        definition: Arc::clone(&state.definition),
                        source: state.source.clone(),
                        file: batch[0],
                    });
                } else {
                    jobs.push(ScheduledBuildJob::Files(RepositoryFileJob {
                        collection_index,
                        collection: state.id,
                        definition: Arc::clone(&state.definition),
                        source: state.source.clone(),
                        files: batch.to_vec(),
                    }));
                }
            }
        }
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.job_scheduling_ns = started.elapsed().as_nanos() as u64;
        }
        let extraction_started = crate::detailed_timer::DetailedInstant::now();
        let mut digest_bindings = vec![Vec::new(); discovered.files.len()];
        let mut build_parses = 0u64;
        let mut failed_parses = 0u64;
        let mut source_reads = 0u64;
        let mut source_bytes_read = 0u64;
        let mut schedule_stats = crate::build::BuildScheduleStats::default();
        let mut publication_ns = 0u64;
        let mut normal = Vec::new();
        for job in jobs {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            match job {
                ScheduledBuildJob::Files(job) => normal.push(job),
                ScheduledBuildJob::Large {
                    collection_index,
                    collection,
                    definition,
                    source,
                    file,
                } => {
                    if !normal.is_empty() {
                        let result = crate::build::process_repository_file_batches_profiled(
                            &pool,
                            Arc::new(root.clone()),
                            &normal,
                            Arc::clone(&source_files),
                            options.bigram_alphabet,
                            options.inference,
                            options.detailed_profiling_enabled(),
                            |number, batch| {
                                admit_normal(
                                    number,
                                    batch,
                                    &mut pending,
                                    &mut digest_bindings,
                                    &mut build_parses,
                                    &mut failed_parses,
                                    &mut source_reads,
                                    &mut source_bytes_read,
                                    &index,
                                    &mut expected,
                                    &mut scheduler,
                                    options,
                                    &pool,
                                    &mut on_publication,
                                    &mut publication_ns,
                                    &mut profile,
                                    &cancelled,
                                )
                            },
                        );
                        schedule_stats.absorb(result.map_err(|error| {
                            if cancelled() {
                                IndexError::Cancelled
                            } else {
                                IndexError::Build(error)
                            }
                        })?);
                        normal.clear();
                    }
                    if let Some(sealed) = pending[collection_index].take_segment() {
                        schedule_segment(
                            &mut scheduler,
                            &index,
                            &mut expected,
                            &pending,
                            collection_index,
                            sealed,
                            options,
                            &pool,
                            &mut on_publication,
                            &mut publication_ns,
                            &mut profile,
                            &cancelled,
                        )?;
                    }
                    scheduler.drain(
                        &index,
                        &mut expected,
                        &pending,
                        &mut on_publication,
                        &mut publication_ns,
                        &mut profile,
                    )?;
                    let mut private_group = PrivateSegmentGroup::new(collection_index);
                    let definition_for_private = Arc::clone(&definition);
                    let collection_count = pending.len();
                    let process_result = crate::build::process_framed_file_profiled(
                        &pool,
                        Arc::new(root.clone()),
                        definition,
                        collection,
                        source,
                        file,
                        Arc::clone(&source_files),
                        options.bigram_alphabet,
                        options.inference,
                        options.detailed_profiling_enabled(),
                        |batch| {
                            if cancelled() {
                                return Err("initial construction cancelled".into());
                            }
                            let admission_started =
                                crate::detailed_timer::start_if_enabled(profile.is_some());
                            let mut detail = profile.as_ref().map(|_| AdmissionBreakdown {
                                batches: 1,
                                files: batch.new_file_count() as u64,
                                records: batch.record_count() as u64,
                                ..Default::default()
                            });
                            build_parses = build_parses.saturating_add(batch.record_parses);
                            failed_parses = failed_parses.saturating_add(batch.failed_parses);
                            for &(file, digest) in batch.file_fingerprints() {
                                digest_bindings[file.0 as usize].push((collection, digest));
                            }
                            if let Some(profile) = profile.as_mut() {
                                profile.work.absorb(&batch.profile);
                                profile
                                    .work_by_collection
                                    .entry(pending[collection_index].definition.name.clone())
                                    .or_default()
                                    .absorb(&batch.profile);
                            }
                            if let (Some(started), Some(detail)) =
                                (admission_started, detail.as_mut())
                            {
                                detail.coordinator_ns += started.elapsed().as_nanos() as u64;
                            }
                            if let Some(segment) = pending[collection_index]
                                .admit_large_chunk_profiled(batch, detail.as_mut())?
                            {
                                schedule_private_segment(
                                    &mut scheduler,
                                    &mut private_group,
                                    collection_index,
                                    segment,
                                    Arc::clone(&definition_for_private),
                                    options,
                                    collection_count,
                                    &pool,
                                    &cancelled,
                                )
                                .map_err(|error| error.to_string())?;
                            }
                            finish_admission_profile(
                                &mut profile,
                                &pending[collection_index].definition.name,
                                admission_started,
                                detail,
                            )
                        },
                    );
                    let result = match process_result {
                        Ok(result) => result,
                        Err(error) => {
                            let drain_error = private_group.discard(&mut scheduler).err();
                            let error = match drain_error {
                                Some(drain) => format!("{error}; private segment drain: {drain}"),
                                None => error,
                            };
                            return Err(if cancelled() {
                                IndexError::Cancelled
                            } else {
                                IndexError::Build(error)
                            });
                        }
                    };
                    #[cfg(test)]
                    if definition_for_private.name == "__test_cancel_after_validation__"
                        && let Some(gate) = &scheduler.gate
                    {
                        gate.wait_after_validation();
                    }
                    #[cfg(test)]
                    if definition_for_private.name
                        == "__test_invalidate_after_private_finalization__"
                    {
                        let consume_error = private_group.consume_ready(&mut scheduler, true).err();
                        let finalized = private_group.indexed.len();
                        let drain_error = private_group.discard(&mut scheduler).err();
                        if consume_error.is_none() && finalized == 0 {
                            return Err(IndexError::Build(
                                "late invalidation test observed no finalized segment".into(),
                            ));
                        }
                        return Err(IndexError::Build(match drain_error {
                            Some(error) => format!(
                                "injected late invalidation; private segment drain: {error}"
                            ),
                            None => consume_error.unwrap_or_else(|| {
                                format!(
                                    "injected late framed-file invalidation after {finalized} finalized segment(s)"
                                )
                            }),
                        }));
                    }
                    if cancelled() {
                        let drain_error = private_group.discard(&mut scheduler).err();
                        return Err(match drain_error {
                            Some(error) => IndexError::Build(error),
                            None => IndexError::Cancelled,
                        });
                    }
                    if let Some((file, digest)) = result.file_fingerprint {
                        digest_bindings[file.0 as usize].push((collection, digest));
                    }
                    schedule_stats.absorb(result);
                    if let Some(sealed) = pending[collection_index].take_segment() {
                        let submit_result = schedule_private_segment(
                            &mut scheduler,
                            &mut private_group,
                            collection_index,
                            sealed,
                            Arc::clone(&definition_for_private),
                            options,
                            collection_count,
                            &pool,
                            &cancelled,
                        );
                        if let Err(error) = submit_result {
                            let drain_error = private_group.discard(&mut scheduler).err();
                            return Err(match (cancelled(), drain_error) {
                                (true, _) => IndexError::Cancelled,
                                (false, Some(drain)) => IndexError::Build(format!(
                                    "{error}; private segment drain: {drain}"
                                )),
                                (false, None) => error,
                            });
                        }
                    }
                    let publish_result = publish_private_segment_group(
                        &mut scheduler,
                        &index,
                        &mut expected,
                        private_group,
                        collection_index,
                        &pending,
                        &mut on_publication,
                        &mut publication_ns,
                        &mut profile,
                        &cancelled,
                    );
                    if let Err(error) = publish_result {
                        return Err(if cancelled() {
                            IndexError::Cancelled
                        } else {
                            error
                        });
                    }
                }
            }
        }
        if !normal.is_empty() {
            let result = crate::build::process_repository_file_batches_profiled(
                &pool,
                Arc::new(root.clone()),
                &normal,
                Arc::clone(&source_files),
                options.bigram_alphabet,
                options.inference,
                options.detailed_profiling_enabled(),
                |number, batch| {
                    admit_normal(
                        number,
                        batch,
                        &mut pending,
                        &mut digest_bindings,
                        &mut build_parses,
                        &mut failed_parses,
                        &mut source_reads,
                        &mut source_bytes_read,
                        &index,
                        &mut expected,
                        &mut scheduler,
                        options,
                        &pool,
                        &mut on_publication,
                        &mut publication_ns,
                        &mut profile,
                        &cancelled,
                    )
                },
            );
            schedule_stats.absorb(result.map_err(|error| {
                if cancelled() {
                    IndexError::Cancelled
                } else {
                    IndexError::Build(error)
                }
            })?);
        }
        let extraction_index_ns = extraction_started.elapsed().as_nanos() as u64;
        if let Some(profile) = profile.as_mut() {
            profile.source_work_ns = extraction_index_ns;
            profile.admission_schedule = schedule_stats.admission_schedule;
            profile.extraction_rss_bytes = build_profile_rss_bytes();
        }
        let flush_started = crate::detailed_timer::start_if_enabled(profile.is_some());
        for number in 0..pending.len() {
            if cancelled() {
                return Err(IndexError::Cancelled);
            }
            if let Some(sealed) = pending[number].take_segment() {
                schedule_segment(
                    &mut scheduler,
                    &index,
                    &mut expected,
                    &pending,
                    number,
                    sealed,
                    options,
                    &pool,
                    &mut on_publication,
                    &mut publication_ns,
                    &mut profile,
                    &cancelled,
                )?;
            }
        }
        scheduler.drain(
            &index,
            &mut expected,
            &pending,
            &mut on_publication,
            &mut publication_ns,
            &mut profile,
        )?;
        let peak_segment_jobs = scheduler.peak_running.load(Ordering::Relaxed);
        let peak_segment_window = scheduler.peak_outstanding;
        if let Some(profile) = profile.as_mut() {
            profile.peak_segment_jobs = peak_segment_jobs;
            profile.peak_segment_window = peak_segment_window;
        }
        if let (Some(started), Some(profile)) = (flush_started, profile.as_mut()) {
            profile.final_segment_flush_ns = started.elapsed().as_nanos() as u64;
        }
        let finalization_started = crate::detailed_timer::DetailedInstant::now();
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let base_fingerprints =
            finish_fresh_fingerprints(&discovered, digest_bindings).map_err(IndexError::Build)?;
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.fingerprint_finish_ns = started.elapsed().as_nanos() as u64;
        }
        let phase = crate::detailed_timer::start_if_enabled(profile.is_some());
        let mut final_repository = Self::build_fresh(root, definitions, options, &sources, false)
            .map_err(IndexError::Build)?;
        final_repository.base_fingerprints = base_fingerprints;
        final_repository.build_parses = build_parses;
        final_repository.build_failed_parses = failed_parses;
        let mut inference_build = InferenceBuildStats::default();
        let segment_view = index.view()?;
        let mut final_files = (*discovered).clone();
        let mut pending_relations = Vec::with_capacity(final_repository.collections.len());
        for file in &mut final_files.files {
            if let Some(current) = segment_view.file_entry(file.id) {
                file.status = current.status;
                file.omitted_diagnostics = current.omitted_diagnostics;
                for diagnostic in &current.diagnostics {
                    if !file.diagnostics.iter().any(|candidate| {
                        let mut candidate = candidate.clone();
                        candidate.record = None;
                        let mut diagnostic = diagnostic.clone();
                        diagnostic.record = None;
                        candidate == diagnostic
                    }) {
                        file.diagnostics.push(diagnostic.clone());
                    }
                }
            }
        }
        for (collection, state) in final_repository.collections.iter_mut().zip(&mut pending) {
            let segments = segment_view
                .segments(state.id)
                .ok_or_else(|| IndexError::Build("segment collection missing".into()))?;
            let mut relations = RelationContribution::for_schema(&state.definition.schema);
            let metadata = segment_metadata_generation(
                state.id,
                &state.definition.name,
                segments,
                &mut final_files,
                &mut relations,
            )
            .map_err(IndexError::Build)?;
            collection.generation = metadata.generation;
            collection.id_lookup = metadata.id_lookup;
            collection.identity_order = metadata.identity_order;
            collection.stats = metadata.stats;
            collection.logical_records = metadata.logical_records;
            collection.empty_array_records = metadata.empty_array_records;
            if let Some(first) = segments.first() {
                // Sort bindings need locale/options/direction metadata before
                // segment-local columns can be selected. Keys remain owned by
                // their source segments and are never addressed through this
                // compatibility header.
                collection.sorting = first.index.sorting.clone();
                for column in collection.sorting.columns.values_mut() {
                    column.keys.clear();
                    column.keys.shrink_to_fit();
                    column.ascending.clear();
                    column.ascending.shrink_to_fit();
                    column.descending.clear();
                    column.descending.shrink_to_fit();
                }
                for composite in &mut collection.sorting.composites {
                    composite.order.clear();
                    composite.order.shrink_to_fit();
                }
            }
            pending_relations.push(relations);
            let (_, stats) = state.finish_summary().map_err(IndexError::Build)?;
            collection.global_inference = crate::inference::build::aggregate_segment_inference(
                options.inference,
                &collection.definition.schema,
                segments,
                &collection.definition.name,
            )
            .map_err(IndexError::Build)?;
            inference_build.absorb(stats);
        }
        for (source_index, contributions) in pending_relations.into_iter().enumerate() {
            let source = final_repository.collections[source_index]
                .generation
                .collection;
            for contribution in &contributions {
                let logical_count = final_repository.collections[source_index]
                    .logical_records
                    .len() as u32;
                if let Some(posting) = final_repository.collections[source_index]
                    .postings
                    .fields
                    .get_mut(&contribution.declaration().path)
                {
                    posting.presence = contribution.presence().clone();
                    posting.values = crate::posting::PostingDictionary::from_public(
                        contribution.reverse_postings(),
                    );
                    posting.value_complete = true;
                    posting.covered_records = logical_count;
                }
            }
            let indexes = contributions
                .into_iter()
                .enumerate()
                .map(|(ordinal, contribution)| {
                    contribution.resolve(
                        source,
                        u32::try_from(ordinal).map_err(|_| "too many relations")?,
                        &final_repository.collections,
                    )
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(IndexError::Build)?;
            final_repository.collections[source_index].relations = indexes;
        }
        crate::relation::validate_indexes(&final_repository.collections)
            .map_err(IndexError::Build)?;
        final_repository.files = Arc::new(final_files);
        if let (Some(started), Some(profile)) = (phase, profile.as_mut()) {
            profile.finalize_generation_ns = started.elapsed().as_nanos() as u64;
            profile.finalization_rss_bytes = build_profile_rss_bytes();
        }
        final_repository.startup = StartupStats {
            build_ns: started.elapsed().as_nanos() as u64,
            build_discovery_ns: discovery_ns,
            build_extraction_index_ns: extraction_index_ns,
            build_publication_ns: publication_ns,
            build_finalization_ns: finalization_started.elapsed().as_nanos() as u64,
            build_workers,
            build_source_reads: source_reads.saturating_add(schedule_stats.source_reads),
            build_source_bytes_read: source_bytes_read
                .saturating_add(schedule_stats.source_bytes_read),
            peak_build_admitted_bytes: schedule_stats.peak_weight_bytes,
            build_weight_limit_bytes: schedule_stats.weight_limit_bytes,
            build_peak_active_jobs: schedule_stats.peak_active_jobs,
            build_peak_ready_batches: schedule_stats.peak_ready_batches,
            build_peak_segment_jobs: peak_segment_jobs,
            build_peak_segment_window: peak_segment_window,
            build_job_execution_ns: schedule_stats.job_execution_ns,
            build_receive_wait_ns: schedule_stats.receive_wait_ns,
            build_admission_ns: schedule_stats.admission_ns,
            inference_build,
            build_profile: profile,
            ..StartupStats::default()
        };
        if cancelled() {
            return Err(IndexError::Cancelled);
        }
        index.finish_initial_construction(expected, Arc::new(final_repository))?;
        Ok(index)
    }
}
