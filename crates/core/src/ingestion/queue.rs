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

const RECEIPTS_PER_BLOCK: u64 = 256;

/// Holds unfinished blocks and at most one partially filled tail. Completed
/// blocks remain alive only while callers hold receipts for them.
#[derive(Default)]
struct ReceiptLedger {
    blocks: Mutex<VecDeque<(u64, Arc<ReceiptState>)>>,
}

impl ReceiptLedger {
    fn accept(&self, sequence: u64) -> Receipt {
        let end = ((sequence - 1) / RECEIPTS_PER_BLOCK + 1) * RECEIPTS_PER_BLOCK;
        let mut blocks = self.blocks.lock().expect("receipt ledger poisoned");
        if blocks.back().is_none_or(|(last, _)| *last != end) {
            blocks.push_back((end, Arc::new(ReceiptState::new())));
        }
        Receipt {
            sequence,
            state: Arc::clone(&blocks.back().expect("open receipt block").1),
        }
    }

    fn complete_through(&self, sequence: u64, result: PublishedRevision) {
        let mut blocks = self.blocks.lock().expect("receipt ledger poisoned");
        while let Some((end, state)) = blocks.front() {
            state.complete_through(sequence.min(*end), result.clone());
            if *end > sequence {
                break;
            }
            let reached_end = *end == sequence;
            blocks.pop_front();
            if reached_end {
                break;
            }
        }
    }

    fn close(&self) {
        self.blocks.lock().expect("receipt ledger poisoned").clear();
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionMetrics {
    pub accepted_batches: u64,
    pub published_batches: u64,
    pub root_installations: u64,
    pub pending_ram_bytes: usize,
    pub pending_spool_bytes: u64,
    pub high_water_ram_bytes: usize,
    pub high_water_spool_bytes: u64,
    pub high_water_waiting_descriptors: usize,
    pub spool_written_bytes: u64,
    pub spool_read_bytes: u64,
    pub in_flight_batches: usize,
    pub high_water_in_flight_batches: usize,
    pub in_flight_estimated_bytes: usize,
    pub high_water_in_flight_estimated_bytes: usize,
    pub overflow_write_ns: u128,
    pub failure_sequence: Option<u64>,
}

struct State {
    items: VecDeque<Queued>,
    overflow: u64,
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
    receipts: ReceiptLedger,
    changed: Condvar,
    spool: Mutex<SpoolManager>,
    encoding: Mutex<()>,
    options: IngestionOptions,
    group_target_records: usize,
    cancelled: AtomicBool,
    publication_decision: Mutex<()>,
    #[cfg(test)]
    worker_entry: Mutex<Option<WorkerEntryHook>>,
    #[cfg(test)]
    prepared: Mutex<Option<WorkerEntryHook>>,
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
                overflow: 0,
                next_sequence: 1,
                completed: 0,
                last_revision: revision,
                closed: false,
                paused: false,
                failure: None,
                metrics: SessionMetrics::default(),
            }),
            changed: Condvar::new(),
            receipts: ReceiptLedger::default(),
            spool: Mutex::new(SpoolManager::new(id, &options)),
            encoding: Mutex::new(()),
            options,
            group_target_records,
            cancelled: AtomicBool::new(false),
            publication_decision: Mutex::new(()),
            #[cfg(test)]
            worker_entry: Mutex::new(None),
            #[cfg(test)]
            prepared: Mutex::new(None),
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
            && state.overflow == 0
            && state.metrics.in_flight_estimated_bytes == 0
    }

    pub(crate) fn wait_drained(&self) {
        let mut state = self.queue.state.lock().expect("queue mutex poisoned");
        while state.completed != state.next_sequence - 1
            || !state.items.is_empty()
            || state.overflow != 0
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
}

impl RecordWriter {
    pub fn batch(&self) -> Batch {
        Batch::new(self.core.id, self.registration).with_reader_capability(self.reader_available)
    }

    pub fn submit(&self, batch: Batch) -> Result<Receipt, SubmitError<Batch>> {
        self.offer(batch, false)
    }

