//! Bounded result retention, grouped source access, and payload materialization.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashSet};
use std::time::Instant;

use roaring::RoaringBitmap;
use serde::Serialize;

use crate::executor::{
    Plan, TermChoice, estimate_expr, estimate_fts_candidates, fts_candidates, plan_expr_restricted,
};
use crate::inventory::{ByteRange, RecordEntry};
use crate::query::{Operator, Query};
use crate::reference::{Derived, RecordContext, eval_filter, eval_fts_query};
use crate::repository::{Payload, Repository, SearchHit, SearchResult};
use crate::sorting::SortKey;
use crate::source::{read_record_ranges, reader};
use crate::verified::{CachedMember, CachedResult};
use crate::workload::{ReturnMode, WorkCounters};

#[derive(Clone)]
struct RankKey {
    parts: Vec<(SortKey, bool)>,
    collection: String,
    id: String,
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
        for ((a, descending), (b, _)) in self.parts.iter().zip(&other.parts) {
            let order = a.compare(b, *descending);
            if !order.is_eq() {
                return order;
            }
        }
        (&self.collection, &self.id).cmp(&(&other.collection, &other.id))
    }
}

struct Winner {
    key: RankKey,
    collection_index: usize,
    slot: u32,
    hit: SearchHit,
}

#[derive(Clone, Debug, Serialize)]
pub struct QueryPlanTrace {
    pub collection: String,
    pub generation: u64,
    pub input_records: u64,
    pub fts_strategy: String,
    pub filter_exact: bool,
    pub terms: Vec<TermChoice>,
    pub candidates: u64,
    pub actual: Option<PlanActual>,
}

#[derive(Clone, Debug, Serialize)]
pub struct PlanActual {
    pub files_opened: u64,
    pub bytes_read: u64,
    pub record_parses: u64,
    pub residual_evaluations: u64,
    pub index_proven_matches: u64,
}

fn finish_trace(traces: &mut [QueryPlanTrace], before: &WorkCounters, after: &WorkCounters) {
    if let Some(trace) = traces.last_mut() {
        trace.actual = Some(PlanActual {
            files_opened: after.files_opened - before.files_opened,
            bytes_read: after.bytes_read - before.bytes_read,
            record_parses: after.record_parses - before.record_parses,
            residual_evaluations: after.residual_evaluations - before.residual_evaluations,
            index_proven_matches: after.exact_posting_matches - before.exact_posting_matches,
        });
    }
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

fn rank_key(
    repo: &Repository,
    query: &Query,
    collection_index: usize,
    record: &RecordEntry,
) -> RankKey {
    let collection = &repo.collections[collection_index];
    let parts = query
        .options
        .sort
        .iter()
        .map(|field| {
            let key = collection
                .sorting
                .key(&field.field, record.slot)
                .expect("sort validated")
                .clone();
            (key, field.descending)
        })
        .collect();
    RankKey {
        parts,
        collection: collection.definition.name.clone(),
        id: record.id.0.clone(),
    }
}

fn offer(
    repo: &Repository,
    query: &Query,
    collection_index: usize,
    record: &RecordEntry,
    evidence: HashSet<String>,
    payload: Option<Payload>,
    winners: &mut BinaryHeap<Winner>,
    cap: usize,
    counters: &mut WorkCounters,
    mut cache_members: Option<&mut Vec<CachedMember>>,
) {
    counters.total_matches += 1;
    let mut matched_fields: Vec<_> = if query.options.evidence {
        evidence.into_iter().collect()
    } else {
        Vec::new()
    };
    matched_fields.sort();
    if let Some(members) = cache_members.as_deref_mut() {
        members.push(CachedMember {
            collection_index,
            slot: record.slot.0,
            matched_fields: matched_fields.clone(),
        });
    }
    if cap == 0 {
        return;
    }
    let key = rank_key(repo, query, collection_index, record);
    if winners.len() == cap && winners.peek().is_some_and(|worst| worst.key <= key) {
        return;
    }
    if winners.len() == cap {
        winners.pop();
    }
    let file = &repo.files.files[record.file.0 as usize];
    let hit = SearchHit {
        collection: repo.collections[collection_index].definition.name.clone(),
        record_id: record.id.0.clone(),
        file_path: file.relative_path.clone(),
        byte_start: record.range.start,
        byte_end: record.range.end,
        modified_ns: record.revision.modified_ns,
        matched_fields,
        payload,
    };
    winners.push(Winner {
        key,
        collection_index,
        slot: record.slot.0,
        hit,
    });
}

pub fn search(repo: &Repository, query: &Query) -> Result<SearchResult, String> {
    let enabled = repo.verified.lock().map_err(|e| e.to_string())?.enabled();
    if !enabled {
        return search_impl(repo, query, None, None);
    }
    let key = crate::verified::key(query);
    let cached = repo.verified.lock().map_err(|e| e.to_string())?.get(&key);
    if let Some(cached) = cached {
        return materialize_cached(repo, query, &cached);
    }
    let mut members = Vec::new();
    let result = search_impl(repo, query, None, Some(&mut members))?;
    debug_assert_eq!(members.len(), result.count);
    sort_cached_members(repo, query, &mut members);
    repo.verified
        .lock()
        .map_err(|e| e.to_string())?
        .insert(key, members);
    Ok(result)
}

fn sort_cached_members(repo: &Repository, query: &Query, members: &mut Vec<CachedMember>) {
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
        let mut keyed: Vec<_> = members
            .drain(..)
            .map(|member| {
                let record = &repo.collections[member.collection_index].generation.records
                    [member.slot as usize];
                (
                    rank_key(repo, query, member.collection_index, record),
                    member,
                )
            })
            .collect();
        keyed.sort_by(|a, b| a.0.cmp(&b.0));
        members.extend(keyed.into_iter().map(|(_, member)| member));
    }
}

