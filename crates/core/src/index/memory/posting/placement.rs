use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use crate::construction::batch::map_binding_value;
use crate::index::memory::segment::projection::SlotMap;
use crate::index::memory::worker::FactBuffer;
use crate::index::traits::component::ComponentKind;
use crate::index::traits::posting::{
    POSTING_BLOCK_LIMIT, PostingDefinition, PostingFact, PostingReader, PostingStore,
    PostingWriter, visit_posting_blocks,
};
use crate::index::traits::transfer::{StorageError, StorageResult};
use crate::model::{RecordSlot, Value};
use crate::query::semantics::record::data_values_small;
use crate::schema::Schema;

type Binding = (Vec<String>, Option<String>);
const OP: &str = "write selected posting";

#[derive(Default)]
pub(in crate::index::memory) struct PostingBatches {
    pub store: Option<Arc<dyn PostingStore>>,
    workers: HashMap<Binding, PostingWorker>,
}

struct PostingWorker {
    facts: FactBuffer<PostingFact>,
    writer: Box<dyn PostingWriter>,
    complete: bool,
}

fn selected_definitions(
    schema: &Schema,
    store: &dyn PostingStore,
) -> HashMap<Binding, (PostingDefinition, bool)> {
    let mut definitions = HashMap::new();
    let mut select = |field: &crate::schema::FieldDefinition, binding: Option<String>, complete| {
        let mut definition = PostingDefinition {
            field: field.clone(),
            binding: binding.clone(),
            inferred: false,
            length_complete: field.multi,
            empty_string_complete: field.kind == crate::model::LogicalType::String,
        };
        if !field.indexed && schema.is_auto_index_prohibited(&field.path) && binding.is_none() {
            definition.length_complete = false;
            definition.empty_string_complete = false;
        }
        if store.accepts(&definition) {
            definitions.insert((field.path.clone(), binding), (definition, complete));
        }
    };
    for field in schema.fields.values() {
        select(field, None, true);
    }
    for ((path, binding), registered) in &schema.operations {
        if registered.operation.posting() {
            select(
                &schema.fields[path],
                Some(binding.clone()),
                registered.operation.complete_coverage(),
            );
        }
    }
    definitions
}

fn has_binding<T>(bindings: &HashMap<Binding, T>, path: &[String], binding: Option<&str>) -> bool {
    !bindings.is_empty() && bindings.contains_key(&(path.to_vec(), binding.map(str::to_owned)))
}

impl PostingBatches {
    pub fn new(schema: &Schema, store: Option<Arc<dyn PostingStore>>) -> StorageResult<Self> {
        let Some(store) = store else {
            return Ok(Self::default());
        };
        let workers = selected_definitions(schema, store.as_ref())
            .into_iter()
            .map(|(key, (definition, complete))| {
                Ok((
                    key,
                    PostingWorker {
                        facts: FactBuffer::new(POSTING_BLOCK_LIMIT),
                        writer: store.start(&definition)?,
                        complete,
                    },
                ))
            })
            .collect::<StorageResult<_>>()?;
        Ok(Self {
            store: Some(store),
            workers,
        })
    }

    pub fn has(&self, path: &[String], binding: Option<&str>) -> bool {
        has_binding(&self.workers, path, binding)
    }

    pub fn add(&mut self, schema: &Schema, slot: RecordSlot, value: &Value) -> StorageResult<()> {
        for (key @ (path, binding), worker) in &mut self.workers {
            let selected = data_values_small(value, path);
            let selected = if binding.is_some() {
                let name = key.1.as_ref().expect("named binding");
                let operation = &schema.operations[&(path.clone(), name.clone())].operation;
                selected.and_then(|values| {
                    let mapped: Option<Vec<Value>> = values
                        .iter()
                        .map(|value| map_binding_value(operation.as_ref(), value))
                        .collect();
                    if mapped.is_none() {
                        worker.complete = false;
                    }
                    mapped
                })
            } else {
                selected.map(|values| values.iter().map(|value| (*value).clone()).collect())
            };
            let bytes = selected
                .iter()
                .flatten()
                .map(crate::ingestion::estimated_value_bytes)
                .fold(size_of::<PostingFact>(), usize::saturating_add);
            worker
                .facts
                .add(OP, PostingFact { slot, selected }, bytes, |batch| {
                    worker.writer.push_facts(0, batch)
                })?;
        }
        Ok(())
    }

