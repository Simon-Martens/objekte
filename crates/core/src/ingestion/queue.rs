//! Ordered admission, bounded resident input, and receipt completion.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use crate::lifecycle::{Index, IndexError, IndexRevision, RecordRegistration};

use super::spool::{self, SpoolManager};
use super::{Batch, IngestionError, IngestionOptions, PublishedRevision, SubmitError};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const MAX_PENDING_DESCRIPTORS: usize = 65_536;

struct ReceiptState {
    result: Mutex<Option<PublishedRevision>>,
    changed: Condvar,
}

impl ReceiptState {
    fn new() -> Self {
        Self {
            result: Mutex::new(None),
            changed: Condvar::new(),
        }
    }
    fn complete(&self, result: PublishedRevision) {
        let mut guard = self.result.lock().expect("receipt mutex poisoned");
        if guard.is_none() {
            *guard = Some(result);
            self.changed.notify_all();
        }
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
        let mut guard = self.state.result.lock().expect("receipt mutex poisoned");
        while guard.is_none() {
            guard = self
                .state
                .changed
                .wait(guard)
                .expect("receipt mutex poisoned");
        }
        guard.as_ref().expect("terminal receipt").clone()
    }
}

enum Storage {
    Memory(Batch),
    Spool(PathBuf),
}

