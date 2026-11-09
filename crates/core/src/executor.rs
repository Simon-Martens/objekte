//! Exact posting restrictions plus one residual scan over surviving records.

use std::collections::{BTreeMap, HashMap};

use roaring::RoaringBitmap;

use crate::build::IndexedCollectionSegment;
use crate::inventory::ActiveRecords;
use crate::model::{LogicalType, MetadataField, Number, PathRoot, RecordId, Value};
use crate::posting::ScalarKey;
use crate::query::{FtsMode, Operator, Quantifier};
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, Derived, RecordContext, Transform, eval_term,
};
use crate::repository::{CollectionState, Repository};
use crate::sorting::SortKey;
use crate::source::LogicalTextCoverage;
use serde::Serialize;

/// Conservative predicate bounds for one locally slotted delta. `may` is a
/// superset of true records; `must` is a subset. Carrying both through NOT
/// prevents an unavailable or unindexed value from becoming a false proof.
pub(crate) struct SegmentCandidateBounds {
    pub may: RoaringBitmap,
    pub must: RoaringBitmap,
}

impl SegmentCandidateBounds {
    fn unknown(domain: &RoaringBitmap) -> Self {
        Self {
            may: domain.clone(),
            must: RoaringBitmap::new(),
        }
    }
}

pub(crate) fn segment_filter_bounds(
    segment: &IndexedCollectionSegment,
    active: &ActiveRecords,
    expr: &BoundExpr,
    collection: &CollectionState,
) -> Result<SegmentCandidateBounds, String> {
    if active.slot_count() as usize != segment.records.len() {
        return Err("segment active domain length mismatch".into());
    }
    let domain = active.bitmap();
    let logical_active = &domain & &segment.logical_records;
    let fallback_active = &domain - &segment.logical_records;
    Ok(segment_expr_bounds(
        segment,
        expr,
        collection,
        &domain,
        &logical_active,
        &fallback_active,
    ))
}

