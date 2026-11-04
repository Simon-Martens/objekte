//! Queries over application supplied typed values and the existing B-tree postings.

use super::{
    ordered_result_cache::{OrderedCoverage, OrderedResultKey},
    predicate_cache::PredicateCacheKey,
    *,
};
use crate::{
    Direction, SortReduce,
    extensions::{Extraction, TypedValue},
    file_picker::TypedKey,
};


#[derive(Clone, Debug, Serialize)]
pub enum DerivedPredicate {
    AnyValue,
    Eq(TypedValue),
    Ne(TypedValue),
    Lt(TypedValue),
    Le(TypedValue),
    Gt(TypedValue),
    Ge(TypedValue),
}


#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub enum DerivedBackend {
    Indexed,
    Direct,
}


#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
pub struct DerivedSort {
    pub direction: Direction,
    /// Multi-valued derived fields require an explicit minimum or maximum.
    pub reduce: SortReduce,
}


#[derive(Clone, Debug)]
pub struct DerivedQuery {
    pub collection: String,
    pub extractor: String,
    pub predicate: DerivedPredicate,
    pub sort: Option<DerivedSort>,
    pub offset: usize,
    pub limit: usize,
    pub count: bool,
    pub backend: DerivedBackend,
}


pub struct DerivedQueryResult {
    pub records: Vec<Record>,
    pub total: Option<usize>,
    pub truncated: bool,
}


impl DerivedPredicate {
    fn key(&self) -> Result<Option<TypedKey>> {
        match self {
            Self::AnyValue => Ok(None),
            Self::Eq(value)
            | Self::Ne(value)
            | Self::Lt(value)
            | Self::Le(value)
            | Self::Gt(value)
            | Self::Ge(value) => Ok(Some(TypedKey::from_extracted(value.clone())?)),
        }
    }

    fn matches(&self, values: &[TypedKey], key: Option<&TypedKey>) -> bool {
        values.iter().any(|value| self.matches_one(value, key))
    }

    fn matches_one(&self, value: &TypedKey, key: Option<&TypedKey>) -> bool {
        if matches!(value, TypedKey::Missing) {
            return false;
        }
        if matches!(self, Self::AnyValue) {
            return true;
        }
        let key = key.expect("comparison key");
        match self {
            Self::AnyValue => true,
            Self::Eq(_) => value == key,
            Self::Ne(_) => value != key,
            Self::Lt(_) => value.same_kind(key) && value < key,
            Self::Le(_) => value.same_kind(key) && value <= key,
            Self::Gt(_) => value.same_kind(key) && value > key,
            Self::Ge(_) => value.same_kind(key) && value >= key,
        }
    }
}


fn reduced_key(values: &[TypedKey], sort: Option<DerivedSort>) -> Result<Option<TypedKey>> {
    let Some(sort) = sort else { return Ok(None) };
    let value = match sort.reduce {
        SortReduce::Min => values.iter().min(),
        SortReduce::Max => values.iter().max(),
        SortReduce::First | SortReduce::Last => {
            return Err(Error::Request(
                "indexed derived sorting supports min or max reduction".into(),
            ));
        }
    };
    Ok(value.cloned())
}

fn compare_sort_keys(
    left: Option<&TypedKey>,
    right: Option<&TypedKey>,
    direction: Direction,
) -> std::cmp::Ordering {
    let order = match (left, right) {
        (Some(a), Some(b)) => a.cmp(b),
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
    };
    if direction == Direction::Desc {
        order.reverse()
    } else {
        order
    }
}


