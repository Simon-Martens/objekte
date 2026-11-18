//! Stable ranking keys, resolved stored sort columns, and retained winners.

use std::cmp::Ordering;
use std::sync::Arc;

use crate::build::IndexedCollectionSegment;
use crate::inventory::RecordEntry;
use crate::lifecycle::PublishedSegment;
use crate::lifecycle_catalog::FileCatalog;
use crate::model::Value;
use crate::query::Query;
use crate::record_preparation::prepare_query_record_with_execution;
use crate::reference::{BoundExpr, RecordContext};
use crate::repository::{Repository, SearchHit};
use crate::sort_binding::{BoundSort, BoundSortSource, MergedSortTargetCache, SortTargetCache};
use crate::sorting::{SortColumn, SortKey, SortOptions};
use crate::source::{ExtractedRecord, ScanProjection, read_record_range_for_search};
use crate::streaming::{ExecutionContext, ExecutionErrorKind};

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

pub(crate) fn segment_rank_key(
    repo: &Repository,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    segment: &IndexedCollectionSegment,
    record: &RecordEntry,
    ctx: Option<&RecordContext<'_>>,
) -> Result<RankKey, String> {
    let terms = sort.bound.terms[collection_index]
        .as_ref()
        .ok_or("segment sort collection missing")?;
    let mut parts = Vec::with_capacity(terms.len());
    for term in terms {
        let key = match &term.source {
            BoundSortSource::Stored(base_column) => {
                let column = segment.sorting.column(&base_column.path.display())?;
                column
                    .keys
                    .get(record.slot.0 as usize)
                    .ok_or("segment sort slot missing")?
                    .clone()
            }
            BoundSortSource::Computed(computed) => {
                computed.key_local(ctx.ok_or("computed segment sort needs record context")?)?
            }
        };
        parts.push((key, term.descending, term.options));
    }
    Ok(RankKey {
        parts,
        collection: repo.collections[collection_index].definition.name.clone(),
        id: record.id.0.to_string(),
    })
}

/// Compare a stored segment row with the current page boundary without
/// allocating rank parts, IDs, or a SearchHit for rows below the page.
pub(crate) fn segment_compare_to_rank(
    repo: &Repository,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    segment: &IndexedCollectionSegment,
    slot: crate::model::RecordSlot,
    worst: &RankKey,
) -> Result<Ordering, String> {
    let terms = sort.bound.terms[collection_index]
        .as_ref()
        .ok_or("segment sort collection missing")?;
    if terms.len() != worst.parts.len() {
        return Err("segment sort rank arity mismatch".into());
    }
    for (term, (other, _, _)) in terms.iter().zip(&worst.parts) {
        let BoundSortSource::Stored(base_column) = &term.source else {
            return Err("computed segment sort has no stored comparison".into());
        };
        let column = segment
            .sorting
            .columns
            .get(&base_column.path)
            .ok_or("segment sort column missing")?;
        let key = column
            .keys
            .get(slot.0 as usize)
            .ok_or("segment sort slot missing")?;
        let order = key.compare_with(other, term.descending, term.options);
        if !order.is_eq() {
            return Ok(order);
        }
    }
    Ok((
        repo.collections[collection_index].definition.name.as_str(),
        segment
            .record(slot)
            .map(|record| record.id.0.as_ref())
            .ok_or("segment sort slot missing")?,
    )
        .cmp(&(worst.collection.as_str(), worst.id.as_str())))
}

pub(crate) fn base_rank_key_merged(
    repo: &Repository,
    sort: &ResolvedSort<'_>,
    collection_index: usize,
    record: &RecordEntry,
    catalog: &FileCatalog,
    context: Option<&ExecutionContext>,
) -> Result<RankKey, String> {
    if !sort.bound.has_computed {
        return Ok(rank_key(sort, repo, collection_index, record));
    }
    let collection = &repo.collections[collection_index];
    let file = catalog.entry(record.file).ok_or("base sort file retired")?;
    let bytes = if sort.bound.needs_source {
        if let Some(context) = context {
            context.check_string()?;
            if record.range.len() > context.limits.source_record_bytes {
                return Err(context.fail(
                    ExecutionErrorKind::Resource,
                    "base sort record exceeds source read allowance",
                ));
            }
            context.charge_reads(1)?;
            context.charge_bytes(record.range.len())?;
        }
        let path = repo.files.root.join(&file.relative_path);
        Some(if let Some(context) = context {
            read_record_range_for_search(&path, file.revision, record.range, Some(context))?
        } else {
            read_record_range_for_search(&path, file.revision, record.range, None)?
        })
    } else {
        None
    };
    let extracted = if let Some(bytes) = bytes.as_ref() {
        prepare_query_record_with_execution(
            record.class,
            &collection.source,
            &collection.definition.schema,
            bytes,
            &ScanProjection::full(false),
            false,
            context,
        )?
    } else {
        ExtractedRecord {
            value: Value::Object(Default::default()),
            text: Vec::new(),
        }
    };
    let ctx = RecordContext {
        value: &extracted.value,
        source_bytes: bytes.as_deref(),
        record,
        file,
        collection_name: &collection.definition.name,
        repository_root: &repo.files.root,
    };
    rank_key_from_context(sort, repo, collection_index, &ctx, None)
}

