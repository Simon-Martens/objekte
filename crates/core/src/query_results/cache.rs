//! Verified result-cache admission accounting.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use super::{Payload, SearchHit, SearchResult};
use crate::index::segment::PublishedSegment;
use crate::inventory::{ActiveRecords, ByteRange, RecordClass, RecordEntry};
use crate::lifecycle::IndexRevision;
use crate::lifecycle::catalog::FileCatalog;
use crate::measurement::WorkCounters;
use crate::model::RecordSlot;
use crate::query::Query;
use crate::query::options::ReturnMode;
use crate::query_diagnostics::trace::PlanActual;
use crate::query_execution::runner::{
    SegmentRecordRef, SourceHitLocator, direct_payload, get_segment_record_by_id, make_hit,
    record_unavailable_hit, segment_locator_hit, segment_record_ref,
};
use crate::query_execution::streaming::{ExecutionContext, ExecutionErrorKind, StreamMode};
use crate::query_results::payload::payload_from_bytes;
use crate::query_results::ranking::ResolvedSort;
use crate::query_results::verified::{CachedMember, CachedResult};
use crate::repository::Repository;
use crate::source::access::SourceAccessSession;
use crate::source::access::read_record_range_for_search;

/// One cached member that needs a source read to compute its sort key.
pub(crate) struct CachedSortRead {
    pub(crate) collection_index: usize,
    pub(crate) file_id: u32,
    pub(crate) member_index: u32,
    pub(crate) range: crate::inventory::ByteRange,
}

pub(crate) fn cached_sort_fixed_bytes(cached: &CachedResult, reads: usize) -> Result<u64, String> {
    let mut bytes = cached
        .members
        .capacity()
        .checked_mul(std::mem::size_of::<CachedMember>())
        .and_then(|used| used.checked_add(std::mem::size_of::<CachedResult>()))
        .ok_or("cached sort memory count overflow")?;
    for member in &cached.members {
        bytes = bytes
            .checked_add(
                member
                    .matched_fields
                    .len()
                    .checked_mul(std::mem::size_of::<String>())
                    .ok_or("cached sort memory count overflow")?,
            )
            .ok_or("cached sort memory count overflow")?;
        for field in member.matched_fields.iter() {
            bytes = bytes
                .checked_add(field.capacity())
                .ok_or("cached sort memory count overflow")?;
        }
    }
    bytes = bytes
        .checked_add(
            reads
                .checked_mul(std::mem::size_of::<CachedSortRead>())
                .ok_or("cached sort memory count overflow")?,
        )
        .and_then(|used| {
            used.checked_add(128 * std::mem::size_of::<(u32, crate::inventory::ByteRange)>())
        })
        .ok_or("cached sort memory count overflow")?;
    u64::try_from(bytes).map_err(|_| "cached sort memory count overflow".into())
}

pub(crate) fn sort_cached_members(repo: &Repository, query: &Query, members: &mut [CachedMember]) {
    let sort = ResolvedSort::new(repo, query).expect("sort validated");
    if query.options.sort.is_empty() {
        members.sort_by(|a, b| {
            let a_collection = &repo.collections[a.collection_index];
            let b_collection = &repo.collections[b.collection_index];
            (
                &a_collection.definition.name,
                &a_collection.generation.records[a.slot as usize].id.0,
            )
                .cmp(&(
                    &b_collection.definition.name,
                    &b_collection.generation.records[b.slot as usize].id.0,
                ))
        });
    } else {
        members.sort_by(|a, b| {
            let a_collection = &repo.collections[a.collection_index];
            let b_collection = &repo.collections[b.collection_index];
            let a_record = &a_collection.generation.records[a.slot as usize];
            let b_record = &b_collection.generation.records[b.slot as usize];
            for ((a_column, descending), (b_column, _)) in sort.columns[a.collection_index]
                .as_ref()
                .unwrap()
                .iter()
                .zip(sort.columns[b.collection_index].as_ref().unwrap())
            {
                let order = a_column.keys[a.slot as usize].compare_with(
                    &b_column.keys[b.slot as usize],
                    *descending,
                    a_column.options,
                );
                if !order.is_eq() {
                    return order;
                }
            }
            (&a_collection.definition.name, &a_record.id)
                .cmp(&(&b_collection.definition.name, &b_record.id))
        });
    }
}

