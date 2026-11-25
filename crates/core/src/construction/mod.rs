//! Core construction worker boundaries.

pub(crate) mod batch;
pub(crate) mod discovery;
pub(crate) mod file_preparation;
pub(crate) mod initial;
pub(crate) mod repository_state;
pub(crate) mod scheduling;
pub(crate) mod segment_jobs;

#[cfg(test)]
mod tests;

#[cfg(feature = "test-hooks")]
pub(crate) mod file_shard_diagnostics;