/// Conservative local bigram candidates for a lifecycle segment. `None`
/// means the caller must keep the whole active domain.
pub(crate) fn segment_fts_candidates(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<RoaringBitmap> {
    if fts.fields.iter().any(|field| field.related.is_some()) {
        return None;
    }
    if matches!(fts.mode, FtsMode::Regex(_)) {
        let condition = fts.fallback_regex.as_ref()?.condition()?;
        if fts.fields.is_empty() {
            if segment.raw.record_count as usize != segment.records.len()
                || collection
                    .definition
                    .schema
                    .raw
                    .as_ref()
                    .is_some_and(|binding| !binding.operation.complete_coverage())
            {
                return None;
            }
            let mut may = condition.candidates(&segment.raw)?;
            if segment.logical_records.len() != segment.records.len() as u64 {
                may |= condition.candidates(&segment.fallback_text)?;
            }
            return Some(may);
        }
        let mut may = RoaringBitmap::new();
        for field in &fts.fields {
            if field.path.root != PathRoot::Data {
                return None;
            }
            let key = if matches!(field.binding_name.as_str(), "default" | "lower") {
                crate::repository::built_in_field_text_key(
                    &collection.definition.schema,
                    &field.path.segments,
                )?
            } else {
                crate::repository::canonical_custom_text_key(
                    &collection.definition.schema,
                    &field.path.segments,
                    &field.binding_name,
                )
            };
            let index = if collection.source.reader().logical_text_coverage()
                == LogicalTextCoverage::AllStringFields
            {
                segment.custom_text.get(&key)
            } else {
                None
            };
            if let Some(index) = index {
                may |= condition.candidates(index)?;
            } else if matches!(field.binding_name.as_str(), "default" | "lower")
                && field.operation.is_none()
                && segment_raw_covers_identity_field(segment, collection)
            {
                may |= condition.candidates(&segment.raw)?;
            } else {
                return None;
            }
        }
        return Some(may);
    }
    if fts.fields.is_empty() {
        if fts
            .raw_matcher
            .as_ref()
            .is_some_and(|matcher| !matcher.bigram_compatible())
            || segment.raw.record_count as usize != segment.records.len()
            || collection
                .definition
                .schema
                .raw
                .as_ref()
                .is_some_and(|binding| !binding.operation.complete_coverage())
        {
            return None;
        }
        let mut may = segment.raw.candidates_prepared(&fts.gram_request)?;
        if segment.logical_records.len() != segment.records.len() as u64 {
            may |= segment
                .fallback_text
                .candidates_prepared(&fts.fallback_gram_request)?;
        }
        return Some(may);
    }
    let mut may = RoaringBitmap::new();
    for field in &fts.fields {
        if field.path.root != PathRoot::Data
            || field
                .matcher
                .as_ref()
                .is_some_and(|matcher| !matcher.bigram_compatible())
        {
            return None;
        }
        let key = if matches!(field.binding_name.as_str(), "default" | "lower") {
            crate::repository::built_in_field_text_key(
                &collection.definition.schema,
                &field.path.segments,
            )?
        } else {
            crate::repository::canonical_custom_text_key(
                &collection.definition.schema,
                &field.path.segments,
                &field.binding_name,
            )
        };
        let index = if collection.source.reader().logical_text_coverage()
            == LogicalTextCoverage::AllStringFields
        {
            segment.custom_text.get(&key)
        } else {
            None
        };
        if let Some(index) = index {
            may |= index.candidates_prepared(&field.gram_request)?;
        } else if matches!(field.binding_name.as_str(), "default" | "lower")
            && field.operation.is_none()
            && segment_raw_covers_identity_field(segment, collection)
        {
            may |= segment.raw.candidates_prepared(&field.gram_request)?;
        } else {
            return None;
        }
    }
    Some(may)
}

fn segment_text_candidates(
    segment: &IndexedCollectionSegment,
    term: &BoundTerm,
    collection: &CollectionState,
) -> Option<RoaringBitmap> {
    if term.derived != Derived::None
        || !matches!(term.operator, Operator::Contains | Operator::Eq)
        || !custom_bigram_compatible(term)
    {
        return None;
    }
    let Value::String(literal) = &term.literal else { return None };
    if literal.contains(['%', '\\']) {
        return None;
    }
    let key = if matches!(term.binding_name.as_str(), "default" | "lower") {
        crate::repository::built_in_field_text_key(&collection.definition.schema, &term.path.segments)
    } else {
        Some(crate::repository::canonical_custom_text_key(
            &collection.definition.schema, &term.path.segments, &term.binding_name,
        ))
    };
    if let Some(index) = key.as_ref().and_then(|key| segment.custom_text.get(key)) {
        return index.candidates_prepared(term.gram_request.as_ref()?);
    }
    if matches!(term.binding_name.as_str(), "default" | "lower")
        && matches!(term.transform, crate::reference::Transform::Identity | crate::reference::Transform::CaseFold)
        && term.operation.is_none()
        && term.matcher.is_none()
        && segment_raw_covers_identity_field(segment, collection)
    {
        return segment.raw.candidates_prepared(term.gram_request.as_ref()?);
    }
    None
}

fn segment_dictionary_contains(
    segment: &IndexedCollectionSegment,
    term: &BoundTerm,
    logical_active: &RoaringBitmap,
) -> Option<RoaringBitmap> {
    if term.kind != LogicalType::String
        || term.transform != Transform::Identity
        || term.operation.is_some()
        || term.derived != Derived::None
        || term.period.is_some()
        || term.operator != Operator::Contains
        || (term.multi && term.quantifier != Quantifier::Any)
    {
        return None;
    }
    let Value::String(literal) = &term.literal else {
        return None;
    };
    if literal.contains(['%', '\\']) {
        return None;
    }
    let posting = segment.postings.fields.get(&term.path.segments)?;
    if posting.covered_records as u64 != segment.logical_records.len() {
        return None;
    }
    let mut matches = RoaringBitmap::new();
    for (key, slots) in &posting.values {
        if let ScalarKey::String(value) = key
            && value.contains(literal)
        {
            matches |= slots & logical_active;
        }
    }
    Some(matches)
}

fn segment_file_size_matches(
    segment: &IndexedCollectionSegment,
    domain: &RoaringBitmap,
    term: &BoundTerm,
) -> Option<RoaringBitmap> {
    if term.path.root != PathRoot::Metadata(MetadataField::FileSize)
        || term.transform != Transform::Identity
        || term.derived != Derived::None
        || term.period.is_some()
        || term.rhs.is_some()
    {
        return None;
    }
    let Value::Number(literal) = &term.literal else {
        return None;
    };
    let mut matches = RoaringBitmap::new();
    for file in &segment.files {
        let order = Number::integer(file.entry.revision.size as i128).cmp(literal);
        if order_matches(order, term.operator) {
            matches.extend(file.records.iter().map(|slot| slot.0));
        }
    }
    matches &= domain;
    Some(matches)
}

fn segment_file_modified_matches(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    domain: &RoaringBitmap,
    term: &BoundTerm,
) -> Option<RoaringBitmap> {
    if term.path.root != PathRoot::Metadata(MetadataField::FileModified) || term.rhs.is_some() {
        return None;
    }
    // Compatible moves retain the indexed revision; changed metadata takes
    // the replacement route. The segment's immutable file revision is exact.
    let empty = Value::Object(BTreeMap::new());
    let mut matches = RoaringBitmap::new();
    for file in &segment.files {
        let Some(first) = file.records.first() else {
            continue;
        };
        let record = segment.records.get(first.0 as usize)?;
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record,
            file: &file.entry,
            collection_name: &collection.definition.name,
            repository_root: std::path::Path::new(""),
        };
        if eval_term(term, &ctx).ok()? {
            matches.extend(file.records.iter().map(|slot| slot.0));
        }
    }
    matches &= domain;
    Some(matches)
}