impl Repository {
    /// Query a registered derived field. The indexed route uses the engine's
    /// B-tree posting; the direct route invokes the same extractor on records.
    pub fn query_derived(&mut self, query: &DerivedQuery) -> Result<DerivedQueryResult> {
        let collection = self.collection(&query.collection)?;
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no value extractors are registered".into()))?;
        let binding = extensions.bind_value(
            &query.extractor,
            &query.collection,
            extensions
                .schema(&query.collection)
                .map(|schema| schema.as_ref()),
        )?;
        let comparison = query.predicate.key()?;
        reduced_key(&[], query.sort)?;
        self.ensure_picker(&collection)?;
        if matches!(self.options.freshness, Freshness::MetadataOnRequest) {
            let picker = self
                .file_pickers
                .get_mut(&query.collection)
                .expect("picker exists");
            if picker.refresh()? > 0 {
                self.query_cache.invalidate(&query.collection);
            }
        }
        if query.backend == DerivedBackend::Indexed
            && !self.file_pickers[&query.collection].has_active_derived(&binding.path)
        {
            self.file_pickers
                .get_mut(&query.collection)
                .expect("picker exists")
                .build_derived_posting(binding.clone())?;
        }

        let predicate_key = PredicateCacheKey {
            collection: query.collection.clone(),
            expression: serde_json::to_vec(&("derived/v1", &binding.path, &query.predicate))
                .expect("query identity serializes"),
        };
        let ordered_key = OrderedResultKey {
            predicate: predicate_key,
            sort: serde_json::to_vec(&query.sort).expect("sort identity serializes"),
        };
        let can_cache = query.backend == DerivedBackend::Indexed
            && binding.cacheable
            && self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest
            && self.file_pickers[&query.collection].verified();
        let cached = if can_cache {
            self.query_cache
                .ordered(&ordered_key)
                .and_then(|(slots, coverage)| {
                    (coverage == OrderedCoverage::Complete).then(|| slots.to_vec())
                })
        } else {
            None
        };
        let cache_hit = cached.is_some();
        let slots = if let Some(slots) = cached {
            slots
                .into_iter()
                .map(|slot| slot as usize)
                .collect::<Vec<_>>()
        } else if query.backend == DerivedBackend::Indexed {
            let picker = &self.file_pickers[&query.collection];
            let mut membership = vec![false; picker.slot_count()];
            let mut sort_keys = vec![None::<&TypedKey>; picker.slot_count()];
            picker
                .visit_derived(&binding.path, |slot, value| {
                    membership[slot] |= query.predicate.matches_one(value, comparison.as_ref());
                    if let Some(sort) = query.sort {
                        let current = &mut sort_keys[slot];
                        if current.is_none_or(|previous| {
                            if sort.reduce == SortReduce::Min {
                                value < previous
                            } else {
                                value > previous
                            }
                        }) {
                            *current = Some(value);
                        }
                    }
                })
                .ok_or_else(|| Error::Cache("derived posting is unavailable".into()))?;
            let mut matched = picker
                .sorted_slots()
                .iter()
                .copied()
                .filter(|&slot| membership[slot])
                .collect::<Vec<_>>();
            if let Some(sort) = query.sort {
                matched.sort_by(|&left, &right| {
                    let order =
                        compare_sort_keys(sort_keys[left], sort_keys[right], sort.direction);
                    order.then_with(|| picker.view_record(left).0.cmp(picker.view_record(right).0))
                });
            }
            matched
        } else {
            let ids = {
                let picker = &self.file_pickers[&query.collection];
                picker
                    .sorted_slots()
                    .iter()
                    .map(|&slot| (slot, picker.view_record(slot).0.to_owned()))
                    .collect::<Vec<_>>()
            };
            let mut extracted = Vec::new();
            let mut keys = Vec::new();
            let mut matched = Vec::new();
            for (slot, id) in ids {
                let record = self
                    .get_record(&query.collection, &id)?
                    .ok_or_else(|| Error::Unstable(collection.path.join(format!("{id}.json"))))?;
                extracted.clear();
                let state = binding.extractor.extract(&record.data, &mut extracted)?;
                if state == Extraction::Missing && !extracted.is_empty() {
                    return Err(Error::Request(format!(
                        "extractor {} emitted values for a missing field",
                        binding.identity,
                    )));
                }
                keys.clear();
                for value in extracted.drain(..) {
                    keys.push(TypedKey::from_extracted(value)?);
                }
                if query.predicate.matches(&keys, comparison.as_ref()) {
                    matched.push((slot, reduced_key(&keys, query.sort)?));
                }
            }
            if let Some(sort) = query.sort {
                let picker = &self.file_pickers[&query.collection];
                matched.sort_by(|(left, left_key), (right, right_key)| {
                    compare_sort_keys(left_key.as_ref(), right_key.as_ref(), sort.direction)
                        .then_with(|| {
                            picker
                                .view_record(*left)
                                .0
                                .cmp(picker.view_record(*right).0)
                        })
                });
            }
            matched.into_iter().map(|(slot, _)| slot).collect()
        };
        if can_cache && !cache_hit {
            self.query_cache
                .insert_ordered(ordered_key, slots.iter().map(|&slot| slot as u32).collect());
        }
        let total = query.count.then_some(slots.len());
        let truncated = slots.len() > self.options.total_limit;
        let end = slots.len().min(self.options.total_limit);
        let page = if query.offset >= end {
            &[][..]
        } else {
            &slots[query.offset..end.min(query.offset.saturating_add(query.limit))]
        };
        let selected = {
            let picker = &self.file_pickers[&query.collection];
            page.iter()
                .map(|&slot| {
                    let (id, stamp) = picker.view_record(slot);
                    (id.to_owned(), stamp.clone())
                })
                .collect::<Vec<_>>()
        };
        let mut records = Vec::with_capacity(selected.len());
        for (id, expected) in selected {
            let record = self
                .get_record(&query.collection, &id)?
                .ok_or_else(|| Error::Unstable(collection.path.join(format!("{id}.json"))))?;
            if record.size_bytes != expected.size || record.modified_unix_nanos != expected.modified
            {
                return Err(Error::Unstable(collection.path.join(format!("{id}.json"))));
            }
            let mut extracted = Vec::new();
            let state = binding.extractor.extract(&record.data, &mut extracted)?;
            if state == Extraction::Missing && !extracted.is_empty() {
                return Err(Error::Request(format!(
                    "extractor {} emitted values for a missing field",
                    binding.identity,
                )));
            }
            let values = extracted
                .into_iter()
                .map(TypedKey::from_extracted)
                .collect::<Result<Vec<_>>>()?;
            if !query.predicate.matches(&values, comparison.as_ref()) {
                return Err(Error::Unstable(collection.path.join(format!("{id}.json"))));
            }
            records.push(record);
        }
        Ok(DerivedQueryResult {
            records,
            total,
            truncated,
        })
    }
}
