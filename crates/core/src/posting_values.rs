//! Read-only typed posting dictionaries and distinct record counts.

use std::ops::Bound::{self, Excluded, Included, Unbounded};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use roaring::RoaringBitmap;

use crate::model::{FieldPath, LogicalType, PathRoot};
use crate::posting::{PostingIndex, ScalarKey};
use crate::query::Query;
use crate::repository::{CollectionState, Repository};

static NEXT_SELECTION_ID: AtomicU64 = AtomicU64::new(1);
pub const MAX_POSTING_VALUES_PAGE: usize = 4096;


/// Inclusive or exclusive typed dictionary boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PostingValueBound {
    pub value: ScalarKey,
    pub inclusive: bool,
}


/// Complete verified membership for one collection in one repository instance.
#[derive(Clone, Debug)]
pub struct ExactSelection(Arc<SelectionData>);


#[derive(Debug)]
struct SelectionData {
    id: u64,
    instance_id: u64,
    collection_index: usize,
    generation: u64,
    members: RoaringBitmap,
}


impl ExactSelection {
    pub fn len(&self) -> u64 {
        self.0.members.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.members.is_empty()
    }
}


/// One bounded page over an indexed field's stored value representation.
#[derive(Clone, Debug)]
pub struct PostingValuesRequest {
    pub collection: String,
    pub field: String,
    pub binding: Option<String>,
    pub limit: usize,
    pub descending: bool,
    pub continuation: Option<PostingValuesCursor>,
    pub lower: Option<PostingValueBound>,
    pub upper: Option<PostingValueBound>,
    pub prefix: Option<String>,
    pub selection: Option<ExactSelection>,
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
            lower: None,
            upper: None,
            prefix: None,
            selection: None,
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
    lower: Option<PostingValueBound>,
    upper: Option<PostingValueBound>,
    prefix: Option<String>,
    selection_id: Option<u64>,
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

fn key_kind(kind: &LogicalType) -> &LogicalType {
    match kind {
        LogicalType::Array(inner) => key_kind(inner),
        other => other,
    }
}

fn compatible(kind: &LogicalType, key: &ScalarKey) -> bool {
    matches!(
        (key_kind(kind), key),
        (LogicalType::String, ScalarKey::String(_))
            | (LogicalType::Number, ScalarKey::Number(_))
            | (LogicalType::Bool, ScalarKey::Bool(_))
            | (LogicalType::Date, ScalarKey::Date(_))
            | (LogicalType::Datetime, ScalarKey::Datetime(_))
            | (LogicalType::Unixtime, ScalarKey::Unixtime(_))
    )
}

fn prefix_successor(prefix: &str) -> Option<String> {
    let mut chars: Vec<char> = prefix.chars().collect();
    for index in (0..chars.len()).rev() {
        let mut next = chars[index] as u32 + 1;
        if next > char::MAX as u32 {
            continue;
        }
        if (0xD800..=0xDFFF).contains(&next) {
            next = 0xE000;
        }
        chars[index] = char::from_u32(next)?;
        chars.truncate(index + 1);
        return Some(chars.into_iter().collect());
    }
    None
}

fn request_bound(bound: &Option<PostingValueBound>) -> Bound<ScalarKey> {
    match bound {
        Some(bound) if bound.inclusive => Included(bound.value.clone()),
        Some(bound) => Excluded(bound.value.clone()),
        None => Unbounded,
    }
}

fn bound_key(bound: &Bound<ScalarKey>) -> Option<(&ScalarKey, bool)> {
    match bound {
        Included(key) => Some((key, false)),
        Excluded(key) => Some((key, true)),
        Unbounded => None,
    }
}

fn tighten_lower(a: Bound<ScalarKey>, b: Bound<ScalarKey>) -> Bound<ScalarKey> {
    match (bound_key(&a), bound_key(&b)) {
        (None, _) => b,
        (_, None) => a,
        (Some((ak, ax)), Some((bk, bx))) => {
            if ak > bk || (ak == bk && (ax || !bx)) {
                a
            } else {
                b
            }
        }
    }
}

fn tighten_upper(a: Bound<ScalarKey>, b: Bound<ScalarKey>) -> Bound<ScalarKey> {
    match (bound_key(&a), bound_key(&b)) {
        (None, _) => b,
        (_, None) => a,
        (Some((ak, ax)), Some((bk, bx))) => {
            if ak < bk || (ak == bk && (ax || !bx)) {
                a
            } else {
                b
            }
        }
    }
}

fn empty_range(lower: &Bound<ScalarKey>, upper: &Bound<ScalarKey>) -> bool {
    match (bound_key(lower), bound_key(upper)) {
        (Some((lo, lo_exclusive)), Some((hi, hi_exclusive))) => {
            lo > hi || (lo == hi && (lo_exclusive || hi_exclusive))
        }
        _ => false,
    }
}


impl Repository {
    pub fn select_exact(&self, collection: &str, query: &Query) -> Result<ExactSelection, String> {
        let (collection_index, state) = self
            .collections
            .iter()
            .enumerate()
            .find(|(_, state)| state.definition.name == collection)
            .ok_or("unknown collection")?;
        if !query.collections.is_empty() && !query.collections.iter().any(|name| name == collection)
        {
            return Err("selection query excludes requested collection".into());
        }
        let members = crate::run::exact_members(self, query, collection_index)?;
        Ok(ExactSelection(Arc::new(SelectionData {
            id: NEXT_SELECTION_ID.fetch_add(1, Ordering::Relaxed),
            instance_id: self.instance_id,
            collection_index,
            generation: state.generation.generation,
            members,
        })))
    }

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
        if request.limit > MAX_POSTING_VALUES_PAGE {
            return Err("posting value page limit exceeds maximum".into());
        }
        let (index, binding, binding_version) = resolve(collection, request, &path)?;
        if index.covered_records as usize != collection.generation.records.len() {
            return Err("incomplete posting slot coverage".into());
        }
        let selection = request.selection.as_ref().map(|handle| &handle.0);
        if selection.is_some_and(|selection| {
            selection.instance_id != self.instance_id
                || selection.collection_index != collection_index
                || selection.generation != collection.generation.generation
        }) {
            return Err("exact selection scope mismatch".into());
        }
        for bound in [&request.lower, &request.upper].into_iter().flatten() {
            if !compatible(&index.kind, &bound.value) {
                return Err("posting value bound has incompatible type".into());
            }
        }
        let prepared_prefix = if let Some(prefix) = &request.prefix {
            if key_kind(&index.kind) != &LogicalType::String {
                return Err("posting prefix requires a string field".into());
            }
            if binding == "default" {
                Some(prefix.clone())
            } else {
                let key = (path.segments.clone(), binding.clone());
                Some(
                    collection.definition.schema.operations[&key]
                        .operation
                        .prepare_posting_prefix(prefix)?
                        .ok_or("binding does not preserve prefix semantics")?,
                )
            }
        } else {
            None
        };
        let selected_domain = selection.map(|selection| &selection.members);
        let selected_total = selected_domain.map(RoaringBitmap::len);
        let eligible = selected_domain.map_or(collection.logical_records.len(), |selected| {
            selected.intersection_len(&collection.logical_records)
        });
        let unavailable =
            selected_total.unwrap_or(collection.generation.records.len() as u64) - eligible;
        let coverage = PostingValuesCoverage {
            eligible_records: eligible,
            unavailable_records: unavailable,
            selected_records: selected_total,
            present_records: selected_domain.map_or(index.presence.len(), |selected| {
                selected.intersection_len(&index.presence)
            }),
            explicit_null_records: selected_domain.map_or(index.nulls.len(), |selected| {
                selected.intersection_len(&index.nulls)
            }),
        };
        let last = if let Some(cursor) = &request.continuation {
            if cursor.instance_id != self.instance_id
                || cursor.collection_index != collection_index
                || cursor.generation != collection.generation.generation
                || cursor.field != path.segments
                || cursor.binding != binding
                || cursor.binding_version != binding_version
                || cursor.descending != request.descending
                || cursor.lower != request.lower
                || cursor.upper != request.upper
                || cursor.prefix != request.prefix
                || cursor.selection_id != selection.map(|selected| selected.id)
            {
                return Err("posting value continuation scope mismatch".into());
            }
            Some(&cursor.last_key)
        } else {
            None
        };
        if request.limit == 0 {
            return Ok(PostingValuesPage {
                values: Vec::new(),
                continuation: None,
                coverage,
                visited_keys: 0,
            });
        }
        let mut lower = request_bound(&request.lower);
        let mut upper = request_bound(&request.upper);
        if let Some(prefix) = &prepared_prefix {
            if !prefix.is_empty() {
                lower = tighten_lower(lower, Included(ScalarKey::String(prefix.clone())));
                if let Some(end) = prefix_successor(prefix) {
                    upper = tighten_upper(upper, Excluded(ScalarKey::String(end)));
                }
            }
        }
        if let Some(last) = last {
            if request.descending {
                upper = tighten_upper(upper, Excluded(last.clone()));
            } else {
                lower = tighten_lower(lower, Excluded(last.clone()));
            }
        }
        if empty_range(&lower, &upper) {
            return Ok(PostingValuesPage {
                values: Vec::new(),
                continuation: None,
                coverage,
                visited_keys: 0,
            });
        }
        let mut iterator: Box<dyn Iterator<Item = (&ScalarKey, &RoaringBitmap)> + '_> =
            if request.descending {
                Box::new(index.values.range((lower, upper)).rev())
            } else {
                Box::new(index.values.range((lower, upper)))
            };
        let mut values = Vec::new();
        let mut visited_keys = 0;
        while values.len() < request.limit {
            let Some((value, posting)) = iterator.next() else {
                break;
            };
            visited_keys += 1;
            let count = selected_domain
                .map_or(posting.len(), |selected| posting.intersection_len(selected));
            if count > 0 {
                values.push(PostingValueCount {
                    value: value.clone(),
                    count,
                });
            }
        }
        let mut has_more = false;
        if values.len() == request.limit {
            for (_, posting) in iterator {
                visited_keys += 1;
                if selected_domain
                    .map_or(posting.len(), |selected| posting.intersection_len(selected))
                    > 0
                {
                    has_more = true;
                    break;
                }
            }
        }
        let continuation = if has_more {
            values.last().map(|entry| PostingValuesCursor {
                instance_id: self.instance_id,
                collection_index,
                generation: collection.generation.generation,
                field: path.segments.clone(),
                binding,
                binding_version,
                descending: request.descending,
                lower: request.lower.clone(),
                upper: request.upper.clone(),
                prefix: request.prefix.clone(),
                selection_id: selection.map(|selected| selected.id),
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