pub(crate) fn record_base_cache_hits(repo: &Repository, cached: &CachedResult) {
    let mut members_by_collection = HashMap::<usize, u64>::new();
    for member in &cached.members {
        *members_by_collection
            .entry(member.collection_index)
            .or_default() += 1;
    }
    for (collection_index, output_records) in members_by_collection {
        let Some(collection) = repo.collections.get(collection_index) else {
            continue;
        };
        crate::query_execution::request::record_cache_hit_observation(
            repo.instance_id,
            crate::query_execution::domain::domain_id(
                repo.instance_id,
                collection.generation.collection.0,
                collection.generation.generation,
                None,
            ),
            collection.generation.collection.0,
            cached.members.len() as u64,
            output_records,
        );
    }
}

/// Compact cache membership is budgeted independently of page payloads.
pub(crate) struct CacheBudget {
    pub(crate) initial: usize,
    remaining: AtomicUsize,
    complete: AtomicBool,
}

impl CacheBudget {
    pub(crate) fn new(bytes: usize) -> Self {
        Self {
            initial: bytes,
            remaining: AtomicUsize::new(bytes),
            complete: AtomicBool::new(true),
        }
    }

    pub(crate) fn complete(&self) -> bool {
        self.complete.load(Ordering::Relaxed)
    }

    pub(crate) fn used(&self) -> usize {
        self.initial
            .saturating_sub(self.remaining.load(Ordering::Relaxed))
    }

    pub(crate) fn reserve(&self, fields: &[String]) -> bool {
        if !self.complete() {
            return false;
        }
        let bytes = std::mem::size_of::<CachedMember>()
            + fields
                .iter()
                .map(|field| field.len() + std::mem::size_of::<String>())
                .sum::<usize>();
        if self
            .remaining
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(bytes)
            })
            .is_ok()
        {
            true
        } else {
            self.complete.store(false, Ordering::Relaxed);
            false
        }
    }
}

