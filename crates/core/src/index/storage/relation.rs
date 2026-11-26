//! Logical relation endpoints and occurrences.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::model::RecordSlot;
use crate::query_access::membership::SlotSet;
use crate::schema::relations::RelationDefinition;

use super::transfer::StorageResult;

pub(crate) struct RelationRow<'a> {
    pub targets: Vec<(Cow<'a, str>, Cow<'a, [u32]>)>,
    pub present: bool,
    pub null: bool,
    pub empty: bool,
    pub unavailable: bool,
}

/// Exact reverse membership for a set of target IDs in one local domain.
pub(crate) struct RelationMatches<'a> {
    pub slots: SlotSet<'a>,
    pub lists: u64,
    pub members: u64,
}

pub(crate) type VisitRelationTarget<'a> = dyn FnMut(&str, &[u32]) -> StorageResult<bool> + 'a;

pub(crate) trait RelationReader: std::any::Any + Send + Sync {
    fn declaration(&self) -> &RelationDefinition;
    fn source_slots(&self) -> usize;
    fn forward(&self, source: RecordSlot) -> StorageResult<RelationRow<'_>>;

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