fn segment_record_range_matches(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    domain: &RoaringBitmap,
    term: &BoundTerm,
) -> Option<RoaringBitmap> {
    if !matches!(
        term.path.root,
        PathRoot::Metadata(
            MetadataField::RecordByteStart
                | MetadataField::RecordByteEnd
                | MetadataField::RecordByteLength
        )
    ) || term.rhs.is_some()
        || term.matcher.is_some()
    {
        return None;
    }
    let empty = Value::Object(BTreeMap::new());
    let mut matches = RoaringBitmap::new();
    for slot in domain.iter() {
        let record = segment.records.get(slot as usize)?;
        let file = &segment.files[*segment.file_index.get(&record.file)?].entry;
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record,
            file,
            collection_name: &collection.definition.name,
            repository_root: std::path::Path::new(""),
        };
        if eval_term(term, &ctx).ok()? {
            matches.insert(slot);
        }
    }
    Some(matches)
}

fn segment_sort_column_matches(
    segment: &IndexedCollectionSegment,
    logical_active: &RoaringBitmap,
    term: &BoundTerm,
) -> Option<RoaringBitmap> {
    if term.derived != Derived::None
        || term.period.is_some()
        || term.multi
        || term.transform != Transform::Identity
        || term.operation.is_some()
        || term.rhs.is_some()
        || !matches!(
            term.kind,
            LogicalType::Number | LogicalType::Date | LogicalType::Datetime | LogicalType::Unixtime
        )
        || !matches!(
            term.operator,
            Operator::Eq | Operator::Ne | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
        )
    {
        return None;
    }
    let literal = sort_key(&term.literal)?;
    let column = segment.sorting.column(&term.path.display()).ok()?;
    if column.keys.len() != segment.records.len() {
        return None;
    }
    let mut matches = RoaringBitmap::new();
    for slot in logical_active.iter() {
        let key = &column.keys[slot as usize];
        if !matches!(key, SortKey::Missing | SortKey::Null)
            && order_matches(key.compare(&literal, false), term.operator)
        {
            matches.insert(slot);
        }
    }
    Some(matches)
}

fn segment_raw_covers_identity_field(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
) -> bool {
    collection.source.reader().logical_text_coverage() == LogicalTextCoverage::AllStringFields
        && collection.definition.schema.raw.is_none()
        && segment.raw.record_count as usize == segment.records.len()
}