fn materialize_cached(
    repo: &Repository,
    query: &Query,
    cached: &CachedResult,
) -> Result<SearchResult, String> {
    let preparation_start = Instant::now();
    let (_, plan_hit) = crate::prepared::prepare(repo, query)?;
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let mut counters = WorkCounters {
        total_matches: cached.members.len() as u64,
        verified_result_hit: true,
        prepared_plan_hit: plan_hit,
        ..WorkCounters::default()
    };
    counters.preparation_ns = preparation_start.elapsed().as_nanos() as u64;
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
        if query.options.return_mode != ReturnMode::Locator {
            files
                .entry((member.collection_index, record.file.0))
                .or_default()
                .push((index, record.range));
        }
        hits.push(SearchHit {
            collection: collection.definition.name.clone(),
            record_id: record.id.0.clone(),
            file_path: file.relative_path.clone(),
            byte_start: record.range.start,
            byte_end: record.range.end,
            modified_ns: record.revision.modified_ns,
            matched_fields: member.matched_fields.clone(),
            payload: None,
        });
    }
    for ((collection_index, file_id), ranges) in files {
        let collection = &repo.collections[collection_index];
        let path = repo
            .files
            .native_path(crate::model::FileId(file_id))
            .ok_or("missing source path")?;
        let revision = repo.files.files[file_id as usize].revision;
        for batch in ranges.chunks(128) {
            let (records, bytes_read) = read_record_ranges(&path, revision, batch)?;
            counters.files_opened += 1;
            counters.bytes_read += bytes_read;
            for (index, bytes) in records {
                hits[index as usize].payload = Some(match query.options.return_mode {
                    ReturnMode::Locator => unreachable!(),
                    ReturnMode::Bytes => Payload::Bytes(bytes),
                    ReturnMode::Logical => {
                        counters.record_parses += 1;
                        Payload::Logical(reader(collection.definition.format).parse(&bytes)?.value)
                    }
                });
            }
        }
    }
    counters.returned_records = hits.len() as u64;
    Ok(SearchResult {
        count: cached.members.len(),
        hits,
        counters,
    })
}

pub fn search_analyze(
    repo: &Repository,
    query: &Query,
) -> Result<(SearchResult, Vec<QueryPlanTrace>), String> {
    let mut traces = Vec::new();
    let result = search_impl(repo, query, Some(&mut traces), None)?;
    Ok((result, traces))
}

