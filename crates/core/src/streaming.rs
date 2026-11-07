//! Synchronous, bounded delivery of verified query pages.

use std::fmt;
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use crate::clock_context::ClockContext;
use crate::repository::{Payload, SearchHit};
use crate::workload::WorkCounters;

/// Limits apply to estimated retained query state and source reads, not RSS.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionLimits {
    pub relation_edges: u64,
    pub relation_reads: u64,
    pub relation_bytes: u64,
    pub intermediate_slots: u64,
    pub target_record_bytes: u64,
    pub source_record_bytes: u64,
    pub inferred_order_bytes: usize,
    pub computed_sort_bytes: u64,
    /// Estimated retained logical value and reader text for one extension record.
    pub extension_value_bytes: u64,
}

impl Default for ExecutionLimits {
    fn default() -> Self {
        Self {
            relation_edges: 32_000_000,
            relation_reads: 5_000_000,
            relation_bytes: 1024 * 1024 * 1024,
            intermediate_slots: 5_000_000,
            target_record_bytes: 64 * 1024 * 1024,
            source_record_bytes: u64::MAX,
            inferred_order_bytes: 128 * 1024 * 1024,
            computed_sort_bytes: 128 * 1024 * 1024,
            extension_value_bytes: 128 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExecutionErrorKind {
    Cancelled,
    Resource,
    SourceChanged,
    InvalidQuery,
    Extension,
    Execution,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecutionError {
    pub kind: ExecutionErrorKind,
    pub message: String,
}

impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message)
    }
}

impl std::error::Error for ExecutionError {}

#[derive(Default)]
struct ExecutionAccounting {
    edges: AtomicU64,
    reads: AtomicU64,
    bytes: AtomicU64,
    first_failure: Mutex<Option<ExecutionError>>,
    started: AtomicBool,
    #[cfg(test)]
    checks: AtomicU64,
    #[cfg(test)]
    cancel_on_check: AtomicU64,
}

/// State for exactly one request. Clones share counters and its first typed failure;
/// create a fresh context for another search, including another page request.
#[derive(Clone)]
pub struct ExecutionContext {
    pub clock: ClockContext,
    pub limits: ExecutionLimits,
    cancellation: Option<CancellationToken>,
    accounting: Arc<ExecutionAccounting>,
}

impl ExecutionContext {
    pub fn new(
        clock: ClockContext,
        limits: ExecutionLimits,
        cancellation: Option<CancellationToken>,
    ) -> Self {
        Self {
            clock,
            limits,
            cancellation,
            accounting: Arc::new(ExecutionAccounting::default()),
        }
    }

