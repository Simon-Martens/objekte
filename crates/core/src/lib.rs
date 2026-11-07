//! Record-oriented search over local repository files.
//!
//! The generic core is developed here. Fixture-specific schema registration and
//! workload execution live in the CLI integration layer.

/// Version of the in-memory search model and future snapshot envelope.
pub const MODEL_VERSION: u32 = 10;

pub mod bigram;
mod build;
mod byte_size_literal;
mod decimal;
pub mod clock_context;
pub mod discovery;
pub mod executor;
pub mod inference;
pub mod inventory;
mod json_scalar_text;
pub mod model;
pub mod parameters;
pub mod path_repr;
pub mod posting;
pub mod posting_values;
mod prepared;
pub mod query;
pub mod query_path;
mod record_preparation;
pub mod reference;
mod regex_query;
pub mod relation;
pub mod relation_inspection;
mod relation_query;
mod relation_semijoin;
mod relation_target_cache;
pub mod repository;
pub mod run;
mod sort_binding;
pub mod sorting;
pub mod source;
mod source_access;
mod structural;
pub mod storage;
pub mod streaming;
pub mod temporal_query;
mod verified;
pub mod workload;

#[cfg(test)]
#[path = "lib.test.rs"]
mod tests;

#[cfg(test)]
#[path = "foundation_invariants.test.rs"]
mod foundation_invariants;

#[cfg(test)]
#[path = "id.test.rs"]
mod id_tests;

#[cfg(test)]
#[path = "folder.test.rs"]
mod folder_tests;
