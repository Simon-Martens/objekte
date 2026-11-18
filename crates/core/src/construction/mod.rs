//! Core construction worker boundaries.

mod batch;
pub(crate) mod discovery;
mod file_preparation;
pub(crate) mod initial;
pub(crate) mod repository_state;
mod scheduling;
mod segment_admission;
pub(crate) mod segment_jobs;

pub(crate) use batch::*;
pub(crate) use file_preparation::*;
pub(crate) use scheduling::*;
pub(crate) use segment_admission::*;

#[cfg(test)]
#[path = "../build.test.rs"]
mod tests;

pub(crate) use crate::index::segment::{IndexedCollectionSegment, SegmentFile, SegmentId};
