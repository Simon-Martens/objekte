//! Bounded result retention, grouped source access, and payload materialization.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, HashSet};

use roaring::RoaringBitmap;

use crate::executor::{
    Plan, estimate_expr, estimate_fts_candidates, fts_candidates, plan_expr_restricted,
};
use crate::inventory::{ByteRange, RecordEntry};
use crate::query::{Operator, Query};
use crate::reference::{Derived, RecordContext, bind_filter, eval_filter, eval_fts_query};
use crate::repository::{Payload, Repository, SearchHit, SearchResult, validate_sort};
use crate::sorting::SortKey;
use crate::source::{read_record_ranges, reader};
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
) {
    counters.total_matches += 1;
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
    let mut matched_fields: Vec<_> = if query.options.evidence {
        evidence.into_iter().collect()
    } else {
        Vec::new()
    };
    matched_fields.sort();
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
    let mut bound = Vec::new();
    for (collection_index, collection) in repo.collections.iter().enumerate() {
        if !query.collections.is_empty() && !query.collections.contains(&collection.definition.name)
        {
            continue;
        }
        let filter = query
            .filter
            .as_ref()
            .map(|f| bind_filter(f, &collection.definition.schema))
            .transpose()?;
        if let Some(fts) = &query.fts {
            for field in &fts.fields {
                collection.definition.schema.field(&field.path)?;
                if let Some(name) = &field.binding {
                    if !collection
                        .definition
                        .schema
                        .custom
                        .contains_key(&(field.path.segments.clone(), name.clone()))
                    {
                        return Err(format!("unknown FTS binding {name}"));
                    }
                }
            }
        }
        bound.push((collection_index, filter));
    }
    if bound.is_empty() {
        return Err("unknown or unloaded collection scope".into());
    }
    validate_sort(repo, query)?;
    let cap = query
        .options
        .limit
        .checked_add(query.options.offset)
        .ok_or("page size overflow")?;
    let mut winners = BinaryHeap::new();
    let mut counters = WorkCounters::default();
    for (collection_index, filter) in bound {
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
        counters.candidates += candidates.len();
        let needs_scan = !plan.exact || query.fts.is_some();
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
                );
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
                        );
                    }
                }
            }
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
