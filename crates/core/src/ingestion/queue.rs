//! Ordered admission, bounded resident input, and receipt completion.

use std::collections::VecDeque;
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
    Spool(spool::SpoolFrame),
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
    pub spool_written_bytes: u64,
    pub spool_read_bytes: u64,
    pub in_flight_estimated_bytes: usize,
    pub high_water_in_flight_estimated_bytes: usize,
    pub overflow_write_ns: u128,
    pub failure_sequence: Option<u64>,
}

struct State {
    items: VecDeque<Queued>,
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
    changed: Condvar,
    spool: Mutex<SpoolManager>,
    encoding: Mutex<()>,
    options: IngestionOptions,
    cancelled: AtomicBool,
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
                paused: false,
                failure: None,
                metrics: SessionMetrics::default(),
            }),
            changed: Condvar::new(),
            spool: Mutex::new(SpoolManager::new(id, &options)),
            encoding: Mutex::new(()),
            options,
            cancelled: AtomicBool::new(false),
            #[cfg(test)]
            worker_entry: Mutex::new(None),
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
            state.paused = false;
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
        let source = view
            .repository()
            .record_registrations
            .get(registration.ordinal as usize)
            .filter(|source| source.handle == registration);
        let Some(source) = source else {
            return Err(IndexError::ForeignInstance);
        };
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
        self.core.queue.cancelled.store(true, Ordering::Release);
        let mut state = self.core.queue.state.lock().expect("queue mutex poisoned");
        state.closed = true;
        state.paused = false;
        self.core.queue.changed.notify_all();
    }
}

#[derive(Clone)]
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
            state.metrics.pending_spool_bytes -= bytes;
            drop(state);
            queue
                .spool
                .lock()
                .expect("spool mutex poisoned")
                .release(&frame);
            return Err(SubmitError::Closed(batch));
        }
        if state.items.len() >= MAX_PENDING_DESCRIPTORS {
            state.metrics.pending_spool_bytes -= bytes;
            drop(state);
            queue
                .spool
                .lock()
                .expect("spool mutex poisoned")
                .release(&frame);
            return Err(SubmitError::Capacity(batch));
        }
        Ok(enqueue(
            &mut state,
            queue,
            Storage::Spool(frame),
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
            while (state.items.is_empty() || state.paused) && !state.closed {
                state = queue.changed.wait(state).expect("queue mutex poisoned");
            }
            let Some(item) = state.items.pop_front() else {
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
            item
        };
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
            Storage::Spool(frame) => {
                let result = spool::read(&frame, queue.options.spool_bytes, session_id)
                    .map_err(IngestionError::Spool);
                queue
                    .spool
                    .lock()
                    .expect("spool mutex poisoned")
                    .release(&frame);
                let mut state = queue.state.lock().expect("queue mutex poisoned");
                state.metrics.pending_spool_bytes -= item.encoded_bytes;
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
                        batch,
                        item.sequence,
                    )
                }))
                .unwrap_or_else(|_| Err(IngestionError::Spool("ingestion worker panicked".into())))
            })
        };
        item.receipt.complete(result.clone());
        let mut state = queue.state.lock().expect("queue mutex poisoned");
        state.metrics.in_flight_estimated_bytes = 0;
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
                        if let Storage::Spool(frame) = pending.storage {
                            queue
                                .spool
                                .lock()
                                .expect("spool mutex poisoned")
                                .release(&frame);
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
        let third = writer.submit(third).unwrap();
        release.send(()).unwrap();
        second.wait_published().unwrap();
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
