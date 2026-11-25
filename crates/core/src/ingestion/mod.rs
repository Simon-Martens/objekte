//! Owned record batches and an ordered, bounded ingestion session.

pub(crate) mod publication;
pub(crate) mod queue;
mod spool;

use std::collections::HashSet;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::lifecycle::{IndexError, IndexRevision, RecordRegistration};
use crate::model::{RecordId, Value};
use crate::records::RecordVersion;
use crate::source::TextSegment;

pub use queue::{IngestionSession, Receipt, RecordWriter, SessionMetrics};

/// One owned logical input with independently chosen text, provider version
/// without filesystem provenance.
pub struct RecordInput {
    value: Value,
    text: Vec<TextSegment>,
    version: Option<RecordVersion>,
}

impl RecordInput {
    pub fn new(value: Value) -> Self {
        Self {
            value,
            text: Vec::new(),
            version: None,
        }
    }

    pub fn with_text(mut self, text: Vec<TextSegment>) -> Self {
        self.text = text;
        self
    }

    pub fn from_reader(mut self, version: RecordVersion) -> Self {
        self.version = Some(version);
        self
    }
}

/// Pending input capacity. These budgets cover accepted input, not final
/// index storage or all in-flight construction allocations.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestionOptions {
    pub pending_ram_bytes: usize,
    pub max_batch_bytes: usize,
    pub spool_bytes: u64,
    pub spool_directory: Option<PathBuf>,
}

impl Default for IngestionOptions {
    fn default() -> Self {
        Self {
            pending_ram_bytes: 64 * 1024 * 1024,
            max_batch_bytes: 64 * 1024 * 1024,
            spool_bytes: 4 * 1024 * 1024 * 1024,
            spool_directory: None,
        }
    }
}

/// One owned change. A batch may contain each ID at most once.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum Operation {
    Insert {
        id: RecordId,
        value: Value,
        text: Vec<TextSegment>,
        version: Option<RecordVersion>,
    },
    Replace {
        id: RecordId,
        value: Value,
        text: Vec<TextSegment>,
        version: Option<RecordVersion>,
    },
    Delete {
        id: RecordId,
    },
}

impl Operation {
    pub(crate) fn id(&self) -> &RecordId {
        match self {
            Self::Insert { id, .. } | Self::Replace { id, .. } | Self::Delete { id } => id,
        }
    }
}

/// A producer-owned batch. Failed submission returns this exact input for
/// retry; successful submission transfers ownership to the session.
pub struct Batch {
    reader_available: bool,
    pub(crate) session_id: u64,
    pub(crate) registration: RecordRegistration,
    pub(crate) operations: Vec<Operation>,
    keys: HashSet<RecordId>,
    pub(crate) estimated_bytes: usize,
}

impl Batch {
    pub(crate) fn new(session_id: u64, registration: RecordRegistration) -> Self {
        Self {
            reader_available: false,
            session_id,
            registration,
            operations: Vec::new(),
            keys: HashSet::new(),
            estimated_bytes: std::mem::size_of::<Self>(),
        }
    }

    pub(crate) fn with_reader_capability(mut self, reader: bool) -> Self {
        self.reader_available = reader;
        self
    }

    fn push(&mut self, operation: Operation, value_bytes: usize) -> Result<(), String> {
        let id = operation.id();
        let id_bytes = id.0.len();
        let old_keys = self.keys.capacity();
        let old_operations = self.operations.capacity();
        if !self.keys.insert(id.clone()) {
            return Err(format!("conflicting operations for record {}", id.0));
        }
        self.operations.push(operation);
        self.estimated_bytes = self
            .estimated_bytes
            .saturating_add(id_bytes.saturating_mul(2).saturating_add(value_bytes));
        self.estimated_bytes = self
            .estimated_bytes
            .saturating_add(
                self.operations
                    .capacity()
                    .saturating_sub(old_operations)
                    .saturating_mul(std::mem::size_of::<Operation>()),
            )
            .saturating_add(
                self.keys
                    .capacity()
                    .saturating_sub(old_keys)
                    .saturating_mul(std::mem::size_of::<RecordId>() + std::mem::size_of::<usize>()),
            );
        Ok(())
    }