pub fn explain(repo: &Repository, query: &Query) -> Result<Vec<QueryPlanTrace>, String> {
    let (prepared, _) = crate::prepared::prepare(repo, query)?;
    let mut traces = Vec::new();
    for (collection_index, filter) in prepared.collections {
        let collection = &repo.collections[collection_index];
        let domain: RoaringBitmap = (0..collection.generation.records.len() as u32).collect();
        let plan = filter
            .as_ref()
            .map(|f| plan_expr_restricted(repo, collection, f, &domain, &domain))
            .transpose()?
            .unwrap_or(Plan {
                candidates: None,
                exact: true,
                proofs: Vec::new(),
                choices: Vec::new(),
            });
        traces.push(QueryPlanTrace {
            collection: collection.definition.name.clone(),
            generation: collection.generation.generation,
            input_records: domain.len(),
            fts_strategy: if let Some(fts) = &query.fts {
                if estimate_fts_candidates(collection, fts).is_some() {
                    "compatible bigram candidate or restricted scanner"
                } else {
                    "restricted scanner"
                }
            } else {
                "absent"
            }
            .into(),
            filter_exact: plan.exact,
            terms: plan.choices,
            candidates: plan
                .candidates
                .as_ref()
                .map(RoaringBitmap::len)
                .unwrap_or(domain.len()),
            actual: None,
        });
    }
    Ok(traces)
}

