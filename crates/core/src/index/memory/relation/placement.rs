use std::collections::HashMap;
use std::fmt;
use std::num::NonZeroUsize;
use std::sync::Arc;

use super::{RelationContribution, visit_reference};
use crate::index::memory::segment::projection::SlotMap;
use crate::index::memory::worker::FactBuffer;
use crate::index::storage::relation::{
    RelationBlock, RelationFact, RelationFactKind, RelationFacts, RelationReader, RelationStore,
    RelationWriter, visit_relation_blocks,
};
use crate::index::storage::transfer::{BatchLimit, StorageError, StorageResult};
use crate::model::{RecordSlot, Value};
use crate::schema::relations::RelationDefinition;

const OP: &str = "write selected relation";
const LIMIT: BatchLimit = BatchLimit::new(
    NonZeroUsize::new(128).unwrap(),
    NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
);

pub(in crate::index::memory) enum RelationWorker {
    Native(RelationContribution),
    Placed {
        definition: RelationDefinition,
        count: u32,
        facts: FactBuffer<RelationFact>,
        writer: Box<dyn RelationWriter>,
    },
}

impl RelationWorker {
    pub fn new(
        definition: &RelationDefinition,
        store: Option<&dyn RelationStore>,
    ) -> StorageResult<Self> {
        Ok(
            if let Some(store) = store.filter(|store| store.accepts(definition)) {
                Self::Placed {
                    definition: definition.clone(),
                    count: 0,
                    facts: FactBuffer::new(LIMIT),
                    writer: store.start(definition)?,
                }
            } else {
                Self::Native(RelationContribution::empty(definition.clone()))
            },
        )
    }

    pub fn add_logical(&mut self, slot: RecordSlot, record: &Value) -> Result<(), String> {
        match self {
            Self::Native(native) => native.add_logical(slot, record),
            Self::Placed {
                definition,
                count,
                facts,
                writer,
            } => {
                advance(count, slot)?;
                let mut write = |kind| {
                    let fact = RelationFact { slot, kind };
                    let size = bytes(&fact);
                    facts
                        .add(OP, fact, size, |batch| writer.push(0, batch))
                        .map_err(|error| error.to_string())
                };
                let mut null = false;
                let mut empty = false;
                let present = visit_reference(
                    record,
                    &definition.path,
                    &mut Vec::new(),
                    &mut null,
                    &mut empty,
                    &mut |id, positions| {
                        write(RelationFactKind::Target {
                            id: id.to_owned(),
                            positions: positions.to_vec(),
                        })
                    },
                )?;
                write(RelationFactKind::Source(RelationFacts {
                    present,
                    null,
                    empty,
                    unavailable: false,
                }))
            }
        }
    }

    pub fn add_unavailable(&mut self, slot: RecordSlot) -> Result<(), String> {
        match self {
            Self::Native(native) => native.add_unavailable(slot),
            Self::Placed {
                count,
                facts,
                writer,
                ..
            } => {
                advance(count, slot)?;
                let fact = RelationFact {
                    slot,
                    kind: RelationFactKind::Source(RelationFacts {
                        unavailable: true,
                        ..Default::default()
                    }),
                };
                facts.add(OP, fact, size_of::<RelationFact>(), |batch| {
                    writer.push(0, batch)
                })?;
                Ok(())
            }
        }
    }

    pub fn finish(&mut self) -> StorageResult<()> {
        if let Self::Placed { facts, writer, .. } = self {
            facts.flush(|batch| writer.push(0, batch))?;
        }
        Ok(())
    }
}

fn advance(count: &mut u32, slot: RecordSlot) -> Result<(), String> {
    if slot.0 != *count {
        return Err("relation slots out of order".into());
    }
    *count = count.checked_add(1).ok_or("relation slot count overflow")?;
    Ok(())
}

pub(in crate::index::memory) enum RelationAggregate {
    Native(RelationContribution),
    Placed {
        definition: RelationDefinition,
        writer: Box<dyn RelationWriter>,
        store: Arc<dyn RelationStore>,
        count: u32,
    },
}

impl RelationAggregate {
    pub fn new(
        definition: &RelationDefinition,
        store: Option<&Arc<dyn RelationStore>>,
    ) -> StorageResult<Self> {
        if let Some(store) = store.filter(|store| store.accepts(definition)) {
            Ok(Self::Placed {
                definition: definition.clone(),
                writer: store.start(definition)?,
                store: Arc::clone(store),
                count: 0,
            })
        } else {
            Ok(Self::Native(RelationContribution::empty(
                definition.clone(),
            )))
        }
    }

    pub fn merge_shifted(
        &mut self,
        batch: RelationWorker,
        base: u32,
        slots: u32,
    ) -> Result<(), String> {
        match (self, batch) {
            (Self::Native(to), RelationWorker::Native(from)) => to.merge_shifted(from, base, slots),
            (
                Self::Placed { writer, count, .. },
                RelationWorker::Placed {
                    count: incoming,
                    writer: contribution,
                    ..
                },
            ) => {
                if base != *count || incoming != slots {
                    return Err("relation batch coverage mismatch".into());
                }
                writer.merge_worker(base, contribution)?;
                *count = count
                    .checked_add(incoming)
                    .ok_or("relation slot count overflow")?;
                Ok(())
            }
            _ => Err("relation batch placement mismatch".into()),
        }
    }