    /// Insert any supported combination of logical value, supplied text,
    /// provider version.
    pub fn insert_input(&mut self, id: impl AsRef<str>, input: RecordInput) -> Result<(), String> {
        self.push_input(id, input, false)
    }

    /// Replace using the same input choices as `insert_input`.
    pub fn replace_input(&mut self, id: impl AsRef<str>, input: RecordInput) -> Result<(), String> {
        self.push_input(id, input, true)
    }

    fn push_input(
        &mut self,
        id: impl AsRef<str>,
        input: RecordInput,
        replace: bool,
    ) -> Result<(), String> {
        if input.version.is_some() && !self.reader_available {
            return Err("provider input needs a registered reader".into());
        }
        let id = RecordId::intrinsic(id.as_ref())?;
        let bytes =
            estimated_value_bytes(&input.value).saturating_add(estimated_text_bytes(&input.text));
        let operation = if replace {
            Operation::Replace {
                id,
                value: input.value,
                text: input.text,
                version: input.version,
            }
        } else {
            Operation::Insert {
                id,
                value: input.value,
                text: input.text,
                version: input.version,
            }
        };
        self.push(operation, bytes)
    }

    pub fn insert(&mut self, id: impl AsRef<str>, value: Value) -> Result<(), String> {
        self.insert_with_text(id, value, Vec::new())
    }

    pub fn insert_with_text(
        &mut self,
        id: impl AsRef<str>,
        value: Value,
        text: Vec<TextSegment>,
    ) -> Result<(), String> {
        let id = RecordId::intrinsic(id.as_ref())?;
        let bytes = estimated_value_bytes(&value).saturating_add(estimated_text_bytes(&text));
        self.push(
            Operation::Insert {
                id,
                value,
                text,
                version: None,
            },
            bytes,
        )
    }

    pub fn replace(&mut self, id: impl AsRef<str>, value: Value) -> Result<(), String> {
        self.replace_with_text(id, value, Vec::new())
    }

    pub fn replace_with_text(
        &mut self,
        id: impl AsRef<str>,
        value: Value,
        text: Vec<TextSegment>,
    ) -> Result<(), String> {
        let id = RecordId::intrinsic(id.as_ref())?;
        let bytes = estimated_value_bytes(&value).saturating_add(estimated_text_bytes(&text));
        self.push(
            Operation::Replace {
                id,
                value,
                text,
                version: None,
            },
            bytes,
        )
    }

    pub fn delete(&mut self, id: impl AsRef<str>) -> Result<(), String> {
        let id = RecordId::intrinsic(id.as_ref())?;
        self.push(Operation::Delete { id }, 0)
    }

    /// Admit a decoded value for indexing, then reread its versioned source
    /// for residual evaluation or payloads after publication.
    pub fn insert_from_reader(
        &mut self,
        id: impl AsRef<str>,
        value: Value,
        version: RecordVersion,
    ) -> Result<(), String> {
        self.insert_from_reader_with_text(id, value, Vec::new(), version)
    }

    pub fn insert_from_reader_with_text(
        &mut self,
        id: impl AsRef<str>,
        value: Value,
        text: Vec<TextSegment>,
        version: RecordVersion,
    ) -> Result<(), String> {
        if !self.reader_available {
            return Err("provider input needs a registered reader".into());
        }
        let id = RecordId::intrinsic(id.as_ref())?;
        let bytes = estimated_value_bytes(&value).saturating_add(estimated_text_bytes(&text));
        self.push(
            Operation::Insert {
                id,
                value,
                text,
                version: Some(version),
            },
            bytes,
        )
    }

    pub fn replace_from_reader(
        &mut self,
        id: impl AsRef<str>,
        value: Value,
        version: RecordVersion,
    ) -> Result<(), String> {
        if !self.reader_available {
            return Err("provider input needs a registered reader".into());
        }
        let id = RecordId::intrinsic(id.as_ref())?;
        let bytes = estimated_value_bytes(&value);
        self.push(
            Operation::Replace {
                id,
                value,
                text: Vec::new(),
                version: Some(version),
            },
            bytes,
        )
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty()
    }