pub(crate) fn relation_rank_key_merged(
    repo: &Repository,
    sort: &ResolvedSort<'_>,
    view: &crate::relation_query::MergedRelationView<'_>,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    address: crate::relation_query::MergedRecordSlot,
    ctx: &RecordContext<'_>,
    cache: &mut MergedSortTargetCache,
    filter: Option<&BoundExpr>,
) -> Result<RankKey, String> {
    let collection_index = address.collection.0 as usize;
    let terms = sort.bound.terms[collection_index]
        .as_ref()
        .ok_or("merged sort collection missing")?;
    let mut parts = Vec::with_capacity(terms.len());
    for term in terms {
        let key = match &term.source {
            BoundSortSource::Stored(column) => {
                if let Some(segment_index) = address.segment {
                    let segment = &segments[collection_index][segment_index].index;
                    segment
                        .sorting
                        .column(&column.path.display())?
                        .keys
                        .get(address.slot.0 as usize)
                        .ok_or("merged segment sort slot missing")?
                        .clone()
                } else {
                    column
                        .keys
                        .get(address.slot.0 as usize)
                        .ok_or("merged base sort slot missing")?
                        .clone()
                }
            }
            BoundSortSource::Computed(computed) => {
                if let Some(key) = cached_filter_target_sort_key(view, address, computed, filter)? {
                    cache.reuse += 1;
                    key
                } else if let Some(key) =
                    direct_related_target_sort_key(repo, view, segments, address, computed)?
                {
                    key
                } else {
                    computed.key_merged(view, address, ctx, cache)?
                }
            }
        };
        parts.push((key, term.descending, term.options));
    }
    Ok(RankKey {
        parts,
        collection: repo.collections[collection_index].definition.name.clone(),
        id: ctx.record.id.0.to_string(),
    })
}

fn cached_filter_target_sort_key(
    view: &crate::relation_query::MergedRelationView<'_>,
    address: crate::relation_query::MergedRecordSlot,
    computed: &crate::sort_binding::ComputedSort,
    filter: Option<&BoundExpr>,
) -> Result<Option<SortKey>, String> {
    let Some((hops, terminal)) = computed.related.as_ref() else {
        return Ok(None);
    };
    let [hop] = hops.as_slice() else {
        return Ok(None);
    };
    let Some(field) = terminal.field.segments.first() else {
        return Ok(None);
    };
    if !filter.is_some_and(|filter| filter_projects_related_field(filter, field)) {
        return Ok(None);
    }
    if view.relation_cardinality(hop.relation)? != crate::relation::RelationCardinality::One {
        return Ok(None);
    }
    let (edges, _present, _null, _empty, unavailable) =
        view.forward_edges(address, hop.relation.ordinal as usize)?;
    if unavailable {
        return Ok(None);
    }
    let [(target_id, _positions)] = edges.as_slice() else {
        return Ok(None);
    };
    let Some(target) = view.resolve_id(hop.to, target_id)? else {
        return Ok(None);
    };
    let Some(value) = view.cached_value(target) else {
        return Ok(None);
    };
    let Some(resolved) = view.record(target)? else {
        return Ok(None);
    };
    let ctx = RecordContext {
        value: &value,
        source_bytes: None,
        record: resolved.record,
        file: resolved.file,
        collection_name: &resolved.collection.definition.name,
        repository_root: view.repository_root(),
    };
    Ok(Some(computed.key_from_related_target_value(&ctx)?))
}

fn filter_projects_related_field(filter: &BoundExpr, field: &str) -> bool {
    match filter {
        BoundExpr::Term(term) => term.related.as_ref().is_some_and(|related| {
            related.source_operand.is_some()
                && related
                    .terminal
                    .path
                    .segments
                    .first()
                    .is_some_and(|path| path == field)
        }),
        BoundExpr::And(parts) | BoundExpr::Or(parts) => parts
            .iter()
            .any(|part| filter_projects_related_field(part, field)),
        BoundExpr::Not(inner) => filter_projects_related_field(inner, field),
    }
}

fn direct_related_target_sort_key(
    repo: &Repository,
    view: &crate::relation_query::MergedRelationView<'_>,
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    address: crate::relation_query::MergedRecordSlot,
    computed: &crate::sort_binding::ComputedSort,
) -> Result<Option<SortKey>, String> {
    let Some((hops, _)) = computed.related.as_ref() else {
        return Ok(None);
    };
    let [hop] = hops.as_slice() else {
        return Ok(None);
    };
    if view.relation_cardinality(hop.relation)? != crate::relation::RelationCardinality::One {
        return Ok(None);
    }
    let Some(path) = computed.direct_related_target_column(hop) else {
        return Ok(None);
    };
    let (edges, _present, _null, _empty, unavailable) =
        view.forward_edges(address, hop.relation.ordinal as usize)?;
    if unavailable {
        return Ok(None);
    }
    let [(target_id, _positions)] = edges.as_slice() else {
        return Ok(None);
    };
    let Some(target) = view.resolve_id(hop.to, target_id)? else {
        return Ok(None);
    };
    let target_index = target.collection.0 as usize;
    let column = if let Some(segment_index) = target.segment {
        let Ok(column) = segments[target_index][segment_index]
            .index
            .sorting
            .column(path)
        else {
            return Ok(None);
        };
        column
    } else {
        let Ok(column) = repo.collections[target_index].sorting.column(path) else {
            return Ok(None);
        };
        column
    };
    Ok(Some(
        column
            .keys
            .get(target.slot.0 as usize)
            .ok_or("direct related sort slot missing")?
            .clone(),
    ))
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