    pub fn finish(&mut self) -> StorageResult<()> {
        for worker in self.workers.values_mut() {
            worker
                .facts
                .flush(|batch| worker.writer.push_facts(0, batch))?;
        }
        Ok(())
    }
}

pub(in crate::index::memory) struct PostingWriters {
    entries: HashMap<Binding, (Box<dyn PostingWriter>, PostingDefinition, bool)>,
    store: Option<Arc<dyn PostingStore>>,
}

impl PostingWriters {
    pub fn new(schema: &Schema, store: Option<Arc<dyn PostingStore>>) -> StorageResult<Self> {
        let entries = match &store {
            Some(store) => selected_definitions(schema, store.as_ref())
                .into_iter()
                .map(|(key, (definition, complete))| {
                    Ok((key, (store.start(&definition)?, definition, complete)))
                })
                .collect::<StorageResult<_>>()?,
            None => HashMap::new(),
        };
        Ok(Self { entries, store })
    }

    pub fn has(&self, path: &[String], binding: Option<&str>) -> bool {
        has_binding(&self.entries, path, binding)
    }

    pub fn push(&mut self, batch: PostingBatches, base: u32) -> StorageResult<()> {
        if batch.workers.len() != self.entries.len() {
            return Err(StorageError::invalid(
                OP,
                "posting component coverage mismatch",
            ));
        }
        for (key, worker) in batch.workers {
            let (writer, _, all_complete) = self
                .entries
                .get_mut(&key)
                .ok_or_else(|| StorageError::invalid(OP, "missing selected posting"))?;
            *all_complete &= worker.complete;
            writer.merge_worker(base, worker.writer)?;
        }
        Ok(())
    }

    pub fn finish(self, slots: usize, logical: u64) -> StorageResult<PlacedPostings> {
        let mut output = HashMap::new();
        if let Some(store) = self.store {
            for (key, (writer, definition, complete)) in self.entries {
                let reader = writer.seal(slots, None)?;
                let descriptor = reader.descriptor();
                if descriptor.path != definition.field.path
                    || descriptor.kind != &definition.field.kind
                    || descriptor.coverage.covered_records as u64 != logical
                {
                    return Err(StorageError::invalid(OP, "incompatible posting descriptor"));
                }
                output.insert(
                    key,
                    PlacedPosting {
                        definition,
                        reader,
                        store: Arc::clone(&store),
                        complete,
                    },
                );
            }
        }
        Ok(PlacedPostings(output))
    }
}

#[derive(Clone)]
pub(in crate::index::memory) struct PlacedPosting {
    definition: PostingDefinition,
    pub reader: Arc<dyn PostingReader>,
    store: Arc<dyn PostingStore>,
    pub complete: bool,
}

#[derive(Clone, Default)]
pub(in crate::index::memory) struct PlacedPostings(HashMap<Binding, PlacedPosting>);

impl fmt::Debug for PlacedPostings {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_set().entries(self.0.keys()).finish()
    }
}

impl PlacedPostings {
    pub fn resident_bytes(
        &self,
        counter: &mut crate::index::traits::component::ResidentComponentCounter,
    ) -> usize {
        self.0.values().fold(0usize, |total, placed| {
            total.saturating_add(counter.charge(&placed.reader, placed.reader.resident_bytes()))
        })
    }

