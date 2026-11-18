//! Compatibility façade for the repository's query execution entry points.
//!
//! The implementation moved behind the private `query_execution` module while
//! these crate and public paths remain stable for existing callers.

pub use crate::query_execution::runner::*;
