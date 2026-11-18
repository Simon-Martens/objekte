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

use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::sync::Arc;
#[cfg(feature = "detailed-profiling")]
use std::sync::atomic::AtomicU64;

use rayon::ThreadPool;
use roaring::RoaringBitmap;
use sha2::Digest;

use crate::bigram::BigramContribution;
use crate::inference::{InferenceBuilder, InferenceConfig};
use crate::inventory::{
    CollectionBuilder, DiagnosticRef, FileEntry, FileInventory, FileStatus, RecordClass,
    RecordEntry, append_shifted_bitmap,
};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot};
use crate::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use crate::relation::RelationContribution;
use crate::repository::{CollectionDefinition, canonical_custom_text_key};
use crate::sorting::SortBuilder;
use crate::source::PreparedSource;
use crate::workload::BigramAlphabet;

#[cfg(test)]
#[path = "../build.test.rs"]
mod tests;

#[path = "../mutation_segment.rs"]
mod mutation_segment;
pub(crate) use crate::native_segment::{IndexedCollectionSegment, SegmentFile, SegmentId};
pub(crate) use mutation_segment::PartitionRouting;

pub(crate) use mutation_segment::{CollectionSegment, SegmentAdmission};