fn search_impl(
    repo: &Repository,
    query: &Query,
    mut traces: Option<&mut Vec<QueryPlanTrace>>,
    mut cache_members: Option<&mut Vec<CachedMember>>,
) -> Result<SearchResult, String> {
    let preparation_start = Instant::now();
    let (prepared, plan_hit) = crate::prepared::prepare(repo, query)?;
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let mut winners = BinaryHeap::new();
    let mut counters = WorkCounters::default();
    counters.preparation_ns = preparation_start.elapsed().as_nanos() as u64;
    counters.prepared_plan_hit = plan_hit;
    for (collection_index, filter) in prepared.collections {
        let planning_start = Instant::now();
        let before = traces.as_ref().map(|_| counters.clone());
        let collection = &repo.collections[collection_index];
        let domain: RoaringBitmap = (0..collection.generation.records.len() as u32).collect();
        let mut input = domain.clone();
        let text_estimate = query
            .fts
            .as_ref()
            .and_then(|fts| estimate_fts_candidates(collection, fts));
        let filter_estimate = filter
            .as_ref()
            .map(|f| estimate_expr(collection, f, domain.len()));
        let source_unit = 30.0 + collection.stats.mean_record_bytes() / 128.0;
        let text_first = match (text_estimate, filter_estimate) {
            (Some((text_count, text_lookup)), Some((filter_work, filter_count))) => {
                text_lookup + text_count as f64 * source_unit * 0.1
                    < filter_work + filter_count as f64 * source_unit * 0.1
            }
            (Some(_), None) => true,
            _ => false,
        };
        if text_first {
            if let Some(fts) = &query.fts {
                if let Some(text) = fts_candidates(collection, fts) {
                    input &= text;
                }
            }
        }
        let plan = filter
            .as_ref()
            .map(|f| plan_expr_restricted(repo, collection, f, &domain, &input))
            .transpose()?
            .unwrap_or(Plan {
                candidates: None,
                exact: true,
                proofs: Vec::new(),
                choices: Vec::new(),
            });
        let mut candidates = plan.candidates.unwrap_or(input);
        if !text_first {
            if let (Some(fts), Some((estimated, lookup))) = (&query.fts, text_estimate) {
                if lookup + estimated as f64 * 0.05 < candidates.len() as f64 * source_unit {
                    if let Some(text_candidates) = fts_candidates(collection, fts) {
                        candidates &= text_candidates;
                    }
                }
            }
        }
        counters.planning_ns += planning_start.elapsed().as_nanos() as u64;
        counters.candidates += candidates.len();
        let needs_scan = !plan.exact || query.fts.is_some();
        if let Some(traces) = traces.as_deref_mut() {
            traces.push(QueryPlanTrace {
                collection: collection.definition.name.clone(),
                generation: collection.generation.generation,
                input_records: domain.len(),
                fts_strategy: if query.fts.is_none() {
                    "absent"
                } else if text_first {
                    "bigram before filter"
                } else if candidates.len() < domain.len() {
                    "filter before text verification"
                } else {
                    "text verification scan"
                }
                .into(),
                filter_exact: plan.exact,
                terms: plan.choices.clone(),
                candidates: candidates.len(),
                actual: None,
            });
        }
        if !needs_scan {
            for slot in candidates.iter() {
                let record = &collection.generation.records[slot as usize];
                let mut evidence = HashSet::new();
                if query.options.evidence {
                    for proof in &plan.proofs {
                        if proof.matches.contains(slot)
                            && proof.term.derived == Derived::None
                            && !matches!(proof.term.operator, Operator::Ne | Operator::NotContains)
                        {
                            evidence.insert(proof.term.path.display());
                        }
                    }
                }
                counters.exact_posting_matches += 1;
                offer(
                    repo,
                    query,
                    collection_index,
                    record,
                    evidence,
                    None,
                    &mut winners,
                    cap,
                    &mut counters,
                    cache_members.as_deref_mut(),
                );
            }
            if let (Some(traces), Some(before)) = (traces.as_deref_mut(), before.as_ref()) {
                finish_trace(traces, before, &counters);
            }
            continue;
        }
        // Record slots are grouped by source file before opening any source.
        let mut files: BTreeMap<u32, Vec<(u32, ByteRange)>> = BTreeMap::new();
        for slot in candidates.iter() {
            let record = &collection.generation.records[slot as usize];
            files
                .entry(record.file.0)
                .or_default()
                .push((slot, record.range));
        }
        for (file_id, ranges) in files {
            let path = repo
                .files
                .native_path(crate::model::FileId(file_id))
                .ok_or("missing source path")?;
            let revision = repo.files.files[file_id as usize].revision;
            for batch in ranges.chunks(128) {
                let (records, bytes_read) = read_record_ranges(&path, revision, batch)?;
                counters.files_opened += 1;
                counters.bytes_read += bytes_read;
                for (slot, bytes) in records {
                    let record = &collection.generation.records[slot as usize];
                    let file = &repo.files.files[file_id as usize];
                    let extracted = reader(collection.definition.format).parse(&bytes)?;
                    counters.record_parses += 1;
                    counters.scanned_records += 1;
                    let ctx = RecordContext {
                        value: &extracted.value,
                        record,
                        file,
                        collection_name: &collection.definition.name,
                        repository_root: &repo.files.root,
                    };
                    let mut evidence = HashSet::new();
                    let fts_match = query
                        .fts
                        .as_ref()
                        .map(|fts| {
                            eval_fts_query(
                                fts,
                                &collection.definition.schema,
                                &extracted.text,
                                &mut evidence,
                            )
                        })
                        .transpose()?
                        .unwrap_or(true);
                    let filter_match = filter
                        .as_ref()
                        .map(|f| eval_filter(f, &ctx, &mut evidence))
                        .transpose()?
                        .unwrap_or(true);
                    counters.residual_evaluations += u64::from(filter.is_some());
                    if fts_match && filter_match {
                        let payload = match query.options.return_mode {
                            ReturnMode::Locator => None,
                            ReturnMode::Bytes => Some(Payload::Bytes(bytes)),
                            ReturnMode::Logical => Some(Payload::Logical(extracted.value)),
                        };
                        offer(
                            repo,
                            query,
                            collection_index,
                            record,
                            evidence,
                            payload,
                            &mut winners,
                            cap,
                            &mut counters,
                            cache_members.as_deref_mut(),
                        );
                    }
                }
            }
        }
        if let (Some(traces), Some(before)) = (traces.as_deref_mut(), before.as_ref()) {
            finish_trace(traces, before, &counters);
        }
    }
    let count = counters.total_matches as usize;
    let ordered = winners.into_sorted_vec();
    let mut page: Vec<_> = ordered
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .collect();
    // Exact index membership may need a page payload. Read only those ranges.
    if query.options.return_mode != ReturnMode::Locator {
        let mut files: BTreeMap<(usize, u32), Vec<(u32, ByteRange)>> = BTreeMap::new();
        for (index, winner) in page.iter().enumerate() {
            if winner.hit.payload.is_none() {
                let record = &repo.collections[winner.collection_index].generation.records
                    [winner.slot as usize];
                files
                    .entry((winner.collection_index, record.file.0))
                    .or_default()
                    .push((index as u32, record.range));
            }
        }
        for ((collection_index, file_id), ranges) in files {
            let collection = &repo.collections[collection_index];
            let path = repo
                .files
                .native_path(crate::model::FileId(file_id))
                .ok_or("missing source path")?;
            let revision = repo.files.files[file_id as usize].revision;
            for batch in ranges.chunks(128) {
                let (records, bytes_read) = read_record_ranges(&path, revision, batch)?;
                counters.files_opened += 1;
                counters.bytes_read += bytes_read;
                if let Some(traces) = traces.as_deref_mut() {
                    if let Some(actual) = traces
                        .iter_mut()
                        .find(|trace| trace.collection == collection.definition.name)
                        .and_then(|trace| trace.actual.as_mut())
                    {
                        actual.files_opened += 1;
                        actual.bytes_read += bytes_read;
                        if query.options.return_mode == ReturnMode::Logical {
                            actual.record_parses += records.len() as u64;
                        }
                    }
                }
                for (index, bytes) in records {
                    let payload = match query.options.return_mode {
                        ReturnMode::Locator => unreachable!(),
                        ReturnMode::Bytes => Payload::Bytes(bytes),
                        ReturnMode::Logical => {
                            counters.record_parses += 1;
                            Payload::Logical(
                                reader(collection.definition.format).parse(&bytes)?.value,
                            )
                        }
                    };
                    page[index as usize].hit.payload = Some(payload);
                }
            }
        }
    }
    counters.returned_records = page.len() as u64;
    Ok(SearchResult {
        count,
        hits: page.into_iter().map(|winner| winner.hit).collect(),
        counters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LogicalType;
    use crate::query::{QueryOptions, parse_filter};
    use crate::reference::{FieldDefinition, Schema};
    use crate::repository::{CollectionDefinition, IdentityPolicy};
    use crate::source::SourceFormat;

    #[test]
    fn exact_membership_materializes_only_page_payload() {
        let root = std::env::temp_dir().join(format!("objekte-payload-{}", std::process::id()));
        std::fs::create_dir_all(root.join("data")).unwrap();
        for id in ["a", "b", "c"] {
            std::fs::write(
                root.join("data").join(format!("{id}.json")),
                format!("{{\"kind\":\"yes\",\"id\":\"{id}\"}}"),
            )
            .unwrap();
        }
        let mut schema = Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["kind".into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: true,
                multi: false,
            })
            .unwrap();
        let repo = Repository::open(
            &root,
            vec![CollectionDefinition {
                name: "data".into(),
                source_path: "data".into(),
                format: SourceFormat::Json,
                identity: IdentityPolicy::Filename,
                schema,
                sort_fields: vec![],
                sort_locale: "de".into(),
            }],
        )
        .unwrap();
        for (mode, parses, reads) in [
            (ReturnMode::Locator, 0, 0),
            (ReturnMode::Bytes, 0, 1),
            (ReturnMode::Logical, 1, 1),
        ] {
            let query = Query {
                collections: vec![],
                fts: None,
                filter: Some(parse_filter("kind = 'yes'").unwrap()),
                options: QueryOptions {
                    sort: vec![],
                    limit: 1,
                    offset: 1,
                    count: true,
                    return_mode: mode.clone(),
                    evidence: true,
                },
            };
            let result = repo.search(&query).unwrap();
            assert_eq!(
                result.counters.prepared_plan_hit,
                mode != ReturnMode::Locator
            );
            assert!(!result.counters.verified_result_hit);
            if mode == ReturnMode::Locator {
                let plan = repo.explain(&query).unwrap();
                assert!(plan[0].actual.is_none());
                assert_eq!(plan[0].terms[0].field, "kind");
                let (analyzed, trace) = repo.explain_analyze(&query).unwrap();
                assert_eq!(analyzed.count, result.count);
                assert_eq!(trace[0].actual.as_ref().unwrap().files_opened, 0);
                assert_eq!(trace[0].actual.as_ref().unwrap().index_proven_matches, 3);
            }
            assert_eq!(result.count, 3);
            assert_eq!(result.hits.len(), 1);
            assert_eq!(result.hits[0].record_id, "b");
            assert_eq!(result.counters.record_parses, parses);
            assert_eq!(result.counters.files_opened, reads);
            assert_eq!(result.hits[0].matched_fields, ["kind"]);
            match mode {
                ReturnMode::Locator => assert!(result.hits[0].payload.is_none()),
                ReturnMode::Bytes => assert!(
                    matches!(&result.hits[0].payload,Some(Payload::Bytes(b)) if b.starts_with(b"{\"kind\""))
                ),
                ReturnMode::Logical => {
                    assert!(matches!(&result.hits[0].payload, Some(Payload::Logical(_))))
                }
            }
        }
        repo.configure_verified_cache(1024 * 1024).unwrap();
        let mut query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter("kind = 'yes'").unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 1,
                offset: 1,
                count: true,
                return_mode: ReturnMode::Locator,
                evidence: true,
            },
        };
        let miss = repo.search(&query).unwrap();
        assert!(!miss.counters.verified_result_hit);
        let hit = repo.search(&query).unwrap();
        assert!(hit.counters.verified_result_hit);
        assert_eq!(hit.count, 3);
        assert_eq!(hit.hits[0].record_id, "b");
        assert_eq!(hit.hits[0].matched_fields, ["kind"]);
        assert_eq!(hit.counters.files_opened, 0);
        query.options.offset = 2;
        query.options.return_mode = ReturnMode::Logical;
        let page = repo.search(&query).unwrap();
        assert!(page.counters.verified_result_hit);
        assert_eq!(page.hits[0].record_id, "c");
        assert_eq!(page.counters.record_parses, 1);
        query.filter = Some(parse_filter("kind = 'absent'").unwrap());
        assert!(!repo.search(&query).unwrap().counters.verified_result_hit);
        let empty = repo.search(&query).unwrap();
        assert!(empty.counters.verified_result_hit);
        assert_eq!(empty.count, 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn multi_record_scan_opens_file_once_and_parses_each_range_once() {
        let root = std::env::temp_dir().join(format!("objekte-multi-{}", std::process::id()));
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(
            root.join("data/multi.jsonl"),
            "{\"name\":\"Ähre\"}\n{\"name\":\"Zug\"}\n",
        )
        .unwrap();
        let mut schema = Schema::default();
        schema
            .register_field(FieldDefinition {
                path: vec!["name".into()],
                kind: LogicalType::String,
                required: true,
                nullable: false,
                indexed: false,
                multi: false,
            })
            .unwrap();
        let repo = Repository::open(
            &root,
            vec![CollectionDefinition {
                name: "data".into(),
                source_path: "data".into(),
                format: SourceFormat::JsonLines,
                identity: IdentityPolicy::Generated,
                schema,
                sort_fields: vec![],
                sort_locale: "de".into(),
            }],
        )
        .unwrap();
        assert_eq!(
            repo.collections[0]
                .generation
                .file_records
                .records_for(crate::model::FileId(0))
                .unwrap()
                .len(),
            2
        );
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter("name:exists = true").unwrap()),
            options: QueryOptions {
                sort: vec![],
                limit: 2,
                offset: 0,
                count: true,
                return_mode: ReturnMode::Logical,
                evidence: true,
            },
        };
        let result = repo.search(&query).unwrap();
        assert_eq!(result.count, 2);
        assert_eq!(result.counters.files_opened, 1);
        assert_eq!(result.counters.record_parses, 2);
        assert!(
            result
                .hits
                .iter()
                .all(|hit| matches!(hit.payload, Some(Payload::Logical(_))))
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
