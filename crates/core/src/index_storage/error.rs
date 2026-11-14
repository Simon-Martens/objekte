//! Typed failures for packed index storage.
//!
//! Storage errors are deliberately separate from query and lifecycle errors:
//! callers can distinguish missing or incompatible persistent state from a
//! corrupt object, and a failed mapped read is never converted into an empty
//! posting or a missing dictionary key.

use std::fmt;
use std::io;
use std::path::PathBuf;


/// One checked storage operation failed.
#[derive(Debug)]
pub enum StorageError {
    /// An operating-system operation failed.
    Io { path: PathBuf, source: io::Error },
    /// A file or object violated the format contract.
    Corrupt {
        path: Option<PathBuf>,
        message: String,
    },
    /// The store or object uses a format this build does not understand.
    UnsupportedFormat(String),
    /// The persisted configuration does not match the requested runtime.
    Incompatible(String),
    /// A referenced content-addressed object is absent.
    MissingObject(String),
    /// The generation manifest is absent, unreadable, or invalid.
    InvalidManifest(String),
    /// An object header is invalid or truncated.
    InvalidHeader(String),
    /// A lazily validated region failed its checksum.
    ChecksumMismatch { path: Option<PathBuf>, offset: u64 },
    /// A configured bounded resource was exhausted or exceeded.
    ResourceLimit(String),
    /// The operation observed a cancellation request.
    Cancelled,
    /// A concurrent writer changed the destination during the operation.
    Conflict(String),
    /// A lock could not be acquired.
    Lock(String),
    /// The requested path or generation does not exist.
    NotFound(PathBuf),
    /// The caller supplied an unusable argument.
    InvalidArgument(String),
}


/// Result alias used throughout the storage subsystem.
pub type StorageResult<T> = Result<T, StorageError>;


impl StorageError {
    pub fn io(path: impl Into<PathBuf>, source: io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }

    pub fn corrupt(message: impl Into<String>) -> Self {
        Self::Corrupt {
            path: None,
            message: message.into(),
        }
    }

    pub fn corrupt_at(path: impl Into<PathBuf>, message: impl Into<String>) -> Self {
        Self::Corrupt {
            path: Some(path.into()),
            message: message.into(),
        }
    }

    pub fn checksum(offset: u64) -> Self {
        Self::ChecksumMismatch { path: None, offset }
    }

    /// Attach a path to an error that did not have one.
    pub fn with_path(self, path: impl Into<PathBuf>) -> Self {
        let path = path.into();
        match self {
            Self::Corrupt {
                path: None,
                message,
            } => Self::Corrupt {
                path: Some(path),
                message,
            },
            Self::ChecksumMismatch { path: None, offset } => Self::ChecksumMismatch {
                path: Some(path),
                offset,
            },
            other => other,
        }
    }
}


impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "index storage I/O {}: {source}", path.display())
            }
            Self::Corrupt {
                path: Some(path),
                message,
            } => write!(
                formatter,
                "corrupt index storage {}: {message}",
                path.display()
            ),
            Self::Corrupt {
                path: None,
                message,
            } => {
                write!(formatter, "corrupt index storage: {message}")
            }
            Self::UnsupportedFormat(message) => {
                write!(formatter, "unsupported index storage format: {message}")
            }
            Self::Incompatible(message) => {
                write!(formatter, "incompatible index storage: {message}")
            }
            Self::MissingObject(name) => write!(formatter, "missing index object {name}"),
            Self::InvalidManifest(message) => {
                write!(formatter, "invalid index manifest: {message}")
            }
            Self::InvalidHeader(message) => {
                write!(formatter, "invalid index object header: {message}")
            }
            Self::ChecksumMismatch {
                path: Some(path),
                offset,
            } => write!(
                formatter,
                "index object checksum mismatch at offset {offset} in {}",
                path.display()
            ),
            Self::ChecksumMismatch { path: None, offset } => {
                write!(
                    formatter,
                    "index object checksum mismatch at offset {offset}"
                )
            }
            Self::ResourceLimit(message) => {
                write!(formatter, "index storage resource limit: {message}")
            }
            Self::Cancelled => formatter.write_str("index storage operation cancelled"),
            Self::Conflict(message) => write!(formatter, "index storage conflict: {message}"),
            Self::Lock(message) => write!(formatter, "index storage lock: {message}"),
            Self::NotFound(path) => {
                write!(formatter, "index storage not found: {}", path.display())
            }
            Self::InvalidArgument(message) => {
                write!(formatter, "invalid index storage argument: {message}")
            }
        }
    }
}


impl std::error::Error for StorageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}


impl From<io::Error> for StorageError {
    fn from(source: io::Error) -> Self {
        Self::Io {
            path: PathBuf::new(),
            source,
        }
    }
}
