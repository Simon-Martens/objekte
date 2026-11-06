//! Read-only typed posting dictionaries and distinct record counts.

use std::ops::Bound::{Excluded, Unbounded};

use roaring::RoaringBitmap;

use crate::model::{FieldPath, PathRoot};
use crate::posting::{PostingIndex, ScalarKey};
use crate::repository::{CollectionState, Repository};


/// One bounded page over an indexed field's stored value representation.
#[derive(Clone, Debug)]
pub struct PostingValuesRequest {
    pub collection: String,
    pub field: String,
    pub binding: Option<String>,
    pub limit: usize,
    pub descending: bool,
    pub continuation: Option<PostingValuesCursor>,
}


impl PostingValuesRequest {
    pub fn new(collection: impl Into<String>, field: impl Into<String>, limit: usize) -> Self {
        Self {
            collection: collection.into(),
            field: field.into(),
            binding: None,
            limit,
            descending: false,
            continuation: None,
        }
    }
}


/// One typed key and the number of distinct eligible records containing it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValueCount {
    pub value: ScalarKey,
    pub count: u64,
}


/// Domain and special-state totals for the selected posting representation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValuesCoverage {
    pub eligible_records: u64,
    pub unavailable_records: u64,
    pub selected_records: Option<u64>,
    pub present_records: u64,
    pub explicit_null_records: u64,
}


/// In-process exclusive key that belongs to one repository generation and binding.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValuesCursor {
    instance_id: u64,
    collection_index: usize,
    generation: u64,
    field: Vec<String>,
    binding: String,
    binding_version: u32,
    descending: bool,
    last_key: ScalarKey,
}


/// Values in posting-key order, with a cursor only when another key exists.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValuesPage {
    pub values: Vec<PostingValueCount>,
    pub continuation: Option<PostingValuesCursor>,
    pub coverage: PostingValuesCoverage,
    pub visited_keys: u64,
}


fn resolve<'a>(
    collection: &'a CollectionState,
    request: &PostingValuesRequest,
    path: &FieldPath,
) -> Result<(&'a PostingIndex, String, u32), String> {
    if path.root != PathRoot::Data {
        return Err("posting values require a logical data field".into());
    }
    let field = collection
        .definition
        .schema
        .fields
        .get(&path.segments)
        .ok_or("posting values require a registered field")?;
    let binding = request
        .binding
        .clone()
        .or_else(|| {
            collection
                .definition
                .schema
                .defaults
                .get(&path.segments)
                .cloned()
        })
        .unwrap_or_else(|| "default".into());
    if binding == "default" {
        if !field.indexed {
            return Err("field has no complete posting index".into());
        }
        let index = collection
            .postings
            .fields
            .get(&path.segments)
            .ok_or("posting index missing")?;
        return Ok((index, binding, 0));
    }
    let key = (path.segments.clone(), binding.clone());
    let registration = collection
        .definition
        .schema
        .operations
        .get(&key)
        .ok_or("binding has no registered posting operation")?;
    let custom = collection
        .custom_postings
        .get(&key)
        .ok_or("binding has no posting index")?;
    if !custom.complete {
        return Err("incomplete custom posting coverage".into());
    }
    Ok((&custom.index, binding, registration.version))
}


impl Repository {
    pub fn posting_values(
        &self,
        request: &PostingValuesRequest,
    ) -> Result<PostingValuesPage, String> {
        let (collection_index, collection) = self
            .collections
            .iter()
            .enumerate()
            .find(|(_, collection)| collection.definition.name == request.collection)
            .ok_or("unknown collection")?;
        let path = FieldPath::parse(&request.field)?;
        let (index, binding, binding_version) = resolve(collection, request, &path)?;
        if index.covered_records as usize != collection.generation.records.len() {
            return Err("incomplete posting slot coverage".into());
        }
        let unavailable =
            collection.generation.records.len() as u64 - collection.logical_records.len();
        let coverage = PostingValuesCoverage {
            eligible_records: collection.logical_records.len(),
            unavailable_records: unavailable,
            selected_records: None,
            present_records: index.presence.len(),
            explicit_null_records: index.nulls.len(),
        };
        if request.limit == 0 {
            return Ok(PostingValuesPage {
                values: Vec::new(),
                continuation: None,
                coverage,
                visited_keys: 0,
            });
        }
        let last = if let Some(cursor) = &request.continuation {
            if cursor.instance_id != self.instance_id
                || cursor.collection_index != collection_index
                || cursor.generation != collection.generation.generation
                || cursor.field != path.segments
                || cursor.binding != binding
                || cursor.binding_version != binding_version
                || cursor.descending != request.descending
            {
                return Err("posting value continuation scope mismatch".into());
            }
            Some(&cursor.last_key)
        } else {
            None
        };
        let mut iterator: Box<dyn Iterator<Item = (&ScalarKey, &RoaringBitmap)> + '_> =
            if request.descending {
                if let Some(last) = last {
                    Box::new(index.values.range((Unbounded, Excluded(last))).rev())
                } else {
                    Box::new(index.values.iter().rev())
                }
            } else if let Some(last) = last {
                Box::new(index.values.range((Excluded(last), Unbounded)))
            } else {
                Box::new(index.values.iter())
            };
        let mut values = Vec::new();
        let mut visited_keys = 0;
        while values.len() < request.limit {
            let Some((value, posting)) = iterator.next() else {
                break;
            };
            visited_keys += 1;
            values.push(PostingValueCount {
                value: value.clone(),
                count: posting.len(),
            });
        }
        let has_more = iterator.next().is_some();
        visited_keys += u64::from(has_more);
        let continuation = if has_more {
            values.last().map(|entry| PostingValuesCursor {
                instance_id: self.instance_id,
                collection_index,
                generation: collection.generation.generation,
                field: path.segments.clone(),
                binding,
                binding_version,
                descending: request.descending,
                last_key: entry.value.clone(),
            })
        } else {
            None
        };
        Ok(PostingValuesPage {
            values,
            continuation,
            coverage,
            visited_keys,
        })
    }
}


#[cfg(test)]
#[path = "posting_values.test.rs"]
mod tests;
