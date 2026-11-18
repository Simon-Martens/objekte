//! Verified result-cache admission accounting.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::execution_options::ReturnMode;
use crate::inventory::{ActiveRecords, RecordClass, RecordEntry};
use crate::lifecycle::{IndexRevision, PublishedSegment};
use crate::lifecycle_catalog::FileCatalog;
use crate::materialization::payload_from_bytes;
use crate::measurement::WorkCounters;
use crate::model::RecordSlot;
use crate::query::Query;
use crate::query_execution::runner::{
    SegmentRecordRef, SourceHitLocator, get_segment_record_by_id, record_unavailable_hit,
    segment_locator_hit, segment_record_ref,
};
use crate::repository::{Repository, SearchResult};
use crate::source::read_record_range_for_search;
use crate::streaming::{ExecutionContext, ExecutionErrorKind};
use crate::verified::{CachedMember, CachedResult};

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
            charge_lifecycle_cached_payload(context, query, resolved.record)?;
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
                let path = repo.files.root.join(&file.relative_path);
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
            (record, file, payload)
        };
        let mut hit = segment_locator_hit(
            collection,
            SegmentRecordRef { record, file },
            revision,
            member.matched_fields.clone(),
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
                    SourceHitLocator::from(&hit),
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