    pub fn paths(&self) -> impl Iterator<Item = &[String]> {
        self.0.keys().map(|(path, _)| path.as_slice())
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn update_inferred(
        &mut self,
        path: &[String],
        slots: usize,
        target: Option<InferredCoverage>,
    ) -> StorageResult<()> {
        use crate::index::traits::posting::{PostingKey, ScalarKey};

        let identity = (path.to_vec(), None);
        let Some(placed) = self.0.get_mut(&identity) else {
            return Ok(());
        };
        let Some(target) = target else {
            self.0.remove(&identity);
            return Ok(());
        };
        let coverage = placed.reader.descriptor().coverage;
        if target.compatible_with(coverage) {
            return Ok(());
        }
        let mut writer = placed.store.start(&placed.definition)?;
        visit_posting_blocks(
            placed.reader.as_ref(),
            slots,
            &|| false,
            &mut |label, members| {
                let selected = match label {
                    PostingKey::Presence | PostingKey::Value(ScalarKey::Null) => true,
                    PostingKey::Length(_) => target.length,
                    PostingKey::Value(ScalarKey::String(text)) if text.is_empty() => {
                        target.general_values || target.short_values || target.empty_string
                    }
                    PostingKey::Value(_) => target.general_values || target.short_values,
                };
                if selected {
                    writer.push_members(label, members)?;
                }
                Ok(())
            },
        )?;
        placed.reader = writer.seal(
            slots,
            Some(crate::index::traits::posting::PostingCoverage {
                values_indexed: target.general_values || target.short_values || target.empty_string,
                value_complete: target.general_values,
                length_complete: target.length,
                empty_string_complete: target.empty_string,
                ..coverage
            }),
        )?;
        Ok(())
    }

    pub fn place_inferred<'a>(
        &mut self,
        store: &Arc<dyn PostingStore>,
        definition: PostingDefinition,
        coverage: crate::index::traits::posting::PostingCoverage,
        slots: usize,
        blocks: impl Iterator<
            Item = (
                crate::index::traits::posting::PostingKey<'a>,
                &'a roaring::RoaringBitmap,
            ),
        >,
    ) -> StorageResult<()> {
        if !definition.inferred {
            return Err(StorageError::invalid(
                OP,
                "expected inferred posting identity",
            ));
        }
        let mut writer = store.start(&definition)?;
        let mut scratch = Vec::new();
        for (key, members) in blocks {
            let maximum = POSTING_BLOCK_LIMIT.fixed_batch_len_after(
                OP,
                key.bytes(),
                size_of::<RecordSlot>(),
            )?;
            for member in members.iter() {
                if member as usize >= slots {
                    return Err(StorageError::invalid(OP, "inferred member outside segment"));
                }
                scratch.push(RecordSlot(member));
                if scratch.len() == maximum {
                    writer.push_members(key, &scratch)?;
                    scratch.clear();
                }
            }
            if !scratch.is_empty() {
                writer.push_members(key, &scratch)?;
                scratch.clear();
            }
        }
        let reader = writer.seal(slots, Some(coverage))?;
        let descriptor = reader.descriptor();
        if descriptor.path != definition.field.path
            || descriptor.kind != &definition.field.kind
            || descriptor.coverage.covered_records != coverage.covered_records
            || descriptor.coverage.value_complete != coverage.value_complete
        {
            return Err(StorageError::invalid(OP, "incompatible inferred posting"));
        }
        self.0.insert(
            (definition.field.path.clone(), None),
            PlacedPosting {
                definition,
                reader,
                store: Arc::clone(store),
                complete: true,
            },
        );
        Ok(())
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self.0.iter().all(|(key, placed)| {
                other.0.get(key).is_some_and(|other| {
                    Arc::ptr_eq(&placed.store, &other.store)
                        || (placed.reader.descriptor().kind == other.reader.descriptor().kind
                            && placed.definition.binding == other.definition.binding)
                })
            })
    }

