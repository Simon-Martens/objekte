//! Exact posting restrictions plus one residual scan over surviving records.

use std::collections::BTreeMap;

use roaring::RoaringBitmap;

use crate::model::{LogicalType, MetadataField, PathRoot, Value};
use crate::posting::ScalarKey;
use crate::query::{Operator, Quantifier};
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, Derived, RecordContext, Transform, eval_term,
};
use crate::repository::{CollectionState, Repository};
use crate::sorting::SortKey;
use serde::Serialize;


#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessPathKind {
    Metadata,
    Posting,
    SortColumn,
    Dictionary,
    CustomBigram,
    RawBigram,
    Scanner,
}


/// Estimated cost and coverage of one physical query path.
#[derive(Clone, Debug, Serialize)]
pub struct AccessPathEstimate {
    pub kind: AccessPathKind,
    pub exact: bool,
    pub coverage: u32,
    pub estimated_output: u64,
    pub estimated_work: f64,
    pub estimated_files: f64,
    pub estimated_bytes: f64,
}


/// Planner decision and alternatives for one bound term.
#[derive(Clone, Debug, Serialize)]
pub struct TermChoice {
    pub span: (usize, usize),
    pub field: String,
    pub logical_type: String,
    pub binding: String,
    pub selected: AccessPathEstimate,
    pub alternatives: Vec<AccessPathEstimate>,
    pub actual_input: u64,
    pub actual_output: Option<u64>,
    pub reason: String,
    pub residual_verification: bool,
    pub executed: bool,
}


fn skipped_choices(collection: &CollectionState, expr: &BoundExpr, output: &mut Vec<TermChoice>) {
    match expr {
        BoundExpr::Term(term) => {
            let mut paths = enumerate_paths(collection, term, 0);
            paths.sort_by(|a, b| a.estimated_work.total_cmp(&b.estimated_work));
            let selected = paths.remove(0);
            output.push(TermChoice {
                span: term.span,
                field: term.path.display(),
                logical_type: format!("{:?}", term.kind),
                binding: format!("{}@{}", term.binding_name, term.binding_version),
                selected,
                alternatives: paths,
                actual_input: 0,
                actual_output: Some(0),
                reason: "skipped after an exact empty conjunct".into(),
                residual_verification: false,
                executed: false,
            });
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            for part in parts {
                skipped_choices(collection, part, output);
            }
        }
        BoundExpr::Not(inner) => skipped_choices(collection, inner, output),
    }
}


/// Exact membership evidence produced by an index path.
pub(crate) struct Proof {
    pub term: BoundTerm,
    pub matches: RoaringBitmap,
}


/// Candidate set, exactness, and diagnostics for a planned expression.
pub(crate) struct Plan {
    pub candidates: Option<RoaringBitmap>,
    pub exact: bool,
    pub proofs: Vec<Proof>,
    pub choices: Vec<TermChoice>,
}


