//! Predicate-owned source projections, residual programs and direct-source eligibility.

use std::collections::HashSet;

use roaring::RoaringBitmap;
use smallvec::SmallVec;

use crate::executor::{Proof, estimate_expr};
use crate::model::{LogicalType, PathRoot};
use crate::query::{Operator, Query};
use crate::query_results::ranking::ResolvedSort;
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, Derived, ExactInput, RecordContext, Transform, same_bound_term,
};
use crate::repository::{CollectionState, Repository};
use crate::source::ScanProjection;

pub(crate) fn scan_projection(
    query: &Query,
    residual: Option<&Residual<'_>>,
    candidates: &RoaringBitmap,
    fts_proven: bool,
) -> ScanProjection {
    let collect_text = query.fts.is_some() && !fts_proven;
    fn uses_root(expr: &Residual<'_>) -> bool {
        match expr {
            Residual::Term(term, _, _, _) => {
                (term.path.root == PathRoot::Data && term.path.segments.is_empty())
                    || term.rhs.as_ref().is_some_and(|rhs| {
                        rhs.path.root == PathRoot::Data && rhs.path.segments.is_empty()
                    })
            }
            Residual::And(parts) | Residual::Or(parts) => parts.iter().any(uses_root),
            Residual::Not(inner) => uses_root(inner),
        }
    }
    if residual.is_some_and(uses_root)
        || query.fts.as_ref().is_some_and(|fts| {
            fts.fields
                .iter()
                .any(|field| field.path.root == PathRoot::Data && field.path.segments.is_empty())
        })
    {
        let mut projection = ScanProjection::full(collect_text);
        projection.resolved_text_only =
            collect_text && query.fts.as_ref().is_some_and(|fts| !fts.fields.is_empty());
        return projection;
    }
    if !fts_proven && query.fts.as_ref().is_some_and(|fts| fts.fields.is_empty()) {
        return ScanProjection::full(true);
    }

    fn add_filter_fields(
        expr: &Residual<'_>,
        candidates: &RoaringBitmap,
        fields: &mut HashSet<String>,
    ) {
        match expr {
            Residual::Term(term, proof, _, _) => {
                let fully_proven = proof.is_some_and(|proof| candidates.is_subset(&proof.input));
                if !fully_proven
                    && term.path.root == PathRoot::Data
                    && !term
                        .matcher
                        .as_ref()
                        .is_some_and(|matcher| matcher.input() == ExactInput::OriginalRecordBytes)
                {
                    if let Some(first) = term.path.segments.first() {
                        fields.insert(first.clone());
                    }
                }
                if let Some(related) = &term.related
                    && let Some(source) = &related.source_operand
                    && source.path.root == PathRoot::Data
                    && let Some(first) = source.path.segments.first()
                {
                    fields.insert(first.clone());
                }
                if !fully_proven
                    && let Some(rhs) = &term.rhs
                    && rhs.path.root == PathRoot::Data
                    && let Some(first) = rhs.path.segments.first()
                {
                    fields.insert(first.clone());
                }
            }
            Residual::And(parts) | Residual::Or(parts) => {
                for part in parts {
                    add_filter_fields(part, candidates, fields);
                }
            }
            Residual::Not(inner) => add_filter_fields(inner, candidates, fields),
        }
    }
    let mut fields = HashSet::new();
    if let Some(residual) = residual {
        add_filter_fields(residual, candidates, &mut fields);
    }
    if let Some(fts) = &query.fts
        && !fts_proven
    {
        for field in &fts.fields {
            if field.path.root == PathRoot::Data {
                if let Some(first) = field.path.segments.first() {
                    fields.insert(first.clone());
                }
            }
        }
    }
    ScanProjection {
        top_level: Some(fields),
        collect_text,
        resolved_text_only: collect_text
            && query.fts.as_ref().is_some_and(|fts| !fts.fields.is_empty()),
    }
}

