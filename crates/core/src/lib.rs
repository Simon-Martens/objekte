mod config;
mod documents;
mod media;

pub use config::{CategoryEntry, CategoryLevel, Collection, Config, IdStrategy};
pub use documents::{CategoryNode, Document, Editor, RecordSummary, Reference, ValidationIssue};
pub use media::{MediaEntry, MediaKind};

use std::path::{Path, PathBuf};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("invalid configuration: {0}")]
    Config(String),
    #[error("invalid path or identifier: {0}")]
    InvalidPath(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
    #[error("validation failed")]
    Validation(Vec<ValidationIssue>),
    #[error("{0}")]
    Data(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

pub fn safe_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name.contains(['/', '\\', '\0'])
        || name.chars().any(char::is_control)
    {
        return Err(Error::InvalidPath(name.into()));
    }
    Ok(())
}

pub(crate) fn contained(root: &Path, relative: &str) -> Result<PathBuf> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(Error::InvalidPath(relative.display().to_string()));
    }
    let path = root.join(relative);
    let canonical = path.canonicalize()?;
    if !canonical.starts_with(root) {
        return Err(Error::InvalidPath(relative.display().to_string()));
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests;
