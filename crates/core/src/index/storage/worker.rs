//! Ownership transferred from worker-local accumulation to ordered admission.

use std::any::Any;
use std::path::Path;

use super::text::BigramAlphabet;
use crate::construction::batch::{BatchContribution, BuildWorkProfile, SourceFileInput};
use crate::inference::InferenceConfig;
use crate::inventory::{ByteRange, FileEntry};
use crate::model::{CollectionId, FileId, RecordId, RecordSlot, Value};
use crate::query_access::membership::MembershipView;
use crate::records::{RecordHeader, RetainedBody};
use crate::repository::configuration::{
    CollectionDeclaration, CollectionDefinition, RepositoryOptions,
};
use crate::source::{
    ExtractedRecord, SourceOutcome, TextDerivation, TextSegment, registry::PreparedSource,
};

use super::transfer::StorageResult;
use crate::repository::statistics::BigramConstructionStats;

/// A worker seals this part after the last admitted record. The coordinator
/// can collect build statistics and a logical slot observation without
/// accessing any of the component accumulators.
pub(crate) trait PreparedIndexes: Any + Send {
    fn posting_store(&self) -> Option<std::sync::Arc<dyn super::posting::PostingStore>> {
        None
    }

    fn gram_store(&self) -> Option<std::sync::Arc<dyn super::text::GramStore>> {
        None
    }

    fn relation_store(&self) -> Option<std::sync::Arc<dyn super::relation::RelationStore>> {
        None
    }

    fn logical_slots(&self) -> MembershipView<'_>;
    fn bigram_storage_stats(&self) -> BigramConstructionStats;
}

/// Statically dispatched by a backend while it owns a worker batch.
pub(crate) trait WorkerWriter: Send + Sized {
    fn add_logical_indexes(
        &mut self,
        slot: RecordSlot,
        id: &RecordId,
        extracted: &ExtractedRecord,
        file: Option<(&FileEntry, Option<ByteRange>)>,
        root: Option<&Path>,
        text_derivation: TextDerivation,
        sample: bool,
        profile: &mut BuildWorkProfile,
    ) -> StorageResult<()>;

    fn add_fallback(
        &mut self,
        slot: RecordSlot,
        id: &RecordId,
        bytes: &[u8],
        file: &FileEntry,
        range: ByteRange,
        root: &Path,
    ) -> StorageResult<()>;

    fn finish(self) -> StorageResult<Box<dyn PreparedIndexes>>;
}

pub(crate) type DirectRecordInput = (RecordId, Value, Vec<TextSegment>);

pub(crate) struct DirectRecordBatch<'a> {
    pub inputs: &'a mut [DirectRecordInput],
    pub declaration: &'a CollectionDeclaration,
    pub options: RepositoryOptions,
    pub retain_bodies: bool,
    pub completed_records: &'a mut usize,
}

pub(crate) struct PreparedDirectBatch {
    pub headers: Vec<RecordHeader>,
    pub bodies: Vec<RetainedBody>,
    pub contribution: BatchContribution,
}

/// A bounded file worker task. Only one storage dispatch is needed to start it.
pub(crate) struct FileWorkerBatch<'a> {
    pub root: &'a Path,
    pub definition: &'a CollectionDefinition,
    pub collection: CollectionId,
    pub source: &'a PreparedSource,
    pub alphabet: BigramAlphabet,
    pub inference: InferenceConfig,
    pub inference_jobs: usize,
    pub input: FileWorkerInput<'a>,
}

pub(crate) enum FileWorkerInput<'a> {
    WholeFiles {
        ids: &'a [FileId],
        descriptors: &'a [SourceFileInput],
        profile: bool,
    },
    Framed {
        input: &'a SourceFileInput,
        bytes: &'a [u8],
        ranges: &'a [ByteRange],
        file_offset: u64,
        preceding_lines: u64,
        first_range: bool,
    },
    Outcome {
        input: &'a SourceFileInput,
        outcome: SourceOutcome,
    },
}
