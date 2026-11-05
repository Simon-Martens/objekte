//! Exact posting restrictions plus one residual scan over surviving records.

use std::collections::{BTreeMap, HashSet};

use roaring::RoaringBitmap;

use crate::model::{PathRoot, Value};
use crate::query::{Operator, Query};
use crate::reference::{
    BoundExpr, BoundTerm, Derived, RecordContext, bind_filter, eval_filter, eval_fts, eval_term,
};
use crate::repository::{CollectionState, Repository, SearchHit, SearchResult};
use crate::source::{read_record_range, reader};
use crate::workload::{ReturnMode, WorkCounters};

struct Proof {
    term: BoundTerm,
    matches: RoaringBitmap,
}
struct Plan {
    candidates: Option<RoaringBitmap>,
    exact: bool,
    proofs: Vec<Proof>,
}

fn term_set(
    repo: &Repository,
    collection: &CollectionState,
    term: &BoundTerm,
    domain: &RoaringBitmap,
) -> Result<Option<RoaringBitmap>, String> {
    if let PathRoot::Metadata(_) = term.path.root {
        let empty = Value::Object(BTreeMap::new());
        let mut result = RoaringBitmap::new();
        for record in &collection.generation.records {
            let file = &repo.files.files[record.file.0 as usize];
            let ctx = RecordContext {
                value: &empty,
                record,
                file,
                collection_name: &collection.definition.name,
                repository_root: &repo.files.root,
            };
            if eval_term(term, &ctx)? {
                result.insert(record.slot.0);
            }
        }
        return Ok(Some(result));
    }
    Ok(collection
        .postings
        .fields
        .get(&term.path.segments)
        .and_then(|index| index.exact_set(term, domain)))
}

fn plan_expr(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
) -> Result<Plan, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let candidates = term_set(repo, collection, term, domain)?;
            let exact = candidates.is_some();
            let proofs = candidates
                .as_ref()
                .map(|set| {
                    vec![Proof {
                        term: term.clone(),
                        matches: set.clone(),
                    }]
                })
                .unwrap_or_default();
            Plan {
                candidates,
                exact,
                proofs,
            }
        }
        BoundExpr::And(parts) => {
            let mut children = parts
                .iter()
                .map(|p| plan_expr(repo, collection, p, domain))
                .collect::<Result<Vec<_>, _>>()?;
            let exact = children.iter().all(|p| p.exact);
            children.sort_by_key(|p| p.candidates.as_ref().map(|s| s.len()).unwrap_or(u64::MAX));
            let mut candidates = None;
            let mut proofs = Vec::new();
            for child in children {
                if let Some(set) = child.candidates {
                    candidates = Some(match candidates {
                        None => set,
                        Some(prior) => &prior & &set,
                    });
                }
                proofs.extend(child.proofs);
                if candidates.as_ref().is_some_and(RoaringBitmap::is_empty) {
                    break;
                }
            }
            Plan {
                candidates,
                exact,
                proofs,
            }
        }
        BoundExpr::Or(parts) => {
            let children = parts
                .iter()
                .map(|p| plan_expr(repo, collection, p, domain))
                .collect::<Result<Vec<_>, _>>()?;
            if children.iter().all(|p| p.exact) {
                let mut candidates = RoaringBitmap::new();
                let mut proofs = Vec::new();
                for child in children {
                    candidates |= child.candidates.unwrap();
                    proofs.extend(child.proofs);
                }
                Plan {
                    candidates: Some(candidates),
                    exact: true,
                    proofs,
                }
            } else {
                Plan {
                    candidates: None,
                    exact: false,
                    proofs: Vec::new(),
                }
            }
        }
        BoundExpr::Not(inner) => {
            let child = plan_expr(repo, collection, inner, domain)?;
            if child.exact {
                Plan {
                    candidates: Some(domain - &child.candidates.unwrap()),
                    exact: true,
                    proofs: Vec::new(),
                }
            } else {
                Plan {
                    candidates: None,
                    exact: false,
                    proofs: Vec::new(),
                }
            }
        }
    })
}

