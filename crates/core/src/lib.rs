//! Record-oriented search over local repository files.
//!
//! The generic core is developed here. Fixture-specific schema registration and
//! workload execution live in the CLI integration layer.
//!
//! # Ownership map
//!
//! | Contribution | Public entry point | Implementation owner |
//! |---|---|---|
//! | Register a source reader | `source::registry::SourceRegistry` | External source reader libraries and host registrations |
//! | Change physical posting storage | `index::posting` | `index::posting`, `index::bigram` and segment projection |
//! | Change a planner decision | `query::Query`, `query_execution::restrictions` | `query_plan`, `query_access` and relation provider modules |
//! | Declare a relation | `schema::relations`, `Schema::register_relation` | schema relation validation and `index::relation` physical storage |
//! | Publish a revised index | `lifecycle` | construction, segment publication and repository state |

pub mod adapter;
mod construction;
pub mod files;
pub mod index;
pub mod inference;
pub mod ingestion;
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
pub mod records;
pub mod reference;
pub mod repository;
pub mod schema;
pub mod source;

#[cfg(test)]
mod tests;
