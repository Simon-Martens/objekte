//! Exact posting restrictions plus one residual scan over surviving records.

use std::collections::BTreeMap;

use roaring::RoaringBitmap;

use crate::model::{LogicalType, MetadataField, PathRoot, Value};
use crate::posting::ScalarKey;
use crate::query::{FtsMode, Operator, Quantifier};
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, Derived, RecordContext, Transform, eval_term,
};
use crate::repository::{CollectionState, Repository};
use crate::sorting::SortKey;
use crate::source::LogicalTextCoverage;
use serde::Serialize;


#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessPathKind {
    Metadata,
    Posting,
    InferredPosting,
    SortColumn,
    Dictionary,
    CustomBigram,
    RawBigram,
    RelationTargetFirst,
    RelationSourceRestricted,
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
    pub exact_case_sensitivity: Option<crate::query::CaseSensitivity>,
    pub selected: AccessPathEstimate,
    pub alternatives: Vec<AccessPathEstimate>,
    pub actual_input: u64,
    pub actual_output: Option<u64>,
    pub reason: String,
    pub dynamic_binding: bool,
    pub gram_candidates: Option<u64>,
    pub structural_candidates: Option<u64>,
    pub residual_verification: bool,
    pub executed: bool,
}


fn term_case_sensitivity(term: &BoundTerm) -> Option<crate::query::CaseSensitivity> {
    (matches!(term.kind, LogicalType::String | LogicalType::Unknown)
        && matches!(term.literal, Value::String(_))
        && term.operation.is_none())
    .then_some(if term.transform == Transform::CaseFold {
        crate::query::CaseSensitivity::Insensitive
    } else {
        crate::query::CaseSensitivity::Sensitive
    })
}

