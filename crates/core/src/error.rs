use std::path::{Path, PathBuf};


#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("configuration {path}#{location}: {message}")]
    Config {
        path: PathBuf,
        location: String,
        message: String,
    },
    #[error("record {path}#{location}: {message}")]
    Record {
        path: PathBuf,
        location: String,
        message: String,
    },
    #[error("invalid request: {0}")]
    Request(String),
    #[error("source changed during reconciliation: {0}")]
    Unstable(PathBuf),
    #[error("cache: {0}")]
    Cache(String),
    #[error("SQLite: {0}")]
    Sql(#[from] rusqlite::Error),
}


pub type Result<T> = std::result::Result<T, Error>;
pub(crate) fn io(path: impl AsRef<Path>, source: std::io::Error) -> Error {
    Error::Io {
        path: path.as_ref().into(),
        source,
    }
}

pub(crate) fn config(path: &Path, location: impl Into<String>, message: impl ToString) -> Error {
    Error::Config {
        path: path.into(),
        location: location.into(),
        message: message.to_string(),
    }
}

pub(crate) fn record(path: &Path, location: impl Into<String>, message: impl ToString) -> Error {
    Error::Record {
        path: path.into(),
        location: location.into(),
        message: message.to_string(),
    }
}
