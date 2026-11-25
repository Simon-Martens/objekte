//! Native in-memory index storage.

pub mod bigram;
pub(crate) mod file_catalog;
pub mod posting;
pub mod relation;
pub mod segment;
pub(crate) mod segment_catalog;
pub mod sorting;