    pub fn cancellation(&self) -> Option<&CancellationToken> {
        self.cancellation.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn cancel_on_check_for_test(&self, check: u64) {
        self.accounting
            .cancel_on_check
            .store(check, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn checks_for_test(&self) -> u64 {
        self.accounting.checks.load(Ordering::Relaxed)
    }

    pub(crate) fn enter(&self) -> Result<(), ExecutionError> {
        if self.accounting.started.swap(true, Ordering::AcqRel) {
            return Err(ExecutionError {
                kind: ExecutionErrorKind::InvalidQuery,
                message: "execution context already used".into(),
            });
        }
        self.check()
    }

    pub fn check(&self) -> Result<(), ExecutionError> {
        #[cfg(test)]
        {
            let observed = self.accounting.checks.fetch_add(1, Ordering::Relaxed) + 1;
            let cancel_on = self.accounting.cancel_on_check.load(Ordering::Relaxed);
            if cancel_on > 0
                && observed >= cancel_on
                && let Some(cancellation) = &self.cancellation
            {
                cancellation.cancel();
            }
        }
        if self
            .cancellation
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
        {
            return Err(self.error(ExecutionErrorKind::Cancelled, "execution cancelled"));
        }
        Ok(())
    }

    pub(crate) fn check_string(&self) -> Result<(), String> {
        self.check().map_err(|e| e.message)
    }

    pub(crate) fn error(
        &self,
        kind: ExecutionErrorKind,
        message: impl Into<String>,
    ) -> ExecutionError {
        let error = ExecutionError {
            kind,
            message: message.into(),
        };
        if let Ok(mut first_failure) = self.accounting.first_failure.lock()
            && first_failure.is_none()
        {
            *first_failure = Some(error.clone());
        }
        error
    }

    pub(crate) fn fail(&self, kind: ExecutionErrorKind, message: impl Into<String>) -> String {
        self.error(kind, message).message
    }

    pub(crate) fn recorded_error(&self, fallback: String) -> ExecutionError {
        self.accounting
            .first_failure
            .lock()
            .ok()
            .and_then(|error| error.clone())
            .unwrap_or(ExecutionError {
                kind: ExecutionErrorKind::Execution,
                message: fallback,
            })
    }

    fn charge(
        &self,
        counter: &AtomicU64,
        amount: u64,
        limit: u64,
        name: &str,
    ) -> Result<(), String> {
        let result = counter.try_update(Ordering::Relaxed, Ordering::Relaxed, |used| {
            used.checked_add(amount)
        });
        let prior = result.map_err(|_| {
            self.fail(
                ExecutionErrorKind::Resource,
                format!("{name} accounting overflow"),
            )
        })?;
        if prior.saturating_add(amount) > limit {
            return Err(self.fail(
                ExecutionErrorKind::Resource,
                format!("{name} exceeds {limit}"),
            ));
        }
        self.check_string()
    }

    pub(crate) fn charge_edges(&self, amount: u64) -> Result<(), String> {
        self.charge(
            &self.accounting.edges,
            amount,
            self.limits.relation_edges,
            "relation edge visits",
        )
    }

    pub(crate) fn charge_reads(&self, amount: u64) -> Result<(), String> {
        self.charge(
            &self.accounting.reads,
            amount,
            self.limits.relation_reads,
            "relation reads",
        )
    }

    pub(crate) fn charge_bytes(&self, amount: u64) -> Result<(), String> {
        self.charge(
            &self.accounting.bytes,
            amount,
            self.limits.relation_bytes,
            "relation source bytes",
        )
    }

    pub(crate) fn check_slots(&self, slots: u64) -> Result<(), String> {
        if slots > self.limits.intermediate_slots {
            return Err(self.fail(
                ExecutionErrorKind::Resource,
                format!(
                    "relation intermediate exceeds {} slots",
                    self.limits.intermediate_slots
                ),
            ));
        }
        self.check_string()
    }
}

/// How verified requested-page rows are published during execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamMode {
    OrderedRows,
    PageReplacements,
}

/// Semantic result extent for one streaming execution.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResultScope {
    Page { offset: usize, limit: usize },
    All { offset: usize },
}

impl ResultScope {
    pub fn offset(self) -> usize {
        match self {
            Self::Page { offset, .. } | Self::All { offset } => offset,
        }
    }
}

/// Delivery cadence and upper bounds for one streaming execution.
#[derive(Clone, Copy, Debug)]
pub struct StreamOptions {
    pub mode: StreamMode,
    pub batch_rows: usize,
    pub checkpoint_files: usize,
    pub max_event_bytes: usize,
    pub max_buffered_bytes: usize,
}

impl Default for StreamOptions {
    fn default() -> Self {
        Self {
            mode: StreamMode::OrderedRows,
            batch_rows: 16,
            checkpoint_files: 8,
            max_event_bytes: 16 * 1024 * 1024,
            max_buffered_bytes: 64 * 1024 * 1024,
        }
    }
}

impl StreamOptions {
    pub(crate) fn validate(self) -> Result<(), String> {
        if self.batch_rows == 0
            || self.checkpoint_files == 0
            || self.max_event_bytes == 0
            || self.max_buffered_bytes < self.max_event_bytes
        {
            return Err("invalid stream delivery or memory bounds".into());
        }
        Ok(())
    }
}

/// Shareable cooperative cancellation flag for a running request.
#[derive(Clone, Default)]
pub struct CancellationToken(Arc<AtomicBool>);

impl CancellationToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

/// Decision returned by a caller's event callback.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StreamControl {
    Continue,
    Cancel,
}

/// Counters visible at an interim delivery or terminal outcome.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamProgress {
    pub sequence: u64,
    /// Completed source verifications plus index-proven record decisions.
    /// This is monotonic, but excludes candidates rejected by an index alone.
    pub processed_candidates: u64,
    /// Verified matches observed so far; a lower bound until completion.
    pub observed_matches: u64,
    pub emitted_rows: u64,
    pub peak_buffered_bytes: usize,
}

