//! Durable validation work. CPU workers never access SQLite; their results return
//! to the same serialized writer used by ingestion, guarded by snapshot identity.
use crate::{Collection, Error, Result, store::Writer};
use rayon::prelude::*;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    sync::{Arc, Condvar, Mutex},
    time::Instant,
};


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordStatus {
    Pending,
    Valid,
    Invalid,
    IncorrectJson,
}


impl RecordStatus {
    pub(crate) fn text(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Valid => "valid",
            Self::Invalid => "invalid",
            Self::IncorrectJson => "incorrect_json",
        }
    }

    pub(crate) fn read(s: &str) -> Result<Self> {
        match s {
            "pending" => Ok(Self::Pending),
            "valid" => Ok(Self::Valid),
            "invalid" => Ok(Self::Invalid),
            "incorrect_json" => Ok(Self::IncorrectJson),
            _ => Err(Error::Cache(format!("unknown validation state: {s}"))),
        }
    }
}


// INFO: Diagnostic location, code, message, and optional schema path. Saved in Cache DB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ValidationIssue {
    /// JSON Pointer into the original record. Its value remains available in Record.data.
    pub location: String,
    pub schema_path: Option<String>,
    pub code: String,
    pub message: String,
}


impl ValidationIssue {
    pub(crate) fn new(location: &str, code: &str, message: &str) -> Self {
        Self {
            location: location.into(),
            schema_path: None,
            code: code.into(),
            message: message.into(),
        }
    }
}


// INFO: Overall validation status and collected issues for one record. Saved in Cache DB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordState {
    pub status: RecordStatus,
    pub issues: Vec<ValidationIssue>,
}


impl RecordState {
    pub(crate) fn read(status: &str, issues: &str) -> Result<Self> {
        Ok(Self {
            status: RecordStatus::read(status)?,
            issues: sonic_rs::from_str(issues)
                .map_err(|e| Error::Cache(format!("invalid cached diagnostics: {e}")))?,
        })
    }
}


// INFO: Parsed JSON value paired with its validation state.
pub(crate) struct Document {
    pub data: Value,
    pub state: RecordState,
}


impl std::ops::Deref for Document {
    type Target = Value;
    fn deref(&self) -> &Value {
        &self.data
    }
}


impl Document {
    pub fn parse(bytes: &[u8]) -> Self {
        let parsed = std::str::from_utf8(bytes)
            .map_err(|e| e.to_string())
            .and_then(|s| crate::json::parse(s).map_err(|e| e.to_string()));
        match parsed {
            Ok(data) => Self {
                data,
                state: RecordState {
                    status: RecordStatus::Pending,
                    issues: Vec::new(),
                },
            },
            Err(e) => Self {
                data: Value::Null,
                state: RecordState {
                    status: RecordStatus::IncorrectJson,
                    issues: vec![ValidationIssue::new("", "json_syntax", &e)],
                },
            },
        }
    }

    pub fn validate(&mut self, c: &Collection) {
        if self.state.status == RecordStatus::IncorrectJson {
            return;
        }
        self.state.issues = c.schema.issues(&self.data);
        self.state.status = if self.state.issues.is_empty() {
            RecordStatus::Valid
        } else {
            RecordStatus::Invalid
        };
    }
}


// INFO: Counters and timings for background validation work.
#[derive(Debug, Default, Clone, Serialize)]
pub struct ValidationMetrics {
    pub validated: u64,
    pub applied: u64,
    pub stale: u64,
    pub snapshot_read_nanos: u64,
    pub parallel_validation_nanos: u64,
    pub decode_nanos: u64,
    pub validation_nanos: u64,
    pub write_nanos: u64,
}


// INFO: Pending validation jobs, lifecycle flags, updates, errors, and metrics.
#[derive(Default)]
struct Queue {
    jobs: BTreeMap<String, Collection>,
    order: VecDeque<String>,
    active: bool,
    stop: bool,
    error: Option<String>,
    updates: BTreeSet<String>,
    metrics: ValidationMetrics,
}