    pub(crate) fn owned_capacity_bytes(&self) -> usize {
        let mut total =
            std::mem::size_of::<Self>()
                .saturating_add(
                    self.operations
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Operation>()),
                )
                .saturating_add(self.keys.capacity().saturating_mul(
                    std::mem::size_of::<RecordId>() + std::mem::size_of::<usize>(),
                ));
        for key in &self.keys {
            total = total.saturating_add(key.0.len());
        }
        for operation in &self.operations {
            match operation {
                Operation::Insert {
                    id, value, text, ..
                }
                | Operation::Replace {
                    id, value, text, ..
                } => {
                    total = total
                        .saturating_add(id.0.len())
                        .saturating_add(estimated_value_bytes(value))
                        .saturating_add(estimated_text_bytes(text));
                }
                Operation::Delete { id } => {
                    total = total.saturating_add(id.0.len());
                }
            }
        }
        total
    }
}

pub(crate) fn estimated_text_bytes(text: &Vec<TextSegment>) -> usize {
    text.iter().fold(
        text.capacity()
            .saturating_mul(std::mem::size_of::<TextSegment>()),
        |total, segment| {
            total
                .saturating_add(segment.text.capacity())
                .saturating_add(
                    segment
                        .path
                        .capacity()
                        .saturating_mul(std::mem::size_of::<String>()),
                )
                .saturating_add(
                    segment
                        .path
                        .iter()
                        .fold(0usize, |bytes, part| bytes.saturating_add(part.capacity())),
                )
        },
    )
}

pub(crate) fn estimated_value_bytes(value: &Value) -> usize {
    let mut total = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        total = total.saturating_add(std::mem::size_of::<Value>());
        match value {
            Value::String(text) | Value::Id(text) => total = total.saturating_add(text.capacity()),
            Value::Number(crate::model::Number::ExactDecimal(decimal)) => {
                total = total.saturating_add(decimal.digits.capacity());
            }
            Value::Object(fields) => {
                total = total.saturating_add(fields.iter().fold(0usize, |bytes, (key, _)| {
                    bytes.saturating_add(key.capacity()).saturating_add(64)
                }));
                pending.extend(fields.values());
            }
            Value::Array(array) => {
                total = total.saturating_add(
                    array
                        .values
                        .capacity()
                        .saturating_mul(std::mem::size_of::<Value>()),
                );
                pending.extend(&array.values);
            }
            _ => {}
        }
    }
    total
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IngestionError {
    Index(IndexError),
    Spool(String),
    Cancelled { originating_sequence: u64 },
}

impl std::fmt::Display for IngestionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Index(error) => error.fmt(f),
            Self::Spool(message) => write!(f, "ingestion spool: {message}"),
            Self::Cancelled {
                originating_sequence,
            } => write!(
                f,
                "ingestion cancelled after sequence {originating_sequence}"
            ),
        }
    }
}
impl std::error::Error for IngestionError {}

/// Pre-acceptance failures retain the caller's batch.
pub enum SubmitError<B> {
    WouldBlock(B),
    Capacity(B),
    BatchTooLarge(B),
    EmptyBatch(B),
    Closed(B),
    ForeignHandle(B),
    SpoolIo(B, String),
}

impl<B> SubmitError<B> {
    pub fn into_batch(self) -> B {
        match self {
            Self::WouldBlock(batch)
            | Self::Capacity(batch)
            | Self::BatchTooLarge(batch)
            | Self::EmptyBatch(batch)
            | Self::Closed(batch)
            | Self::ForeignHandle(batch)
            | Self::SpoolIo(batch, _) => batch,
        }
    }
}

impl<B> std::fmt::Debug for SubmitError<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            Self::WouldBlock(_) => "WouldBlock",
            Self::Capacity(_) => "Capacity",
            Self::BatchTooLarge(_) => "BatchTooLarge",
            Self::EmptyBatch(_) => "EmptyBatch",
            Self::Closed(_) => "Closed",
            Self::ForeignHandle(_) => "ForeignHandle",
            Self::SpoolIo(_, _) => "SpoolIo",
        };
        f.write_str(name)
    }
}

impl<B> std::fmt::Display for SubmitError<B> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}
impl<B> std::error::Error for SubmitError<B> {}

pub type PublishedRevision = Result<IndexRevision, IngestionError>;
