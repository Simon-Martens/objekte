//! Stable ranking keys, resolved stored sort columns, and retained winners.

use std::cmp::Ordering;

use crate::inventory::RecordEntry;
use crate::model::Value;
use crate::query::Query;
use crate::reference::RecordContext;
use crate::repository::{Repository, SearchHit};
use crate::sort_binding::{BoundSort, BoundSortSource, SortTargetCache};
use crate::sorting::{SortColumn, SortKey, SortOptions};

/// Comparable sort key with collection and ID tie breakers.
#[derive(Clone)]
pub(crate) struct RankKey {
    pub(crate) parts: Vec<(SortKey, bool, SortOptions)>,
    pub(crate) collection: String,
    pub(crate) id: String,
}

/// Sort paths are parsed and columns resolved once per query.
pub(crate) struct ResolvedSort<'a> {
    pub(crate) columns: Vec<Option<Vec<(&'a SortColumn, bool)>>>,
    pub(crate) bound: BoundSort<'a>,
    pub(crate) target_cache: std::sync::Mutex<SortTargetCache>,
}

impl<'a> ResolvedSort<'a> {
    pub(crate) fn new(repo: &'a Repository, query: &Query) -> Result<Self, String> {
        let bound = BoundSort::bind(repo, query)?;
        Ok(Self::from_bound(repo, query, bound))
    }

    pub(crate) fn from_bound(repo: &'a Repository, query: &Query, bound: BoundSort<'a>) -> Self {
        let columns = repo
            .collections
            .iter()
            .enumerate()
            .map(|(index, collection)| {
                if bound.has_computed {
                    return None;
                }
                if !query.collections.is_empty()
                    && !query.collections.contains(&collection.definition.name)
                {
                    return None;
                }
                Some(
                    bound.terms[index]
                        .as_ref()
                        .expect("selected collection")
                        .iter()
                        .map(|term| {
                            let BoundSortSource::Stored(column) = &term.source else {
                                unreachable!("all terms resident")
                            };
                            (*column, term.descending)
                        })
                        .collect(),
                )
            })
            .collect();
        Self {
            columns,
            bound,
            target_cache: std::sync::Mutex::new(SortTargetCache::default()),
        }
    }

    pub(crate) fn compare_to_rank(
        &self,
        collection_index: usize,
        record: &RecordEntry,
        collection_name: &str,
        worst: &RankKey,
    ) -> Ordering {
        if self.bound.has_computed {
            return Ordering::Less;
        }
        for ((column, descending), (other, _, _)) in self.columns[collection_index]
            .as_ref()
            .expect("selected collection")
            .iter()
            .zip(&worst.parts)
        {
            let key = &column.keys[record.slot.0 as usize];
            let order = key.compare_with(other, *descending, column.options);
            if !order.is_eq() {
                return order;
            }
        }
        (collection_name, record.id.0.as_ref()).cmp(&(worst.collection.as_str(), worst.id.as_str()))
    }
}

impl PartialEq for RankKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other).is_eq()
    }
}

impl Eq for RankKey {}

impl PartialOrd for RankKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for RankKey {
    fn cmp(&self, other: &Self) -> Ordering {
        crate::sort_binding::compare_parts(&self.parts, &other.parts)
            .then_with(|| (&self.collection, &self.id).cmp(&(&other.collection, &other.id)))
    }
}

/// Bounded page candidate and its materialized result.
#[derive(Clone)]
pub(crate) struct Winner {
    pub(crate) key: RankKey,
    pub(crate) collection_index: usize,
    pub(crate) slot: u32,
    pub(crate) hit: SearchHit,
    pub(crate) retained_bytes: u64,
}

impl PartialEq for Winner {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for Winner {}

impl PartialOrd for Winner {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Winner {
    fn cmp(&self, other: &Self) -> Ordering {
        self.key.cmp(&other.key)
    }
}

pub(crate) fn rank_key(
    sort: &ResolvedSort<'_>,
    repo: &Repository,
    collection_index: usize,
    record: &RecordEntry,
) -> RankKey {
    let collection = &repo.collections[collection_index];
    let parts = sort.columns[collection_index]
        .as_ref()
        .expect("selected collection")
        .iter()
        .map(|(column, descending)| {
            let key = column.keys[record.slot.0 as usize].clone();
            (key, *descending, column.options)
        })
        .collect();
    RankKey {
        parts,
        collection: collection.definition.name.clone(),
        id: record.id.0.to_string(),
    }
}

pub(crate) fn rank_key_from_context(
    sort: &ResolvedSort<'_>,
    repo: &Repository,
    collection_index: usize,
    ctx: &RecordContext<'_>,
    target_values: Option<&crate::relation_target_cache::TargetValueCache>,
) -> Result<RankKey, String> {
    Ok(RankKey {
        parts: sort.bound.keys(
            repo,
            collection_index,
            ctx,
            &sort.target_cache,
            target_values,
        )?,
        collection: repo.collections[collection_index].definition.name.clone(),
        id: ctx.record.id.0.to_string(),
    })
}

pub(crate) fn rank_key_without_source(
    sort: &ResolvedSort<'_>,
    repo: &Repository,
    collection_index: usize,
    record: &RecordEntry,
) -> Result<RankKey, String> {
    let empty = Value::Object(Default::default());
    let collection = &repo.collections[collection_index];
    let ctx = RecordContext {
        value: &empty,
        source_bytes: None,
        record,
        file: &repo.files.files[record.file.0 as usize],
        collection_name: &collection.definition.name,
        repository_root: &repo.files.root,
    };
    rank_key_from_context(sort, repo, collection_index, &ctx, None)
}

pub(crate) fn rank_key_bytes(key: &RankKey) -> u64 {
    let bytes = std::mem::size_of::<RankKey>()
        .saturating_add(key.parts.capacity().saturating_mul(std::mem::size_of::<(
            SortKey,
            bool,
            SortOptions,
        )>()))
        .saturating_add(key.collection.capacity())
        .saturating_add(key.id.capacity())
        .saturating_add(key.parts.iter().fold(0usize, |total, (part, _, _)| {
            total.saturating_add(crate::sort_binding::key_bytes(part))
        }));
    u64::try_from(bytes).unwrap_or(u64::MAX)
}