/// A segmented residual without FTS or computed sorting needs only its local
/// top-level data fields. Related terminal fields are read from targets, while
/// a source operand is explicitly retained here.
pub(crate) fn segment_filter_source_fields(expr: &BoundExpr) -> Option<HashSet<String>> {
    fn add_path(path: &crate::model::FieldPath, fields: &mut HashSet<String>) -> Option<()> {
        if path.root == PathRoot::Data {
            fields.insert(path.segments.first()?.clone());
        }
        Some(())
    }

    fn collect(expr: &BoundExpr, fields: &mut HashSet<String>) -> Option<()> {
        match expr {
            BoundExpr::Term(term) => {
                add_path(&term.path, fields)?;
                if let Some(rhs) = &term.rhs {
                    add_path(&rhs.path, fields)?;
                }
                if let Some(source) = term
                    .related
                    .as_ref()
                    .and_then(|related| related.source_operand.as_ref())
                {
                    add_path(&source.path, fields)?;
                }
            }
            BoundExpr::And(parts) | BoundExpr::Or(parts) => {
                for part in parts {
                    collect(part, fields)?;
                }
            }
            BoundExpr::Not(inner) => collect(inner, fields)?,
        }
        Some(())
    }
    let mut fields = HashSet::new();
    collect(expr, &mut fields)?;
    Some(fields)
}

pub(crate) fn segment_filter_evidence_bytes(expr: &BoundExpr) -> Option<usize> {
    match expr {
        BoundExpr::Term(term) => {
            let mut bytes = term.query_path.display.len();
            if let Some(rhs) = &term.rhs {
                bytes = bytes.checked_add(rhs.query_path.display.len())?;
            }
            if let Some(related) = &term.related {
                bytes = bytes.checked_add(related.evidence_path.len())?;
            }
            Some(bytes)
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            parts.iter().try_fold(0usize, |sum, part| {
                sum.checked_add(segment_filter_evidence_bytes(part)?)
            })
        }
        BoundExpr::Not(inner) => segment_filter_evidence_bytes(inner),
    }
}

pub(crate) fn local_segment_projection(
    collection: &CollectionState,
    filter: Option<&BoundExpr>,
    fts: Option<&BoundFts>,
    computed_sort: bool,
) -> ScanProjection {
    let mut projection =
        ScanProjection::full(fts.is_some() && !borrowed_value_text(collection, fts));
    if filter.is_none()
        && !computed_sort
        && matches!(collection.source.id(), "json" | "jsonl")
        && let Some(fts) = fts
        && !fts.fields.is_empty()
        && fts.fields.iter().all(|field| {
            field.related.is_none()
                && field.path.root == PathRoot::Data
                && !field.path.segments.is_empty()
        })
    {
        projection.top_level = Some(
            fts.fields
                .iter()
                .map(|field| field.path.segments[0].clone())
                .collect(),
        );
        projection.resolved_text_only = projection.collect_text;
    }
    if fts.is_none()
        && !computed_sort
        && matches!(collection.source.id(), "json" | "jsonl")
        && let Some(fields) = filter.and_then(segment_filter_source_fields)
    {
        projection.top_level = Some(fields);
    }
    projection
}

pub(crate) fn borrowed_value_text(collection: &CollectionState, fts: Option<&BoundFts>) -> bool {
    !collection.source.is_extension()
        && matches!(collection.source.id(), "json" | "jsonl")
        && fts.is_some_and(|fts| fts.fields.iter().all(|field| field.related.is_none()))
}

pub(crate) fn borrowed_relation_source_text(
    collection: &CollectionState,
    fts: Option<&BoundFts>,
) -> bool {
    !collection.source.is_extension()
        && matches!(collection.source.id(), "json" | "jsonl")
        && fts.is_some_and(|fts| {
            fts.fields.iter().any(|field| field.related.is_some())
                && fts
                    .fields
                    .iter()
                    .filter(|field| field.related.is_none())
                    .all(|field| {
                        matches!(field.path.root, PathRoot::Data) && field.query_path.is_none()
                    })
        })
}

pub(crate) fn prepare_value_text_projection(projection: &mut ScanProjection, borrow_text: bool) {
    if borrow_text {
        projection.collect_text = false;
        projection.resolved_text_only = false;
    }
}

