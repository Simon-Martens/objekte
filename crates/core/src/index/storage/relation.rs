//! Logical relation endpoints and occurrences.

use std::borrow::Cow;

use serde::{Deserialize, Serialize};

use crate::model::RecordSlot;
use crate::schema::relations::RelationDefinition;

use super::transfer::StorageResult;

pub(crate) struct RelationRow<'a> {
    pub targets: Vec<(Cow<'a, str>, Cow<'a, [u32]>)>,
    pub present: bool,
    pub null: bool,
    pub empty: bool,
    pub unavailable: bool,
}

pub(crate) trait RelationReader: Send + Sync {
    fn declaration(&self) -> &RelationDefinition;
    fn source_slots(&self) -> usize;
    fn forward(&self, source: RecordSlot) -> StorageResult<RelationRow<'_>>;
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