/// Borrowed, nonterminal event. A replacement supersedes the prior page.
pub enum StreamEvent<'a> {
    OrderedRows {
        sequence: u64,
        start: usize,
        rows: &'a [SearchHit],
        progress: StreamProgress,
    },
    PageReplacement {
        sequence: u64,
        rows: &'a [SearchHit],
        progress: StreamProgress,
    },
    Progress {
        sequence: u64,
        progress: StreamProgress,
    },
}

/// Successful query summary; the last event sequence identifies its final page.
#[derive(Clone, Debug)]
pub struct StreamSummary {
    pub count: Option<usize>,
    pub returned_rows: usize,
    pub final_sequence: u64,
    pub counters: WorkCounters,
    pub progress: StreamProgress,
}

/// Completion and cancellation have separate terminal outcomes.
#[derive(Clone, Debug)]
pub enum StreamOutcome {
    Complete(Box<StreamSummary>),
    Cancelled(StreamProgress),
}

/// Execution, callback, and resource failures retain observed progress.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StreamError {
    Execution {
        kind: ExecutionErrorKind,
        message: String,
        progress: StreamProgress,
    },
    Sink {
        message: String,
        progress: StreamProgress,
    },
    Resource {
        message: String,
        progress: StreamProgress,
    },
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Execution { message, .. } => write!(f, "stream execution: {message}"),
            Self::Sink { message, .. } => write!(f, "stream sink: {message}"),
            Self::Resource { message, .. } => write!(f, "stream resource: {message}"),
        }
    }
}

impl StreamError {
    pub fn kind(&self) -> ExecutionErrorKind {
        match self {
            Self::Execution { kind, .. } => *kind,
            Self::Resource { .. } => ExecutionErrorKind::Resource,
            Self::Sink { .. } => ExecutionErrorKind::Execution,
        }
    }
}

impl std::error::Error for StreamError {}

pub(crate) enum Abort {
    Cancelled,
    Sink(String),
    Resource(String),
}

/// Coordinator-only delivery state; callbacks never run under cache locks.
pub(crate) struct Delivery<'a> {
    options: StreamOptions,
    token: &'a CancellationToken,
    sink: &'a mut dyn for<'e> FnMut(StreamEvent<'e>) -> Result<StreamControl, String>,
    pub(crate) progress: StreamProgress,
    pub(crate) abort: Option<Abort>,
    pub(crate) ordered_delivered: usize,
    last_page_sequence: u64,
    last_page: Vec<(String, String)>,
}

