//! Owned record batches and an ordered, bounded ingestion session.

pub(crate) mod publication;
pub(crate) mod queue;
mod spool;

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::inventory::{ByteRange, SourceRevision};
use crate::lifecycle::{IndexError, IndexRevision, RecordRegistration};
use crate::model::{RecordId, Value};
use crate::source::TextSegment;

pub use queue::{IngestionSession, Receipt, RecordWriter, SessionMetrics};

/// A file declaration local to one batch.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileToken(pub(crate) u32);

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct StagedFile {
    pub(crate) path: PathBuf,
    pub(crate) revision: SourceRevision,
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
        file: Option<(FileToken, Option<ByteRange>)>,
    },
    Replace {
        id: RecordId,
        value: Value,
        text: Vec<TextSegment>,
        file: Option<(FileToken, Option<ByteRange>)>,
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
    pub(crate) session_id: u64,
    pub(crate) registration: RecordRegistration,
    pub(crate) operations: Vec<Operation>,
    pub(crate) files: Vec<StagedFile>,
    keys: HashSet<RecordId>,
    pub(crate) estimated_bytes: usize,
}

impl Batch {
    pub(crate) fn new(session_id: u64, registration: RecordRegistration) -> Self {
        Self {
            session_id,
            registration,
            operations: Vec::new(),
            files: Vec::new(),
            keys: HashSet::new(),
            estimated_bytes: std::mem::size_of::<Self>(),
        }
    }

    fn push(&mut self, operation: Operation, value_bytes: usize) -> Result<(), String> {
        let id = operation.id();
        if !self.keys.insert(id.clone()) {
            return Err(format!("conflicting operations for record {}", id.0));
        }
        self.estimated_bytes = self.estimated_bytes.saturating_add(
            std::mem::size_of::<Operation>()
                .saturating_add(id.0.len())
                .saturating_add(value_bytes),
        );
        self.operations.push(operation);
        Ok(())
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
                file: None,
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
                file: None,
            },
            bytes,
        )
    }

    /// Declare a physical file whose revision was observed by the producer.
    /// A declaration remains useful even when this batch has no linked records.
    pub fn declare_file(
        &mut self,
        relative_path: impl AsRef<Path>,
        observed_file_revision: SourceRevision,
    ) -> Result<FileToken, String> {
        let path = relative_path.as_ref();
        if path.as_os_str().is_empty()
            || !path
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err("file path must be relative and contained".into());
        }
        if self.files.iter().any(|file| file.path == path) {
            return Err("file already declared in batch".into());
        }
        let token =
            FileToken(u32::try_from(self.files.len()).map_err(|_| "too many files in batch")?);
        self.estimated_bytes = self
            .estimated_bytes
            .saturating_add(std::mem::size_of::<StagedFile>())
            .saturating_add(path.as_os_str().len());
        self.files.push(StagedFile {
            path: path.to_path_buf(),
            revision: observed_file_revision,
        });
        Ok(token)
    }

    fn checked_file(
        &self,
        token: FileToken,
        range: Option<ByteRange>,
    ) -> Result<Option<(FileToken, Option<ByteRange>)>, String> {
        let file = self
            .files
            .get(token.0 as usize)
            .ok_or("unknown batch file token")?;
        if range.is_some_and(|range| range.end > file.revision.size || range.start > range.end) {
            return Err("file range exceeds observed file revision".into());
        }
        Ok(Some((token, range)))
    }

    pub fn insert_with_file(
        &mut self,
        id: impl AsRef<str>,
        value: Value,
        token: FileToken,
        range: Option<ByteRange>,
    ) -> Result<(), String> {
        let file = self.checked_file(token, range)?;
        let id = RecordId::intrinsic(id.as_ref())?;
        let bytes = estimated_value_bytes(&value);
        self.push(
            Operation::Insert {
                id,
                value,
                text: Vec::new(),
                file,
            },
            bytes,
        )
    }

    pub fn replace_with_file(
        &mut self,
        id: impl AsRef<str>,
        value: Value,
        token: FileToken,
        range: Option<ByteRange>,
    ) -> Result<(), String> {
        let file = self.checked_file(token, range)?;
        let id = RecordId::intrinsic(id.as_ref())?;
        let bytes = estimated_value_bytes(&value);
        self.push(
            Operation::Replace {
                id,
                value,
                text: Vec::new(),
                file,
            },
            bytes,
        )
    }

    pub fn delete(&mut self, id: impl AsRef<str>) -> Result<(), String> {
        let id = RecordId::intrinsic(id.as_ref())?;
        self.push(Operation::Delete { id }, 0)
    }

    pub fn len(&self) -> usize {
        self.operations.len()
    }
    pub fn is_empty(&self) -> bool {
        self.operations.is_empty() && self.files.is_empty()
    }
}

fn estimated_text_bytes(text: &[TextSegment]) -> usize {
    text.iter().fold(0usize, |total, segment| {
        total
            .saturating_add(std::mem::size_of::<TextSegment>())
            .saturating_add(segment.text.len())
            .saturating_add(segment.path.iter().map(String::len).sum::<usize>())
    })
}

fn estimated_value_bytes(value: &Value) -> usize {
    let mut total = 0usize;
    let mut pending = vec![value];
    while let Some(value) = pending.pop() {
        total = total.saturating_add(std::mem::size_of::<Value>());
        match value {
            Value::String(text) | Value::Id(text) => total = total.saturating_add(text.len()),
            Value::Object(fields) => {
                total =
                    total.saturating_add(fields.iter().map(|(key, _)| key.len()).sum::<usize>());
                pending.extend(fields.values());
            }
            Value::Array(array) => pending.extend(&array.values),
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