pub(crate) fn materialize_lifecycle_cached(
    repo: &Repository,
    query: &Query,
    cached: &CachedResult,
    base_active: &[ActiveRecords],
    segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    catalog: &FileCatalog,
    revision: IndexRevision,
    context: Option<&ExecutionContext>,
) -> Result<SearchResult, String> {
    let mut cached_by_domain = HashMap::<(usize, Option<usize>), u64>::new();
    for member in &cached.members {
        *cached_by_domain
            .entry((member.collection_index, member.segment_index))
            .or_default() += 1;
    }
    for ((collection_index, segment_index), output_records) in cached_by_domain {
        let collection = repo
            .collections
            .get(collection_index)
            .ok_or("cached lifecycle collection missing")?;
        let segment = segment_index.and_then(|index| {
            segments
                .get(collection_index)
                .and_then(|group| group.get(index))
                .map(|published| published.index.id.get())
        });
        crate::query_execution::request::record_cache_hit_observation(
            repo.instance_id,
            crate::query_execution::domain::domain_id(
                repo.instance_id,
                collection.generation.collection.0,
                collection.generation.generation,
                segment,
            ),
            collection.generation.collection.0,
            cached.members.len() as u64,
            output_records,
        );
    }
    let mut counters = WorkCounters {
        total_matches: cached.members.len() as u64,
        match_count_complete: true,
        verified_result_hit: true,
        ..WorkCounters::default()
    };
    let mut hits = Vec::new();
    let mut files = HashSet::new();
    for member in cached
        .members
        .iter()
        .skip(query.options.offset)
        .take(query.options.limit)
    {
        if let Some(context) = context {
            context.check_string()?;
        }
        let collection = repo
            .collections
            .get(member.collection_index)
            .ok_or("cached lifecycle collection missing")?;
        let (record, file, payload) = if let Some(segment_index) = member.segment_index {
            let segment = segments
                .get(member.collection_index)
                .and_then(|group| group.get(segment_index))
                .ok_or("cached lifecycle segment missing")?;
            let resolved = segment_record_ref(
                &segment.index,
                &segment.active,
                catalog,
                RecordSlot(member.slot),
            )?
            .ok_or("cached lifecycle segment slot retired")?;
            charge_lifecycle_cached_payload(context, query, &resolved.record)?;
            let payload = if query.options.return_mode == ReturnMode::Locator {
                Ok(None)
            } else {
                get_segment_record_by_id(
                    repo,
                    member.collection_index,
                    &segment.index,
                    &segment.active,
                    catalog,
                    revision,
                    &resolved.record.id.0,
                    query.options.return_mode.clone(),
                )
                .and_then(|hit| hit.ok_or_else(|| "cached lifecycle record missing".to_owned()))
                .map(|hit| hit.payload)
            };
            (resolved.record, resolved.file, payload)
        } else {
            let record = collection
                .generation
                .records
                .get(member.slot as usize)
                .ok_or("cached lifecycle base slot missing")?;
            if !base_active[member.collection_index].contains(RecordSlot(member.slot)) {
                return Err("cached lifecycle base slot retired".into());
            }
            let file = catalog
                .entry(record.file)
                .ok_or("cached lifecycle base file missing")?;
            charge_lifecycle_cached_payload(context, query, record)?;
            let payload = if query.options.return_mode == ReturnMode::Locator {
                Ok(None)
            } else {
                let path = repo.files.required_root().join(&file.relative_path);
                read_record_range_for_search(&path, file.revision, record.range, None)
                    .and_then(|bytes| {
                        payload_from_bytes(
                            record.class,
                            Some(collection),
                            &query.options.return_mode,
                            bytes,
                        )
                    })
                    .map(|(payload, _)| payload)
            };
            (record.clone(), file, payload)
        };
        let mut hit = segment_locator_hit(
            collection,
            SegmentRecordRef {
                record: record.clone(),
                file,
            },
            revision,
            member.matched_fields.to_vec(),
        );
        if query.options.return_mode != ReturnMode::Locator
            && !(query.options.return_mode == ReturnMode::Logical
                && record.class == RecordClass::RawFallback)
        {
            counters.bytes_read += record.range.len();
            counters.record_parses += u64::from(query.options.return_mode == ReturnMode::Logical);
            files.insert(file.id.0);
        }
        match payload {
            Ok(payload) => hit.payload = payload,
            Err(message) => {
                if record_unavailable_hit(
                    repo,
                    catalog,
                    SourceHitLocator::from_hit(&hit),
                    message.clone(),
                    &mut counters,
                )? {
                    continue;
                }
                return Err(message);
            }
        }
        hits.push(hit);
    }
    counters.distinct_files = files.len() as u64;
    counters.files_opened = files.len() as u64;
    counters.returned_records = hits.len() as u64;
    counters.match_count_complete = counters.source_unavailable_events == 0;
    Ok(SearchResult {
        count: (query.options.count && counters.match_count_complete)
            .then_some(cached.members.len()),
        hits,
        counters,
    })
}

pub(crate) fn materialize_cached_stream(
    repo: &Repository,
    query: &Query,
    cached: &CachedResult,
    delivery: &mut crate::query_execution::streaming::Delivery<'_>,
    execution: Option<&ExecutionContext>,
) -> Result<SearchResult, String> {
    record_base_cache_hits(repo, cached);
    delivery.check()?;
    #[cfg(feature = "detailed-profiling")]
    let started = crate::measurement::timer::DetailedInstant::now();
    let mut counters = WorkCounters {
        total_matches: cached.members.len() as u64,
        match_count_complete: true,
        verified_result_hit: true,
        ..WorkCounters::default()
    };
    #[cfg(feature = "detailed-profiling")]
    {
        counters.preparation_ns = started.elapsed().as_nanos() as u64;
    }
    let mut page = Vec::new();
    let mut page_bytes = 0usize;
    let mut source_access = SourceAccessSession::new(&repo.files);
    if let Some(execution) = execution {
        source_access.configure_execution(execution);
    }
    let mut actual = PlanActual::default();
    for member in cached
        .members
        .iter()
        .skip(query.options.offset)
        .take(query.options.limit)
    {
        delivery.check()?;
        let record =
            &repo.collections[member.collection_index].generation.records[member.slot as usize];
        delivery.check_record_bytes(record.range.len() as usize)?;
        delivery.check_buffer(page_bytes.saturating_add(record.range.len() as usize))?;
        let payload = direct_payload(
            repo,
            query,
            member.collection_index,
            record,
            &mut counters,
            &mut actual,
            &mut source_access,
            Some(delivery.max_buffered_bytes()),
            execution,
        )?;
        let hit = make_hit(
            repo,
            member.collection_index,
            record,
            member.matched_fields.to_vec(),
            payload,
        );
        page_bytes = page_bytes.saturating_add(crate::query_execution::streaming::hit_bytes(&hit));
        page.push(hit);
        delivery.check_buffer(page_bytes)?;
        match delivery.options().mode {
            StreamMode::OrderedRows => delivery.ordered_rows(&page, &counters, false)?,
            StreamMode::PageReplacements if page.len() % delivery.options().batch_rows == 0 => {
                delivery.replacement(&page, &counters, 0)?;
            }
            StreamMode::PageReplacements => {}
        }
    }
    counters.returned_records = page.len() as u64;
    counters.distinct_files = source_access.distinct_files();
    Ok(SearchResult {
        count: Some(cached.members.len()),
        hits: page,
        counters,
    })
}