impl<'a> Delivery<'a> {
    pub(crate) fn new(
        options: StreamOptions,
        token: &'a CancellationToken,
        sink: &'a mut dyn for<'e> FnMut(StreamEvent<'e>) -> Result<StreamControl, String>,
    ) -> Self {
        Self {
            options,
            token,
            sink,
            progress: StreamProgress::default(),
            abort: None,
            ordered_delivered: 0,
            last_page_sequence: 0,
            last_page: Vec::new(),
        }
    }

    pub(crate) fn options(&self) -> StreamOptions {
        self.options
    }

    pub(crate) fn token(&self) -> &CancellationToken {
        self.token
    }

    pub(crate) fn check(&mut self) -> Result<(), String> {
        if self.token.is_cancelled() {
            self.abort = Some(Abort::Cancelled);
            return Err("stream cancelled".into());
        }
        Ok(())
    }

    pub(crate) fn observe(&mut self, counters: &WorkCounters) {
        self.progress.processed_candidates = counters
            .scanned_records
            .saturating_add(counters.exact_posting_matches);
        self.progress.observed_matches = counters.total_matches;
    }

    pub(crate) fn max_buffered_bytes(&self) -> usize {
        self.options.max_buffered_bytes
    }

    pub(crate) fn last_page_sequence(&self) -> u64 {
        self.last_page_sequence
    }

    pub(crate) fn check_record_bytes(&mut self, bytes: usize) -> Result<(), String> {
        if bytes > self.options.max_buffered_bytes {
            self.abort = Some(Abort::Resource(format!(
                "source record {bytes} bytes above {} byte limit",
                self.options.max_buffered_bytes
            )));
            return Err("stream record budget exceeded".into());
        }
        Ok(())
    }

    pub(crate) fn page_changed(&self, identities: &[(String, String)]) -> bool {
        identities != self.last_page
    }

    pub(crate) fn check_buffer(&mut self, bytes: usize) -> Result<(), String> {
        self.progress.peak_buffered_bytes = self.progress.peak_buffered_bytes.max(bytes);
        if bytes > self.options.max_buffered_bytes {
            self.abort = Some(Abort::Resource(format!(
                "stream retained {bytes} bytes above {} byte limit",
                self.options.max_buffered_bytes
            )));
            return Err("stream memory budget exceeded".into());
        }
        Ok(())
    }

    fn call(&mut self, event: StreamEvent<'_>, delivered: StreamProgress) -> Result<(), String> {
        self.check()?;
        match (self.sink)(event) {
            Ok(StreamControl::Continue) => {
                self.progress = delivered;
                self.check()
            }
            Ok(StreamControl::Cancel) => {
                self.progress = delivered;
                self.abort = Some(Abort::Cancelled);
                Err("stream cancelled".into())
            }
            Err(message) => {
                self.abort = Some(Abort::Sink(message));
                Err("stream sink failed".into())
            }
        }
    }

    pub(crate) fn ordered_rows(
        &mut self,
        page: &[SearchHit],
        counters: &WorkCounters,
        force: bool,
    ) -> Result<(), String> {
        if self.options.mode != StreamMode::OrderedRows || page.len() == self.ordered_delivered {
            return Ok(());
        }
        if !force && page.len() - self.ordered_delivered < self.options.batch_rows {
            return Ok(());
        }
        let start = self.ordered_delivered;
        let rows = &page[start..];
        let bytes = rows
            .iter()
            .fold(0usize, |total, row| total.saturating_add(hit_bytes(row)));
        self.check_event(bytes)?;
        self.observe(counters);
        let mut progress = self.progress;
        progress.sequence = progress.sequence.saturating_add(1);
        progress.emitted_rows = progress.emitted_rows.saturating_add(rows.len() as u64);
        self.call(
            StreamEvent::OrderedRows {
                sequence: progress.sequence,
                start,
                rows,
                progress,
            },
            progress,
        )?;
        self.ordered_delivered = page.len();
        self.last_page_sequence = progress.sequence;
        Ok(())
    }

    /// Publish one independent bounded batch from an all-results traversal.
    pub(crate) fn ordered_batch(
        &mut self,
        rows: &[SearchHit],
        counters: &WorkCounters,
    ) -> Result<(), String> {
        if rows.is_empty() {
            return Ok(());
        }
        let bytes = rows
            .iter()
            .fold(0usize, |total, row| total.saturating_add(hit_bytes(row)));
        self.check_event(bytes)?;
        self.observe(counters);
        let mut progress = self.progress;
        progress.sequence = progress.sequence.saturating_add(1);
        let start = usize::try_from(progress.emitted_rows)
            .map_err(|_| "stream result position overflow")?;
        progress.emitted_rows = progress.emitted_rows.saturating_add(rows.len() as u64);
        self.call(
            StreamEvent::OrderedRows {
                sequence: progress.sequence,
                start,
                rows,
                progress,
            },
            progress,
        )?;
        self.last_page_sequence = progress.sequence;
        Ok(())
    }

    pub(crate) fn replacement(
        &mut self,
        page: &[SearchHit],
        counters: &WorkCounters,
        retained_bytes: usize,
    ) -> Result<(), String> {
        if self.options.mode != StreamMode::PageReplacements || page.is_empty() {
            return Ok(());
        }
        let identities: Vec<_> = page
            .iter()
            .map(|hit| (hit.collection.clone(), hit.record_id.clone()))
            .collect();
        if identities == self.last_page {
            return Ok(());
        }
        let event_bytes = page
            .iter()
            .fold(0usize, |total, row| total.saturating_add(hit_bytes(row)));
        let identity_bytes = identities
            .iter()
            .chain(self.last_page.iter())
            .map(|(collection, id)| collection.len() + id.len() + 2 * std::mem::size_of::<String>())
            .sum::<usize>();
        self.check_event(event_bytes)?;
        self.check_buffer(
            retained_bytes
                .saturating_add(event_bytes)
                .saturating_add(identity_bytes),
        )?;
        self.observe(counters);
        let mut progress = self.progress;
        progress.sequence = progress.sequence.saturating_add(1);
        progress.emitted_rows = progress.emitted_rows.saturating_add(page.len() as u64);
        self.call(
            StreamEvent::PageReplacement {
                sequence: progress.sequence,
                rows: page,
                progress,
            },
            progress,
        )?;
        self.last_page = identities;
        self.last_page_sequence = progress.sequence;
        Ok(())
    }

    pub(crate) fn progress_event(&mut self, counters: &WorkCounters) -> Result<(), String> {
        self.observe(counters);
        let mut progress = self.progress;
        progress.sequence = progress.sequence.saturating_add(1);
        self.call(
            StreamEvent::Progress {
                sequence: progress.sequence,
                progress,
            },
            progress,
        )
    }

    fn check_event(&mut self, bytes: usize) -> Result<(), String> {
        if bytes > self.options.max_event_bytes {
            self.abort = Some(Abort::Resource(format!(
                "stream event {bytes} bytes above {} byte limit",
                self.options.max_event_bytes
            )));
            return Err("stream event budget exceeded".into());
        }
        Ok(())
    }
}

pub(crate) fn hit_bytes(hit: &SearchHit) -> usize {
    let mut bytes = std::mem::size_of::<SearchHit>()
        + hit.collection.len()
        + hit.record_id.len()
        + hit.file_path.as_os_str().len()
        + hit
            .matched_fields
            .iter()
            .map(|field| field.len())
            .sum::<usize>();
    bytes += hit.payload.as_ref().map_or(0, payload_bytes);
    bytes
}

pub(crate) fn payload_bytes(payload: &Payload) -> usize {
    match payload {
        Payload::Bytes(data) => data.len(),
        Payload::Logical(value) => value_bytes(value),
        Payload::LogicalUnavailable { .. } => 0,
    }
}

fn value_bytes(value: &crate::model::Value) -> usize {
    use crate::model::Value;
    match value {
        Value::String(text) | Value::Id(text) => text.len() + std::mem::size_of::<String>(),
        Value::Array(array) => {
            array.values.iter().map(value_bytes).sum::<usize>()
                + array.values.len() * std::mem::size_of::<Value>()
        }
        Value::Object(map) => map
            .iter()
            .map(|(name, value)| name.len() + value_bytes(value) + 64)
            .sum(),
        _ => std::mem::size_of::<Value>(),
    }
}

#[cfg(test)]
#[path = "streaming.test.rs"]
mod tests;