// INFO: Mutex-protected validation queue and its wakeup condition variable.
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
}


// INFO: Handle to shared background validation state.
pub(crate) struct Validator {
    shared: Arc<Shared>,
}


impl Validator {
    pub fn new(
        writer: Arc<Writer>,
        workers: usize,
        budget: Option<u32>,
        sql_diagnostics: bool,
    ) -> Result<Self> {
        let pool = rayon::ThreadPoolBuilder::new()
            // Reserve sufficient virtual stack for Sonic's bounded nesting in debug builds.
            .stack_size(16 * 1024 * 1024)
            .num_threads(workers)
            .thread_name(|i| format!("objekte-validation-{i}"))
            .build()
            .map_err(|e| Error::Cache(e.to_string()))?;
        let shared = Arc::new(Shared::default());
        let state = shared.clone();
        std::thread::Builder::new()
            .name("objekte-validation-results".into())
            .spawn(move || {
                loop {
                    let c = {
                        let mut q = state.queue.lock().unwrap();
                        while q.jobs.is_empty() && !q.stop {
                            q = state.changed.wait(q).unwrap();
                        }
                        if q.stop {
                            break;
                        }
                        q.active = true;
                        {
                            let name = q.order.pop_front().unwrap();
                            q.jobs.remove(&name).unwrap()
                        }
                    };
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        batch(&writer, &pool, &c, budget, sql_diagnostics)
                    }));
                    let mut q = state.queue.lock().unwrap();
                    q.active = false;
                    match result {
                        Ok(Ok((more, m))) => {
                            if m.applied > 0 {
                                q.updates.insert(c.name.clone());
                            }
                            q.metrics.validated += m.validated;
                            q.metrics.applied += m.applied;
                            q.metrics.stale += m.stale;
                            q.metrics.snapshot_read_nanos += m.snapshot_read_nanos;
                            q.metrics.parallel_validation_nanos += m.parallel_validation_nanos;
                            q.metrics.decode_nanos += m.decode_nanos;
                            q.metrics.validation_nanos += m.validation_nanos;
                            q.metrics.write_nanos += m.write_nanos;
                            // A newer schema queued during this batch must win.
                            if more && !q.jobs.contains_key(&c.name) {
                                q.order.push_back(c.name.clone());
                                q.jobs.insert(c.name.clone(), c);
                            }
                        }
                        error => {
                            q.error = Some(match error {
                                Ok(Err(e)) => e.to_string(),
                                _ => "validation worker panicked".into(),
                            });
                            q.jobs.clear();
                            q.order.clear();
                        }
                    }
                    state.changed.notify_all();
                }
            })
            .map_err(|e| Error::Cache(format!("start validation: {e}")))?;
        Ok(Self { shared })
    }

    pub fn schedule(&self, c: Collection) {
        let mut q = self.shared.queue.lock().unwrap();
        if !q.jobs.contains_key(&c.name) {
            q.order.push_back(c.name.clone());
        }
        q.jobs.insert(c.name.clone(), c);
        self.shared.changed.notify_all();
    }

    pub fn wait(&self) -> Result<ValidationMetrics> {
        let mut q = self.shared.queue.lock().unwrap();
        while q.active || !q.jobs.is_empty() {
            q = self.shared.changed.wait(q).unwrap();
        }
        if let Some(e) = &q.error {
            return Err(Error::Cache(format!("background validation: {e}")));
        }
        Ok(q.metrics.clone())
    }

    pub fn metrics(&self) -> ValidationMetrics {
        self.shared.queue.lock().unwrap().metrics.clone()
    }

    pub fn updates(&self) -> Result<Vec<String>> {
        let mut q = self.shared.queue.lock().unwrap();
        if let Some(e) = &q.error {
            return Err(Error::Cache(format!("background validation: {e}")));
        }
        Ok(std::mem::take(&mut q.updates).into_iter().collect())
    }
}