pub(crate) enum Residual<'a> {
    Term(
        &'a BoundTerm,
        Option<&'a Proof>,
        crate::query_predicate::TermId,
        &'a crate::query_predicate::PreparedPredicates,
    ),
    And(Vec<Residual<'a>>),
    Or(Vec<Residual<'a>>),
    Not(Box<Residual<'a>>),
}

/// Collection-plan eligibility for matching trusted built-in source scalars.
#[derive(Clone, Copy)]
pub(crate) struct DirectSourcePlan<'a> {
    pub(crate) fts: Option<&'a BoundFts>,
    pub(crate) filter: Option<(&'a BoundTerm, Option<&'a Proof>)>,
    pub(crate) decoded_scalars: bool,
    pub(crate) original_record_matcher: Option<&'a BoundTerm>,
}

pub(crate) fn source_fts_eligible(
    collection: &CollectionState,
    fts: &BoundFts,
    decoded_scalars: bool,
) -> bool {
    fts.mode.is_literal()
        && fts.raw_operation.is_none()
        && fts.raw_matcher.is_none()
        && (decoded_scalars || !collection.definition.schema.has_converted_fields())
        && fts.fields.iter().all(|field| {
            matches!(field.path.root, PathRoot::Data)
                && field.related.is_none()
                && field.operation.is_none()
                && field.matcher.is_none()
                && !field
                    .query_path
                    .as_ref()
                    .is_some_and(|path| path.has_explicit_array_step)
        })
}

impl<'a> DirectSourcePlan<'a> {
    pub(crate) fn prepare(
        collection: &CollectionState,
        sort: &ResolvedSort<'_>,
        fts: Option<&'a BoundFts>,
        residual: Option<&'a Residual<'a>>,
    ) -> Option<Self> {
        #[cfg(test)]
        if crate::query_predicate::alternate_matcher_active() {
            return None;
        }
        let filter = match residual {
            None => None,
            Some(Residual::Term(term, proof, _, _)) => Some((*term, *proof)),
            Some(_) => return None,
        };
        Self::prepare_filter(collection, sort, fts, filter)
    }

    pub(crate) fn prepare_bound(
        collection: &CollectionState,
        sort: &ResolvedSort<'_>,
        fts: Option<&'a BoundFts>,
        filter: Option<&'a BoundExpr>,
    ) -> Option<Self> {
        let filter = match filter {
            None => None,
            Some(BoundExpr::Term(term)) => Some((term, None)),
            Some(_) => return None,
        };
        Self::prepare_filter(collection, sort, fts, filter)
    }

    fn prepare_filter(
        collection: &CollectionState,
        sort: &ResolvedSort<'_>,
        fts: Option<&'a BoundFts>,
        filter: Option<(&'a BoundTerm, Option<&'a Proof>)>,
    ) -> Option<Self> {
        if sort.bound.has_computed {
            return None;
        }
        let decoded_scalars = match collection.source.id() {
            "json_decoded_scalars" => Some(true),
            "json" | "jsonl" => Some(false),
            _ => None,
        };
        if decoded_scalars.is_none() {
            if fts.is_some() {
                return None;
            }
            let Some((term, proof)) = filter else {
                return None;
            };
            if term.path.root != PathRoot::Data
                || term.related.is_some()
                || term.rhs.is_some()
                || term.matcher.as_ref().is_none_or(|matcher| {
                    matcher.input() != crate::reference::ExactInput::OriginalRecordBytes
                })
            {
                return None;
            }
            return Some(Self {
                fts: None,
                filter: Some((term, proof)),
                decoded_scalars: false,
                original_record_matcher: Some(term),
            });
        }
        let decoded_scalars = decoded_scalars.unwrap();
        if let Some(fts) = fts {
            if !source_fts_eligible(collection, fts, decoded_scalars) {
                return None;
            }
        }
        let filter = match filter {
            None => None,
            Some((term, proof))
                if matches!(term.path.root, PathRoot::Data)
                    && term.related.is_none()
                    && term.rhs.is_none()
                    && !term.query_path.has_explicit_array_step
                    && (!term.multi || term.source_kind == LogicalType::Unknown)
                    && !term.implicit_array
                    && matches!(term.source_kind, LogicalType::String | LogicalType::Unknown)
                    && matches!(term.operator, Operator::Contains | Operator::NotContains)
                    && term.derived == Derived::None
                    && (term.prepared_contains.is_some()
                        || term.matcher.as_ref().is_some_and(|matcher| {
                            matcher.input() == crate::reference::ExactInput::SelectedValue
                        })) =>
            {
                Some((term, proof))
            }
            Some(_) => return None,
        };
        (fts.is_some() || filter.is_some()).then_some(Self {
            fts,
            filter,
            decoded_scalars,
            original_record_matcher: None,
        })
    }
}

pub(crate) fn prepare_residual<'a>(
    expr: &'a BoundExpr,
    proofs: &'a [Proof],
    collection: &crate::repository::CollectionState,
    input_len: u64,
    predicates: &'a crate::query_predicate::PreparedPredicates,
) -> Residual<'a> {
    match expr {
        BoundExpr::Term(term) => {
            let term_id = predicates
                .term_for_span(collection.generation.collection.0 as usize, term.span)
                .expect("prepared residual term ID");
            Residual::Term(
                term,
                proofs
                    .iter()
                    .find(|proof| same_bound_term(&proof.term, term)),
                term_id,
                predicates,
            )
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            let is_and = matches!(expr, BoundExpr::And(_));
            let mut ordered = parts
                .iter()
                .enumerate()
                .map(|(index, part)| {
                    // Residual ordering is advisory. The checked execution
                    // path reports any corrupt posting if that access wins.
                    let (cost, output) = estimate_expr(collection, part, input_len)
                        .unwrap_or((input_len as f64, input_len));
                    let fraction = output as f64 / input_len.max(1) as f64;
                    let probability = if is_and { 1.0 - fraction } else { fraction };
                    let score = cost / probability.max(0.05);
                    (
                        score,
                        index,
                        prepare_residual(part, proofs, collection, input_len, predicates),
                    )
                })
                .collect::<Vec<_>>();
            ordered.sort_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            let children = ordered.into_iter().map(|(_, _, child)| child).collect();
            if is_and {
                Residual::And(children)
            } else {
                Residual::Or(children)
            }
        }
        BoundExpr::Not(inner) => Residual::Not(Box::new(prepare_residual(
            inner, proofs, collection, input_len, predicates,
        ))),
    }
}