    pub fn consolidate(&self, shards: &[(&Self, usize, u32)], slots: usize) -> StorageResult<Self> {
        let mut output = HashMap::new();
        let mut scratch = Vec::new();
        for (id, placed) in &self.0 {
            let mut writer = placed.store.start(&placed.definition)?;
            let mut complete = true;
            let mut coverage = placed.reader.descriptor().coverage;
            coverage.covered_records = 0;
            for &(shard, size, base) in shards {
                let source = shard.0.get(id).ok_or_else(|| {
                    StorageError::invalid(OP, "posting placement differs between shards")
                })?;
                let descriptor = source.reader.descriptor();
                let local = descriptor.coverage;
                if (
                    local.multi,
                    local.values_indexed,
                    local.value_complete,
                    local.length_complete,
                    local.empty_string_complete,
                ) != (
                    coverage.multi,
                    coverage.values_indexed,
                    coverage.value_complete,
                    coverage.length_complete,
                    coverage.empty_string_complete,
                ) {
                    return Err(StorageError::invalid(
                        OP,
                        "posting coverage differs between shards",
                    ));
                }
                complete &= source.complete;
                coverage.covered_records = coverage
                    .covered_records
                    .checked_add(local.covered_records)
                    .ok_or_else(|| StorageError::invalid(OP, "posting coverage overflow"))?;
                coverage.legacy_present_equivalent &= local.legacy_present_equivalent;
                coverage.legacy_null_equivalent &= local.legacy_null_equivalent;
                coverage.summary = coverage
                    .summary
                    .zip(local.summary)
                    .map(|(mut left, right)| {
                        left.merge_shifted(right);
                        left
                    });
                visit_posting_blocks(
                    source.reader.as_ref(),
                    size,
                    &|| false,
                    &mut |key, members| {
                        scratch.clear();
                        scratch.extend(members.iter().map(|slot| RecordSlot(base + slot.0)));
                        writer.push_members(key, &scratch)
                    },
                )?;
            }
            output.insert(
                id.clone(),
                PlacedPosting {
                    reader: writer.seal(slots, Some(coverage))?,
                    complete,
                    ..placed.clone()
                },
            );
        }
        Ok(Self(output))
    }

    pub fn get(&self, path: &[String], binding: Option<&str>) -> Option<&PlacedPosting> {
        if self.0.is_empty() {
            return None;
        }
        self.0.get(&(path.to_vec(), binding.map(str::to_owned)))
    }

    pub fn descriptors(
        &self,
    ) -> impl Iterator<Item = (ComponentKind, Cow<'_, [String]>, Option<Cow<'_, str>>)> {
        self.0.keys().map(|(path, binding)| {
            let kind = if binding.is_some() {
                ComponentKind::CustomPosting
            } else {
                ComponentKind::DeclaredPosting
            };
            (
                kind,
                Cow::Borrowed(path.as_slice()),
                binding.as_deref().map(Cow::Borrowed),
            )
        })
    }

    pub fn project(
        &self,
        mapping: &SlotMap<'_>,
        slots: usize,
        logical: u32,
    ) -> StorageResult<Self> {
        let mut output = HashMap::new();
        let mut scratch = Vec::new();
        for (identity, placed) in &self.0 {
            let mut writer = placed.store.start(&placed.definition)?;
            visit_posting_blocks(
                placed.reader.as_ref(),
                mapping.len(),
                &|| false,
                &mut |key, members| {
                    scratch.clear();
                    scratch.extend(
                        members
                            .iter()
                            .filter_map(|slot| mapping.get(slot.0 as usize).flatten()),
                    );
                    writer.push_members(key, &scratch)
                },
            )?;
            let coverage = crate::index::traits::posting::PostingCoverage {
                covered_records: logical,
                ..placed.reader.descriptor().coverage
            };
            output.insert(
                identity.clone(),
                PlacedPosting {
                    reader: writer.seal(slots, Some(coverage))?,
                    ..placed.clone()
                },
            );
        }
        Ok(Self(output))
    }
}

#[derive(Clone, Copy)]
pub(in crate::index::memory) struct InferredCoverage {
    pub general_values: bool,
    pub short_values: bool,
    pub length: bool,
    pub empty_string: bool,
}

impl InferredCoverage {
    pub fn compatible_with(self, current: crate::index::traits::posting::PostingCoverage) -> bool {
        current.value_complete == self.general_values
            && current.length_complete == self.length
            && current.empty_string_complete == self.empty_string
            && current.values_indexed
                == (self.general_values || self.short_values || self.empty_string)
    }
}
