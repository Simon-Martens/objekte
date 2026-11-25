//! Ordered admission, bounded resident input, and receipt completion.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::lifecycle::{Index, IndexError, IndexRevision, RecordRegistration};

use super::spool::{self, SpoolManager};
use super::{Batch, IngestionError, IngestionOptions, Operation, PublishedRevision, SubmitError};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const MAX_PENDING_DESCRIPTORS: usize = 65_536;

struct ReceiptState {
    // Consecutive sequences with the same publication outcome share one span.
    // Receipts do not each retain a mutex and condition variable.
    results: Mutex<Vec<(u64, PublishedRevision)>>,
    changed: Condvar,
}

impl ReceiptState {
    fn new() -> Self {
        Self {
            results: Mutex::new(Vec::new()),
            changed: Condvar::new(),
        }
    }
    fn complete_through(&self, sequence: u64, result: PublishedRevision) {
        let mut guard = self.results.lock().expect("receipt mutex poisoned");
        if guard.last().is_some_and(|(last, _)| *last >= sequence) {
            return;
        }
        guard.push((sequence, result));
        self.changed.notify_all();
    }
}

/// One accepted batch. Dropping this handle never cancels installation.
pub struct Receipt {
    sequence: u64,
    state: Arc<ReceiptState>,
}

impl Receipt {
    pub fn sequence(&self) -> u64 {
        self.sequence
    }
    pub fn wait_published(&self) -> PublishedRevision {
        let mut guard = self.state.results.lock().expect("receipt mutex poisoned");
        while guard.last().is_none_or(|(last, _)| *last < self.sequence) {
            guard = self
                .state
                .changed
                .wait(guard)
                .expect("receipt mutex poisoned");
        }
        let position = guard.partition_point(|(last, _)| *last < self.sequence);
        guard.get(position).expect("terminal receipt").1.clone()
    }
}

enum Storage {
    Memory(Batch),
    Spool(spool::SpoolFrame),
    Failed(IngestionError),
}

#[derive(Clone, Copy, Eq, PartialEq)]
struct GroupInfo {
    registration: RecordRegistration,
    shape: (bool, bool),
    records: usize,
}

impl GroupInfo {
    fn for_batch(batch: &Batch) -> Option<Self> {
        Some(Self {
            registration: batch.registration,
            shape: group_shape(batch)?,
            records: batch.operations.len(),
        })
    }
}

struct Queued {
    sequence: u64,
    weight: usize,
    encoded_bytes: u64,
    storage: Storage,
    group: Option<GroupInfo>,
}

/// Spool overflow keeps only sequence and frame coordinates outside the
/// resident descriptor window. The shared receipt ledger owns completion.
struct OverflowQueued {
    sequence: u64,
    weight: usize,
    encoded_bytes: u64,
    frame: spool::SpoolFrame,
    group: Option<GroupInfo>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionMetrics {
    pub accepted_batches: u64,
    pub published_batches: u64,
    pub root_installations: u64,
    pub pending_ram_bytes: usize,
    pub pending_spool_bytes: u64,
    pub high_water_ram_bytes: usize,
    pub high_water_spool_bytes: u64,
    pub spool_written_bytes: u64,
    pub spool_read_bytes: u64,
    pub in_flight_estimated_bytes: usize,
    pub high_water_in_flight_estimated_bytes: usize,
    pub overflow_write_ns: u128,
    pub failure_sequence: Option<u64>,
}

struct State {
    items: VecDeque<Queued>,
    overflow: VecDeque<OverflowQueued>,
    next_sequence: u64,
    completed: u64,
    last_revision: IndexRevision,
    closed: bool,
    paused: bool,
    failure: Option<(u64, IngestionError)>,
    metrics: SessionMetrics,
}

struct Queue {
    state: Mutex<State>,
    receipts: Arc<ReceiptState>,
    changed: Condvar,
    spool: Mutex<SpoolManager>,
    encoding: Mutex<()>,
    options: IngestionOptions,
    group_target_records: usize,
    cancelled: AtomicBool,
    publication_decision: Mutex<()>,
    #[cfg(test)]
    worker_entry: Mutex<Option<WorkerEntryHook>>,
}

#[cfg(test)]
struct WorkerEntryHook {
    reached: std::sync::mpsc::Sender<u64>,
    release: std::sync::mpsc::Receiver<()>,
}

/// A session is the single ordered producer stream for one index. The index
/// retains a weak reference so finished sessions can be replaced.
pub(crate) struct SessionCore {
    pub(crate) id: u64,
    index: Index,
    queue: Arc<Queue>,
    worker: Mutex<Option<JoinHandle<()>>>,
    public_owners: AtomicUsize,
}

impl SessionCore {
    pub(crate) fn new(index: Index, options: IngestionOptions) -> Arc<Self> {
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let revision = index
            .view()
            .expect("index view available at session creation")
            .revision();
        let group_target_records = index
            .view()
            .expect("index view available at session creation")
            .repository()
            .options()
            .segment_policy
            .target_records
            .get();
        let queue = Arc::new(Queue {
            state: Mutex::new(State {
                items: VecDeque::new(),
                overflow: VecDeque::new(),
                next_sequence: 1,
                completed: 0,
                last_revision: revision,
                closed: false,
                paused: false,
                failure: None,
                metrics: SessionMetrics::default(),
            }),
            changed: Condvar::new(),
            receipts: Arc::new(ReceiptState::new()),
            spool: Mutex::new(SpoolManager::new(id, &options)),
            encoding: Mutex::new(()),
            options,
            group_target_records,
            cancelled: AtomicBool::new(false),
            publication_decision: Mutex::new(()),
            #[cfg(test)]
            worker_entry: Mutex::new(None),
        });
        let core = Arc::new(Self {
            id,
            index: index.clone(),
            queue: Arc::clone(&queue),
            worker: Mutex::new(None),
            public_owners: AtomicUsize::new(0),
        });
        let worker_core = Arc::clone(&core);
        let worker = std::thread::Builder::new()
            .name(format!("objekte-ingestion-{id}"))
            .spawn(move || {
                run_worker(index, queue, id);
                drop(worker_core);
            })
            .expect("ingestion worker thread could not start");
        *core.worker.lock().expect("worker mutex poisoned") = Some(worker);
        core
    }

