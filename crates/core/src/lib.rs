mod collation;
mod collection_config;
mod config;
mod documents;
pub mod draft;
mod json_documents;
mod media;
mod policy;
mod project_config;
pub mod ui;
pub use collation::alphabetic_cmp;

pub use config::{CategoryEntry, CategoryLevel, Collection, Config, IdStrategy, MediaCollection};
pub use documents::{
    CategoryNode, ChildCollection, Document, Editor, HierarchyNode, ListFacet, ListMatch, ListPage,
    ListRequest, ListRow, ParentLink, RecordSummary, Reference, ValidationIssue, display_text,
};
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
pub use policy::insert_markdown_link;
#[cfg(test)]
mod mvp_tests;
