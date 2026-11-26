//! Binding metadata for dictionaries in an empty collection domain.

use std::collections::HashMap;
use std::ops::Bound;

use super::{
    PostingCoverage, PostingDescriptor, PostingKey, PostingReader, PostingValueCursor, ScalarKey,
    VisitPostingBlock,
};
use crate::index::storage::transfer::{BatchLimit, StorageResult};
use crate::model::{FieldPath, LogicalType};
use crate::query_access::membership::{MembershipView, SlotSet};
use crate::schema::{FieldDefinition, Schema};

pub(crate) struct DeclaredPostings {
    scalar: HashMap<Vec<String>, EmptyPosting>,
    named: HashMap<(Vec<String>, String), EmptyPosting>,
}

impl DeclaredPostings {
    pub(crate) fn new(schema: &Schema) -> Self {
        let scalar = schema
            .fields()
            .values()
            .filter(|field| field.indexed)
            .map(|field| (field.path.clone(), EmptyPosting::new(field)))
            .collect();
        let named = schema
            .operations
            .iter()
            .filter(|(_, binding)| {
                binding.operation.posting() && binding.operation.complete_coverage()
            })
            .map(|(key, _)| (key.clone(), EmptyPosting::new(&schema.fields[&key.0])))
            .collect();
        Self { scalar, named }
    }

    pub(crate) fn facet(&self, path: &FieldPath, binding: &str) -> Option<&dyn PostingReader> {
        let reader = if binding == "default" {
            self.scalar.get(&path.segments)
        } else {
            self.named.get(&(path.segments.clone(), binding.to_owned()))
        };
        reader.map(|reader| reader as &dyn PostingReader)
    }
}

struct EmptyPosting {
    path: Vec<String>,
    kind: LogicalType,
    multi: bool,
}

impl EmptyPosting {
    fn new(field: &FieldDefinition) -> Self {
        Self {
            path: field.path.clone(),
            kind: field.kind.clone(),
            multi: field.multi,
        }
    }
}

impl PostingReader for EmptyPosting {
    fn descriptor(&self) -> PostingDescriptor<'_> {
        PostingDescriptor {
            path: &self.path,
            kind: &self.kind,
            coverage: PostingCoverage {
                multi: self.multi,
                values_indexed: true,
                value_complete: true,
                length_complete: matches!(self.kind, LogicalType::Array(_)),
                empty_string_complete: self.kind == LogicalType::String && !self.multi,
                covered_records: 0,
                summary: None,
                legacy_present_equivalent: true,
                legacy_null_equivalent: true,
            },
        }
    }

    fn members(&self, key: PostingKey<'_>) -> StorageResult<Option<SlotSet<'_>>> {
        Ok(matches!(key, PostingKey::Presence).then_some(SlotSet::Empty))
    }

    fn values<'a>(
        &'a self,
        _: Bound<ScalarKey>,
        _: Bound<ScalarKey>,
        _: bool,
        _: Option<MembershipView<'a>>,
    ) -> StorageResult<Box<dyn PostingValueCursor + 'a>> {
        Ok(Box::new(EmptyPostingValues))
    }

    fn visit_blocks(
        &self,
        limit: BatchLimit,
        visit: &mut VisitPostingBlock<'_>,
    ) -> StorageResult<()> {
        limit.accepts("read empty posting", 0, 0, PostingKey::Presence.bytes())?;
        visit(PostingKey::Presence, &[])
    }
}

struct EmptyPostingValues;

impl PostingValueCursor for EmptyPostingValues {
    fn next(&mut self) -> StorageResult<Option<(ScalarKey, u64)>> {
        Ok(None)
    }
}