pub(crate) fn materialize_cached(
    repo: &Repository,
    query: &Query,
    cached: &CachedResult,
) -> Result<SearchResult, String> {
    record_base_cache_hits(repo, cached);
    #[cfg(feature = "detailed-profiling")]
    let preparation_start = crate::measurement::timer::DetailedInstant::now();
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let mut counters = WorkCounters {
        total_matches: cached.members.len() as u64,
        match_count_complete: true,
        verified_result_hit: true,
        ..WorkCounters::default()
    };
    #[cfg(feature = "detailed-profiling")]
    {
        counters.preparation_ns = preparation_start.elapsed().as_nanos() as u64;
    }
    let mut hits = Vec::new();
    let mut files: BTreeMap<(usize, u32), Vec<(u32, ByteRange)>> = BTreeMap::new();
    for member in cached
        .members
        .iter()
        .skip(query.options.offset)
        .take(query.options.limit.min(cap))
    {
        let collection = &repo.collections[member.collection_index];
        let record = &collection.generation.records[member.slot as usize];
        let file = &repo.files.files[record.file.0 as usize];
        let index = hits.len() as u32;
        if query.options.return_mode == ReturnMode::Logical
            && record.class == RecordClass::RawFallback
        {
            hits.push(make_hit(
                repo,
                member.collection_index,
                record,
                member.matched_fields.to_vec(),
                Some(Payload::LogicalUnavailable {
                    diagnostic_ref: record.diagnostic_ref,
                }),
            ));
            continue;
        }
        if query.options.return_mode != ReturnMode::Locator {
            files
                .entry((member.collection_index, record.file.0))
                .or_default()
                .push((index, record.range));
        }
        hits.push(SearchHit {
            collection: collection.definition.name.clone(),
            record_id: record.id.0.to_string(),
            generation: collection.generation.generation,
            file: Some(crate::files::RecordFileInfo::from_entry(
                file,
                Some(record.range),
            )),
            matched_fields: member.matched_fields.to_vec(),
            record_class: record.class,
            diagnostic_ref: record.diagnostic_ref,
            payload: None,
        });
    }
    let mut source_access = SourceAccessSession::new(&repo.files);
    for ((collection_index, file_id), ranges) in files {
        let collection = &repo.collections[collection_index];
        for batch in ranges.chunks(128) {
            let read = source_access.read(crate::model::FileId(file_id), batch)?;
            counters.files_opened += read.opened;
            counters.bytes_read += read.bytes_read;
            for (index, bytes) in read.records {
                let (payload, parses) = payload_from_bytes(
                    hits[index as usize].record_class,
                    Some(collection),
                    &query.options.return_mode,
                    bytes,
                )?;
                counters.record_parses += parses;
                hits[index as usize].payload = payload;
            }
        }
    }
    counters.distinct_files = source_access.distinct_files();
    counters.returned_records = hits.len() as u64;
    Ok(SearchResult {
        count: query.options.count.then_some(cached.members.len()),
        hits,
        counters,
    })
}

fn charge_lifecycle_cached_payload(
    context: Option<&ExecutionContext>,
    query: &Query,
    record: &RecordEntry,
) -> Result<(), String> {
    if query.options.return_mode == ReturnMode::Locator
        || (query.options.return_mode == ReturnMode::Logical
            && record.class == RecordClass::RawFallback)
    {
        return Ok(());
    }
    if let Some(context) = context {
        if record.range.len() > context.limits.source_record_bytes {
            return Err(context.fail(
                ExecutionErrorKind::Resource,
                "cached lifecycle record exceeds source read allowance",
            ));
        }
        context.charge_reads(1)?;
        context.charge_bytes(record.range.len())?;
    }
    Ok(())
}