fn segment_expr_bounds(
    segment: &IndexedCollectionSegment,
    expr: &BoundExpr,
    collection: &CollectionState,
    domain: &RoaringBitmap,
    logical_active: &RoaringBitmap,
    fallback_active: &RoaringBitmap,
) -> SegmentCandidateBounds {
    match expr {
        BoundExpr::Term(term) => {
            if term.related.is_some()
                || term.rhs.is_some()
                || term.query_path.has_explicit_array_step
                || term.query_path.has_position
            {
                return SegmentCandidateBounds::unknown(domain);
            }
            if term.path.root == PathRoot::Metadata(MetadataField::RecordId)
                && term.operator == Operator::Eq
                && term.transform == crate::reference::Transform::Identity
                && let Value::Id(id) = &term.literal
            {
                let mut matches = RoaringBitmap::new();
                if let Some(slot) = segment.id_lookup.get(&RecordId(id.clone()))
                    && domain.contains(slot.0)
                {
                    matches.insert(slot.0);
                }
                return SegmentCandidateBounds { may: matches.clone(), must: matches };
            }
            if let Some(matches) = segment_file_size_matches(segment, domain, term) {
                return SegmentCandidateBounds {
                    may: matches.clone(),
                    must: matches,
                };
            }
            if let Some(matches) = segment_file_modified_matches(segment, collection, domain, term)
            {
                return SegmentCandidateBounds {
                    may: matches.clone(),
                    must: matches,
                };
            }
            if let Some(matches) = segment_record_range_matches(segment, collection, domain, term)
            {
                return SegmentCandidateBounds {
                    may: matches.clone(),
                    must: matches,
                };
            }
            if term.path.root != PathRoot::Data {
                return SegmentCandidateBounds::unknown(domain);
            }
            let exact = if term.operation.is_some() {
                segment
                    .custom_postings
                    .get(&(term.path.segments.clone(), term.binding_name.clone()))
                    .filter(|posting| posting.complete)
                    .and_then(|posting| {
                        posting.index.exact_set_restricted(
                            term,
                            &segment.logical_records,
                            logical_active,
                        )
                    })
            } else {
                segment
                    .postings
                    .fields
                    .get(&term.path.segments)
                    .and_then(|posting| {
                        posting.exact_set_restricted(term, &segment.logical_records, logical_active)
                    })
            };
            match exact {
                Some(matches) => SegmentCandidateBounds {
                    may: &matches | fallback_active,
                    must: matches,
                },
                None => {
                    if let Some(matches) =
                        segment_dictionary_contains(segment, term, logical_active)
                    {
                        return SegmentCandidateBounds {
                            may: &matches | fallback_active,
                            must: matches,
                        };
                    }
                    if let Some((matches, exact)) =
                        segment.inference.as_ref().and_then(|inference| {
                            inference.candidates(term, &segment.logical_records, logical_active)
                        })
                    {
                        return SegmentCandidateBounds {
                            may: &matches | fallback_active,
                            must: if exact { matches } else { RoaringBitmap::new() },
                        };
                    }
                    if let Some(matches) =
                        segment_sort_column_matches(segment, logical_active, term)
                    {
                        return SegmentCandidateBounds {
                            may: &matches | fallback_active,
                            must: matches,
                        };
                    }
                    if let Some(mut grams) = segment_text_candidates(segment, term, collection) {
                        if empty_selection_preservation(term) {
                            grams |= &segment.empty_array_records;
                        }
                        SegmentCandidateBounds {
                            may: (&grams & domain) | fallback_active,
                            must: RoaringBitmap::new(),
                        }
                    } else {
                        SegmentCandidateBounds::unknown(domain)
                    }
                }
            }
        }
        BoundExpr::And(parts) => {
            let mut may = domain.clone();
            let mut must = domain.clone();
            for part in parts {
                let child =
                    segment_expr_bounds(segment, part, collection, domain, logical_active, fallback_active);
                may &= child.may;
                must &= child.must;
            }
            SegmentCandidateBounds { may, must }
        }
        BoundExpr::Or(parts) => {
            let mut may = RoaringBitmap::new();
            let mut must = RoaringBitmap::new();
            for part in parts {
                let child =
                    segment_expr_bounds(segment, part, collection, domain, logical_active, fallback_active);
                may |= child.may;
                must |= child.must;
            }
            SegmentCandidateBounds { may, must }
        }
        BoundExpr::Not(inner) => {
            let child =
                segment_expr_bounds(segment, inner, collection, domain, logical_active, fallback_active);
            SegmentCandidateBounds {
                may: domain - &child.must,
                must: domain - &child.may,
            }
        }
    }
}


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
    /// Set only by estimate-only EXPLAIN; `actual_input` is then unavailable.
    pub estimated_input: Option<u64>,
    pub actual_output: Option<u64>,
    pub reason: String,
    pub dynamic_binding: bool,
    pub gram_candidates: Option<u64>,
    pub structural_candidates: Option<u64>,
    pub residual_verification: bool,
    pub executed: bool,
}