impl Drop for Validator {
    fn drop(&mut self) {
        let mut q = self.shared.queue.lock().unwrap();
        q.stop = true;
        self.shared.changed.notify_all();
        // Do not block shutdown on a collection. An active bounded batch can finish;
        // every remaining pending row is durable for the next explicit run.
    }
}


// INFO: Cached record snapshot and version data for one validation task.
struct Job {
    id: String,
    revision: i64,
    digest: String,
    bytes: Vec<u8>,
}


fn batch(
    writer: &Writer,
    pool: &rayon::ThreadPool,
    c: &Collection,
    budget: Option<u32>,
    sql_diagnostics: bool,
) -> Result<(bool, ValidationMetrics)> {
    let t = Instant::now();
    let jobs = {
        let s = writer.enter_existing(budget, sql_diagnostics)?;
        let table = crate::store::record_table();
        let mut stmt = s.db.prepare(&format!("SELECT id,revision,digest,json FROM {table} WHERE collection=?1 AND validation_state='pending' AND validation_schema_hash=?2 ORDER BY id LIMIT 1024"))?;
        let mut rows = stmt.query(params![c.name, c.schema_hash])?;
        let mut jobs = Vec::new();
        let mut size = 0;
        while let Some(r) = rows.next()? {
            let bytes = r
                .get_ref(3)?
                .as_bytes()
                .map_err(|e| Error::Cache(e.to_string()))?
                .to_vec();
            if !jobs.is_empty() && size + bytes.len() > 8 * 1024 * 1024 {
                break;
            }
            size += bytes.len();
            jobs.push(Job {
                id: r.get(0)?,
                revision: r.get(1)?,
                digest: r.get(2)?,
                bytes,
            });
        }
        jobs
    };
    let mut metrics = ValidationMetrics {
        snapshot_read_nanos: t.elapsed().as_nanos() as u64,
        ..Default::default()
    };
    if jobs.is_empty() {
        return Ok((false, metrics));
    }
    let t = Instant::now();
    let outcomes = pool.install(|| {
        jobs.par_iter()
            .map(|job| {
                let t = Instant::now();
                let mut doc = Document::parse(&job.bytes);
                let decode = t.elapsed().as_nanos() as u64;
                let t = Instant::now();
                doc.validate(c);
                let validate = t.elapsed().as_nanos() as u64;
                (doc.state, decode, validate)
            })
            .collect::<Vec<_>>()
    });
    metrics.parallel_validation_nanos = t.elapsed().as_nanos() as u64;
    let t = Instant::now();
    let s = writer.enter_existing(budget, sql_diagnostics)?;
    let tx = s.db.unchecked_transaction()?;
    for (job, (outcome, decode, validate)) in jobs.iter().zip(outcomes) {
        // IncorrectJson here indicates corrupted cached bytes, not a source error:
        // ingestion marked this exact snapshot successfully parsed.
        if outcome.status == RecordStatus::IncorrectJson {
            return Err(Error::Cache(format!(
                "invalid parsed snapshot {}/{}",
                c.name, job.id
            )));
        }
        let applied = apply(&tx, &c.name, job, &c.schema_hash, &outcome)?;
        metrics.validated += 1;
        metrics.applied += applied as u64;
        metrics.stale += u64::from(applied == 0);
        metrics.decode_nanos += decode;
        metrics.validation_nanos += validate;
    }
    tx.commit()?;
    metrics.write_nanos = t.elapsed().as_nanos() as u64;
    Ok((true, metrics))
}

fn apply(
    db: &rusqlite::Connection,
    collection: &str,
    job: &Job,
    schema: &str,
    state: &RecordState,
) -> Result<usize> {
    let table = crate::store::record_table();
    Ok(db.prepare_cached(&format!("UPDATE {table} SET validation_state=?6,issues=?7 WHERE collection=?1 AND id=?2 AND revision=?3 AND digest=?4 AND validation_schema_hash=?5 AND validation_state='pending'"))?.execute(params![collection, job.id, job.revision, job.digest, schema, state.status.text(), serde_json::to_string(&state.issues).unwrap()])?)
}

#[cfg(test)]
#[path = "validation.tests.rs"]
mod tests;