    pub fn try_submit(&self, batch: Batch) -> Result<Receipt, SubmitError<Batch>> {
        self.offer(batch, true)
    }

    fn offer(&self, batch: Batch, memory_only: bool) -> Result<Receipt, SubmitError<Batch>> {
        let queue = &self.core.queue;
        debug_assert_eq!(batch.estimated_bytes, batch.owned_capacity_bytes());
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
            && state.overflow == 0
            && state.metrics.pending_ram_bytes.saturating_add(weight)
                <= queue.options.pending_ram_bytes
        {
            state.metrics.pending_ram_bytes += weight;
            state.metrics.high_water_ram_bytes = state
                .metrics
                .high_water_ram_bytes
                .max(state.metrics.pending_ram_bytes);
            let group = (batch.operations.len() <= 128)
                .then(|| GroupInfo::for_batch(&batch))
                .flatten();
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
        // Encoders serialize reservation and write. The state lock may enter
        // spool bookkeeping, but spool I/O releases its lock before taking state.
        let _encoding = queue.encoding.lock().expect("encoding mutex poisoned");
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
                .discard_uncommitted_tail(&frame);
            let mut state = queue.state.lock().expect("queue mutex poisoned");
            state.metrics.pending_spool_bytes -= reclaimed;
            return Err(SubmitError::Closed(batch));
        }
        let group = GroupInfo::for_batch(&batch);
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
    if state.items.len() >= MAX_PENDING_DESCRIPTORS || state.overflow != 0 {
        let Storage::Spool(frame) = storage else {
            unreachable!("memory admission checks descriptor capacity")
        };
        queue
            .spool
            .lock()
            .expect("spool mutex poisoned")
            .queue_waiting(&frame);
        state.overflow += 1;
    } else {
        state.items.push_back(Queued {
            sequence,
            weight,
            encoded_bytes,
            storage,
            group,
        });
        state.metrics.high_water_waiting_descriptors = state
            .metrics
            .high_water_waiting_descriptors
            .max(state.items.len());
    }
    let receipt = queue.receipts.accept(sequence);
    queue.changed.notify_all();
    receipt
}

pub(super) fn group_shape(batch: &Batch) -> Option<(bool, bool)> {
    let Operation::Insert { version, text, .. } = batch.operations.first()? else {
        return None;
    };
    let shape = (version.is_some(), text.is_empty());
    batch
        .operations
        .iter()
        .all(|operation| {
            matches!(operation, Operation::Insert { version, text, .. }
            if (version.is_some(), text.is_empty()) == shape)
        })
        .then_some(shape)
}

/// Only the worker reads or advances a waiting cursor. It keeps the suffix
/// present in admission state until the frame becomes active.
fn take_next(
    queue: &Queue,
    wait: bool,
    eligible: impl FnOnce(Option<GroupInfo>, usize) -> bool,
) -> Option<Queued> {
    let mut state = queue.state.lock().expect("queue mutex poisoned");
    if wait {
        while (state.items.is_empty() && state.overflow == 0 || state.paused) && !state.closed {
            state = queue.changed.wait(state).expect("queue mutex poisoned");
        }
    }
    let item = if let Some(front) = state.items.front() {
        if !eligible(front.group, front.weight) {
            return None;
        }
        state.items.pop_front().expect("checked resident prefix")
    } else if state.overflow > 0 {
        let sequence = state.next_sequence - state.overflow;
        drop(state);
        let mut spool = queue.spool.lock().expect("spool mutex poisoned");
        let item = match spool.peek_waiting(queue.options.max_batch_bytes) {
            Ok(waiting) => {
                let group = waiting.shape.map(|shape| GroupInfo {
                    registration: waiting.registration,
                    shape,
                    records: waiting.records,
                });
                if !eligible(group, waiting.weight) {
                    return None;
                }
                spool.consume_waiting(&waiting.frame);
                Queued {
                    sequence,
                    weight: waiting.weight,
                    encoded_bytes: waiting.frame.length,
                    storage: Storage::Spool(waiting.frame),
                    group,
                }
            }
            Err(error) if wait => Queued {
                sequence,
                weight: 0,
                encoded_bytes: 0,
                storage: Storage::Failed(IngestionError::Spool(error)),
                group: None,
            },
            Err(_) => return None,
        };
        drop(spool);
        state = queue.state.lock().expect("queue mutex poisoned");
        state.overflow -= 1;
        item
    } else {
        return None;
    };
    if matches!(item.storage, Storage::Memory(_)) {
        state.metrics.pending_ram_bytes -= item.weight;
    }
    state.metrics.in_flight_batches += 1;
    state.metrics.high_water_in_flight_batches = state
        .metrics
        .high_water_in_flight_batches
        .max(state.metrics.in_flight_batches);
    state.metrics.in_flight_estimated_bytes += item.weight;
    state.metrics.high_water_in_flight_estimated_bytes = state
        .metrics
        .high_water_in_flight_estimated_bytes
        .max(state.metrics.in_flight_estimated_bytes);
    Some(item)
}

fn decode_item(
    queue: &Queue,
    session_id: u64,
    storage: Storage,
    encoded_bytes: u64,
) -> Result<Batch, IngestionError> {
    match storage {
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
            state.metrics.spool_read_bytes =
                state.metrics.spool_read_bytes.saturating_add(encoded_bytes);
            result
        }
    }
}