pub fn search(repo: &Repository, query: &Query) -> Result<SearchResult, String> {
    if !query.options.sort.is_empty() {
        return Err("sorting is not implemented yet".into());
    }
    if query.options.return_mode != ReturnMode::Locator {
        return Err("payload modes are not implemented yet".into());
    }
    // Bind every selected collection before any execution or source access.
    let mut bound = Vec::new();
    for collection in &repo.collections {
        if !query.collections.is_empty() && !query.collections.contains(&collection.definition.name)
        {
            continue;
        }
        let filter = query
            .filter
            .as_ref()
            .map(|f| bind_filter(f, &collection.definition.schema))
            .transpose()?;
        let mut fields = Vec::new();
        if let Some(fts) = &query.fts {
            for field in &fts.fields {
                if field.binding.is_some() {
                    return Err("selected FTS binding is not implemented yet".into());
                }
                collection.definition.schema.field(&field.path)?;
                fields.push(field.path.clone());
            }
        }
        bound.push((collection, filter, fields));
    }
    if bound.is_empty() {
        return Err("unknown or unloaded collection scope".into());
    }
    let mut hits = Vec::new();
    let mut counters = WorkCounters::default();
    for (collection, filter, fields) in bound {
        let domain: RoaringBitmap = (0..collection.generation.records.len() as u32).collect();
        let plan = filter
            .as_ref()
            .map(|f| plan_expr(repo, collection, f, &domain))
            .transpose()?
            .unwrap_or(Plan {
                candidates: None,
                exact: true,
                proofs: Vec::new(),
            });
        let candidates = plan.candidates.unwrap_or(domain);
        counters.candidates += candidates.len();
        let needs_scan = !plan.exact || query.fts.is_some();
        for slot in candidates.iter() {
            let record = &collection.generation.records[slot as usize];
            let file = &repo.files.files[record.file.0 as usize];
            let mut evidence = HashSet::new();
            let is_match = if needs_scan {
                let path = repo
                    .files
                    .native_path(record.file)
                    .ok_or("missing source path")?;
                let bytes = read_record_range(&path, record.revision, record.range)?;
                counters.files_opened += 1;
                counters.bytes_read += bytes.len() as u64;
                counters.scanned_records += 1;
                counters.record_parses += 1;
                let extracted = reader(collection.definition.format).parse(&bytes)?;
                let ctx = RecordContext {
                    value: &extracted.value,
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &repo.files.root,
                };
                let fts_match = query.fts.as_ref().is_none_or(|fts| {
                    eval_fts(&fts.literal, &fields, &extracted.text, &mut evidence)
                });
                let filter_match = filter
                    .as_ref()
                    .map(|f| eval_filter(f, &ctx, &mut evidence))
                    .transpose()?
                    .unwrap_or(true);
                counters.residual_evaluations += u64::from(filter.is_some());
                fts_match && filter_match
            } else {
                counters.exact_posting_matches += 1;
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
                true
            };
            if is_match {
                let mut matched_fields: Vec<_> = if query.options.evidence {
                    evidence.into_iter().collect()
                } else {
                    Vec::new()
                };
                matched_fields.sort();
                hits.push(SearchHit {
                    collection: collection.definition.name.clone(),
                    record_id: record.id.0.clone(),
                    file_path: file.relative_path.clone(),
                    byte_start: record.range.start,
                    byte_end: record.range.end,
                    modified_ns: record.revision.modified_ns,
                    matched_fields,
                });
            }
        }
    }
    hits.sort_by(|a, b| (&a.collection, &a.record_id).cmp(&(&b.collection, &b.record_id)));
    let count = hits.len();
    counters.total_matches = count as u64;
    let hits: Vec<_> = hits
        .into_iter()
        .skip(query.options.offset)
        .take(query.options.limit)
        .collect();
    counters.returned_records = hits.len() as u64;
    Ok(SearchResult {
        count,
        hits,
        counters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LogicalType;
    use crate::query::{QueryOptions, parse_filter};
    use crate::reference::FieldDefinition;
    use crate::repository::{CollectionDefinition, IdentityPolicy};
    use crate::source::SourceFormat;

    #[test]
    fn indexed_restriction_precedes_written_first_scan() {
        let root = std::env::temp_dir().join(format!("objekte-executor-{}", std::process::id()));
        let path = root.join("data");
        std::fs::create_dir_all(&path).unwrap();
        for (id, kind, text) in [
            ("a", "wanted", "needle"),
            ("b", "other", "needle"),
            ("c", "other", "nope"),
        ] {
            std::fs::write(
                path.join(format!("{id}.json")),
                serde_json::json!({"kind":kind,"text":text}).to_string(),
            )
            .unwrap();
        }
        let mut schema = crate::reference::Schema::default();
        for (name, indexed) in [("kind", true), ("text", false)] {
            schema
                .register_field(FieldDefinition {
                    path: vec![name.into()],
                    kind: LogicalType::String,
                    required: true,
                    nullable: false,
                    indexed,
                    multi: false,
                })
                .unwrap();
        }
        let repo = Repository::open(
            &root,
            vec![CollectionDefinition {
                name: "data".into(),
                source_path: "data".into(),
                format: SourceFormat::Json,
                identity: IdentityPolicy::Filename,
                schema,
            }],
        )
        .unwrap();
        let options = QueryOptions {
            sort: vec![],
            limit: 50,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        };
        for expression in [
            "text ~ 'needle' && kind = 'wanted'",
            "kind = 'wanted' && text ~ 'needle'",
        ] {
            let query = Query {
                collections: vec![],
                fts: None,
                filter: Some(parse_filter(expression).unwrap()),
                options: options.clone(),
            };
            let optimized = repo.search(&query).unwrap();
            let reference = repo.search_reference(&query).unwrap();
            assert_eq!(optimized.count, reference.count);
            assert_eq!(optimized.hits[0].record_id, "a");
            assert_eq!(optimized.counters.record_parses, 1);
        }
        let query = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter("text ~ 'needle' && kind = 'absent'").unwrap()),
            options,
        };
        let optimized = repo.search(&query).unwrap();
        assert_eq!(optimized.count, 0);
        assert_eq!(optimized.counters.files_opened, 0);
        let identity = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter("@record.id = 'b'").unwrap()),
            options: query.options.clone(),
        };
        let optimized = repo.search(&identity).unwrap();
        assert_eq!(optimized.count, 1);
        assert_eq!(optimized.hits[0].record_id, "b");
        assert_eq!(optimized.counters.files_opened, 0);
        let disjunction = Query {
            collections: vec![],
            fts: None,
            filter: Some(parse_filter("kind = 'wanted' || text ~ 'nope'").unwrap()),
            options: query.options.clone(),
        };
        let optimized = repo.search(&disjunction).unwrap();
        assert_eq!(optimized.count, 2);
        assert_eq!(
            optimized.count,
            repo.search_reference(&disjunction).unwrap().count
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
