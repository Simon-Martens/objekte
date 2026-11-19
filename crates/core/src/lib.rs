//! Record-oriented search over local repository files.
//!
//! The generic core is developed here. Fixture-specific schema registration and
//! workload execution live in the CLI integration layer.

mod construction;
pub mod index;
pub mod inference;
pub mod inventory;
pub mod lifecycle;
pub mod measurement;
pub mod model;
pub mod query;
mod query_access;
pub mod query_diagnostics;
pub mod query_execution;
mod query_plan;
mod query_predicate;
pub mod query_results;
pub mod reference;
pub mod repository;
pub mod schema;
pub mod source;
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