    pub(crate) fn is_open(&self) -> bool {
        !self
            .queue
            .state
            .lock()
            .expect("queue mutex poisoned")
            .closed
    }

    pub(crate) fn options(&self) -> &IngestionOptions {
        &self.queue.options
    }

    pub(crate) fn is_drained(&self) -> bool {
        let state = self.queue.state.lock().expect("queue mutex poisoned");
        state.completed == state.next_sequence - 1
            && state.items.is_empty()
            && state.overflow.is_empty()
            && state.metrics.in_flight_estimated_bytes == 0
    }

    pub(crate) fn wait_drained(&self) {
        let mut state = self.queue.state.lock().expect("queue mutex poisoned");
        while state.completed != state.next_sequence - 1
            || !state.items.is_empty()
            || !state.overflow.is_empty()
            || state.metrics.in_flight_estimated_bytes != 0
        {
            state = self
                .queue
                .changed
                .wait(state)
                .expect("queue mutex poisoned");
        }
    }

    fn add_owner(&self) {
        self.public_owners.fetch_add(1, Ordering::Relaxed);
    }

    fn release_owner(&self) {
        if self.public_owners.fetch_sub(1, Ordering::AcqRel) == 1 {
            if let Ok(mut state) = self.queue.state.lock() {
                state.closed = true;
                state.paused = false;
                self.queue.changed.notify_all();
            }
        }
    }
}

impl Drop for SessionCore {
    fn drop(&mut self) {
        if let Ok(mut state) = self.queue.state.lock() {
            state.closed = true;
            state.paused = false;
            self.queue.changed.notify_all();
        }
    }
}

pub struct IngestionSession {
    pub(crate) core: Arc<SessionCore>,
}

impl IngestionSession {
    pub(crate) fn from_core(core: Arc<SessionCore>) -> Self {
        core.add_owner();
        Self { core }
    }

    pub fn writer(&self, registration: RecordRegistration) -> Result<RecordWriter, IndexError> {
        if !self.core.is_open() {
            return Err(IndexError::Cancelled);
        }
        let view = self.core.index.view()?;
        let source = view
            .repository()
            .record_registrations
            .get(registration.ordinal as usize)
            .filter(|source| source.handle == registration);
        let Some(source) = source else {
            return Err(IndexError::ForeignInstance);
        };
        self.core.add_owner();
        Ok(RecordWriter {
            core: Arc::clone(&self.core),
            registration,
            reader_available: source.reader.is_some(),
            file_links_available: source.file_links,
        })
    }

    pub fn metrics(&self) -> SessionMetrics {
        self.core
            .queue
            .state
            .lock()
            .expect("queue mutex poisoned")
            .metrics
    }

    /// Park the admission worker before any batch is accepted. This bounded
    /// measurement hook lets a producer exercise RAM and spool capacity while
    /// publication is intentionally held back.
    #[doc(hidden)]
    pub fn pause_worker_for_measurement(&self) -> Result<(), IndexError> {
        let queue = &self.core.queue;
        let mut state = queue.state.lock().map_err(|_| IndexError::LockPoisoned)?;
        if state.closed || state.next_sequence != 1 {
            return Err(IndexError::Build(
                "worker can only be paused before first admission".into(),
            ));
        }
        state.paused = true;
        Ok(())
    }

    /// Release a worker parked by `pause_worker_for_measurement`.
    #[doc(hidden)]
    pub fn resume_worker_for_measurement(&self) {
        if let Ok(mut state) = self.core.queue.state.lock() {
            state.paused = false;
            self.core.queue.changed.notify_all();
        }
    }

    pub fn flush(&self) -> PublishedRevision {
        let queue = &self.core.queue;
        let mut state = queue.state.lock().expect("queue mutex poisoned");
        let target = state.next_sequence - 1;
        while state.completed < target {
            state = queue.changed.wait(state).expect("queue mutex poisoned");
        }
        if let Some((sequence, error)) = &state.failure
            && *sequence <= target
        {
            return Err(error.clone());
        }
        Ok(state.last_revision)
    }

    pub fn finish(&self) -> PublishedRevision {
        {
            let mut state = self.core.queue.state.lock().expect("queue mutex poisoned");
            state.closed = true;
            state.paused = false;
            self.core.queue.changed.notify_all();
        }
        let result = self.flush();
        if let Some(worker) = self
            .core
            .worker
            .lock()
            .expect("worker mutex poisoned")
            .take()
        {
            let _ = worker.join();
        }
        result
    }

    /// Cancel work that has not yet installed. Accepted receipts are woken
    /// with the terminal cancellation sequence.
    pub fn cancel(&self) {
        let _decision = self
            .core
            .queue
            .publication_decision
            .lock()
            .expect("publication decision mutex poisoned");
        self.core.queue.cancelled.store(true, Ordering::Release);
        let mut state = self.core.queue.state.lock().expect("queue mutex poisoned");
        state.closed = true;
        state.paused = false;
        self.core.queue.changed.notify_all();
    }
}

impl Clone for IngestionSession {
    fn clone(&self) -> Self {
        Self::from_core(Arc::clone(&self.core))
    }
}

impl Drop for IngestionSession {
    fn drop(&mut self) {
        self.core.release_owner();
    }
}

pub struct RecordWriter {
    core: Arc<SessionCore>,
    registration: RecordRegistration,
    reader_available: bool,
    file_links_available: bool,
}

impl RecordWriter {
    pub fn batch(&self) -> Batch {
        Batch::new(self.core.id, self.registration)
            .with_capabilities(self.reader_available, self.file_links_available)
    }

    pub fn submit(&self, batch: Batch) -> Result<Receipt, SubmitError<Batch>> {
        self.offer(batch, false)
    }

    pub fn try_submit(&self, batch: Batch) -> Result<Receipt, SubmitError<Batch>> {
        self.offer(batch, true)
    }

