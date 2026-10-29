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
pub mod file_picker;
mod graph;
mod json;
mod json_filter;
mod query;
mod repository;
mod schema;
mod source;
mod validation;
pub use validation::{RecordState, RecordStatus, ValidationIssue, ValidationMetrics};

pub use config::{
    Collection, Direction, IndexField, ListOptions, RepositoryModel, Sort, ViewHashes,
};
pub use config_manifest::{
    ConfigurationChangeKind, ConfigurationChanges, ConfigurationInputChange,
};
pub use error::{Error, Result};
pub use graph::{CollectionGraph, CollectionRelation};
pub use json_filter::{
    JsonFilter, JsonPath, JsonPathStep, JsonQuery, JsonQueryBackend, JsonQueryDependencies,
    JsonQueryResult, JsonQueryStreamSummary,
};
pub use query::{Filter, Operator, ViewRequest, ViewResult, ViewRow};
pub use repository::{
    CacheMode, CacheStatus, Coverage, Freshness, Metrics, Options, Record, RecordRelation,
    RecordRevision, References, Repository, TargetState,
};
pub use schema::{Field, PathStep, ValueType};