struct Queued {
    sequence: u64,
    weight: usize,
    encoded_bytes: u64,
    storage: Storage,
    receipt: Arc<ReceiptState>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SessionMetrics {
    pub accepted_batches: u64,
    pub published_batches: u64,
    pub pending_ram_bytes: usize,
    pub pending_spool_bytes: u64,
    pub high_water_ram_bytes: usize,
    pub high_water_spool_bytes: u64,
    pub overflow_write_ns: u128,
    pub failure_sequence: Option<u64>,
}

struct State {
    items: VecDeque<Queued>,
    next_sequence: u64,
    completed: u64,
    last_revision: IndexRevision,
    closed: bool,
    failure: Option<(u64, IngestionError)>,
    metrics: SessionMetrics,
}

struct Queue {
    state: Mutex<State>,
    changed: Condvar,
    spool: Mutex<SpoolManager>,
    options: IngestionOptions,
    cancelled: AtomicBool,
}

/// A session is the single ordered producer stream for one index. The index
/// retains a weak reference so finished sessions can be replaced.
pub(crate) struct SessionCore {
    pub(crate) id: u64,
    index: Index,
    queue: Arc<Queue>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl SessionCore {
    pub(crate) fn new(index: Index, options: IngestionOptions) -> Arc<Self> {
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let revision = index
            .view()
            .expect("index view available at session creation")
            .revision();
        let queue = Arc::new(Queue {
            state: Mutex::new(State {
                items: VecDeque::new(),
                next_sequence: 1,
                completed: 0,
                last_revision: revision,
                closed: false,
                failure: None,
                metrics: SessionMetrics::default(),
            }),
            changed: Condvar::new(),
            spool: Mutex::new(SpoolManager::new(id, &options)),
            options,
            cancelled: AtomicBool::new(false),
        });
        let core = Arc::new(Self {
            id,
            index: index.clone(),
            queue: Arc::clone(&queue),
            worker: Mutex::new(None),
        });
        let worker = std::thread::Builder::new()
            .name(format!("objekte-ingestion-{id}"))
            .spawn(move || run_worker(index, queue, id))
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
}

impl Drop for SessionCore {
    fn drop(&mut self) {
        if let Ok(mut state) = self.queue.state.lock() {
            state.closed = true;
            self.queue.changed.notify_all();
        }
    }
}

#[derive(Clone)]
pub struct IngestionSession {
    pub(crate) core: Arc<SessionCore>,
}

impl IngestionSession {
    pub(crate) fn from_core(core: Arc<SessionCore>) -> Self {
        Self { core }
    }

    pub fn writer(&self, registration: RecordRegistration) -> Result<RecordWriter, IndexError> {
        if !self.core.is_open() {
            return Err(IndexError::Cancelled);
        }
        let view = self.core.index.view()?;
        let valid = view
            .repository()
            .record_registrations
            .get(registration.ordinal as usize)
            .is_some_and(|source| source.handle == registration);
        if !valid {
            return Err(IndexError::ForeignInstance);
        }
        Ok(RecordWriter {
            core: Arc::clone(&self.core),
            registration,
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
        self.core.queue.cancelled.store(true, Ordering::Release);
        let mut state = self.core.queue.state.lock().expect("queue mutex poisoned");
        state.closed = true;
        self.core.queue.changed.notify_all();
    }
}

#[derive(Clone)]
pub struct RecordWriter {
    core: Arc<SessionCore>,
    registration: RecordRegistration,
}

impl RecordWriter {
    pub fn batch(&self) -> Batch {
        Batch::new(self.core.id, self.registration)
    }

    pub fn submit(&self, batch: Batch) -> Result<Receipt, SubmitError<Batch>> {
        self.offer(batch, false)
    }

    pub fn try_submit(&self, batch: Batch) -> Result<Receipt, SubmitError<Batch>> {
        self.offer(batch, true)
    }

    fn offer(&self, batch: Batch, memory_only: bool) -> Result<Receipt, SubmitError<Batch>> {
        let queue = &self.core.queue;
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
        if state.items.len() >= MAX_PENDING_DESCRIPTORS {
            return Err(SubmitError::Capacity(batch));
        }
        if state.metrics.pending_ram_bytes.saturating_add(weight) <= queue.options.pending_ram_bytes
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
            ));
        }
        if memory_only {
            return Err(SubmitError::WouldBlock(batch));
        }
        drop(state);
        let encoded = match spool::encode(&batch) {
            Ok(encoded) => encoded,
            Err(message) => return Err(SubmitError::SpoolIo(batch, message)),
        };
        let bytes = encoded.len();
        let mut state = queue.state.lock().expect("queue mutex poisoned");
        if state.closed || state.failure.is_some() {
            return Err(SubmitError::Closed(batch));
        }
        if state.items.len() >= MAX_PENDING_DESCRIPTORS
            || state.metrics.pending_spool_bytes.saturating_add(bytes) > queue.options.spool_bytes
        {
            return Err(SubmitError::Capacity(batch));
        }
        state.metrics.pending_spool_bytes += bytes;
        state.metrics.high_water_spool_bytes = state
            .metrics
            .high_water_spool_bytes
            .max(state.metrics.pending_spool_bytes);
        drop(state);
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
        let path = written.expect("spool write succeeded");
        if state.closed || state.failure.is_some() {
            state.metrics.pending_spool_bytes -= bytes;
            drop(state);
            let _ = std::fs::remove_file(path);
            return Err(SubmitError::Closed(batch));
        }
        Ok(enqueue(
            &mut state,
            queue,
            Storage::Spool(path),
            weight,
            bytes,
        ))
    }
}

fn enqueue(
    state: &mut State,
    queue: &Queue,
    storage: Storage,
    weight: usize,
    encoded_bytes: u64,
) -> Receipt {
    let sequence = state.next_sequence;
    state.next_sequence += 1;
    state.metrics.accepted_batches += 1;
    let receipt = Arc::new(ReceiptState::new());
    state.items.push_back(Queued {
        sequence,
        weight,
        encoded_bytes,
        storage,
        receipt: Arc::clone(&receipt),
    });
    queue.changed.notify_all();
    Receipt {
        sequence,
        state: receipt,
    }
}

fn run_worker(index: Index, queue: Arc<Queue>, session_id: u64) {
    loop {
        let item = {
            let mut state = queue.state.lock().expect("queue mutex poisoned");
            while state.items.is_empty() && !state.closed {
                state = queue.changed.wait(state).expect("queue mutex poisoned");
            }
            let Some(item) = state.items.pop_front() else {
                break;
            };
            if matches!(item.storage, Storage::Memory(_)) {
                state.metrics.pending_ram_bytes -= item.weight;
            }
            item
        };
        let batch = match item.storage {
            Storage::Memory(batch) => Ok(batch),
            Storage::Spool(path) => {
                let result = spool::read(&path, queue.options.spool_bytes, session_id)
                    .map_err(IngestionError::Spool);
                let _ = std::fs::remove_file(path);
                let mut state = queue.state.lock().expect("queue mutex poisoned");
                state.metrics.pending_spool_bytes -= item.encoded_bytes;
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
                        batch,
                        item.sequence,
                    )
                }))
                .unwrap_or_else(|_| Err(IngestionError::Spool("ingestion worker panicked".into())))
            })
        };
        item.receipt.complete(result.clone());
        let mut state = queue.state.lock().expect("queue mutex poisoned");
        state.completed = item.sequence;
        match result {
            Ok(revision) => {
                state.last_revision = revision;
                state.metrics.published_batches += 1;
            }
            Err(error) => {
                state.closed = true;
                state.failure = Some((item.sequence, error));
                state.metrics.failure_sequence = Some(item.sequence);
                while let Some(pending) = state.items.pop_front() {
                    if matches!(pending.storage, Storage::Memory(_)) {
                        state.metrics.pending_ram_bytes -= pending.weight;
                    } else {
                        state.metrics.pending_spool_bytes -= pending.encoded_bytes;
                        if let Storage::Spool(path) = pending.storage {
                            let _ = std::fs::remove_file(path);
                        }
                    }
                    pending.receipt.complete(Err(IngestionError::Cancelled {
                        originating_sequence: item.sequence,
                    }));
                    state.completed = pending.sequence;
                }
            }
        }
        queue.changed.notify_all();
    }
    queue.spool.lock().expect("spool mutex poisoned").cleanup();
}