/// Work done to choose and execute access paths for one filter expression.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct PlanningCounters {
    pub term_estimation_calls: u64,
    pub dictionary_keys_visited: u64,
    pub candidate_bitmap_builds: u64,
    pub reused_term_estimates: u64,
    pub reused_order_plans: u64,
}


#[derive(Default)]
struct PlanContext {
    paths: HashMap<usize, Vec<AccessPathEstimate>>,
    counters: PlanningCounters,
    diagnostics: bool,
}


impl PlanContext {
    fn paths(
        &mut self,
        collection: &CollectionState,
        term: &BoundTerm,
        input_len: u64,
    ) -> Vec<AccessPathEstimate> {
        let key = term as *const BoundTerm as usize;
        if let Some(paths) = self.paths.get(&key) {
            self.counters.reused_term_estimates += 1;
            return rescore_paths(collection, term, paths, input_len);
        }
        self.counters.term_estimation_calls += 1;
        let paths = enumerate_paths(
            collection,
            term,
            input_len,
            &mut self.counters.dictionary_keys_visited,
        );
        self.paths.insert(key, paths.clone());
        paths
    }
}


fn rescore_paths(
    collection: &CollectionState,
    term: &BoundTerm,
    paths: &[AccessPathEstimate],
    input_len: u64,
) -> Vec<AccessPathEstimate> {
    let stats = collection.stats;
    let source_unit = 30.0 + stats.mean_record_bytes() / 128.0;
    paths
        .iter()
        .map(|original| {
            let mut path = original.clone();
            let original_unit = if path.exact { 0.02 } else { source_unit };
            let lookup = path.estimated_work - path.estimated_output as f64 * original_unit;
            path.estimated_output = match path.kind {
                AccessPathKind::Scanner => input_len,
                AccessPathKind::Dictionary | AccessPathKind::SortColumn => input_len / 4,
                AccessPathKind::Metadata
                    if term.path.root == PathRoot::Metadata(MetadataField::RecordId)
                        && term.operator == Operator::Eq =>
                {
                    input_len.min(1)
                }
                AccessPathKind::Metadata => input_len / 2,
                _ => path.estimated_output.min(input_len),
            };
            path.estimated_work = lookup + path.estimated_output as f64 * original_unit;
            path.estimated_files = (path.estimated_output as f64
                / stats.mean_records_per_file().max(1.0))
            .min(stats.file_count as f64);
            path.estimated_bytes = path.estimated_output as f64 * stats.mean_record_bytes();
            path
        })
        .collect()
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
            let mut paths = enumerate_paths(collection, term, 0, &mut 0);
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
                estimated_input: None,
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
                estimated_input: None,
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
    pub planning: PlanningCounters,
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
    dictionary_keys_visited: &mut u64,
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
            if let Some((lookup, output)) = index.estimate_exact_with_visits(
                term,
                collection.logical_records.len(),
                dictionary_keys_visited,
            ) {
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
            if let Some((lookup, output, exact)) = inference.estimate_candidates(
                term,
                &collection.logical_records,
                dictionary_keys_visited,
            ) {
                add(AccessPathKind::InferredPosting, exact, lookup, output);
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
                            let output = gram_count
                                .saturating_add(structural_count.unwrap_or(0))
                                .min(input_len);
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
            // Coverage describes every indexed logical slot, including retired
            // incarnations. The input already excludes retired slots.
            let coverage_domain = &collection.logical_records;
            if term.operation.is_some() {
                collection
                    .custom_postings
                    .get(&(term.path.segments.clone(), term.binding_name.clone()))
                    .filter(|posting| posting.complete)
                    .and_then(|posting| {
                        posting
                            .index
                            .exact_set_restricted(term, coverage_domain, input)
                    })
            } else {
                collection
                    .postings
                    .fields
                    .get(&term.path.segments)
                    .and_then(|index| index.exact_set_restricted(term, coverage_domain, input))
            }
        }
        AccessPathKind::InferredPosting => collection
            .inference
            .as_ref()
            .and_then(|inference| inference.candidates(term, &collection.logical_records, input))
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
    estimate_expr_cached(collection, expr, input_len, &mut PlanContext::default())
}


/// An estimate-only physical decision. It has no candidate membership or
/// exact proof and is safe to use for non-analyzing EXPLAIN.
pub(crate) struct EstimatedPlan {
    pub candidates: u64,
    pub exact: bool,
    pub choices: Vec<TermChoice>,
    pub planning: PlanningCounters,
}


struct EstimatedNode {
    candidates: u64,
    exact: bool,
    work: f64,
    choices: Vec<TermChoice>,
}


pub(crate) fn estimate_plan_expr(
    collection: &CollectionState,
    expr: &BoundExpr,
    input_len: u64,
) -> EstimatedPlan {
    let mut context = PlanContext {
        diagnostics: true,
        ..PlanContext::default()
    };
    let node = estimate_plan_node(collection, expr, input_len, &mut context);
    EstimatedPlan {
        candidates: node.candidates,
        exact: node.exact,
        choices: node.choices,
        planning: context.counters,
    }
}

fn estimate_plan_node(
    collection: &CollectionState,
    expr: &BoundExpr,
    input_len: u64,
    context: &mut PlanContext,
) -> EstimatedNode {
    match expr {
        BoundExpr::Term(term) => {
            let mut paths = context.paths(collection, term, input_len);
            paths.sort_by(|a, b| {
                a.estimated_work
                    .total_cmp(&b.estimated_work)
                    .then_with(|| (a.kind as u8).cmp(&(b.kind as u8)))
            });
            let selected = paths.remove(0);
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
            let reason = if selected.kind == AccessPathKind::Scanner
                && term.path.root == PathRoot::Data
                && matches!(term.kind, LogicalType::String | LogicalType::Unknown)
                && matches!(term.binding_name.as_str(), "default" | "lower")
                && term.operation.is_none()
                && matches!(term.operator, Operator::Contains | Operator::Eq)
            {
                raw_covers_identity_field(collection)
                    .err()
                    .map(|coverage| format!("raw bigram rejected: {coverage}; estimate only"))
                    .unwrap_or_else(|| "estimate only; scanner selected".into())
            } else {
                "estimate only; selected path and candidates were not executed".into()
            };
            EstimatedNode {
                candidates: selected.estimated_output,
                exact: selected.exact,
                work: selected.estimated_work,
                choices: vec![TermChoice {
                    span: term.span,
                    field: term.query_path.display.clone(),
                    logical_type: format!("{:?}", term.kind),
                    binding: format!("{}@{}", term.binding_name, term.binding_version),
                    exact_case_sensitivity: term_case_sensitivity(term),
                    residual_verification: !selected.exact,
                    selected,
                    alternatives: paths,
                    actual_input: 0,
                    estimated_input: Some(input_len),
                    actual_output: None,
                    reason,
                    dynamic_binding: term.kind == LogicalType::Unknown,
                    gram_candidates,
                    structural_candidates,
                    executed: false,
                }],
            }
        }
        BoundExpr::And(parts) => {
            let mut remaining = Vec::new();
            for part in parts {
                flatten_and(part, &mut remaining);
            }
            let mut current = input_len;
            let mut exact = true;
            let mut work = 0.0;
            let mut choices = Vec::new();
            while !remaining.is_empty() {
                let source_unit = 30.0 + collection.stats.mean_record_bytes() / 128.0;
                let next = remaining
                    .iter()
                    .enumerate()
                    .map(|(index, part)| {
                        let (estimated_work, output) =
                            estimate_expr_cached(collection, part, current, context);
                        (index, estimated_work + output as f64 * source_unit * 0.1)
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
                    .unwrap()
                    .0;
                let child =
                    estimate_plan_node(collection, remaining.remove(next), current, context);
                current = current.min(child.candidates);
                exact &= child.exact;
                work += child.work;
                choices.extend(child.choices);
            }
            EstimatedNode {
                candidates: current,
                exact,
                work,
                choices,
            }
        }
        BoundExpr::Or(parts) => {
            let mut ordered = parts
                .iter()
                .enumerate()
                .map(|(position, part)| {
                    (
                        position,
                        estimate_expr_cached(collection, part, input_len, context).0,
                        part,
                    )
                })
                .collect::<Vec<_>>();
            ordered.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            let mut candidates = 0u64;
            let mut exact = true;
            let mut work = 0.0;
            let mut choices = Vec::new();
            for (_, _, part) in ordered {
                let child = estimate_plan_node(collection, part, input_len, context);
                candidates = candidates.saturating_add(child.candidates).min(input_len);
                exact &= child.exact;
                work += child.work;
                choices.extend(child.choices);
            }
            EstimatedNode {
                candidates,
                exact,
                work,
                choices,
            }
        }
        BoundExpr::Not(inner) => {
            let mut child = estimate_plan_node(collection, inner, input_len, context);
            child.candidates = if child.exact {
                input_len.saturating_sub(child.candidates)
            } else {
                input_len
            };
            child
        }
    }
}

fn estimate_expr_cached(
    collection: &CollectionState,
    expr: &BoundExpr,
    input_len: u64,
    context: &mut PlanContext,
) -> (f64, u64) {
    match expr {
        BoundExpr::Term(term) => context
            .paths(collection, term, input_len)
            .into_iter()
            .min_by(|a, b| a.estimated_work.total_cmp(&b.estimated_work))
            .map(|p| (p.estimated_work, p.estimated_output))
            .unwrap_or((input_len as f64, input_len)),
        BoundExpr::And(parts) => parts
            .iter()
            .map(|p| estimate_expr_cached(collection, p, input_len, context))
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap_or((0.0, input_len)),
        BoundExpr::Or(parts) => {
            let estimates: Vec<_> = parts
                .iter()
                .map(|p| estimate_expr_cached(collection, p, input_len, context))
                .collect();
            (
                estimates.iter().map(|e| e.0).sum(),
                estimates.iter().map(|e| e.1).sum::<u64>().min(input_len),
            )
        }
        BoundExpr::Not(inner) => {
            let (work, _) = estimate_expr_cached(collection, inner, input_len, context);
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
    plan_expr_restricted_with_execution(repo, collection, expr, domain, input, evidence, true, None)
}

pub(crate) fn plan_expr_restricted_with_execution(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    evidence: bool,
    diagnostics: bool,
    execution: Option<&crate::streaming::ExecutionContext>,
) -> Result<Plan, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    let mut context = PlanContext {
        diagnostics,
        ..PlanContext::default()
    };
    let mut plan = plan_expr_restricted_inner(
        repo,
        collection,
        expr,
        domain,
        input,
        evidence,
        &mut context,
        execution,
    )?;
    plan.planning = context.counters;
    Ok(plan)
}

fn plan_expr_restricted_inner(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    evidence: bool,
    context: &mut PlanContext,
    execution: Option<&crate::streaming::ExecutionContext>,
) -> Result<Plan, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    if input.is_empty() {
        return Ok(Plan {
            candidates: Some(RoaringBitmap::new()),
            exact: true,
            proven_hits: RoaringBitmap::new(),
            proofs: Vec::new(),
            choices: Vec::new(),
            semijoin_work: Default::default(),
            planning: PlanningCounters::default(),
        });
    }
    if collection.logical_records.len() == collection.generation.records.len() as u64 {
        let mut plan = plan_logical_expr(repo, collection, expr, domain, input, evidence, context)?;
        if !collection.relations.is_empty() {
            crate::relation_semijoin::restrict(repo, expr, input, &mut plan, execution)?;
        }
        return Ok(plan);
    }
    let logical_domain = domain & &collection.logical_records;
    let logical_input = input & &logical_domain;
    let fallback_input = input - &collection.logical_records;
    let mut logical = if logical_input.is_empty() {
        Plan {
            candidates: Some(RoaringBitmap::new()),
            exact: true,
            proven_hits: RoaringBitmap::new(),
            proofs: Vec::new(),
            choices: Vec::new(),
            semijoin_work: Default::default(),
            planning: PlanningCounters::default(),
        }
    } else {
        plan_logical_expr(
            repo,
            collection,
            expr,
            &logical_domain,
            &logical_input,
            evidence,
            context,
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
        crate::relation_semijoin::restrict(repo, expr, input, &mut logical, execution)?;
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
    context: &mut PlanContext,
) -> Result<Plan, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let mut paths = context.paths(collection, term, input.len());
            paths.sort_by(|a, b| {
                a.estimated_work
                    .total_cmp(&b.estimated_work)
                    .then_with(|| (a.kind as u8).cmp(&(b.kind as u8)))
            });
            let selected = paths.remove(0);
            context.counters.candidate_bitmap_builds +=
                u64::from(selected.kind != AccessPathKind::Scanner);
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
            let choices = if context.diagnostics {
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
                        "complete inferred posting proves the dynamic presence/null predicate"
                            .into()
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
                vec![TermChoice {
                    span: term.span,
                    field: term.query_path.display.clone(),
                    logical_type: format!("{:?}", term.kind),
                    binding: format!("{}@{}", term.binding_name, term.binding_version),
                    exact_case_sensitivity: term_case_sensitivity(term),
                    selected,
                    alternatives: paths,
                    actual_input: input.len(),
                    estimated_input: None,
                    actual_output: candidates.as_ref().map(RoaringBitmap::len),
                    reason,
                    dynamic_binding,
                    gram_candidates,
                    structural_candidates,
                    residual_verification: !exact,
                    executed: true,
                }]
            } else {
                Vec::new()
            };
            Plan {
                proven_hits: exact_candidates.cloned().unwrap_or_default(),
                candidates,
                exact,
                proofs,
                choices,
                semijoin_work: Default::default(),
                planning: PlanningCounters::default(),
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
                        let (work, output) =
                            estimate_expr_cached(collection, part, current.len(), context);
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
                    context,
                )?;
                if let Some(set) = child.candidates {
                    current &= set;
                }
                exact &= child.exact;
                proofs.extend(child.proofs);
                choices.extend(child.choices);
                if current.is_empty() {
                    exact = true;
                    if context.diagnostics {
                        for part in remaining {
                            skipped_choices(collection, part, &mut choices);
                        }
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
                planning: PlanningCounters::default(),
            }
        }
        BoundExpr::Or(parts) => {
            let mut ordered: Vec<_> = parts
                .iter()
                .enumerate()
                .map(|(position, part)| {
                    (
                        position,
                        estimate_expr_cached(collection, part, input.len(), context).0,
                        part,
                    )
                })
                .collect();
            ordered.sort_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
            let mut candidates = Some(RoaringBitmap::new());
            let mut proven_hits = RoaringBitmap::new();
            let mut proofs = Vec::new();
            let mut choices = Vec::new();
            let mut all_exact = true;
            for (position, (_, _, part)) in ordered.iter().enumerate() {
                let remaining = if evidence {
                    input.clone()
                } else {
                    input - &proven_hits
                };
                if remaining.is_empty() {
                    if context.diagnostics {
                        for (_, _, skipped) in &ordered[position..] {
                            skipped_choices(collection, skipped, &mut choices);
                        }
                    }
                    break;
                }
                let child = plan_logical_expr(
                    repo, collection, part, domain, &remaining, evidence, context,
                )?;
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
                planning: PlanningCounters::default(),
            }
        }
        BoundExpr::Not(inner) => {
            let child =
                plan_logical_expr(repo, collection, inner, domain, input, evidence, context)?;
            if child.exact {
                let matches = input - &child.candidates.unwrap();
                Plan {
                    candidates: Some(matches.clone()),
                    exact: true,
                    proven_hits: matches,
                    proofs: child.proofs,
                    choices: child.choices,
                    semijoin_work: Default::default(),
                    planning: PlanningCounters::default(),
                }
            } else {
                Plan {
                    candidates: None,
                    exact: false,
                    proven_hits: RoaringBitmap::new(),
                    proofs: Vec::new(),
                    choices: child.choices,
                    semijoin_work: Default::default(),
                    planning: PlanningCounters::default(),
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
