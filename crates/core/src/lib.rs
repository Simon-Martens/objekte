//! Record-oriented search over local repository files.
//!
//! The generic core is developed here. Fixture-specific schema registration and
//! workload execution live in the CLI integration layer.

/// Single saved-index compatibility version for the manifest, segment and
/// catalog envelopes, persisted subformats and writer-validation rules.
pub const PROGRAM_VERSION: u32 = 1;

pub mod bigram;
mod bigram_dictionary;
mod build;
mod byte_size_literal;
pub mod clock_context;
pub mod completion;
pub mod configuration;
mod decimal;
mod detailed_timer;
pub mod diagnostics;
pub mod discovery;
pub mod execution_options;
pub mod executor;
mod field_state;
pub mod fts_preview;
mod generation_catalog;
mod hybrid_segment_codec;
mod index_catalog;
pub mod index_storage;
pub mod inference;
pub mod inventory;
mod json_scalar_text;
pub mod lifecycle;
pub mod lifecycle_caller;
mod lifecycle_catalog;
mod lifecycle_compaction;
mod lifecycle_initial;
pub mod lifecycle_mutation;
mod lifecycle_open;
mod lifecycle_partition;
mod lifecycle_scan;
mod lifecycle_storage;
mod lifecycle_verified;
mod managed_store;
mod materialization;
pub mod measurement;
pub mod model;
mod ordered_block;
pub mod parameters;
pub mod path_repr;
pub mod posting;
mod posting_dictionary;
pub mod posting_values;
mod prepared;
pub mod query;
pub mod query_path;
mod query_validation;
mod record_preparation;
pub mod reference;
mod regex_query;
pub mod relation;
pub mod relation_inspection;
mod relation_query;
mod relation_semijoin;
mod relation_target_cache;
pub mod repository;
pub mod repository_configuration;
pub mod run;
pub mod schema_adapter;
mod segment_format;
mod segment_metadata;
mod sort_binding;
pub mod sorting;
pub mod source;
mod source_access;
pub mod source_scan;
pub mod streaming;
mod structural;
pub mod temporal_query;
#[cfg(test)]
mod test_support;
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