fn skipped_choices(collection: &CollectionState, expr: &BoundExpr, output: &mut Vec<TermChoice>) {
    match expr {
        BoundExpr::Term(term) => {
            let mut paths = enumerate_paths(collection, term, 0);
            paths.sort_by(|a, b| a.estimated_work.total_cmp(&b.estimated_work));
            let selected = paths.remove(0);
            output.push(TermChoice {
                span: term.span,
                field: term.query_path.display.clone(),
                logical_type: format!("{:?}", term.kind),
                binding: format!("{}@{}", term.binding_name, term.binding_version),
                exact_case_sensitivity: term_case_sensitivity(term),
                selected,
                alternatives: paths,
                actual_input: 0,
                actual_output: Some(0),
                reason: "skipped after an exact empty conjunct".into(),
                dynamic_binding: term.kind == LogicalType::Unknown,
                gram_candidates: None,
                structural_candidates: None,
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


/// Known true and false fallback slots; all other slots are unavailable.
struct FallbackTruth {
    yes: RoaringBitmap,
    no: RoaringBitmap,
    proofs: Vec<Proof>,
    choices: Vec<TermChoice>,
}


fn fallback_truth(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    input: &RoaringBitmap,
) -> Result<FallbackTruth, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let metadata = term.related.is_none()
                && term.path.root != PathRoot::Data
                && term.path.root != PathRoot::Backlinks
                && term
                    .rhs
                    .as_ref()
                    .is_none_or(|rhs| rhs.path.root != PathRoot::Data);
            let mut yes = RoaringBitmap::new();
            if metadata {
                let empty = Value::Object(BTreeMap::new());
                for slot in input.iter() {
                    let record = &collection.generation.records[slot as usize];
                    let file = &repo.files.files[record.file.0 as usize];
                    let ctx = RecordContext {
                        value: &empty,
                        source_bytes: None,
                        record,
                        file,
                        collection_name: &collection.definition.name,
                        repository_root: &repo.files.root,
                    };
                    if eval_term(term, &ctx)? {
                        yes.insert(slot);
                    }
                }
            }
            let no = if metadata {
                input - &yes
            } else {
                RoaringBitmap::new()
            };
            let proofs = if metadata {
                vec![Proof {
                    term: term.clone(),
                    input: input.clone(),
                    matches: yes.clone(),
                }]
            } else {
                Vec::new()
            };
            let choices = vec![TermChoice {
                span: term.span,
                field: term.query_path.display.clone(),
                logical_type: format!("{:?}", term.kind),
                binding: format!("{}@{}", term.binding_name, term.binding_version),
                exact_case_sensitivity: term_case_sensitivity(term),
                selected: AccessPathEstimate {
                    kind: if metadata {
                        AccessPathKind::Metadata
                    } else {
                        AccessPathKind::Scanner
                    },
                    exact: metadata,
                    coverage: input.len() as u32,
                    estimated_output: yes.len(),
                    estimated_work: if metadata { input.len() as f64 } else { 0.0 },
                    estimated_files: 0.0,
                    estimated_bytes: 0.0,
                },
                alternatives: Vec::new(),
                actual_input: input.len(),
                actual_output: Some(yes.len()),
                reason: if metadata {
                    "fallback metadata evaluated from inventory"
                } else {
                    "logical value unavailable on raw fallback"
                }
                .into(),
                dynamic_binding: term.kind == LogicalType::Unknown,
                gram_candidates: None,
                structural_candidates: None,
                residual_verification: false,
                executed: metadata,
            }];
            FallbackTruth {
                yes,
                no,
                proofs,
                choices,
            }
        }
        BoundExpr::And(parts) => {
            let mut yes = input.clone();
            let mut no = RoaringBitmap::new();
            let mut proofs = Vec::new();
            let mut choices = Vec::new();
            for part in parts {
                let child = fallback_truth(repo, collection, part, input)?;
                yes &= child.yes;
                no |= child.no;
                proofs.extend(child.proofs);
                choices.extend(child.choices);
            }
            FallbackTruth {
                yes,
                no,
                proofs,
                choices,
            }
        }
        BoundExpr::Or(parts) => {
            let mut yes = RoaringBitmap::new();
            let mut no = input.clone();
            let mut proofs = Vec::new();
            let mut choices = Vec::new();
            for part in parts {
                let child = fallback_truth(repo, collection, part, input)?;
                yes |= child.yes;
                no &= child.no;
                proofs.extend(child.proofs);
                choices.extend(child.choices);
            }
            FallbackTruth {
                yes,
                no,
                proofs,
                choices,
            }
        }
        BoundExpr::Not(inner) => {
            let child = fallback_truth(repo, collection, inner, input)?;
            FallbackTruth {
                yes: child.no,
                no: child.yes,
                proofs: child.proofs,
                choices: child.choices,
            }
        }
    })
}


/// Exact membership evidence produced by an index path.
pub(crate) struct Proof {
    pub term: BoundTerm,
    pub input: RoaringBitmap,
    pub matches: RoaringBitmap,
}


/// Candidate set, exactness, and diagnostics for a planned expression.
pub(crate) struct Plan {
    pub candidates: Option<RoaringBitmap>,
    pub exact: bool,
    pub proven_hits: RoaringBitmap,
    pub proofs: Vec<Proof>,
    pub choices: Vec<TermChoice>,
    pub semijoin_work: crate::relation_semijoin::SemijoinWork,
}


/// A whole-record raw index is a field superset only when its source reader
/// supplies every logical string and the indexed representation is identity.
/// Resolve this at planning time, before any record is visited. The source
/// registration and raw binding versions are fixed by the collection's
/// immutable generation and snapshot configuration.
fn raw_covers_identity_field(collection: &CollectionState) -> Result<(), String> {
    if collection.source.reader().logical_text_coverage() != LogicalTextCoverage::AllStringFields {
        return Err(format!(
            "source {}@{} does not cover every logical string field",
            collection.source.id(),
            collection.source.version()
        ));
    }
    if let Some(binding) = &collection.definition.schema.raw {
        return Err(format!(
            "raw representation {}@{} differs from identity field text",
            binding.name, binding.version
        ));
    }
    if collection.raw.record_count as usize != collection.generation.records.len() {
        return Err("raw index has incomplete record coverage".into());
    }
    Ok(())
}

fn empty_selection_preservation(term: &BoundTerm) -> bool {
    term.multi && term.quantifier != Quantifier::Any
}

fn raw_bigram_candidates(collection: &CollectionState, term: &BoundTerm) -> Option<RoaringBitmap> {
    let request = term.gram_request.as_ref()?;
    let mut grams = collection.raw.candidates_prepared(request)?;
    if empty_selection_preservation(term) {
        // The universal comparison may be true on a present empty array with
        // no string witness. This bitmap is deliberately collection-wide.
        grams |= &collection.empty_array_records;
    }
    Some(grams)
}

fn term_set(
    repo: &Repository,
    collection: &CollectionState,
    term: &BoundTerm,
    domain: &RoaringBitmap,
) -> Result<Option<RoaringBitmap>, String> {
    if term.related.is_some() {
        return Ok(None);
    }
    if let PathRoot::Metadata(field) = term.path.root {
        if term.rhs.is_some() {
            let empty = Value::Object(BTreeMap::new());
            let mut result = RoaringBitmap::new();
            for slot in domain.iter() {
                let record = &collection.generation.records[slot as usize];
                let file = &repo.files.files[record.file.0 as usize];
                let ctx = RecordContext {
                    value: &empty,
                    source_bytes: None,
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &repo.files.root,
                };
                if eval_term(term, &ctx)? {
                    result.insert(slot);
                }
            }
            return Ok(Some(result));
        }
        if field == MetadataField::RecordId
            && term.operator == Operator::Eq
            && term.transform == Transform::Identity
        {
            if let Value::Id(id) = &term.literal {
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
                    source_bytes: None,
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
                    source_bytes: None,
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
                source_bytes: None,
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

fn custom_bigram_compatible(term: &BoundTerm) -> bool {
    if term.operation.is_none() {
        // Registered built-in transforms map indexed text and scanned values
        // with the same Transform; they do not install an exact matcher.
        return true;
    }
    term.matcher
        .as_ref()
        .is_some_and(|matcher| matcher.bigram_compatible())
}

fn custom_text_index<'a>(
    collection: &'a CollectionState,
    path: &[String],
    name: &str,
) -> Option<&'a crate::bigram::BigramIndex> {
    let key = if matches!(name, "default" | "lower") {
        crate::repository::built_in_field_text_key(&collection.definition.schema, path)?
    } else {
        crate::repository::canonical_custom_text_key(&collection.definition.schema, path, name)
    };
    collection.custom_text.get(&key)
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
            coverage: if matches!(
                kind,
                AccessPathKind::Posting
                    | AccessPathKind::InferredPosting
                    | AccessPathKind::Dictionary
                    | AccessPathKind::SortColumn
                    | AccessPathKind::RawBigram
                    | AccessPathKind::CustomBigram
            ) {
                collection.logical_records.len() as u32
            } else {
                stats.record_count as u32
            },
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
    if term.related.is_some() {
        // Relation terms retain a three-valued residual until the dedicated
        // semijoin planner can prove target and source memberships together.
        add(AccessPathKind::Scanner, false, 0.0, input_len);
        return paths;
    }
    if term
        .rhs
        .as_ref()
        .is_some_and(|rhs| term.path.root != PathRoot::Data && rhs.path.root != PathRoot::Data)
    {
        add(
            AccessPathKind::Metadata,
            true,
            stats.record_count as f64,
            input_len / 2,
        );
        return paths;
    }
    if term.rhs.is_some() {
        // Ordinary postings have record membership, not array occurrence
        // positions. A positional predicate always keeps a residual scan.
        add(AccessPathKind::Scanner, false, 0.0, input_len);
        return paths;
    }
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
        if let Some(index) = index.filter(|_| !term.query_path.has_position) {
            if let Some((lookup, output)) =
                index.estimate_exact(term, collection.logical_records.len())
            {
                add(AccessPathKind::Posting, true, lookup, output);
            }
            if term.kind == LogicalType::String
                && term.transform == Transform::Identity
                && term.operation.is_none()
                && term.derived == Derived::None
                && term.operator == Operator::Contains
                && index.covered_records as u64 == collection.logical_records.len()
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
        if let Some(inference) = &collection.inference {
            if let Some((candidates, exact)) = inference.candidates(
                term,
                &collection.logical_records,
                &collection.logical_records,
            ) {
                let lookup = inference
                    .indexes
                    .get(&term.path.segments)
                    .map_or(1.0, |index| {
                        if term.operator == Operator::Eq {
                            1.0
                        } else {
                            index.values.len() as f64
                        }
                    });
                add(
                    AccessPathKind::InferredPosting,
                    exact,
                    lookup,
                    candidates.len(),
                );
            }
        }
        if term.derived == Derived::None
            && term.period.is_none()
            && !term.multi
            && !term.query_path.has_position
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
            && matches!(term.kind, LogicalType::String | LogicalType::Unknown)
        {
            if let Value::String(literal) = &term.literal {
                if !literal.contains(['%', '\\']) {
                    if matches!(term.binding_name.as_str(), "default" | "lower")
                        && matches!(term.transform, Transform::Identity | Transform::CaseFold)
                        && term.operation.is_none()
                        && term.matcher.is_none()
                        && raw_covers_identity_field(collection).is_ok()
                    {
                        if let Some(gram_count) = term
                            .gram_request
                            .as_ref()
                            .and_then(|request| collection.raw.estimate_prepared(request))
                        {
                            let structural_count = empty_selection_preservation(term)
                                .then_some(collection.empty_array_records.len());
                            let output = if structural_count.is_some() {
                                raw_bigram_candidates(collection, term)
                                    .map_or(input_len, |set| set.len())
                            } else {
                                gram_count
                            };
                            add(
                                AccessPathKind::RawBigram,
                                false,
                                literal.len() as f64,
                                output,
                            );
                        }
                    }
                    if !term.query_path.has_position
                        && (!term.multi || term.quantifier == Quantifier::Any)
                        && custom_bigram_compatible(term)
                    {
                        if let Some(index) =
                            custom_text_index(collection, &term.path.segments, &term.binding_name)
                        {
                            if let Some(output) = term
                                .gram_request
                                .as_ref()
                                .and_then(|request| index.estimate_prepared(request))
                            {
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
        AccessPathKind::InferredPosting => collection
            .inference
            .as_ref()
            .and_then(|inference| inference.candidates(term, domain, input))
            .map(|(set, _)| set),
        AccessPathKind::CustomBigram => custom_candidates(collection, term),
        AccessPathKind::RawBigram => raw_bigram_candidates(collection, term),
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
        AccessPathKind::RelationTargetFirst | AccessPathKind::RelationSourceRestricted => {
            return Err("relation strategy is selected after local planning".into());
        }
        AccessPathKind::Scanner => None,
    })
}

fn sort_key(value: &Value) -> Option<SortKey> {
    Some(match value {
        Value::Number(n) => SortKey::Number(n.clone()),
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
    evidence: bool,
) -> Result<Plan, String> {
    if input.is_empty() {
        return Ok(Plan {
            candidates: Some(RoaringBitmap::new()),
            exact: true,
            proven_hits: RoaringBitmap::new(),
            proofs: Vec::new(),
            choices: Vec::new(),
            semijoin_work: Default::default(),
        });
    }
    if collection.logical_records.len() == collection.generation.records.len() as u64 {
        let mut plan = plan_logical_expr(repo, collection, expr, domain, input, evidence)?;
        if !collection.relations.is_empty() {
            crate::relation_semijoin::restrict(repo, expr, input, &mut plan)?;
        }
        return Ok(plan);
    }
    let logical_input = input & &collection.logical_records;
    let fallback_input = input - &collection.logical_records;
    let mut logical = if logical_input.is_empty() {
        Plan {
            candidates: Some(RoaringBitmap::new()),
            exact: true,
            proven_hits: RoaringBitmap::new(),
            proofs: Vec::new(),
            choices: Vec::new(),
            semijoin_work: Default::default(),
        }
    } else {
        plan_logical_expr(
            repo,
            collection,
            expr,
            &collection.logical_records,
            &logical_input,
            evidence,
        )?
    };
    let mut candidates = logical.candidates.take().unwrap_or(logical_input);
    if !fallback_input.is_empty() {
        let fallback = fallback_truth(repo, collection, expr, &fallback_input)?;
        candidates |= &fallback.yes;
        logical.proven_hits |= &fallback.yes;
        logical.proofs.extend(fallback.proofs);
        logical.choices.extend(fallback.choices);
    }
    logical.candidates = Some(candidates);
    if !collection.relations.is_empty() {
        crate::relation_semijoin::restrict(repo, expr, input, &mut logical)?;
    }
    Ok(logical)
}

fn plan_logical_expr(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    evidence: bool,
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
            if selected.exact && candidates.is_none() {
                return Err("selected exact access path did not return a proof".into());
            }
            let exact = selected.exact;
            let exact_candidates = if exact { candidates.as_ref() } else { None };
            let proofs = exact_candidates
                .map(|set| {
                    vec![Proof {
                        term: term.clone(),
                        input: input.clone(),
                        matches: set.clone(),
                    }]
                })
                .unwrap_or_default();
            let dynamic_binding = term.kind == LogicalType::Unknown;
            let gram_candidates = (selected.kind == AccessPathKind::RawBigram)
                .then(|| {
                    term.gram_request
                        .as_ref()
                        .and_then(|request| collection.raw.estimate_prepared(request))
                })
                .flatten();
            let structural_candidates = (selected.kind == AccessPathKind::RawBigram
                && empty_selection_preservation(term))
            .then_some(collection.empty_array_records.len());
            let reason = if selected.kind == AccessPathKind::InferredPosting {
                if selected.exact {
                    "complete inferred posting proves the dynamic presence/null predicate".into()
                } else {
                    "complete inferred posting restricts dynamic values; selector and quantifier retain residual verification".into()
                }
            } else if selected.kind == AccessPathKind::RawBigram && dynamic_binding {
                format!(
                    "undeclared dynamic string-witness restriction via shared raw index {}@{}; empty-selection preservation: {}; exact field residual retained",
                    collection.source.id(),
                    collection.source.version(),
                    structural_candidates.is_some()
                )
            } else if selected.kind == AccessPathKind::Scanner
                && term.kind == LogicalType::Unknown
                && term.path.root == PathRoot::Data
            {
                let inference_reason = if let Some(inference) = &collection.inference {
                    if inference.report.mode == crate::inference::InferenceMode::ReportOnly {
                        "inference is report-only; dynamic predicate uses scanner".into()
                    } else if inference.indexes.contains_key(&term.path.segments) {
                        "inferred posting exists; this operation or current cost favors scanner"
                            .into()
                    } else if let Some(field) = inference
                        .report
                        .fields
                        .iter()
                        .find(|field| field.path == term.path.segments)
                    {
                        format!(
                            "inferred index unavailable ({:?}: {}); scanner selected",
                            field.index_status, field.reason
                        )
                    } else {
                        "no complete inferred index for dynamic field; scanner selected".into()
                    }
                } else {
                    "inference disabled; dynamic predicate uses scanner".into()
                };
                if matches!(term.binding_name.as_str(), "default" | "lower")
                    && term.operation.is_none()
                    && matches!(term.operator, Operator::Contains | Operator::Eq)
                {
                    if let Err(coverage) = raw_covers_identity_field(collection) {
                        format!("raw bigram rejected: {coverage}; {inference_reason}")
                    } else {
                        inference_reason
                    }
                } else {
                    inference_reason
                }
            } else if selected.kind == AccessPathKind::Scanner
                && term.path.root == PathRoot::Data
                && matches!(term.kind, LogicalType::String | LogicalType::Unknown)
                && matches!(term.binding_name.as_str(), "default" | "lower")
                && term.operation.is_none()
                && matches!(term.operator, Operator::Contains | Operator::Eq)
            {
                raw_covers_identity_field(collection)
                    .err()
                    .map(|reason| {
                        format!(
                            "raw bigram rejected: {reason}; dynamic binding: {dynamic_binding}; scanner selected"
                        )
                    })
                    .unwrap_or_else(|| {
                        if term.gram_request.is_none()
                            || term.gram_request.as_ref().is_some_and(|request| {
                                collection.raw.estimate_prepared(request).is_none()
                            })
                        {
                            "raw bigram has no usable necessary grams; scanner selected".into()
                        } else {
                            "lowest estimated lookup plus remaining source work for current input"
                                .into()
                        }
                    })
            } else {
                "lowest estimated lookup plus remaining source work for current input".into()
            };
            let choices = vec![TermChoice {
                span: term.span,
                field: term.query_path.display.clone(),
                logical_type: format!("{:?}", term.kind),
                binding: format!("{}@{}", term.binding_name, term.binding_version),
                exact_case_sensitivity: term_case_sensitivity(term),
                selected,
                alternatives: paths,
                actual_input: input.len(),
                actual_output: candidates.as_ref().map(RoaringBitmap::len),
                reason,
                dynamic_binding,
                gram_candidates,
                structural_candidates,
                residual_verification: !exact,
                executed: true,
            }];
            Plan {
                proven_hits: exact_candidates.cloned().unwrap_or_default(),
                candidates,
                exact,
                proofs,
                choices,
                semijoin_work: Default::default(),
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
                let child = plan_logical_expr(
                    repo,
                    collection,
                    remaining.remove(next),
                    domain,
                    &current,
                    evidence,
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
                proven_hits: if exact {
                    current.clone()
                } else {
                    RoaringBitmap::new()
                },
                candidates: Some(current),
                exact,
                proofs,
                choices,
                semijoin_work: Default::default(),
            }
        }
        BoundExpr::Or(parts) => {
            let mut ordered: Vec<_> = parts.iter().collect();
            ordered.sort_by(|a, b| {
                estimate_expr(collection, a, input.len())
                    .0
                    .total_cmp(&estimate_expr(collection, b, input.len()).0)
            });
            let mut candidates = Some(RoaringBitmap::new());
            let mut proven_hits = RoaringBitmap::new();
            let mut proofs = Vec::new();
            let mut choices = Vec::new();
            let mut all_exact = true;
            for (position, part) in ordered.iter().enumerate() {
                let remaining = if evidence {
                    input.clone()
                } else {
                    input - &proven_hits
                };
                if remaining.is_empty() {
                    for skipped in &ordered[position..] {
                        skipped_choices(collection, skipped, &mut choices);
                    }
                    break;
                }
                let child =
                    plan_logical_expr(repo, collection, part, domain, &remaining, evidence)?;
                all_exact &= child.exact;
                proven_hits |= &child.proven_hits;
                candidates = match (candidates, child.candidates) {
                    (Some(mut union), Some(set)) => {
                        union |= set;


                        Some(union)
                    }
                    _ => None,
                };
                proofs.extend(child.proofs);
                choices.extend(child.choices);
            }
            let exact = all_exact || (!evidence && proven_hits.len() == input.len());
            if !evidence && proven_hits.len() == input.len() {
                candidates = Some(proven_hits.clone());
            }
            Plan {
                candidates,
                exact,
                proven_hits,
                proofs,
                choices,
                semijoin_work: Default::default(),
            }
        }
        BoundExpr::Not(inner) => {
            let child = plan_logical_expr(repo, collection, inner, domain, input, evidence)?;
            if child.exact {
                let matches = input - &child.candidates.unwrap();
                Plan {
                    candidates: Some(matches.clone()),
                    exact: true,
                    proven_hits: matches,
                    proofs: child.proofs,
                    choices: child.choices,
                    semijoin_work: Default::default(),
                }
            } else {
                Plan {
                    candidates: None,
                    exact: false,
                    proven_hits: RoaringBitmap::new(),
                    proofs: Vec::new(),
                    choices: child.choices,
                    semijoin_work: Default::default(),
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
    if !custom_bigram_compatible(term) {
        return None;
    }
    let Value::String(literal) = &term.literal else {
        return None;
    };
    if literal.contains(['%', '\\']) {
        return None;
    }
    custom_text_index(collection, &term.path.segments, &term.binding_name)?
        .candidates_prepared(term.gram_request.as_ref()?)
}

pub(crate) fn fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<RoaringBitmap> {
    if fts.fields.iter().any(|field| field.related.is_some()) {
        return None;
    }
    if matches!(fts.mode, FtsMode::Regex(_)) {
        return regex_fts_candidates(collection, fts);
    }
    if fts.fields.is_empty() {
        let has_fallback =
            collection.logical_records.len() != collection.generation.records.len() as u64;
        if fts
            .raw_matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.bigram_compatible())
        {
            return None;
        }
        if collection
            .definition
            .schema
            .raw
            .as_ref()
            .is_some_and(|binding| !binding.operation.complete_coverage())
        {
            return None;
        }
        let mut valid = collection.raw.candidates_prepared(&fts.gram_request)?;
        if has_fallback {
            valid |= collection
                .fallback_text
                .candidates_prepared(&fts.fallback_gram_request)?;
        }
        return Some(valid);
    }
    let mut union = RoaringBitmap::new();
    for field in &fts.fields {
        union |= fts_field_index(collection, field)?.candidates_prepared(&field.gram_request)?;
    }
    Some(union)
}

pub(crate) fn estimate_fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<(u64, f64)> {
    if fts.fields.iter().any(|field| field.related.is_some()) {
        return None;
    }
    if matches!(fts.mode, FtsMode::Regex(_)) {
        return estimate_regex_fts_candidates(collection, fts);
    }
    if fts.fields.is_empty() {
        if fts
            .raw_matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.bigram_compatible())
        {
            return None;
        }
        if collection
            .definition
            .schema
            .raw
            .as_ref()
            .is_some_and(|binding| !binding.operation.complete_coverage())
        {
            return None;
        }
        let mut count = collection.raw.estimate_prepared(&fts.gram_request)?;
        if collection.logical_records.len() != collection.generation.records.len() as u64 {
            count = count.saturating_add(
                collection
                    .fallback_text
                    .estimate_prepared(&fts.fallback_gram_request)?,
            );
        }
        return Some((
            count.min(collection.stats.record_count as u64),
            fts.literal.len() as f64,
        ));
    }
    let mut count = 0u64;
    let mut work = 0.0;
    for field in &fts.fields {
        let estimate =
            fts_field_index(collection, field)?.estimate_prepared(&field.gram_request)?;
        work += field.literal.len() as f64;
        count = count.saturating_add(estimate);
    }
    Some((count.min(collection.stats.record_count as u64), work))
}

fn regex_fts_candidates(collection: &CollectionState, fts: &BoundFts) -> Option<RoaringBitmap> {
    let condition = fts.fallback_regex.as_ref()?.condition()?;
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
        let mut valid = condition.candidates(&collection.raw)?;
        if collection.logical_records.len() != collection.generation.records.len() as u64 {
            valid |= condition.candidates(&collection.fallback_text)?;
        }
        return Some(valid);
    }
    let mut union = RoaringBitmap::new();
    for field in &fts.fields {
        union |= condition.candidates(fts_field_index(collection, field)?)?;
    }
    Some(union)
}

fn estimate_regex_fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<(u64, f64)> {
    let condition = fts.fallback_regex.as_ref()?.condition()?;
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
        let (mut count, mut work) = condition.estimate(&collection.raw)?;
        if collection.logical_records.len() != collection.generation.records.len() as u64 {
            let (fallback_count, fallback_work) = condition.estimate(&collection.fallback_text)?;
            count = count.saturating_add(fallback_count);
            work += fallback_work;
        }
        return Some((count.min(collection.stats.record_count as u64), work));
    }
    let mut count = 0u64;
    let mut work = 0.0;
    for field in &fts.fields {
        let (field_count, field_work) = condition.estimate(fts_field_index(collection, field)?)?;
        count = count.saturating_add(field_count);
        work += field_work;
    }
    Some((count.min(collection.stats.record_count as u64), work))
}

fn fts_field_index<'a>(
    collection: &'a CollectionState,
    field: &crate::reference::BoundFtsField,
) -> Option<&'a crate::bigram::BigramIndex> {
    if field.path.root != PathRoot::Data
        || field
            .matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.bigram_compatible())
    {
        return None;
    }
    if collection.source.reader().logical_text_coverage() == LogicalTextCoverage::AllStringFields {
        if let Some(index) =
            custom_text_index(collection, &field.path.segments, &field.binding_name)
        {
            return Some(index);
        }
    }
    if matches!(field.binding_name.as_str(), "default" | "lower")
        && field.operation.is_none()
        && raw_covers_identity_field(collection).is_ok()
    {
        return Some(&collection.raw);
    }
    None
}

#[cfg(test)]
#[path = "executor.test.rs"]
mod tests;

#[cfg(test)]
#[path = "dynamic_filter.test.rs"]
mod dynamic_filter_tests;