fn term_set(
    repo: &Repository,
    collection: &CollectionState,
    term: &BoundTerm,
    domain: &RoaringBitmap,
) -> Result<Option<RoaringBitmap>, String> {
    if let PathRoot::Metadata(field) = term.path.root {
        if field == MetadataField::RecordId
            && term.operator == Operator::Eq
            && term.transform == Transform::Identity
        {
            if let Value::String(id) = &term.literal {
                let mut result = RoaringBitmap::new();
                if let Some(slot) = collection
                    .id_lookup
                    .get(&crate::model::RecordId(id.clone()))
                {
                    result.insert(slot.0);
                }
                return Ok(Some(result));
            }
        }
        let empty = Value::Object(BTreeMap::new());
        let mut result = RoaringBitmap::new();
        let file_level = matches!(
            field,
            MetadataField::FileId
                | MetadataField::FilePath
                | MetadataField::FileSize
                | MetadataField::FileCreated
                | MetadataField::FileModified
        );
        if file_level {
            for file_id in &collection.generation.files {
                let Some(slots) = collection.generation.file_records.records_for(*file_id) else {
                    continue;
                };
                let Some(first) = slots.first() else {
                    continue;
                };
                let record = &collection.generation.records[first.0 as usize];
                let file = &repo.files.files[file_id.0 as usize];
                let ctx = RecordContext {
                    value: &empty,
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &repo.files.root,
                };
                if eval_term(term, &ctx)? {
                    result.extend(slots.iter().map(|s| s.0));
                }
            }
            return Ok(Some(result));
        }
        if matches!(
            field,
            MetadataField::CollectionName | MetadataField::RepositoryRoot
        ) {
            if let Some(record) = collection.generation.records.first() {
                let file = &repo.files.files[record.file.0 as usize];
                let ctx = RecordContext {
                    value: &empty,
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &repo.files.root,
                };
                if eval_term(term, &ctx)? {
                    return Ok(Some(domain.clone()));
                }
            }
            return Ok(Some(result));
        }
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
    Ok(if term.operation.is_some() {
        collection
            .custom_postings
            .get(&(term.path.segments.clone(), term.binding_name.clone()))
            .filter(|posting| posting.complete)
            .and_then(|posting| posting.index.exact_set(term, domain))
    } else {
        collection
            .postings
            .fields
            .get(&term.path.segments)
            .and_then(|index| index.exact_set(term, domain))
    })
}

fn enumerate_paths(
    collection: &CollectionState,
    term: &BoundTerm,
    input_len: u64,
) -> Vec<AccessPathEstimate> {
    let stats = collection.stats;
    let source_unit = 30.0 + stats.mean_record_bytes() / 128.0;
    let mut paths = Vec::new();
    let mut add = |kind, exact, lookup: f64, output: u64| {
        let output = output.min(input_len);
        let files =
            (output as f64 / stats.mean_records_per_file().max(1.0)).min(stats.file_count as f64);
        paths.push(AccessPathEstimate {
            kind,
            exact,
            coverage: stats.record_count as u32,
            estimated_output: output,
            estimated_work: lookup
                + if exact {
                    output as f64 * 0.02
                } else {
                    output as f64 * source_unit
                },
            estimated_files: files,
            estimated_bytes: output as f64 * stats.mean_record_bytes(),
        });
    };
    if let PathRoot::Metadata(field) = term.path.root {
        let direct_id_lookup = field == MetadataField::RecordId
            && term.operator == Operator::Eq
            && term.transform == Transform::Identity;
        let work = if direct_id_lookup {
            1.0
        } else if matches!(
            field,
            MetadataField::CollectionName | MetadataField::RepositoryRoot
        ) {
            1.0
        } else if matches!(
            field,
            MetadataField::RecordByteStart
                | MetadataField::RecordByteEnd
                | MetadataField::RecordByteLength
        ) {
            stats.record_count as f64
        } else {
            stats.file_count as f64
        };
        let output = if direct_id_lookup { 1 } else { input_len / 2 };
        add(AccessPathKind::Metadata, true, work, output);
    } else {
        let index = if term.operation.is_some() {
            collection
                .custom_postings
                .get(&(term.path.segments.clone(), term.binding_name.clone()))
                .filter(|posting| posting.complete)
                .map(|posting| &posting.index)
        } else {
            collection.postings.fields.get(&term.path.segments)
        };
        if let Some(index) = index {
            if let Some((lookup, output)) = index.estimate_exact(term, stats.record_count as u64) {
                add(AccessPathKind::Posting, true, lookup, output);
            }
            if term.kind == LogicalType::String
                && term.transform == Transform::Identity
                && term.operation.is_none()
                && term.derived == Derived::None
                && term.operator == Operator::Contains
                && (!term.multi || term.quantifier == Quantifier::Any)
                && matches!(&term.literal,Value::String(s) if !s.contains(['%','\\']))
            {
                add(
                    AccessPathKind::Dictionary,
                    true,
                    index.stats.distinct_count as f64 * 2.0,
                    input_len / 4,
                );
            }
        }
        if term.derived == Derived::None
            && !term.multi
            && term.transform == Transform::Identity
            && term.operation.is_none()
            && matches!(
                term.kind,
                LogicalType::Number
                    | LogicalType::Date
                    | LogicalType::Datetime
                    | LogicalType::Unixtime
            )
            && matches!(
                term.operator,
                Operator::Eq
                    | Operator::Ne
                    | Operator::Gt
                    | Operator::Ge
                    | Operator::Lt
                    | Operator::Le
            )
            && !matches!(term.literal, Value::Null)
            && collection.sorting.column(&term.path.display()).is_ok()
        {
            add(
                AccessPathKind::SortColumn,
                true,
                stats.record_count as f64,
                input_len / 4,
            );
        }
        if term.derived == Derived::None
            && matches!(term.operator, Operator::Contains | Operator::Eq)
            && term.kind == LogicalType::String
            && (!term.multi || term.quantifier == Quantifier::Any)
        {
            if let Value::String(literal) = &term.literal {
                if !literal.contains(['%', '\\']) {
                    if matches!(term.binding_name.as_str(), "default")
                        && term.transform == Transform::Identity
                        && term.operation.is_none()
                    {
                        if let Some(output) = collection.raw.estimate_min_cardinality(literal) {
                            add(
                                AccessPathKind::RawBigram,
                                false,
                                literal.len() as f64,
                                output,
                            );
                        }
                    } else if !matches!(term.binding_name.as_str(), "lower") {
                        if let Some(index) = collection
                            .custom_text
                            .get(&(term.path.segments.clone(), term.binding_name.clone()))
                        {
                            if let Some(output) = index.estimate_min_cardinality(literal) {
                                add(
                                    AccessPathKind::CustomBigram,
                                    false,
                                    literal.len() as f64,
                                    output,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    add(AccessPathKind::Scanner, false, 0.0, input_len);
    paths
}

fn execute_path(
    repo: &Repository,
    collection: &CollectionState,
    term: &BoundTerm,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    kind: AccessPathKind,
) -> Result<Option<RoaringBitmap>, String> {
    Ok(match kind {
        AccessPathKind::Metadata => {
            term_set(repo, collection, term, domain)?.map(|set| &set & input)
        }
        AccessPathKind::Posting => {
            if term.operation.is_some() {
                collection
                    .custom_postings
                    .get(&(term.path.segments.clone(), term.binding_name.clone()))
                    .filter(|posting| posting.complete)
                    .and_then(|posting| posting.index.exact_set_restricted(term, domain, input))
            } else {
                collection
                    .postings
                    .fields
                    .get(&term.path.segments)
                    .and_then(|index| index.exact_set_restricted(term, domain, input))
            }
        }
        AccessPathKind::CustomBigram => custom_candidates(collection, term),
        AccessPathKind::RawBigram => match &term.literal {
            Value::String(s) => collection.raw.candidates(s),
            _ => None,
        },
        AccessPathKind::Dictionary => {
            let index = collection
                .postings
                .fields
                .get(&term.path.segments)
                .ok_or("missing dictionary")?;
            let Value::String(literal) = &term.literal else {
                return Err("dictionary requires string".into());
            };
            let mut result = RoaringBitmap::new();
            for (key, posting) in &index.values {
                if let ScalarKey::String(value) = key {
                    if value.contains(literal) {
                        result |= posting;
                    }
                }
            }
            Some(result)
        }
        AccessPathKind::SortColumn => {
            let column = collection.sorting.column(&term.path.display())?;
            let literal = sort_key(&term.literal).ok_or("sort-column predicate is not scalar")?;
            let mut result = RoaringBitmap::new();
            for (slot, key) in column.keys.iter().enumerate() {
                if !matches!(key, SortKey::Missing | SortKey::Null)
                    && order_matches(key.compare(&literal, false), term.operator)
                {
                    result.insert(slot as u32);
                }
            }
            Some(result)
        }
        AccessPathKind::Scanner => None,
    })
}

fn sort_key(value: &Value) -> Option<SortKey> {
    Some(match value {
        Value::Number(n) => SortKey::Number(*n),
        Value::Date(d) => SortKey::Date(*d),
        Value::Datetime(d) => SortKey::Datetime(*d),
        Value::Unixtime(t) => SortKey::Unixtime(*t),
        _ => return None,
    })
}

fn order_matches(order: std::cmp::Ordering, op: Operator) -> bool {
    match op {
        Operator::Eq => order.is_eq(),
        Operator::Ne => !order.is_eq(),
        Operator::Gt => order.is_gt(),
        Operator::Ge => order.is_ge(),
        Operator::Lt => order.is_lt(),
        Operator::Le => order.is_le(),
        _ => false,
    }
}

pub(crate) fn estimate_expr(
    collection: &CollectionState,
    expr: &BoundExpr,
    input_len: u64,
) -> (f64, u64) {
    match expr {
        BoundExpr::Term(term) => enumerate_paths(collection, term, input_len)
            .into_iter()
            .min_by(|a, b| a.estimated_work.total_cmp(&b.estimated_work))
            .map(|p| (p.estimated_work, p.estimated_output))
            .unwrap_or((input_len as f64, input_len)),
        BoundExpr::And(parts) => parts
            .iter()
            .map(|p| estimate_expr(collection, p, input_len))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap_or((0.0, input_len)),
        BoundExpr::Or(parts) => {
            let estimates: Vec<_> = parts
                .iter()
                .map(|p| estimate_expr(collection, p, input_len))
                .collect();
            (
                estimates.iter().map(|e| e.0).sum(),
                estimates.iter().map(|e| e.1).sum::<u64>().min(input_len),
            )
        }
        BoundExpr::Not(inner) => {
            let (work, _) = estimate_expr(collection, inner, input_len);
            (work, input_len / 2)
        }
    }
}

fn flatten_and<'a>(expr: &'a BoundExpr, output: &mut Vec<&'a BoundExpr>) {
    if let BoundExpr::And(parts) = expr {
        for part in parts {
            flatten_and(part, output);
        }
    } else {
        output.push(expr);
    }
}

pub(crate) fn plan_expr_restricted(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
) -> Result<Plan, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let mut paths = enumerate_paths(collection, term, input.len());
            paths.sort_by(|a, b| {
                a.estimated_work
                    .total_cmp(&b.estimated_work)
                    .then_with(|| (a.kind as u8).cmp(&(b.kind as u8)))
            });
            let selected = paths.remove(0);
            let candidates = execute_path(repo, collection, term, domain, input, selected.kind)?
                .map(|set| {
                    if matches!(
                        selected.kind,
                        AccessPathKind::Metadata | AccessPathKind::Posting
                    ) {
                        set
                    } else {
                        &set & input
                    }
                });
            let exact = selected.exact;
            let exact_candidates = if exact { candidates.as_ref() } else { None };
            let proofs = exact_candidates
                .map(|set| {
                    vec![Proof {
                        term: term.clone(),
                        matches: set.clone(),
                    }]
                })
                .unwrap_or_default();
            let choices = vec![TermChoice {
                span: term.span,
                field: term.path.display(),
                logical_type: format!("{:?}", term.kind),
                binding: format!("{}@{}", term.binding_name, term.binding_version),
                selected,
                alternatives: paths,
                actual_input: input.len(),
                actual_output: candidates.as_ref().map(RoaringBitmap::len),
                reason: "lowest estimated lookup plus remaining source work for current input"
                    .into(),
                residual_verification: !exact,
                executed: true,
            }];
            Plan {
                candidates,
                exact,
                proofs,
                choices,
            }
        }
        BoundExpr::And(parts) => {
            let mut remaining = Vec::new();
            for part in parts {
                flatten_and(part, &mut remaining);
            }
            let mut current = input.clone();
            let mut exact = true;
            let mut proofs = Vec::new();
            let mut choices = Vec::new();
            while !remaining.is_empty() {
                let source_unit = 30.0 + collection.stats.mean_record_bytes() / 128.0;
                let next = remaining
                    .iter()
                    .enumerate()
                    .map(|(index, part)| {
                        let (work, output) = estimate_expr(collection, part, current.len());
                        let score = work + output as f64 * source_unit * 0.1;
                        (index, score)
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
                    .unwrap()
                    .0;
                let child = plan_expr_restricted(
                    repo,
                    collection,
                    remaining.remove(next),
                    domain,
                    &current,
                )?;
                if let Some(set) = child.candidates {
                    current &= set;
                }
                exact &= child.exact;
                proofs.extend(child.proofs);
                choices.extend(child.choices);
                if current.is_empty() {
                    exact = true;
                    for part in remaining {
                        skipped_choices(collection, part, &mut choices);
                    }
                    break;
                }
            }
            Plan {
                candidates: Some(current),
                exact,
                proofs,
                choices,
            }
        }
        BoundExpr::Or(parts) => {
            let mut ordered: Vec<_> = parts.iter().collect();
            ordered.sort_by(|a, b| {
                estimate_expr(collection, a, input.len())
                    .0
                    .total_cmp(&estimate_expr(collection, b, input.len()).0)
            });
            let children = ordered
                .into_iter()
                .map(|p| plan_expr_restricted(repo, collection, p, domain, input))
                .collect::<Result<Vec<_>, _>>()?;
            if children.iter().all(|p| p.candidates.is_some()) {
                let mut candidates = RoaringBitmap::new();
                let mut proofs = Vec::new();
                let mut choices = Vec::new();
                let exact = children.iter().all(|p| p.exact);
                for child in children {
                    candidates |= child.candidates.unwrap();
                    proofs.extend(child.proofs);
                    choices.extend(child.choices);
                }
                Plan {
                    candidates: Some(candidates),
                    exact,
                    proofs,
                    choices,
                }
            } else {
                Plan {
                    candidates: None,
                    exact: false,
                    proofs: Vec::new(),
                    choices: children
                        .into_iter()
                        .flat_map(|child| child.choices)
                        .collect(),
                }
            }
        }
        BoundExpr::Not(inner) => {
            let child = plan_expr_restricted(repo, collection, inner, domain, input)?;
            if child.exact {
                Plan {
                    candidates: Some(input - &child.candidates.unwrap()),
                    exact: true,
                    proofs: child.proofs,
                    choices: child.choices,
                }
            } else {
                Plan {
                    candidates: None,
                    exact: false,
                    proofs: Vec::new(),
                    choices: child.choices,
                }
            }
        }
    })
}

fn custom_candidates(collection: &CollectionState, term: &BoundTerm) -> Option<RoaringBitmap> {
    if term.derived != Derived::None || !matches!(term.operator, Operator::Contains | Operator::Eq)
    {
        return None;
    }
    if matches!(term.binding_name.as_str(), "default" | "lower") {
        return None;
    }
    let Value::String(literal) = &term.literal else {
        return None;
    };
    if literal.contains(['%', '\\']) {
        return None;
    }
    collection
        .custom_text
        .get(&(term.path.segments.clone(), term.binding_name.clone()))?
        .candidates(literal)
}

pub(crate) fn fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<RoaringBitmap> {
    if fts.fields.is_empty() {
        if collection
            .definition
            .schema
            .raw
            .as_ref()
            .is_some_and(|binding| !binding.operation.complete_coverage())
        {
            return None;
        }
        return collection.raw.candidates(&fts.literal);
    }
    let mut union = RoaringBitmap::new();
    for field in &fts.fields {
        if field.path.root != PathRoot::Data || field.binding_name == "lower" {
            return None;
        }
        let candidates = if field.binding_name != "default" {
            let key = (field.path.segments.clone(), field.binding_name.clone());
            collection
                .custom_text
                .get(&key)?
                .candidates(&field.literal)?
        } else {
            collection.raw.candidates(&field.literal)?
        };


        union |= candidates;
    }
    Some(union)
}

pub(crate) fn estimate_fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<(u64, f64)> {
    if fts.fields.is_empty() {
        if collection
            .definition
            .schema
            .raw
            .as_ref()
            .is_some_and(|binding| !binding.operation.complete_coverage())
        {
            return None;
        }
        return collection
            .raw
            .estimate_min_cardinality(&fts.literal)
            .map(|count| (count, fts.literal.len() as f64));
    }
    let mut count = 0u64;
    let mut work = 0.0;
    for field in &fts.fields {
        if field.path.root != PathRoot::Data || field.binding_name == "lower" {
            return None;
        }
        let estimate = if field.binding_name != "default" {
            let key = (field.path.segments.clone(), field.binding_name.clone());
            work += field.literal.len() as f64;
            collection
                .custom_text
                .get(&key)?
                .estimate_min_cardinality(&field.literal)?
        } else {
            work += field.literal.len() as f64;
            collection.raw.estimate_min_cardinality(&field.literal)?
        };
        count = count.saturating_add(estimate);
    }
    Some((count.min(collection.stats.record_count as u64), work))
}

#[cfg(test)]
#[path = "executor.test.rs"]
mod tests;
