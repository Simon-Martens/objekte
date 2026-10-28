//! File-backed repositories with explicit relationships and disposable indexes.
//! Metadata loading is separate from opening a view. All source files are read-only.
//!
//! ```no_run
//! use objekte_core::{Options, Repository, ViewRequest};
//! let mut repository = Repository::load_repository(
//!     "examples/musenalm", Options::new("/tmp/objekte-example"),
//! )?;
//! let page = repository.open_view("places", &ViewRequest::default())?;
//! assert!(page.rows.len() <= 50);
//! repository.refresh("places")?;
//! # Ok::<(), objekte_core::Error>(())
//! ```
mod config;
mod config_manifest;
mod error;
mod graph;
mod json;
mod json_filter;
mod query;
mod repository;
mod schema;
mod source;
mod store;
mod validation;
pub use validation::{
    RecordState, RecordStatus, ValidationIssue, ValidationMetrics, ValidationMode,
};

pub use config::{
    Collection, Direction, IndexField, ListOptions, RepositoryModel, Sort, ViewHashes,
};
pub use config_manifest::{
    ConfigurationChangeKind, ConfigurationChanges, ConfigurationInputChange,
};
pub use error::{Error, Result};
pub use graph::{CollectionGraph, CollectionRelation};
pub use json_filter::{
    JsonFilter, JsonPath, JsonPathStep, JsonQuery, JsonQueryDependencies, JsonQueryResult,
};
pub use query::{Filter, Operator, ViewRequest, ViewResult, ViewRow};
pub use repository::{
    CacheMode, CacheStatus, Coverage, Freshness, Metrics, Options, Record, RecordRelation,
    References, Repository, TargetState,
};
pub use schema::{Field, PathStep, ValueType};

/// Register the deterministic functions and collation used by cache expression
/// indexes on an externally opened SQLite connection (for example, before running
/// `PRAGMA integrity_check`). Repository-managed connections register automatically.
/// The database remains a disposable cache; authoritative edits belong in source files.
pub fn register_cache_sqlite_functions(db: &rusqlite::Connection) -> Result<()> {
    json_filter::register(db)?;
    json_filter::indexes::register(db)
}