type FoldKey<'a> = (&'a [String], &'a str, u32);

fn fold_key(term: &BoundTerm) -> Option<FoldKey<'_>> {
    (term.path.root == PathRoot::Data
        && term.transform == Transform::CaseFold
        && term.rhs.is_none()
        && term.related.is_none()
        && term.operation.is_none()
        && term.matcher.is_none()
        && term.derived == Derived::None)
        .then_some((
            &term.path.segments,
            term.binding_name.as_str(),
            term.binding_version,
        ))
}

fn repeated_fold_key<'a>(term: &'a BoundTerm, seen: &mut SmallVec<[FoldKey<'a>; 8]>) -> bool {
    let Some(key) = fold_key(term) else {
        return false;
    };
    if seen.contains(&key) {
        return true;
    }
    seen.push(key);
    false
}

pub(crate) fn shared_fold_residual(residual: Option<&Residual<'_>>) -> bool {
    fn visit<'a>(node: &'a Residual<'a>, seen: &mut SmallVec<[FoldKey<'a>; 8]>) -> bool {
        match node {
            Residual::Term(term, _, _, _) => repeated_fold_key(term, seen),
            Residual::And(parts) | Residual::Or(parts) => {
                parts.iter().any(|part| visit(part, seen))
            }
            Residual::Not(inner) => visit(inner, seen),
        }
    }
    let mut seen = SmallVec::new();
    residual.is_some_and(|node| visit(node, &mut seen))
}

pub(crate) fn shared_fold_bound(filter: Option<&BoundExpr>) -> bool {
    fn visit<'a>(node: &'a BoundExpr, seen: &mut SmallVec<[FoldKey<'a>; 8]>) -> bool {
        match node {
            BoundExpr::Term(term) => repeated_fold_key(term, seen),
            BoundExpr::And(parts) | BoundExpr::Or(parts) => {
                parts.iter().any(|part| visit(part, seen))
            }
            BoundExpr::Not(inner) => visit(inner, seen),
        }
    }
    let mut seen = SmallVec::new();
    filter.is_some_and(|node| visit(node, &mut seen))
}

