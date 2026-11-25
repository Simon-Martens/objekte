//! Worker-local in-memory index payload handed to native segment admission.

use std::any::Any;
use std::collections::HashMap;
use std::path::Path;

use roaring::RoaringBitmap;

use crate::construction::batch::{
    BuildWorkProfile, canonical_custom_text_key, map_binding_value, string_values,
};
use crate::index::memory::inference::InferenceBuilder;
use crate::index::storage::transfer::{StorageError, StorageResult};
use crate::index::storage::worker::{PreparedIndexes, WorkerWriter};
use crate::inference::InferenceConfig;
use crate::inventory::{ByteRange, FileEntry, RecordClass};
use crate::model::FieldPath;
use crate::model::{RecordId, RecordSlot, Value};
use crate::query::semantics::record::data_values;
use crate::query_access::membership::MembershipView;
use crate::repository::configuration::CollectionPolicy;
use crate::repository::statistics::BigramConstructionStats;
use crate::schema::Schema;
use crate::source::{ExtractedRecord, TextDerivation, text::visit_logical_strings_and_empty};

use super::bigram::{BigramAlphabet, BigramBuilder, BigramContribution};
use super::posting::{CustomPostingIndex, PostingCollection, PostingIndex};
use super::relation::RelationContribution;
use super::sorting::{SortBuilder, SortKey};

pub(super) struct NativeIndexBatch {
    pub(super) postings: PostingCollection,
    pub(super) inference: Option<InferenceBuilder>,
    pub(super) custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    pub(super) raw: BigramContribution,
    pub(super) fallback_text: BigramContribution,
    pub(super) custom_text: HashMap<(Vec<String>, String), BigramContribution>,
    pub(super) logical_records: RoaringBitmap,
    pub(super) empty_array_records: RoaringBitmap,
    pub(super) sort_keys: HashMap<FieldPath, Vec<SortKey>>,
    pub(super) relations: Vec<RelationContribution>,
}

impl NativeIndexBatch {
    pub(super) fn take(indexes: Box<dyn PreparedIndexes>) -> Result<Box<Self>, String> {
        let indexes: Box<dyn Any + Send> = indexes;
        indexes
            .downcast()
            .map_err(|_| "segment builder received incompatible worker components".into())
    }
}

impl PreparedIndexes for NativeIndexBatch {
    fn logical_slots(&self) -> MembershipView<'_> {
        MembershipView::Borrowed(&self.logical_records)
    }

    fn bigram_storage_stats(&self) -> BigramConstructionStats {
        let mut stats = self.raw.storage_stats();
        stats.absorb(self.fallback_text.storage_stats());
        for contribution in self.custom_text.values() {
            stats.absorb(contribution.storage_stats());
        }
        stats
    }
}

/// Accumulators owned exclusively by one worker until it finishes a batch.
pub(super) struct MemoryWorker<'a> {
    schema: &'a Schema,
    collection_name: &'a str,
    postings: PostingCollection,
    inference: Option<InferenceBuilder>,
    custom_postings: HashMap<(Vec<String>, String), CustomPostingIndex>,
    raw: BigramBuilder,
    fallback_text: BigramBuilder,
    custom_text: HashMap<(Vec<String>, String), BigramBuilder>,
    logical_records: RoaringBitmap,
    empty_array_records: RoaringBitmap,
    sorting: SortBuilder,
    relations: Vec<RelationContribution>,
}

impl<'a> MemoryWorker<'a> {
    pub(super) fn new(
        definition: &'a dyn CollectionPolicy,
        alphabet: BigramAlphabet,
        inference: InferenceConfig,
        inference_jobs: usize,
    ) -> Result<Self, String> {
        let schema = definition.schema();
        let custom_postings = schema
            .operations
            .iter()
            .filter(|(_, binding)| binding.operation.posting())
            .map(|(key, binding)| {
                (
                    key.clone(),
                    CustomPostingIndex {
                        index: PostingIndex::new(&schema.fields[&key.0]),
                        complete: binding.operation.complete_coverage(),
                    },
                )
            })
            .collect();
        let custom_text = schema
            .custom
            .keys()
            .chain(
                schema
                    .operations
                    .iter()
                    .filter(|(_, binding)| {
                        binding.operation.bigram() && binding.operation.complete_coverage()
                    })
                    .map(|(key, _)| key),
            )
            .map(|key| {
                (
                    canonical_custom_text_key(schema, &key.0, &key.1),
                    BigramBuilder::new(alphabet),
                )
            })
            .collect();
        Ok(Self {
            schema,
            collection_name: definition.name(),
            postings: PostingCollection::new(schema),
            inference: InferenceBuilder::with_job_count(inference, true, inference_jobs),
            custom_postings,
            raw: BigramBuilder::new(alphabet),
            fallback_text: BigramBuilder::new(alphabet),
            custom_text,
            logical_records: RoaringBitmap::new(),
            empty_array_records: RoaringBitmap::new(),
            sorting: SortBuilder::new(
                definition.sort_fields(),
                schema,
                definition.sort_locale(),
                definition.sort_options(),
                definition.sort_overrides(),
            )?,
            relations: RelationContribution::for_schema(schema),
        })
    }

    fn add_fallback(
        &mut self,
        slot: RecordSlot,
        id: &RecordId,
        bytes: &[u8],
        file: &FileEntry,
        range: ByteRange,
        root: &Path,
    ) -> Result<(), String> {
        self.raw.add_empty(slot)?;
        for relation in &mut self.relations {
            relation.add_unavailable(slot)?;
        }
        let text = String::from_utf8_lossy(bytes);
        self.fallback_text
            .add_record(slot, std::iter::once(text.as_ref()))?;
        for index in self.custom_text.values_mut() {
            index.add_empty(slot)?;
        }
        let empty = Value::Object(Default::default());
        self.sorting.add(
            slot,
            &empty,
            id,
            RecordClass::RawFallback,
            Some((file, Some(range))),
            Some(root),
            self.collection_name,
        )?;
        Ok(())
    }

