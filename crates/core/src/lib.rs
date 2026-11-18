//! Record-oriented search over local repository files.
//!
//! The generic core is developed here. Fixture-specific schema registration and
//! workload execution live in the CLI integration layer.

pub mod completion;
pub mod configuration;
mod construction;
mod decimal;
mod detailed_timer;
pub mod diagnostics;
pub mod discovery;
pub mod execution_options;
pub mod index;
pub mod inference;
pub mod inventory;
pub mod lifecycle;
mod lifecycle_catalog;
mod lifecycle_compaction;
mod lifecycle_initial;
pub mod lifecycle_mutation;
mod lifecycle_partition;
mod lifecycle_scan;
mod lifecycle_verified;
pub mod measurement;
pub mod model;
pub mod path_repr;
pub mod posting_values;
pub mod query;
mod query_access;
pub mod query_diagnostics;
pub mod query_execution;
mod query_plan;
mod query_predicate;
pub mod query_results;
mod record_preparation;
pub mod reference;
pub mod relation;
pub mod relation_inspection;
mod relation_query;
mod relation_semijoin;
mod relation_target_cache;
pub mod repository;
pub mod repository_configuration;
pub mod schema;
pub mod source;
pub mod streaming;
mod structural;
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