impl Residual<'_> {
    pub(crate) fn unavailable(
        &self,
        ctx: &RecordContext<'_>,
        evidence: &mut HashSet<String>,
        evaluations: &mut u64,
    ) -> Result<Option<bool>, String> {
        match self {
            Self::Term(term, _, term_id, predicates) => {
                if term.related.is_some()
                    || term.path.root == PathRoot::Data
                    || term.path.root == PathRoot::Backlinks
                    || term
                        .rhs
                        .as_ref()
                        .is_some_and(|rhs| rhs.path.root == PathRoot::Data)
                {
                    return Ok(None);
                }
                *evaluations += 1;
                let mut matched_fields = HashSet::new();
                let matched = match predicates.verify(
                    *term_id,
                    &crate::query_predicate::RecordInput::new(ctx, &[]),
                    &mut crate::query_predicate::EvidenceSink::new(&mut matched_fields),
                )? {
                    crate::query_predicate::Truth::True => true,
                    crate::query_predicate::Truth::False => false,
                    crate::query_predicate::Truth::Unknown => return Ok(None),
                };
                if matched && self.can_emit_evidence() {
                    evidence.extend(matched_fields);
                    evidence.insert(term.query_path.display.clone());
                    if let Some(rhs) = &term.rhs {
                        evidence.insert(rhs.query_path.display.clone());
                    }
                }
                Ok(Some(matched))
            }
            Self::Not(inner) => Ok(inner
                .unavailable(ctx, &mut HashSet::new(), evaluations)?
                .map(|v| !v)),
            Self::And(parts) => {
                let mut unknown = false;
                let mut branch = HashSet::new();
                for part in parts {
                    match part.unavailable(ctx, &mut branch, evaluations)? {
                        Some(false) => return Ok(Some(false)),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown {
                    Ok(None)
                } else {
                    evidence.extend(branch);
                    Ok(Some(true))
                }
            }
            Self::Or(parts) => {
                let mut unknown = false;
                let mut matched = false;
                for part in parts {
                    let mut branch = HashSet::new();
                    match part.unavailable(ctx, &mut branch, evaluations)? {
                        Some(true) => {
                            matched = true;
                            evidence.extend(branch);
                        }
                        None => unknown = true,
                        Some(false) => {}
                    }
                }
                if matched {
                    Ok(Some(true))
                } else if unknown {
                    Ok(None)
                } else {
                    Ok(Some(false))
                }
            }
        }
    }

    pub(crate) fn proven(proof: Option<&Proof>, slot: u32) -> Option<bool> {
        proof
            .filter(|proof| proof.input.contains(slot))
            .map(|proof| proof.matches.contains(slot))
    }

    fn can_emit_evidence(&self) -> bool {
        match self {
            Self::Term(term, _, _, _) => {
                term.derived == Derived::None
                    && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            }
            Self::And(parts) | Self::Or(parts) => parts.iter().any(Self::can_emit_evidence),
            Self::Not(_) => false,
        }
    }

    pub(crate) fn membership(
        &self,
        repo: &Repository,
        slot: u32,
        ctx: &RecordContext<'_>,
        evaluations: &mut u64,
        relation_work: &mut crate::relation_query::RelationWork,
        target_values: Option<&crate::relation_target_cache::TargetValueCache>,
    ) -> Result<Option<bool>, String> {
        Ok(match self {
            Self::Term(term, proof, term_id, predicates) => match Self::proven(*proof, slot) {
                Some(result) => Some(result),
                None => {
                    *evaluations += 1;
                    if term.related.is_some() {
                        crate::relation_query::eval_related_term(
                            repo,
                            term,
                            ctx,
                            relation_work,
                            target_values,
                        )?
                    } else {
                        let mut evidence = HashSet::new();
                        Some(
                            match predicates.verify(
                                *term_id,
                                &crate::query_predicate::RecordInput::new(ctx, &[]),
                                &mut crate::query_predicate::EvidenceSink::new(&mut evidence),
                            )? {
                                crate::query_predicate::Truth::True => true,
                                crate::query_predicate::Truth::False => false,
                                crate::query_predicate::Truth::Unknown => return Ok(None),
                            },
                        )
                    }
                }
            },
            Self::And(parts) => {
                let mut unknown = false;
                for part in parts {
                    match part.membership(
                        repo,
                        slot,
                        ctx,
                        evaluations,
                        relation_work,
                        target_values,
                    )? {
                        Some(false) => return Ok(Some(false)),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown { None } else { Some(true) }
            }
            Self::Or(parts) => {
                let mut unknown = false;
                for part in parts {
                    match part.membership(
                        repo,
                        slot,
                        ctx,
                        evaluations,
                        relation_work,
                        target_values,
                    )? {
                        Some(true) => return Ok(Some(true)),
                        None => unknown = true,
                        Some(false) => {}
                    }
                }
                if unknown { None } else { Some(false) }
            }
            Self::Not(inner) => inner
                .membership(repo, slot, ctx, evaluations, relation_work, target_values)?
                .map(|value| !value),
        })
    }

    pub(crate) fn with_evidence(
        &self,
        repo: &Repository,
        slot: u32,
        ctx: &RecordContext<'_>,
        evidence: &mut HashSet<String>,
        evaluations: &mut u64,
        relation_work: &mut crate::relation_query::RelationWork,
        target_values: Option<&crate::relation_target_cache::TargetValueCache>,
    ) -> Result<Option<bool>, String> {
        Ok(match self {
            Self::Term(term, proof, term_id, predicates) => {
                let matched = match Self::proven(*proof, slot) {
                    Some(result) => Some(result),
                    None => {
                        *evaluations += 1;
                        if term.related.is_some() {
                            crate::relation_query::eval_related_term(
                                repo,
                                term,
                                ctx,
                                relation_work,
                                target_values,
                            )?
                        } else {
                            let mut fields = HashSet::new();
                            Some(
                                match predicates.verify(
                                    *term_id,
                                    &crate::query_predicate::RecordInput::new(ctx, &[]),
                                    &mut crate::query_predicate::EvidenceSink::new(&mut fields),
                                )? {
                                    crate::query_predicate::Truth::True => true,
                                    crate::query_predicate::Truth::False => false,
                                    crate::query_predicate::Truth::Unknown => return Ok(None),
                                },
                            )
                        }
                    }
                };
                if matched == Some(true) && self.can_emit_evidence() {
                    evidence.insert(term.query_path.display.clone());
                    if let Some(rhs) = &term.rhs {
                        evidence.insert(rhs.query_path.display.clone());
                    }
                    if let Some(related) = &term.related {
                        evidence.insert(related.evidence_path.clone());
                    }
                }
                matched
            }
            Self::And(parts) => {
                let mut branch = HashSet::new();
                let mut unknown = false;
                for part in parts {
                    match part.with_evidence(
                        repo,
                        slot,
                        ctx,
                        &mut branch,
                        evaluations,
                        relation_work,
                        target_values,
                    )? {
                        Some(false) => return Ok(Some(false)),
                        None => unknown = true,
                        Some(true) => {}
                    }
                }
                if unknown {
                    None
                } else {
                    evidence.extend(branch);
                    Some(true)
                }
            }
            Self::Or(parts) => {
                let mut matched = false;
                let mut unknown = false;
                for part in parts {
                    if matched && !part.can_emit_evidence() {
                        continue;
                    }
                    let mut branch = HashSet::new();
                    match part.with_evidence(
                        repo,
                        slot,
                        ctx,
                        &mut branch,
                        evaluations,
                        relation_work,
                        target_values,
                    )? {
                        Some(true) => {
                            matched = true;
                            evidence.extend(branch);
                        }
                        None => unknown = true,
                        Some(false) => {}
                    }
                }
                if matched {
                    Some(true)
                } else if unknown {
                    None
                } else {
                    Some(false)
                }
            }
            Self::Not(inner) => inner
                .membership(repo, slot, ctx, evaluations, relation_work, target_values)?
                .map(|value| !value),
        })
    }
}