    fn finish(self) -> Result<Box<dyn PreparedIndexes>, String> {
        let custom_text = self
            .custom_text
            .into_iter()
            .map(|(key, index)| index.finish_blocks().map(|value| (key, value)))
            .collect::<Result<HashMap<_, _>, _>>()?;
        Ok(Box::new(NativeIndexBatch {
            postings: self.postings,
            inference: self.inference,
            custom_postings: self.custom_postings,
            raw: self.raw.finish_blocks()?,
            fallback_text: self.fallback_text.finish_blocks()?,
            custom_text,
            logical_records: self.logical_records,
            empty_array_records: self.empty_array_records,
            sort_keys: self.sorting.into_keys(),
            relations: self.relations,
        }))
    }
}

impl MemoryWorker<'_> {
    /// Index one validated logical value after its registrar has resolved the
    /// record identity. Source-specific fallback and identity remain outside.
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
    ) -> Result<(), String> {
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        self.postings.add(slot, &extracted.value)?;
        if let Some(started) = stage_started {
            profile.postings_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        if let Some(inference) = &mut self.inference {
            inference.observe_record_with_schema(slot, &extracted.value, Some(self.schema))?;
        }
        if let Some(started) = stage_started {
            profile.inference_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        for relation in self.relations.iter_mut() {
            relation.add_logical(slot, &extracted.value)?;
        }
        if let Some(started) = stage_started {
            profile.relations_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        for (key, custom) in self.custom_postings.iter_mut() {
            let binding = &self.schema.operations[key];
            let selected = data_values(&extracted.value, &key.0);
            let mapped = selected.as_ref().map(|values| {
                values
                    .iter()
                    .map(|value| map_binding_value(binding.operation.as_ref(), value))
                    .collect::<Option<Vec<_>>>()
            });
            let mapped = match mapped {
                Some(Some(values)) => Some(values),
                Some(None) => {
                    custom.complete = false;
                    None
                }
                None => None,
            };
            let refs = mapped
                .as_ref()
                .map(|values| values.iter().collect::<Vec<_>>());
            custom.index.add_values(slot, refs.as_deref())?;
        }
        if let Some(started) = stage_started {
            profile.custom_postings_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        let has_empty_array;
        if text_derivation == TextDerivation::LogicalStringsAfterConversion {
            if let Some(binding) = &self.schema.raw {
                let mut observed_empty = false;
                self.raw.add_record_with(slot, |emit| {
                    observed_empty =
                        visit_logical_strings_and_empty(&extracted.value, &mut |text| {
                            emit(&binding.operation.map_text(text));
                        });
                })?;
                has_empty_array = observed_empty;
            } else {
                let mut observed_empty = false;
                self.raw.add_record_with(slot, |emit| {
                    observed_empty = visit_logical_strings_and_empty(&extracted.value, emit);
                })?;
                has_empty_array = observed_empty;
            }
        } else {
            has_empty_array = crate::model::structure::contains_empty_array(&extracted.value);
            if let Some(binding) = &self.schema.raw {
                self.raw.add_record(
                    slot,
                    extracted
                        .text
                        .iter()
                        .map(|segment| binding.operation.map_text(&segment.text)),
                )?;
            } else {
                self.raw.add_record(
                    slot,
                    extracted.text.iter().map(|segment| segment.text.as_str()),
                )?;
            }
        }
        self.fallback_text.add_empty(slot)?;
        self.logical_records.insert(slot.0);
        if has_empty_array {
            self.empty_array_records.insert(slot.0);
        }
        if let Some(started) = stage_started {
            profile.text_bigram_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        self.sorting.add(
            slot,
            &extracted.value,
            id,
            RecordClass::Logical,
            file,
            root,
            self.collection_name,
        )?;
        if let Some(started) = stage_started {
            profile.sorting_ns += started.elapsed().as_nanos() as u64;
        }
        let stage_started = crate::measurement::timer::start_if_enabled(sample);
        for (key, index) in self.custom_text.iter_mut() {
            let values = data_values(&extracted.value, &key.0).unwrap_or_default();
            if let Some(binding) = self.schema.operations.get(key) {
                index.add_record(
                    slot,
                    values
                        .into_iter()
                        .flat_map(string_values)
                        .map(|text| binding.operation.map_text(text)),
                )?;
            } else {
                index.add_record(slot, values.into_iter().flat_map(string_values))?;
            }
        }
        if let Some(started) = stage_started {
            profile.custom_text_ns += started.elapsed().as_nanos() as u64;
        }
        Ok(())
    }
}

impl WorkerWriter for MemoryWorker<'_> {
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
    ) -> StorageResult<()> {
        MemoryWorker::add_logical_indexes(
            self,
            slot,
            id,
            extracted,
            file,
            root,
            text_derivation,
            sample,
            profile,
        )
        .map_err(|message| StorageError::invalid("accumulate worker indexes", message))
    }

    fn add_fallback(
        &mut self,
        slot: RecordSlot,
        id: &RecordId,
        bytes: &[u8],
        file: &FileEntry,
        range: ByteRange,
        root: &Path,
    ) -> StorageResult<()> {
        MemoryWorker::add_fallback(self, slot, id, bytes, file, range, root)
            .map_err(|message| StorageError::invalid("accumulate fallback indexes", message))
    }

    fn finish(self) -> StorageResult<Box<dyn PreparedIndexes>> {
        MemoryWorker::finish(self)
            .map_err(|message| StorageError::invalid("seal worker indexes", message))
    }
}
