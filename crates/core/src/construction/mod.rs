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

#[path = "../mutation_segment.rs"]
mod mutation_segment;
pub(crate) use crate::native_segment::{IndexedCollectionSegment, SegmentFile, SegmentId};
pub(crate) use mutation_segment::PartitionRouting;