fn run_worker(index: Index, queue: Arc<Queue>, session_id: u64) {
    loop {
        let Some(mut item) = take_next(&queue, true, |_, _| true) else {
            break;
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
            && let Some(info) = item.group
            && info.records < queue.group_target_records
        {
            // Large batches already amortize construction. Limiting their
            // coalesced group keeps transient indexed columns and retained
            // bodies from raising peak RSS merely to save root swaps.
            let group_record_limit = queue.group_target_records.min(if info.records == 1 {
                usize::MAX
            } else {
                info.records.saturating_mul(4).max(512)
            });
            let mut records = info.records;
            let mut bytes = item.weight;
            let mut group = vec![item];
            while let Some(next) = take_next(&queue, false, |next_info, next_weight| {
                next_info.is_some_and(|next_info| {
                    next_info.registration == info.registration
                        && next_info.shape == info.shape
                        && records.saturating_add(next_info.records) <= group_record_limit
                        && bytes.saturating_add(next_weight) <= 4 * 1024 * 1024
                })
            }) {
                records += next.group.expect("compatible group member").records;
                bytes += next.weight;
                group.push(next);
            }
            if group.len() > 1 {
                let first_sequence = group[0].sequence;
                let mut members = Vec::with_capacity(group.len());
                let mut failure = None;
                for member in group {
                    if queue.cancelled.load(Ordering::Acquire) {
                        failure = Some((
                            member.sequence,
                            IngestionError::Cancelled {
                                originating_sequence: member.sequence,
                            },
                        ));
                        break;
                    }
                    match decode_item(&queue, session_id, member.storage, member.encoded_bytes) {
                        Ok(batch) => members.push((member.sequence, batch)),
                        Err(error) => {
                            failure = Some((member.sequence, error));
                            break;
                        }
                    }
                }
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    super::publication::install_group(
                        &index,
                        &queue.cancelled,
                        &queue.publication_decision,
                        info.registration,
                        members,
                        records,
                        failure,
                        #[cfg(test)]
                        || {
                            if let Some(hook) = queue.prepared.lock().unwrap().take() {
                                hook.reached.send(first_sequence).unwrap();
                                hook.release.recv().unwrap();
                            }
                        },
                    )
                }))
                .unwrap_or_else(|_| super::publication::GroupPublication {
                    committed: None,
                    failure: Some((
                        first_sequence,
                        IngestionError::Spool("ingestion worker panicked".into()),
                    )),
                });
                if let Some((last_sequence, revision)) = outcome.committed {
                    queue.receipts.complete_through(last_sequence, Ok(revision));
                    let mut state = queue.state.lock().expect("queue mutex poisoned");
                    state.completed = last_sequence;
                    state.last_revision = revision;
                    state.metrics.published_batches += last_sequence - first_sequence + 1;
                    state.metrics.root_installations += 1;
                    if outcome.failure.is_none() {
                        state.metrics.in_flight_estimated_bytes = 0;
                        state.metrics.in_flight_batches = 0;
                    }
                    queue.changed.notify_all();
                }
                let Some((sequence, error)) = outcome.failure else {
                    continue;
                };
                item = Queued {
                    sequence,
                    weight: 0,
                    encoded_bytes: 0,
                    storage: Storage::Failed(error),
                    group: None,
                };
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
        let batch = decode_item(&queue, session_id, item.storage, item.encoded_bytes);
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
                        #[cfg(test)]
                        || {
                            if let Some(hook) = queue.prepared.lock().unwrap().take() {
                                hook.reached.send(item.sequence).unwrap();
                                hook.release.recv().unwrap();
                            }
                        },
                    )
                }))
                .unwrap_or_else(|_| Err(IngestionError::Spool("ingestion worker panicked".into())))
            })
        };
        queue
            .receipts
            .complete_through(item.sequence, result.clone());
        let mut state = queue.state.lock().expect("queue mutex poisoned");
        state.metrics.in_flight_estimated_bytes = 0;
        state.metrics.in_flight_batches = 0;
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
                state.items.clear();
                state.overflow = 0;
                state.metrics.pending_ram_bytes = 0;
                state.metrics.in_flight_estimated_bytes = 0;
                state.completed = state.next_sequence - 1;
                if state.completed > item.sequence {
                    queue.receipts.complete_through(
                        state.completed,
                        Err(IngestionError::Cancelled {
                            originating_sequence: item.sequence,
                        }),
                    );
                }
                drop(state);
                let _encoding = queue.encoding.lock().expect("encoding mutex poisoned");
                queue.spool.lock().expect("spool mutex poisoned").cleanup();
                let mut state = queue.state.lock().expect("queue mutex poisoned");
                state.metrics.pending_spool_bytes = 0;
                queue.changed.notify_all();
                continue;
            }
        }
        queue.changed.notify_all();
    }
    queue.receipts.close();
    let _encoding = queue.encoding.lock().expect("encoding mutex poisoned");
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
    fn receipt_ledger_releases_old_completed_blocks() {
        let ledger = ReceiptLedger::default();
        let early = ledger.accept(1);
        let revision = crate::lifecycle::IndexRevision::INITIAL;
        ledger.complete_through(1, Ok(revision));
        for sequence in 2..=4 * RECEIPTS_PER_BLOCK {
            drop(ledger.accept(sequence));
            ledger.complete_through(sequence, Ok(revision));
        }
        assert!(ledger.blocks.lock().unwrap().is_empty());
        assert_eq!(
            early.state.results.lock().unwrap().len(),
            RECEIPTS_PER_BLOCK as usize
        );
        assert_eq!(early.wait_published().unwrap(), revision);
        assert_eq!(early.wait_published().unwrap(), revision);
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
        for number in 0..120_000 {
            if number == 100_000 {
                let metrics = session.metrics();
                assert_eq!(metrics.accepted_batches, 100_000);
                assert_eq!(
                    metrics.high_water_waiting_descriptors,
                    MAX_PENDING_DESCRIPTORS
                );
            }
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
        assert_eq!(session.metrics().accepted_batches, 120_000);
        assert_eq!(
            session.metrics().high_water_waiting_descriptors,
            MAX_PENDING_DESCRIPTORS
        );
        assert_eq!(
            session.core.queue.state.lock().unwrap().overflow,
            120_000 - MAX_PENDING_DESCRIPTORS as u64
        );
        assert!(matches!(
            writer.try_submit(batch(session.core.id, registration, "later")),
            Err(SubmitError::WouldBlock(_))
        ));
        let paused = session.metrics();
        assert!(paused.pending_spool_bytes > 0);
        let start = Arc::new(std::sync::Barrier::new(5));
        let mut producers = Vec::new();
        for producer in 0..4 {
            let writer = writer.clone();
            let start = Arc::clone(&start);
            let session_id = session.core.id;
            producers.push(std::thread::spawn(move || {
                start.wait();
                (0..256)
                    .map(|number| {
                        writer
                            .submit(batch(
                                session_id,
                                registration,
                                &format!("tail-{producer}-{number}"),
                            ))
                            .unwrap()
                    })
                    .collect::<Vec<_>>()
            }));
        }
        session.resume_worker_for_measurement();
        start.wait();
        for producer in producers {
            receipts.extend(producer.join().unwrap());
        }
        receipts.sort_unstable_by_key(Receipt::sequence);
        for (number, receipt) in receipts.into_iter().enumerate() {
            assert_eq!(receipt.sequence(), number as u64 + 1);
            receipt.wait_published().unwrap();
        }
        session.finish().unwrap();
        let finished = session.metrics();
        assert_eq!(finished.pending_spool_bytes, 0);
        assert_eq!(finished.spool_read_bytes, finished.spool_written_bytes);
        assert_eq!(index.status().unwrap().live_records, 121_024);
        eprintln!("paused={paused:?} finished={finished:?}");
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
    fn group_seals_at_policy_target_and_preserves_later_ordered_changes() {
        let mut builder =
            Index::builder().with_options(crate::repository::configuration::RepositoryOptions {
                segment_policy: crate::index::segment::SegmentPolicy::with_target_records(
                    std::num::NonZeroUsize::new(4).unwrap(),
                )
                .unwrap(),
                ..Default::default()
            });
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "group-target", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index.ingestion();
        session.pause_worker_for_measurement().unwrap();
        let writer = session.writer(registration).unwrap();
        let mut large = writer.batch();
        for number in 0..6 {
            large
                .insert(
                    format!("large-{number}"),
                    crate::model::Value::String("large".into()),
                )
                .unwrap();
        }
        let large_receipt = writer.submit(large).unwrap();
        let mut receipts = Vec::new();
        for number in 0..9 {
            receipts.push(
                writer
                    .submit(batch(
                        session.core.id,
                        registration,
                        &format!("small-{number}"),
                    ))
                    .unwrap(),
            );
        }
        session.resume_worker_for_measurement();
        let large_revision = large_receipt.wait_published().unwrap();
        let revisions = receipts
            .iter()
            .map(Receipt::wait_published)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(revisions[0] > large_revision);
        assert!(
            revisions[0..4]
                .iter()
                .all(|revision| *revision == revisions[0])
        );
        assert!(
            revisions[4..8]
                .iter()
                .all(|revision| *revision == revisions[4])
        );
        assert!(revisions[8] > revisions[4]);
        assert_eq!(index.status().unwrap().live_records, 15);

        let mut replacement = writer.batch();
        replacement
            .replace("small-0", crate::model::Value::String("replaced".into()))
            .unwrap();
        let replaced_revision = writer
            .submit(replacement)
            .unwrap()
            .wait_published()
            .unwrap();
        assert!(replaced_revision > revisions[8]);
        let mut deletion = writer.batch();
        deletion.delete("small-1").unwrap();
        writer.submit(deletion).unwrap().wait_published().unwrap();
        let idle = writer
            .submit(batch(session.core.id, registration, "idle"))
            .unwrap();
        assert_eq!(session.flush().unwrap(), idle.wait_published().unwrap());
        let later = writer
            .submit(batch(session.core.id, registration, "later"))
            .unwrap();
        assert_eq!(session.flush().unwrap(), later.wait_published().unwrap());
        session.finish().unwrap();
        assert_eq!(index.status().unwrap().live_records, 16);
        assert_eq!(session.metrics().accepted_batches, 14);
        assert_eq!(session.metrics().published_batches, 14);
        let view = index.view().unwrap();
        assert!(
            view.get_record(
                "entries",
                "small-1",
                crate::query::options::ReturnMode::Locator
            )
            .unwrap()
            .is_none()
        );
        assert!(
            view.get_record(
                "entries",
                "later",
                crate::query::options::ReturnMode::Locator
            )
            .unwrap()
            .is_some()
        );
    }

    #[test]
    fn prepared_direct_batches_survive_unrelated_file_publication() {
        use crate::files::FileCatalog;
        use crate::query::options::ReturnMode;
        use crate::repository::configuration::IdentityPolicy;

        for (grouped, collision) in [(false, false), (true, false), (false, true), (true, true)] {
            let root = std::env::temp_dir().join(format!(
                "objekte-prepared-direct-{}-{grouped}-{collision}",
                std::process::id()
            ));
            std::fs::create_dir_all(root.join("data")).unwrap();
            let mut builder = Index::builder();
            let collection = builder.collection("entries", Schema::default()).unwrap();
            let registration = builder
                .register_records(collection, "prepared-test", 1)
                .unwrap();
            builder
                .register_file_catalog(FileCatalog::new(&root).unwrap())
                .unwrap();
            let mut definition = crate::source::test_support::document_folder("entries", "data");
            definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
            builder
                .register_file_source(collection, definition.file_source())
                .unwrap();
            let index = builder.build().unwrap();
            let session = index.ingestion();
            let writer = session.writer(registration).unwrap();
            writer
                .submit(batch(session.core.id, registration, "prefix"))
                .unwrap()
                .wait_published()
                .unwrap();
            let (reached, arrivals) = mpsc::channel();
            let (release, exits) = mpsc::channel();
            *session.core.queue.prepared.lock().unwrap() = Some(WorkerEntryHook {
                reached,
                release: exits,
            });
            if grouped {
                session.core.queue.state.lock().unwrap().paused = true;
            }
            let first = writer
                .submit(batch(session.core.id, registration, "manual-a"))
                .unwrap();
            if !grouped {
                assert_eq!(
                    arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
                    first.sequence()
                );
            }
            let mut second_batch = writer.batch();
            if grouped && collision {
                second_batch
                    .insert("must-not-leak", Value::String("staged".into()))
                    .unwrap();
            }
            second_batch
                .insert("manual-b", Value::String("Leipzig".into()))
                .unwrap();
            let second = writer.submit(second_batch).unwrap();
            let suffix = (grouped && collision).then(|| {
                writer
                    .submit(batch(session.core.id, registration, "cancelled-tail"))
                    .unwrap()
            });
            if grouped {
                session.resume_worker_for_measurement();
                assert_eq!(
                    arrivals.recv_timeout(Duration::from_secs(5)).unwrap(),
                    first.sequence()
                );
            }
            let file_id = match (grouped, collision) {
                (true, true) => "manual-b",
                (false, true) => "manual-a",
                (_, false) => "file-a",
            };
            let path = root.join("data/item.json");
            std::fs::write(&path, format!(r#"{{"id":"{file_id}"}}"#)).unwrap();
            index.add_file(&path).unwrap();
            let pinned = index.view().unwrap();
            assert!(
                pinned
                    .get_record("entries", file_id, ReturnMode::Locator)
                    .unwrap()
                    .unwrap()
                    .file
                    .is_some()
            );
            assert!(
                pinned
                    .get_record(
                        "entries",
                        if file_id == "manual-b" {
                            "manual-a"
                        } else {
                            "manual-b"
                        },
                        ReturnMode::Locator
                    )
                    .unwrap()
                    .is_none()
            );
            release.send(()).unwrap();
            if let Some(suffix) = suffix {
                assert!(first.wait_published().unwrap() > pinned.revision());
                assert!(matches!(
                    second.wait_published(),
                    Err(IngestionError::Index(
                        crate::lifecycle::IndexError::DuplicateRecordId { .. }
                    ))
                ));
                assert!(
                    matches!(suffix.wait_published(), Err(IngestionError::Cancelled {
                    originating_sequence
                }) if originating_sequence == second.sequence())
                );
                let view = index.view().unwrap();
                assert!(
                    view.get_record("entries", "must-not-leak", ReturnMode::Locator)
                        .unwrap()
                        .is_none()
                );
                assert_eq!(index.status().unwrap().live_records, 3);
            } else if collision {
                assert!(matches!(
                    first.wait_published(),
                    Err(IngestionError::Index(
                        crate::lifecycle::IndexError::DuplicateRecordId { .. }
                    ))
                ));
                assert!(
                    matches!(second.wait_published(), Err(IngestionError::Cancelled {
                    originating_sequence
                }) if originating_sequence == first.sequence())
                );
                assert_eq!(index.status().unwrap().live_records, 2);
            } else {
                let first_revision = first.wait_published().unwrap();
                let second_revision = second.wait_published().unwrap();
                assert!(first_revision > pinned.revision());
                assert_eq!(first_revision == second_revision, grouped);
                assert_eq!(index.status().unwrap().live_records, 4);
            }
            let view = index.view().unwrap();
            assert!(
                view.get_record("entries", "prefix", ReturnMode::Locator)
                    .unwrap()
                    .is_some()
            );
            assert!(
                pinned
                    .get_record(
                        "entries",
                        if file_id == "manual-b" {
                            "manual-a"
                        } else {
                            "manual-b"
                        },
                        ReturnMode::Locator
                    )
                    .unwrap()
                    .is_none()
            );
            let _ = session.finish();
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn coalesced_retained_payload_keeps_its_owned_string() {
        let mut builder = Index::builder();
        let collection = builder.collection("entries", Schema::default()).unwrap();
        let registration = builder
            .register_records(collection, "group-payload", 1)
            .unwrap();
        let index = builder.build().unwrap();
        let session = index.ingestion();
        let writer = session.writer(registration).unwrap();
        session.pause_worker_for_measurement().unwrap();
        let first = "Leipzig".repeat(8192);
        let original_allocation = first.as_ptr();
        let mut input = writer.batch();
        input.insert("first", Value::String(first)).unwrap();
        let a = writer.submit(input).unwrap();
        let b = writer
            .submit(batch(session.core.id, registration, "second"))
            .unwrap();
        session.resume_worker_for_measurement();
        assert_eq!(a.wait_published().unwrap(), b.wait_published().unwrap());
        session.finish().unwrap();
        let view = index.view().unwrap();
        let segment = &view.0.segments[0][0].index;
        let body = segment
            .backing
            .retained_body(crate::model::RecordSlot(0))
            .unwrap();
        let Value::String(retained) = &body.value else {
            panic!("expected retained string")
        };
        assert_eq!(retained.as_ptr(), original_allocation);
        assert_eq!(session.metrics().root_installations, 1);
        assert_eq!(view.0.segments[0].len(), 1);
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
        let second = writer
            .submit(batch(session.core.id, registration, "second"))
            .unwrap();
        let mut failing = batch(session.core.id, registration, "must-not-leak");
        failing
            .insert("first", Value::String("duplicate".into()))
            .unwrap();
        let duplicate = writer.submit(failing).unwrap();
        let suffix = writer
            .submit(batch(session.core.id, registration, "suffix"))
            .unwrap();
        session.resume_worker_for_measurement();
        assert_eq!(
            first.wait_published().unwrap(),
            second.wait_published().unwrap()
        );
        assert!(duplicate.wait_published().is_err());
        assert!(
            matches!(suffix.wait_published(), Err(IngestionError::Cancelled { originating_sequence }) if originating_sequence == duplicate.sequence())
        );
        assert_eq!(session.metrics().root_installations, 1);
        assert_eq!(index.status().unwrap().live_records, 2);
        assert!(
            index
                .view()
                .unwrap()
                .get_record(
                    "entries",
                    "must-not-leak",
                    crate::query::options::ReturnMode::Locator
                )
                .unwrap()
                .is_none()
        );
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
