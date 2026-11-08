//! Errors that the desktop can display without parsing CLI stderr.

use std::fmt;


/// The operation that failed or was cancelled.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BackendErrorKind {
    Options,
    Open,
    Query,
    Cancelled,
    Output,
}


/// One actionable error from a background data operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BackendError {
    pub kind: BackendErrorKind,
    pub message: String,
}


impl BackendError {
    pub fn new(kind: BackendErrorKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
        }
    }

    pub fn cancelled() -> Self {
        Self::new(BackendErrorKind::Cancelled, "operation cancelled")
    }
}


impl fmt::Display for BackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}


impl std::error::Error for BackendError {}
