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
mod error;
mod graph;
mod json;
mod query;
mod repository;
mod schema;
mod source;
mod store;

pub use config::{
    Collection, Direction, IndexField, ListOptions, RepositoryModel, Sort, ViewHashes,
};
pub use error::{Error, Result};
pub use graph::{CollectionGraph, CollectionRelation};
pub use query::{Filter, Operator, ViewRequest, ViewResult, ViewRow};
pub use repository::{
    CacheMode, CacheStatus, Coverage, Freshness, Metrics, Options, Record, RecordRelation,
    References, Repository, TargetState,
};
pub use schema::{Field, PathStep, ValueType};