    pub fn seal(
        self,
        ordinal: usize,
        placed: &mut PlacedRelations,
    ) -> StorageResult<RelationContribution> {
        match self {
            Self::Native(native) => Ok(native),
            Self::Placed {
                definition,
                writer,
                store,
                count,
            } => {
                let reader = writer.seal(count)?;
                check_reader(reader.as_ref(), &definition, count)?;
                placed.0.insert(ordinal, PlacedRelation { reader, store });
                let mut empty = RelationContribution::empty(definition);
                empty.record_count = count;
                Ok(empty)
            }
        }
    }
}

fn bytes(fact: &RelationFact) -> usize {
    size_of::<RelationFact>().saturating_add(match &fact.kind {
        RelationFactKind::Source(_) => 0,
        RelationFactKind::Target { id, positions } => {
            id.len().saturating_add(size_of_val(positions.as_slice()))
        }
    })
}

fn check_reader(
    reader: &dyn RelationReader,
    definition: &RelationDefinition,
    count: u32,
) -> StorageResult<()> {
    if reader.declaration() != definition || reader.source_slots() != count as usize {
        return Err(StorageError::invalid(OP, "incompatible relation reader"));
    }
    Ok(())
}

#[derive(Clone)]
struct PlacedRelation {
    reader: Arc<dyn RelationReader>,
    store: Arc<dyn RelationStore>,
}

#[derive(Clone, Default)]
pub(in crate::index::memory) struct PlacedRelations(HashMap<usize, PlacedRelation>);

impl fmt::Debug for PlacedRelations {
    fn fmt(&self, out: &mut fmt::Formatter<'_>) -> fmt::Result {
        out.debug_set().entries(self.0.keys()).finish()
    }
}

impl PlacedRelations {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, ordinal: usize) -> Option<&dyn RelationReader> {
        self.0
            .get(&ordinal)
            .map(|relation| relation.reader.as_ref())
    }

    pub fn is_compatible_with(&self, other: &Self) -> bool {
        self.0.len() == other.0.len()
            && self.0.iter().all(|(&ordinal, relation)| {
                other
                    .get(ordinal)
                    .is_some_and(|other| relation.reader.declaration() == other.declaration())
            })
    }

    pub fn consolidate(&self, shards: &[(&Self, usize, u32)], count: u32) -> StorageResult<Self> {
        let mut output = HashMap::new();
        for (&ordinal, relation) in &self.0 {
            let declaration = relation.reader.declaration();
            let mut writer = relation.store.start(declaration)?;
            for &(shard, count, base) in shards {
                let reader = shard.get(ordinal).ok_or_else(|| {
                    StorageError::invalid(OP, "relation placement differs between shards")
                })?;
                let slots = (0..count as u32).map(|slot| (slot as usize, RecordSlot(slot)));
                transfer_rows(reader, slots, writer.as_mut(), base)?;
            }
            let reader = writer.seal(count)?;
            check_reader(reader.as_ref(), declaration, count)?;
            output.insert(
                ordinal,
                PlacedRelation {
                    reader,
                    store: Arc::clone(&relation.store),
                },
            );
        }
        Ok(Self(output))
    }

    pub fn project(&self, map: &SlotMap<'_>, count: u32) -> StorageResult<Self> {
        let mut output = HashMap::new();
        for (&ordinal, relation) in &self.0 {
            let declaration = relation.reader.declaration();
            let mut writer = relation.store.start(declaration)?;
            transfer_rows(relation.reader.as_ref(), map.selected(), writer.as_mut(), 0)?;
            let reader = writer.seal(count)?;
            check_reader(reader.as_ref(), declaration, count)?;
            output.insert(
                ordinal,
                PlacedRelation {
                    reader,
                    store: Arc::clone(&relation.store),
                },
            );
        }
        Ok(Self(output))
    }
}

fn transfer_rows(
    reader: &dyn RelationReader,
    slots: impl Iterator<Item = (usize, RecordSlot)>,
    writer: &mut dyn RelationWriter,
    base: u32,
) -> StorageResult<()> {
    let mut batch = Vec::new();
    let mut used = 0usize;
    for (source, destination) in slots {
        visit_relation_blocks(reader, RecordSlot(source as u32), &|| false, &mut |block| {
            let kind = match block {
                RelationBlock::Facts(facts) => RelationFactKind::Source(facts),
                RelationBlock::Target { id, positions } => RelationFactKind::Target {
                    id: id.to_owned(),
                    positions: positions.to_vec(),
                },
            };
            let fact = RelationFact {
                slot: destination,
                kind,
            };
            let size = bytes(&fact);
            if !LIMIT.accepts(OP, batch.len(), used, size)? {
                writer.push(base, &batch)?;
                used = 0;
                batch.clear();
            }
            used += size;
            batch.push(fact);
            Ok(())
        })?;
    }
    if !batch.is_empty() {
        writer.push(base, &batch)?;
    }
    Ok(())
}
