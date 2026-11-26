//! Bounded logical transfer between storage representations.

use std::fmt;
use std::num::NonZeroUsize;

pub(crate) type StorageResult<T> = Result<T, StorageError>;

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum StorageError {
    Cancelled,
    ResourceLimit(String),
    InvalidTransfer {
        operation: &'static str,
        detail: String,
    },
    OversizedItem {
        operation: &'static str,
        bytes: usize,
        limit: usize,
    },
}

impl StorageError {
    pub(crate) fn invalid(operation: &'static str, detail: impl Into<String>) -> Self {
        Self::InvalidTransfer {
            operation,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for StorageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cancelled => f.write_str("storage transfer cancelled"),
            Self::ResourceLimit(message) => f.write_str(message),
            Self::InvalidTransfer { operation, detail } => write!(f, "{operation}: {detail}"),
            Self::OversizedItem {
                operation,
                bytes,
                limit,
            } => write!(
                f,
                "{operation}: item needs {bytes} bytes, batch limit is {limit}"
            ),
        }
    }
}

impl std::error::Error for StorageError {}

impl From<StorageError> for String {
    fn from(error: StorageError) -> Self {
        error.to_string()
    }
}

#[derive(Clone, Copy)]
pub(crate) struct BatchLimit {
    items: NonZeroUsize,
    bytes: NonZeroUsize,
}

impl BatchLimit {
    pub(crate) const fn new(items: NonZeroUsize, bytes: NonZeroUsize) -> Self {
        Self { items, bytes }
    }

    pub(crate) fn fixed_batch_len(
        self,
        operation: &'static str,
        item_bytes: usize,
    ) -> StorageResult<usize> {
        self.accepts(operation, 0, 0, item_bytes)?;
        Ok(self.items.get().min(self.bytes.get() / item_bytes.max(1)))
    }

    pub(crate) fn fixed_batch_len_after(
        self,
        operation: &'static str,
        fixed_bytes: usize,
        item_bytes: usize,
    ) -> StorageResult<usize> {
        self.accepts(operation, 0, 0, fixed_bytes.saturating_add(item_bytes))?;
        Ok(self
            .items
            .get()
            .min((self.bytes.get() - fixed_bytes) / item_bytes.max(1)))
    }

    /// Check before consuming the next item. A single oversized item fails
    /// explicitly instead of exceeding the limit or making no progress.
    pub(crate) fn accepts(
        self,
        operation: &'static str,
        items_used: usize,
        bytes_used: usize,
        next_bytes: usize,
    ) -> StorageResult<bool> {
        if items_used >= self.items.get() {
            return Ok(false);
        }
        if items_used == 0 && next_bytes > self.bytes.get() {
            return Err(StorageError::OversizedItem {
                operation,
                bytes: next_bytes,
                limit: self.bytes.get(),
            });
        }
        Ok(next_bytes <= self.bytes.get().saturating_sub(bytes_used))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BatchStatus {
    More,
    Done,
}