    fn offer(&self, mut batch: Batch, memory_only: bool) -> Result<Receipt, SubmitError<Batch>> {
        let queue = &self.core.queue;
        batch.estimated_bytes = batch.owned_capacity_bytes();
        if batch.session_id != self.core.id || batch.registration != self.registration {
            return Err(SubmitError::ForeignHandle(batch));
        }
        if batch.is_empty() {
            return Err(SubmitError::EmptyBatch(batch));
        }
        if batch.estimated_bytes > queue.options.max_batch_bytes {
            return Err(SubmitError::BatchTooLarge(batch));
        }
        let weight = batch.estimated_bytes;
        let group = GroupInfo::for_batch(&batch);
        let lock = if memory_only {
            queue.state.try_lock().ok()
        } else {
            queue.state.lock().ok()
        };
        let Some(mut state) = lock else {
            return Err(SubmitError::WouldBlock(batch));
        };
        if state.closed || state.failure.is_some() {
            return Err(SubmitError::Closed(batch));
        }
        if state.items.len() < MAX_PENDING_DESCRIPTORS
            && state.overflow.is_empty()
            && state.metrics.pending_ram_bytes.saturating_add(weight)
                <= queue.options.pending_ram_bytes
        {
            state.metrics.pending_ram_bytes += weight;
            state.metrics.high_water_ram_bytes = state
                .metrics
                .high_water_ram_bytes
                .max(state.metrics.pending_ram_bytes);
            return Ok(enqueue(
                &mut state,
                queue,
                Storage::Memory(batch),
                weight,
                0,
                group,
            ));
        }
        if memory_only {
            return Err(SubmitError::WouldBlock(batch));
        }
        if queue.options.spool_bytes == 0 {
            return Err(SubmitError::Capacity(batch));
        }
        drop(state);
        let bytes = match spool::encoded_len(&batch) {
            Ok(bytes) => bytes,
            Err(message) => return Err(SubmitError::SpoolIo(batch, message)),
        };
        let mut state = queue.state.lock().expect("queue mutex poisoned");
        if state.closed || state.failure.is_some() {
            return Err(SubmitError::Closed(batch));
        }
        if state.metrics.pending_spool_bytes.saturating_add(bytes) > queue.options.spool_bytes {
            return Err(SubmitError::Capacity(batch));
        }
        state.metrics.pending_spool_bytes += bytes;
        state.metrics.high_water_spool_bytes = state
            .metrics
            .high_water_spool_bytes
            .max(state.metrics.pending_spool_bytes);
        drop(state);
        let _encoding = queue.encoding.lock().expect("encoding mutex poisoned");
        let encoded = match spool::encode(&batch) {
            Ok(encoded) => encoded,
            Err(message) => {
                let mut state = queue.state.lock().expect("queue mutex poisoned");
                state.metrics.pending_spool_bytes -= bytes;
                return Err(SubmitError::SpoolIo(batch, message));
            }
        };
        let started = Instant::now();
        let written = queue
            .spool
            .lock()
            .expect("spool mutex poisoned")
            .write(&encoded);
        let elapsed = started.elapsed().as_nanos();
        let mut state = queue.state.lock().expect("queue mutex poisoned");
        state.metrics.overflow_write_ns += elapsed;
        if let Err(message) = written {
            state.metrics.pending_spool_bytes -= bytes;
            return Err(SubmitError::SpoolIo(batch, message));
        }
        let frame = written.expect("spool write succeeded");
        state.metrics.spool_written_bytes = state.metrics.spool_written_bytes.saturating_add(bytes);
        if state.closed || state.failure.is_some() {
            drop(state);
            let reclaimed = queue
                .spool
                .lock()
                .expect("spool mutex poisoned")
                .release(&frame);
            let mut state = queue.state.lock().expect("queue mutex poisoned");
            state.metrics.pending_spool_bytes -= reclaimed;
            return Err(SubmitError::Closed(batch));
        }
        Ok(enqueue(
            &mut state,
            queue,
            Storage::Spool(frame),
            weight,
            bytes,
            group,
        ))
    }
}

impl Clone for RecordWriter {
    fn clone(&self) -> Self {
        self.core.add_owner();
        Self {
            core: Arc::clone(&self.core),
            registration: self.registration,
            reader_available: self.reader_available,
            file_links_available: self.file_links_available,
        }
    }
}

impl Drop for RecordWriter {
    fn drop(&mut self) {
        self.core.release_owner();
    }
}

fn enqueue(
    state: &mut State,
    queue: &Queue,
    storage: Storage,
    weight: usize,
    encoded_bytes: u64,
    group: Option<GroupInfo>,
) -> Receipt {
    let sequence = state.next_sequence;
    state.next_sequence += 1;
    state.metrics.accepted_batches += 1;
    if state.items.len() >= MAX_PENDING_DESCRIPTORS || !state.overflow.is_empty() {
        let Storage::Spool(frame) = storage else {
            unreachable!("memory admission checks descriptor capacity")
        };
        state.overflow.push_back(OverflowQueued {
            sequence,
            weight,
            encoded_bytes,
            frame,
            group,
        });
    } else {
        state.items.push_back(Queued {
            sequence,
            weight,
            encoded_bytes,
            storage,
            group,
        });
    }
    queue.changed.notify_all();
    Receipt {
        sequence,
        state: Arc::clone(&queue.receipts),
    }
}

fn group_shape(batch: &Batch) -> Option<(bool, bool)> {
    if !batch.files.is_empty() {
        return None;
    }
    let Operation::Insert {
        file: None,
        version,
        text,
        ..
    } = batch.operations.first()?
    else {
        return None;
    };
    let shape = (version.is_some(), text.is_empty());
    batch
        .operations
        .iter()
        .all(|operation| {
            matches!(operation, Operation::Insert { file: None, version, text, .. }
            if (version.is_some(), text.is_empty()) == shape)
        })
        .then_some(shape)
}

fn pop_queued(state: &mut State) -> Option<Queued> {
    state.items.pop_front().or_else(|| {
        state.overflow.pop_front().map(|overflow| Queued {
            sequence: overflow.sequence,
            weight: overflow.weight,
            encoded_bytes: overflow.encoded_bytes,
            storage: Storage::Spool(overflow.frame),
            group: overflow.group,
        })
    })
}

fn run_worker(index: Index, queue: Arc<Queue>, session_id: u64) {
    // A failed coalesced attempt is replayed in ordinal order from this
    // bounded in-flight prefix. No decoded spool batch is put back into the
    // resident admission window or charged against its configured RAM cap.
    let mut retry = VecDeque::<Queued>::new();
    loop {
        let (mut item, from_retry) = if let Some(item) = retry.pop_front() {
            (item, true)
        } else {
            let mut state = queue.state.lock().expect("queue mutex poisoned");
            while (state.items.is_empty() && state.overflow.is_empty() || state.paused)
                && !state.closed
            {
                state = queue.changed.wait(state).expect("queue mutex poisoned");
            }
            let Some(item) = pop_queued(&mut state) else {
                break;
            };
            if matches!(item.storage, Storage::Memory(_)) {
                state.metrics.pending_ram_bytes -= item.weight;
            }
            state.metrics.in_flight_estimated_bytes = item.weight;
            state.metrics.high_water_in_flight_estimated_bytes = state
                .metrics
                .high_water_in_flight_estimated_bytes
                .max(item.weight);
            (item, false)
        };
        let group_enabled = {
            #[cfg(test)]
            {
                queue
                    .worker_entry
                    .lock()
                    .expect("worker hook mutex poisoned")
                    .is_none()
            }
            #[cfg(not(test))]
            {
                true
            }
        };
        if group_enabled
            && !from_retry
            && let Some(info) = item.group
            && info.records < queue.group_target_records
        {
            let mut records = info.records;
            let mut bytes = item.weight;
            let mut group = vec![item];
            {
                let mut state = queue.state.lock().expect("queue mutex poisoned");
                loop {
                    let front = state
                        .items
                        .front()
                        .map(|queued| (queued.group, queued.weight))
                        .or_else(|| {
                            state
                                .overflow
                                .front()
                                .map(|queued| (queued.group, queued.weight))
                        });
                    let Some((Some(next_info), next_weight)) = front else {
                        break;
                    };
                    if next_info.registration != info.registration
                        || next_info.shape != info.shape
                        || records.saturating_add(next_info.records) > queue.group_target_records
                        || bytes.saturating_add(next_weight) > 4 * 1024 * 1024
                    {
                        break;
                    }
                    let next = pop_queued(&mut state).expect("front batch exists");
                    if matches!(next.storage, Storage::Memory(_)) {
                        state.metrics.pending_ram_bytes -= next.weight;
                    }
                    state.metrics.in_flight_estimated_bytes = state
                        .metrics
                        .in_flight_estimated_bytes
                        .saturating_add(next.weight);
                    state.metrics.high_water_in_flight_estimated_bytes = state
                        .metrics
                        .high_water_in_flight_estimated_bytes
                        .max(state.metrics.in_flight_estimated_bytes);
                    records += next_info.records;
                    bytes += next.weight;
                    group.push(next);
                }
            }
            if group.len() > 1 {
                let mut reclaimed = 0u64;
                let mut read_bytes = 0u64;
                for member in &mut group {
                    if matches!(member.storage, Storage::Spool(_)) {
                        let mut old = Storage::Failed(IngestionError::Cancelled {
                            originating_sequence: 0,
                        });
                        std::mem::swap(&mut member.storage, &mut old);
                        let Storage::Spool(frame) = old else {
                            unreachable!("spool member was selected")
                        };
                        let decoded = spool::read(&frame, queue.options.spool_bytes, session_id)
                            .map_err(IngestionError::Spool);
                        reclaimed = reclaimed.saturating_add(
                            queue
                                .spool
                                .lock()
                                .expect("spool mutex poisoned")
                                .release(&frame),
                        );
                        read_bytes = read_bytes.saturating_add(member.encoded_bytes);
                        member.storage = match decoded {
                            Ok(batch) => Storage::Memory(batch),
                            Err(error) => Storage::Failed(error),
                        };
                    }
                }
                let mut state = queue.state.lock().expect("queue mutex poisoned");
                state.metrics.pending_spool_bytes -= reclaimed;
                state.metrics.spool_read_bytes =
                    state.metrics.spool_read_bytes.saturating_add(read_bytes);
                drop(state);
                let first_sequence = group[0].sequence;
                let grouped = if group
                    .iter()
                    .any(|member| matches!(member.storage, Storage::Failed(_)))
                {
                    Err(IngestionError::Spool(
                        "group contains an unreadable frame".into(),
                    ))
                } else if queue.cancelled.load(Ordering::Acquire) {
                    Err(IngestionError::Cancelled {
                        originating_sequence: first_sequence,
                    })
                } else {
                    let mut combined = Batch::new(session_id, info.registration);
                    combined.operations.reserve(records);
                    for member in &group {
                        let Storage::Memory(batch) = &member.storage else {
                            unreachable!("group member decoded")
                        };
                        combined.operations.extend(batch.operations.iter().cloned());
                    }
                    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        super::publication::install_batch(
                            &index,
                            &queue.cancelled,
                            &queue.publication_decision,
                            combined,
                            first_sequence,
                        )
                    }))
                    .unwrap_or_else(|_| {
                        Err(IngestionError::Spool("ingestion worker panicked".into()))
                    })
                };
                if let Ok(revision) = grouped {
                    let last_sequence = group.last().expect("group has members").sequence;
                    let count = group.len() as u64;
                    queue.receipts.complete_through(last_sequence, Ok(revision));
                    let mut state = queue.state.lock().expect("queue mutex poisoned");
                    state.metrics.in_flight_estimated_bytes = 0;
                    state.completed = last_sequence;
                    state.last_revision = revision;
                    state.metrics.published_batches += count;
                    state.metrics.root_installations += 1;
                    queue.changed.notify_all();
                    continue;
                }
                retry = group.into();
                continue;
            } else {
                item = group.pop().expect("single group member remains");
            }
        }
        #[cfg(test)]
        {
            let hook = queue
                .worker_entry
                .lock()
                .expect("worker hook mutex poisoned");
            if let Some(hook) = hook.as_ref() {
                let _ = hook.reached.send(item.sequence);
                let _ = hook.release.recv();
            }
        }
        let batch = match item.storage {
            Storage::Memory(batch) => Ok(batch),
            Storage::Failed(error) => Err(error),
            Storage::Spool(frame) => {
                let result = spool::read(&frame, queue.options.spool_bytes, session_id)
                    .map_err(IngestionError::Spool);
                let reclaimed = queue
                    .spool
                    .lock()
                    .expect("spool mutex poisoned")
                    .release(&frame);
                let mut state = queue.state.lock().expect("queue mutex poisoned");
                state.metrics.pending_spool_bytes -= reclaimed;
                state.metrics.spool_read_bytes = state
                    .metrics
                    .spool_read_bytes
                    .saturating_add(item.encoded_bytes);
                result
            }
        };
        let result = if queue.cancelled.load(Ordering::Acquire) {
            Err(IngestionError::Cancelled {
                originating_sequence: item.sequence,
            })
        } else {
            batch.and_then(|batch| {
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::publication::install_batch(
                        &index,
                        &queue.cancelled,
                        &queue.publication_decision,
                        batch,
                        item.sequence,
                    )
                }))
                .unwrap_or_else(|_| Err(IngestionError::Spool("ingestion worker panicked".into())))
            })
        };
        queue
            .receipts
            .complete_through(item.sequence, result.clone());
        let mut state = queue.state.lock().expect("queue mutex poisoned");
        state.metrics.in_flight_estimated_bytes -= item.weight;
        state.completed = item.sequence;
        match result {
            Ok(revision) => {
                state.last_revision = revision;
                state.metrics.published_batches += 1;
                state.metrics.root_installations += 1;
            }
            Err(error) => {
                state.closed = true;
                state.failure = Some((item.sequence, error));
                state.metrics.failure_sequence = Some(item.sequence);
                let mut abandoned_frames = Vec::new();
                while let Some(pending) = retry.pop_front() {
                    state.metrics.in_flight_estimated_bytes -= pending.weight;
                    if let Storage::Spool(frame) = pending.storage {
                        abandoned_frames.push(frame);
                    }
                    state.completed = pending.sequence;
                }
                while let Some(pending) = state.items.pop_front() {
                    if matches!(pending.storage, Storage::Memory(_)) {
                        state.metrics.pending_ram_bytes -= pending.weight;
                    } else {
                        if let Storage::Spool(frame) = pending.storage {
                            abandoned_frames.push(frame);
                        }
                    }
                    state.completed = pending.sequence;
                }
                while let Some(pending) = state.overflow.pop_front() {
                    abandoned_frames.push(pending.frame);
                    state.completed = pending.sequence;
                }
                if state.completed > item.sequence {
                    queue.receipts.complete_through(
                        state.completed,
                        Err(IngestionError::Cancelled {
                            originating_sequence: item.sequence,
                        }),
                    );
                }
                drop(state);
                let mut spool = queue.spool.lock().expect("spool mutex poisoned");
                let mut reclaimed = 0u64;
                for frame in abandoned_frames {
                    reclaimed = reclaimed.saturating_add(spool.release(&frame));
                }
                drop(spool);
                let mut state = queue.state.lock().expect("queue mutex poisoned");
                state.metrics.pending_spool_bytes -= reclaimed;
                queue.changed.notify_all();
                continue;
            }
        }
        queue.changed.notify_all();
    }
    queue.spool.lock().expect("spool mutex poisoned").cleanup();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Value;
    use crate::schema::Schema;
    use std::sync::mpsc;
    use std::time::Duration;

    fn batch(session_id: u64, registration: RecordRegistration, id: &str) -> Batch {
        let mut batch = Batch::new(session_id, registration);
        batch.insert(id, Value::String("Leipzig".into())).unwrap();
        batch
    }

    #[test]
    #[ignore = "large paused-worker descriptor overflow stress"]
    fn spool_overflow_accepts_beyond_resident_descriptor_window() {
        let root = std::env::temp_dir().join(format!(
            "objekte-queue-descriptor-overflow-{}",
            std::process::id()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder.register_records(collection, "overflow", 1).unwrap();
        let index = builder.build().unwrap();
        let session = index
            .ingestion_with_options(IngestionOptions {
                pending_ram_bytes: 0,
                max_batch_bytes: 1024,
                spool_bytes: 64 * 1024 * 1024,
                spool_directory: Some(root.clone()),
            })
            .unwrap();
        session.pause_worker_for_measurement().unwrap();
        let writer = session.writer(registration).unwrap();
        let mut receipts = Vec::new();
        for number in 0..MAX_PENDING_DESCRIPTORS + 4 {
            receipts.push(
                writer
                    .submit(batch(
                        session.core.id,
                        registration,
                        &format!("r{number:05}"),
                    ))
                    .unwrap(),
            );
        }
        assert_eq!(
            session.core.queue.state.lock().unwrap().items.len(),
            MAX_PENDING_DESCRIPTORS
        );
        assert_eq!(session.core.queue.state.lock().unwrap().overflow.len(), 4);
        assert!(matches!(
            writer.try_submit(batch(session.core.id, registration, "later")),
            Err(SubmitError::WouldBlock(_))
        ));
        receipts.remove(100);
        session.cancel();
        for receipt in receipts {
            assert!(matches!(
                receipt.wait_published(),
                Err(IngestionError::Cancelled {
                    originating_sequence: 1
                })
            ));
        }
        assert!(session.finish().is_err());
        assert_eq!(session.metrics().pending_spool_bytes, 0);
        assert_eq!(index.status().unwrap().live_records, 0);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn paused_spooled_singletons_publish_as_one_initial_segment() {
        let root = std::env::temp_dir().join(format!(
            "objekte-queue-spool-group-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "spool-group", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index
            .ingestion_with_options(IngestionOptions {
                pending_ram_bytes: 0,
                max_batch_bytes: 1024,
                spool_bytes: 1024 * 1024,
                spool_directory: Some(root.clone()),
            })
            .unwrap();
        session.pause_worker_for_measurement().unwrap();
        let writer = session.writer(registration).unwrap();
        let receipts = (0..128)
            .map(|number| {
                writer
                    .submit(batch(
                        session.core.id,
                        registration,
                        &format!("r{number:03}"),
                    ))
                    .unwrap()
            })
            .collect::<Vec<_>>();
        session.resume_worker_for_measurement();
        let revision = receipts[0].wait_published().unwrap();
        for receipt in receipts.iter().skip(1) {
            assert_eq!(receipt.wait_published(), Ok(revision));
        }
        session.finish().unwrap();
        assert_eq!(session.metrics().root_installations, 1);
        assert_eq!(session.metrics().pending_spool_bytes, 0);
        assert_eq!(index.view().unwrap().segment_summaries().len(), 1);
        assert_eq!(index.status().unwrap().live_records, 128);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn paused_worker_spools_bounded_prefix_and_returns_retryable_batch() {
        let root = std::env::temp_dir().join(format!(
            "objekte-queue-pressure-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder.register_records(collection, "pressure", 1).unwrap();
        let frame_bytes = spool::encode(&batch(0, registration, "record-1"))
            .unwrap()
            .len();
        let index = builder.build().unwrap();
        let session = index
            .ingestion_with_options(IngestionOptions {
                pending_ram_bytes: 0,
                max_batch_bytes: 1024 * 1024,
                spool_bytes: frame_bytes * 2,
                spool_directory: Some(root.clone()),
            })
            .unwrap();
        let writer = session.writer(registration).unwrap();
        let (reached, arrivals) = mpsc::channel();
        let (release, exits) = mpsc::channel();
        *session.core.queue.worker_entry.lock().unwrap() = Some(WorkerEntryHook {
            reached,
            release: exits,
        });
        let first = writer
            .submit(batch(session.core.id, registration, "record-1"))
            .unwrap();
        assert_eq!(
            arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
            first.sequence()
        );
        let second = writer
            .submit(batch(session.core.id, registration, "record-2"))
            .unwrap();
        let spool_root = std::fs::read_dir(&root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read_dir(spool_root).unwrap().count(), 1);
        let third = match writer.try_submit(batch(session.core.id, registration, "record-3")) {
            Err(SubmitError::WouldBlock(batch)) => batch,
            _ => panic!("memory-only submission must return its batch"),
        };
        let third = match writer.submit(third) {
            Err(SubmitError::Capacity(batch)) => batch,
            _ => panic!("full spool must return its batch"),
        };
        assert_eq!(session.metrics().pending_spool_bytes, frame_bytes * 2);
        release.send(()).unwrap();
        first.wait_published().unwrap();
        assert_eq!(
            arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
            second.sequence()
        );
        // The first frame's bytes remain allocated in the shared chunk until
        // its last pending frame is consumed.
        let third = match writer.submit(third) {
            Err(SubmitError::Capacity(batch)) => batch,
            _ => panic!("partially consumed chunk still occupies spool quota"),
        };
        release.send(()).unwrap();
        second.wait_published().unwrap();
        let third = writer.submit(third).unwrap();
        assert_eq!(
            arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
            third.sequence()
        );
        release.send(()).unwrap();
        third.wait_published().unwrap();
        session.finish().unwrap();
        assert_eq!(session.metrics().accepted_batches, 3);
        assert_eq!(session.metrics().published_batches, 3);
        assert_eq!(session.metrics().pending_spool_bytes, 0);
        for id in ["record-1", "record-2", "record-3"] {
            assert!(
                index
                    .view()
                    .unwrap()
                    .get_record("entries", id, crate::query::options::ReturnMode::Locator)
                    .unwrap()
                    .is_some()
            );
        }
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn partial_spool_frame_fails_receipt_without_publication() {
        let root = std::env::temp_dir().join(format!(
            "objekte-queue-corruption-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "corruption", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index
            .ingestion_with_options(IngestionOptions {
                pending_ram_bytes: 0,
                max_batch_bytes: 1024 * 1024,
                spool_bytes: 1024 * 1024,
                spool_directory: Some(root.clone()),
            })
            .unwrap();
        let writer = session.writer(registration).unwrap();
        let (reached, arrivals) = mpsc::channel();
        let (release, exits) = mpsc::channel();
        *session.core.queue.worker_entry.lock().unwrap() = Some(WorkerEntryHook {
            reached,
            release: exits,
        });
        let first = writer
            .submit(batch(session.core.id, registration, "record-1"))
            .unwrap();
        assert_eq!(
            arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
            first.sequence()
        );
        let second = writer
            .submit(batch(session.core.id, registration, "record-2"))
            .unwrap();
        let directory = std::fs::read_dir(&root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let frame = std::fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .min()
            .unwrap();
        std::fs::write(frame, b"OBRQ").unwrap();
        release.send(()).unwrap();
        assert!(matches!(
            first.wait_published(),
            Err(IngestionError::Spool(_))
        ));
        assert!(
            matches!(second.wait_published(), Err(IngestionError::Cancelled { originating_sequence }) if originating_sequence == first.sequence())
        );
        assert!(session.finish().is_err());
        assert_eq!(session.metrics().published_batches, 0);
        assert!(
            index
                .view()
                .unwrap()
                .get_record(
                    "entries",
                    "record-1",
                    crate::query::options::ReturnMode::Locator
                )
                .unwrap()
                .is_none()
        );
        assert!(
            index
                .view()
                .unwrap()
                .get_record(
                    "entries",
                    "record-2",
                    crate::query::options::ReturnMode::Locator
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn measurement_pause_keeps_accepted_prefix_unpublished_until_resume() {
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder.register_records(collection, "paused", 1).unwrap();
        let index = builder.build().unwrap();
        let session = index
            .ingestion_with_options(IngestionOptions {
                pending_ram_bytes: 0,
                max_batch_bytes: 1024 * 1024,
                spool_bytes: 1024 * 1024,
                spool_directory: None,
            })
            .unwrap();
        session.pause_worker_for_measurement().unwrap();
        let writer = session.writer(registration).unwrap();
        let receipt = writer
            .submit(batch(session.core.id, registration, "record-1"))
            .unwrap();
        assert_eq!(session.metrics().accepted_batches, 1);
        assert_eq!(session.metrics().published_batches, 0);
        assert!(session.metrics().pending_spool_bytes > 0);
        assert!(
            index
                .view()
                .unwrap()
                .get_record(
                    "entries",
                    "record-1",
                    crate::query::options::ReturnMode::Locator
                )
                .unwrap()
                .is_none()
        );
        session.resume_worker_for_measurement();
        receipt.wait_published().unwrap();
        session.finish().unwrap();
        assert!(
            index
                .view()
                .unwrap()
                .get_record(
                    "entries",
                    "record-1",
                    crate::query::options::ReturnMode::Locator
                )
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn successor_waits_for_closed_session_to_publish_accepted_prefix() {
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder.register_records(collection, "drain", 1).unwrap();
        let index = builder.build().unwrap();
        let session = index.ingestion();
        let writer = session.writer(registration).unwrap();
        let (reached, arrivals) = mpsc::channel();
        let (release, exits) = mpsc::channel();
        *session.core.queue.worker_entry.lock().unwrap() = Some(WorkerEntryHook {
            reached,
            release: exits,
        });
        let first = writer
            .submit(batch(session.core.id, registration, "first"))
            .unwrap();
        assert_eq!(
            arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
            first.sequence()
        );
        let finish_session = session.clone();
        let finish = std::thread::spawn(move || finish_session.finish());
        while session.core.is_open() {
            std::thread::yield_now();
        }
        let (successor_sender, successor_receiver) = mpsc::channel();
        let successor_index = index.clone();
        let successor = std::thread::spawn(move || {
            let next = successor_index.ingestion();
            successor_sender.send(next).unwrap();
        });
        assert!(
            successor_receiver
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );
        release.send(()).unwrap();
        let revision = first.wait_published().unwrap();
        assert_eq!(finish.join().unwrap().unwrap(), revision);
        let next = successor_receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        successor.join().unwrap();
        let next_writer = next.writer(registration).unwrap();
        let second = next_writer
            .submit(batch(next.core.id, registration, "second"))
            .unwrap();
        assert!(second.wait_published().unwrap() > revision);
        assert!(
            index
                .view()
                .unwrap()
                .get_record(
                    "entries",
                    "first",
                    crate::query::options::ReturnMode::Locator
                )
                .unwrap()
                .is_some()
        );
        assert!(
            index
                .view()
                .unwrap()
                .get_record(
                    "entries",
                    "second",
                    crate::query::options::ReturnMode::Locator
                )
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn last_public_owner_drop_keeps_old_worker_until_accepted_prefix_finishes() {
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "owner-drop", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index.ingestion();
        let writer = session.writer(registration).unwrap();
        let (reached, arrivals) = mpsc::channel();
        let (release, exits) = mpsc::channel();
        *session.core.queue.worker_entry.lock().unwrap() = Some(WorkerEntryHook {
            reached,
            release: exits,
        });
        let first = writer
            .submit(batch(session.core.id, registration, "first"))
            .unwrap();
        assert_eq!(
            arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
            first.sequence()
        );
        drop(writer);
        drop(session);
        let (successor_sender, successor_receiver) = mpsc::channel();
        let successor_index = index.clone();
        let successor = std::thread::spawn(move || {
            successor_sender.send(successor_index.ingestion()).unwrap();
        });
        assert!(
            successor_receiver
                .recv_timeout(Duration::from_millis(50))
                .is_err()
        );
        release.send(()).unwrap();
        first.wait_published().unwrap();
        let next = successor_receiver
            .recv_timeout(Duration::from_secs(5))
            .unwrap();
        successor.join().unwrap();
        let next_writer = next.writer(registration).unwrap();
        next_writer
            .submit(batch(next.core.id, registration, "second"))
            .unwrap()
            .wait_published()
            .unwrap();
        for id in ["first", "second"] {
            assert!(
                index
                    .view()
                    .unwrap()
                    .get_record("entries", id, crate::query::options::ReturnMode::Locator)
                    .unwrap()
                    .is_some()
            );
        }
    }

    #[test]
    fn cancellation_keeps_installed_prefix_and_cancels_waiting_suffix() {
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder.register_records(collection, "cancel", 1).unwrap();
        let index = builder.build().unwrap();
        let session = index.ingestion();
        let writer = session.writer(registration).unwrap();
        let (reached, arrivals) = mpsc::channel();
        let (release, exits) = mpsc::channel();
        *session.core.queue.worker_entry.lock().unwrap() = Some(WorkerEntryHook {
            reached,
            release: exits,
        });
        let first = writer
            .submit(batch(session.core.id, registration, "first"))
            .unwrap();
        assert_eq!(
            arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
            first.sequence()
        );
        release.send(()).unwrap();
        let published = first.wait_published().unwrap();
        let second = writer
            .submit(batch(session.core.id, registration, "second"))
            .unwrap();
        let third = writer
            .submit(batch(session.core.id, registration, "third"))
            .unwrap();
        assert_eq!(
            arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
            second.sequence()
        );
        session.cancel();
        release.send(()).unwrap();
        assert!(
            matches!(second.wait_published(), Err(IngestionError::Cancelled { originating_sequence }) if originating_sequence == second.sequence())
        );
        assert!(
            matches!(third.wait_published(), Err(IngestionError::Cancelled { originating_sequence }) if originating_sequence == second.sequence())
        );
        assert_eq!(index.view().unwrap().revision(), published);
        assert!(
            index
                .view()
                .unwrap()
                .get_record(
                    "entries",
                    "first",
                    crate::query::options::ReturnMode::Locator
                )
                .unwrap()
                .is_some()
        );
        for id in ["second", "third"] {
            assert!(
                index
                    .view()
                    .unwrap()
                    .get_record("entries", id, crate::query::options::ReturnMode::Locator)
                    .unwrap()
                    .is_none()
            );
        }
        let next = index.ingestion();
        assert_ne!(next.core.id, session.core.id);
    }

    #[test]
    fn compatible_backlog_seals_one_segment_and_one_root_for_many_receipts() {
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder.register_records(collection, "group", 1).unwrap();
        let index = builder.build().unwrap();
        let session = index.ingestion();
        session.pause_worker_for_measurement().unwrap();
        let writer = session.writer(registration).unwrap();
        let mut receipts = Vec::new();
        for number in 0..128 {
            receipts.push(
                writer
                    .submit(batch(
                        session.core.id,
                        registration,
                        &format!("record-{number}"),
                    ))
                    .unwrap(),
            );
        }
        session.resume_worker_for_measurement();
        let revisions = receipts
            .iter()
            .map(Receipt::wait_published)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(revisions.iter().all(|revision| *revision == revisions[0]));
        session.finish().unwrap();
        assert_eq!(session.metrics().accepted_batches, 128);
        assert_eq!(session.metrics().published_batches, 128);
        assert_eq!(session.metrics().root_installations, 1);
        assert!(
            index
                .view()
                .unwrap()
                .segment_summaries()
                .iter()
                .any(|segment| segment.active_records == 128)
        );
        assert_eq!(index.status().unwrap().live_records, 128);
    }

    #[test]
    fn failed_middle_group_member_publishes_valid_prefix_only() {
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "group-failure", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index.ingestion();
        session.pause_worker_for_measurement().unwrap();
        let writer = session.writer(registration).unwrap();
        let first = writer
            .submit(batch(session.core.id, registration, "first"))
            .unwrap();
        let duplicate = writer
            .submit(batch(session.core.id, registration, "first"))
            .unwrap();
        let suffix = writer
            .submit(batch(session.core.id, registration, "suffix"))
            .unwrap();
        session.resume_worker_for_measurement();
        first.wait_published().unwrap();
        assert!(duplicate.wait_published().is_err());
        assert!(
            matches!(suffix.wait_published(), Err(IngestionError::Cancelled { originating_sequence }) if originating_sequence == duplicate.sequence())
        );
        assert_eq!(session.metrics().root_installations, 1);
        assert_eq!(index.status().unwrap().live_records, 1);
    }

    #[test]
    fn failed_middle_spooled_group_member_publishes_valid_prefix_only() {
        let root = std::env::temp_dir().join(format!(
            "objekte-spooled-group-failure-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "spooled-failure", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index
            .ingestion_with_options(IngestionOptions {
                pending_ram_bytes: 0,
                max_batch_bytes: 1024,
                spool_bytes: 1024 * 1024,
                spool_directory: Some(root.clone()),
            })
            .unwrap();
        session.pause_worker_for_measurement().unwrap();
        let writer = session.writer(registration).unwrap();
        let first = writer
            .submit(batch(session.core.id, registration, "first"))
            .unwrap();
        let duplicate = writer
            .submit(batch(session.core.id, registration, "first"))
            .unwrap();
        let suffix = writer
            .submit(batch(session.core.id, registration, "suffix"))
            .unwrap();
        session.resume_worker_for_measurement();
        first.wait_published().unwrap();
        assert!(duplicate.wait_published().is_err());
        assert!(matches!(
            suffix.wait_published(),
            Err(IngestionError::Cancelled { originating_sequence }) if originating_sequence == duplicate.sequence()
        ));
        assert!(session.finish().is_err());
        assert_eq!(session.metrics().pending_spool_bytes, 0);
        assert_eq!(session.metrics().root_installations, 1);
        assert_eq!(index.status().unwrap().live_records, 1);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(root).unwrap();
    }

    #[test]
    fn failed_spool_write_returns_batch_without_admission_ordinal() {
        let missing_parent = std::env::temp_dir().join(format!(
            "objekte-missing-spool-parent-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        assert!(!missing_parent.exists());
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "write-failure", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index
            .ingestion_with_options(IngestionOptions {
                pending_ram_bytes: 0,
                max_batch_bytes: 1024 * 1024,
                spool_bytes: 1024 * 1024,
                spool_directory: Some(missing_parent),
            })
            .unwrap();
        let writer = session.writer(registration).unwrap();
        let rejected = writer.submit(batch(session.core.id, registration, "record-1"));
        let rejected = match rejected {
            Err(SubmitError::SpoolIo(batch, _)) => batch,
            _ => panic!("unwritable spool must return the owned input"),
        };
        assert!(!rejected.is_empty());
        assert_eq!(session.metrics().accepted_batches, 0);
        assert_eq!(session.metrics().pending_spool_bytes, 0);
        session.finish().unwrap();
    }

    #[test]
    fn disk_full_after_partial_write_rolls_back_and_allows_retry() {
        let root = std::env::temp_dir().join(format!(
            "objekte-disk-full-spool-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir(&root).unwrap();
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "disk-full", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index
            .ingestion_with_options(IngestionOptions {
                pending_ram_bytes: 0,
                max_batch_bytes: 1024 * 1024,
                spool_bytes: 1024 * 1024,
                spool_directory: Some(root.clone()),
            })
            .unwrap();
        session
            .core
            .queue
            .spool
            .lock()
            .unwrap()
            .inject_disk_full_after(12);
        let writer = session.writer(registration).unwrap();
        let rejected = writer.submit(batch(session.core.id, registration, "record-1"));
        let rejected = match rejected {
            Err(SubmitError::SpoolIo(batch, message)) => {
                assert!(message.contains("No space left on device"));
                batch
            }
            _ => panic!("disk-full write must return owned input"),
        };
        assert_eq!(session.metrics().accepted_batches, 0);
        assert_eq!(session.metrics().pending_spool_bytes, 0);
        let spool_root = std::fs::read_dir(&root)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(std::fs::read_dir(&spool_root).unwrap().count(), 0);
        let receipt = writer.submit(rejected).unwrap();
        assert_eq!(receipt.sequence(), 1);
        receipt.wait_published().unwrap();
        session.finish().unwrap();
        assert_eq!(session.metrics().published_batches, 1);
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 0);
        std::fs::remove_dir(root).unwrap();
    }
}
