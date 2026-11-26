//! Logical relation endpoints and occurrences.

use std::borrow::Cow;
use std::num::NonZeroUsize;

use serde::{Deserialize, Serialize};

use crate::model::RecordSlot;
use crate::query_access::membership::SlotSet;
use crate::schema::relations::RelationDefinition;

use super::transfer::{BatchLimit, StorageError, StorageResult};

pub(crate) struct RelationRow<'a> {
    pub targets: Vec<(Cow<'a, str>, Cow<'a, [u32]>)>,
    pub present: bool,
    pub null: bool,
    pub empty: bool,
    pub unavailable: bool,
}

#[derive(Clone, Copy, Default)]
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "consumed by the independent conformance writer")
)]
pub(crate) struct RelationFacts {
    pub present: bool,
    pub null: bool,
    pub empty: bool,
    pub unavailable: bool,
}

pub(crate) enum RelationBlock<'a> {
    Facts(RelationFacts),
    Target { id: &'a str, positions: &'a [u32] },
}

/// Exact reverse membership for a set of target IDs in one local domain.
pub(crate) struct RelationMatches<'a> {
    pub slots: SlotSet<'a>,
    pub lists: u64,
    pub members: u64,
}

pub(crate) struct DeclaredRelation<'a> {
    pub id: crate::schema::relations::RelationId,
    pub declaration: &'a RelationDefinition,
    pub target_collection: crate::model::CollectionId,
}

pub(crate) type VisitRelationTarget<'a> = dyn FnMut(&str, &[u32]) -> StorageResult<bool> + 'a;

pub(crate) trait RelationReader: std::any::Any + Send + Sync {
    /// Resident bytes owned by this sealed component. Encoded backing file
    /// lengths are excluded.
    fn resident_bytes(&self) -> super::component::ResidentComponentBytes {
        super::component::ResidentComponentBytes::default()
    }

    fn declaration(&self) -> &RelationDefinition;
    fn source_slots(&self) -> usize;
    fn edge_count(&self) -> usize {
        0
    }
    fn reverse_members(&self, _target: &str) -> StorageResult<Option<SlotSet<'_>>> {
        Ok(None)
    }

    fn forward(&self, source: RecordSlot) -> StorageResult<RelationRow<'_>>;

    fn source_facts(&self, source: RecordSlot) -> StorageResult<RelationFacts> {
        let row = self.forward(source)?;
        Ok(RelationFacts {
            present: row.present,
            null: row.null,
            empty: row.empty,
            unavailable: row.unavailable,
        })
    }

    fn source_unavailable(&self, source: RecordSlot) -> StorageResult<bool> {
        Ok(self.forward(source)?.unavailable)
    }

    /// Visit ordered occurrences. A true callback result stops this read.
    fn visit_targets(
        &self,
        source: RecordSlot,
        visit: &mut VisitRelationTarget<'_>,
    ) -> StorageResult<()> {
        for (id, positions) in self.forward(source)?.targets {
            if visit(&id, &positions)? {
                break;
            }
        }
        Ok(())
    }
}

/// Transfer one source's facts and stream its ordered occurrences. Each
/// occurrence is indivisible under the same oversized-item rule as other keys.
pub(crate) fn visit_relation_blocks(
    reader: &dyn RelationReader,
    source: RecordSlot,
    cancelled: &dyn Fn() -> bool,
    visit: &mut dyn FnMut(RelationBlock<'_>) -> StorageResult<()>,
) -> StorageResult<()> {
    const OP: &str = "transfer relation rows";
    const LIMIT: BatchLimit = BatchLimit::new(
        NonZeroUsize::new(1).unwrap(),
        NonZeroUsize::new(16 * 1024 * 1024).unwrap(),
    );
    if cancelled() {
        return Err(StorageError::Cancelled);
    }
    if source.0 as usize >= reader.source_slots() {
        return Err(StorageError::invalid(
            OP,
            "source is outside the relation domain",
        ));
    }
    visit(RelationBlock::Facts(reader.source_facts(source)?))?;
    reader.visit_targets(source, &mut |id, positions| {
        if cancelled() {
            return Err(StorageError::Cancelled);
        }
        let bytes = size_of::<RelationBlock<'_>>()
            .saturating_add(id.len())
            .saturating_add(size_of_val(positions));
        LIMIT.accepts(OP, 0, 0, bytes)?;
        visit(RelationBlock::Target { id, positions })?;
        Ok(false)
    })
}

/// Resolved target slot or an ID that has no target in its declared namespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RelationTarget {
    Resolved(RecordSlot),
    Unresolved(String),
}

/// One source occurrence of a declared ID, with array indexes from outer to inner.
/// The logical target ID remains available when the resolved slot is retired.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationEdge {
    pub target: RelationTarget,
    pub(crate) target_id: String,
    pub positions: Vec<u32>,
}

impl RelationEdge {
    pub fn target_id(&self) -> &str {
        &self.target_id
    }
}

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "consumed by the independent conformance writer")
)]
pub(crate) enum RelationFactKind {
    Source(RelationFacts),
    Target { id: String, positions: Vec<u32> },
}

#[cfg_attr(
    not(test),
    allow(dead_code, reason = "consumed by backend worker writers")
)]
pub(crate) struct RelationFact {
    pub slot: RecordSlot,
    pub kind: RelationFactKind,
}

pub(crate) trait RelationWriter: std::any::Any + Send {
    /// Adopt a prepared local writer from the same store at its assigned slot base.
    fn merge_worker(&mut self, base: u32, worker: Box<dyn RelationWriter>) -> StorageResult<()>;

    fn push(&mut self, base: u32, facts: &[RelationFact]) -> StorageResult<()>;
    fn seal(self: Box<Self>, slots: u32) -> StorageResult<std::sync::Arc<dyn RelationReader>>;
}

pub(crate) trait RelationStore: Send + Sync {
    fn accepts(&self, definition: &RelationDefinition) -> bool;
    fn start(&self, definition: &RelationDefinition) -> StorageResult<Box<dyn RelationWriter>>;
}
