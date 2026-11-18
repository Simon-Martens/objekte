//! Opaque prepared predicate identity and planning properties.

#![allow(
    dead_code,
    reason = "the prepared predicate contract retains complete verification and planner metadata"
)]

use std::collections::HashSet;
use std::fmt;

use crate::query_access::contract::{
    AccessExecution, AccessId, AccessRequest, AccessResult, IndexAccessProvider, InputEstimate,
    OfferList, Restriction,
};
use crate::query_access::membership::SlotInput;
use crate::reference::{BoundExpr, BoundFts, BoundTerm, RecordContext, eval_bound_fts, eval_term};
use crate::source::TextSegment;

mod structured;
pub(crate) use structured::proven_evidence;
pub(crate) mod scan;
pub(crate) use scan::{
    DirectSourcePlan, Residual, borrowed_relation_source_text, borrowed_value_text,
    local_segment_projection, prepare_residual, prepare_value_text_projection, scan_projection,
    segment_filter_evidence_bytes, segment_filter_source_fields, shared_fold_bound,
    shared_fold_residual, source_fts_eligible,
};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct TermId(u32);

impl TermId {
    pub(crate) const fn new(value: u32) -> Self {
        Self(value)
    }

    pub(crate) const fn get(self) -> u32 {
        self.0
    }
}

impl fmt::Display for TermId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "term#{}", self.0)
    }
}

/// Logical structure visible to planning. Bound filters, matchers and index
/// requests remain in the predicate table behind these opaque leaves.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum PreparedExpr {
    Term(TermId),
    And(Vec<PreparedExpr>),
    Or(Vec<PreparedExpr>),
    Not(Box<PreparedExpr>),
}

impl PreparedExpr {
    pub(crate) fn term_ids(&self) -> Vec<TermId> {
        fn collect(expr: &PreparedExpr, ids: &mut Vec<TermId>) {
            match expr {
                PreparedExpr::Term(id) => ids.push(*id),
                PreparedExpr::And(parts) | PreparedExpr::Or(parts) => {
                    for part in parts {
                        collect(part, ids);
                    }
                }
                PreparedExpr::Not(inner) => collect(inner, ids),
            }
        }
        let mut ids = Vec::new();
        collect(self, &mut ids);
        ids
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct PredicateProperties {
    pub(crate) reorderable: bool,
    pub(crate) may_be_unknown: bool,
    pub(crate) requires_source: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct PreparedLeaf {
    pub(crate) id: TermId,
    pub(crate) collection_index: usize,
    pub(crate) span: (usize, usize),
    pub(crate) display_path: String,
    pub(crate) logical_type: String,
    pub(crate) binding: String,
    pub(crate) properties: PredicateProperties,
    source: PreparedLeafSource,
}

#[derive(Clone, Debug)]
enum PreparedLeafSource {
    Structured(BoundTerm),
    Text(BoundFts),
}

pub(crate) struct RecordInput<'a> {
    context: &'a RecordContext<'a>,
    text_segments: &'a [TextSegment],
}

impl<'a> RecordInput<'a> {
    pub(crate) fn new(context: &'a RecordContext<'a>, text_segments: &'a [TextSegment]) -> Self {
        Self {
            context,
            text_segments,
        }
    }
}

pub(crate) struct EvidenceSink<'a> {
    fields: &'a mut HashSet<String>,
}

impl<'a> EvidenceSink<'a> {
    pub(crate) fn new(fields: &'a mut HashSet<String>) -> Self {
        Self { fields }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Truth {
    True,
    False,
    Unknown,
}

#[cfg(test)]
struct AlternateMatcher {
    field: String,
    literal: String,
    invocations: std::sync::atomic::AtomicU64,
}

#[cfg(test)]
pub(crate) struct AlternateMatcherGuard;

#[cfg(test)]
impl Drop for AlternateMatcherGuard {
    fn drop(&mut self) {
        *test_alternate_matcher()
            .lock()
            .expect("alternate matcher registry poisoned") = None;
    }
}

#[cfg(test)]
pub(crate) fn install_alternate_matcher(field: &str, literal: &str) -> AlternateMatcherGuard {
    *test_alternate_matcher()
        .lock()
        .expect("alternate matcher registry poisoned") = Some(AlternateMatcher {
        field: field.into(),
        literal: literal.into(),
        invocations: std::sync::atomic::AtomicU64::new(0),
    });
    AlternateMatcherGuard
}

#[cfg(test)]
pub(crate) fn alternate_matcher_invocations() -> u64 {
    test_alternate_matcher()
        .lock()
        .expect("alternate matcher registry poisoned")
        .as_ref()
        .map_or(0, |matcher| {
            matcher
                .invocations
                .load(std::sync::atomic::Ordering::Relaxed)
        })
}

#[cfg(test)]
pub(crate) fn alternate_matcher_active() -> bool {
    test_alternate_matcher()
        .lock()
        .expect("alternate matcher registry poisoned")
        .is_some()
}

#[cfg(test)]
fn test_alternate_matcher() -> &'static std::sync::Mutex<Option<AlternateMatcher>> {
    static MATCHER: std::sync::OnceLock<std::sync::Mutex<Option<AlternateMatcher>>> =
        std::sync::OnceLock::new();
    MATCHER.get_or_init(|| std::sync::Mutex::new(None))
}

pub(crate) struct PredicateBuilder {
    next: u32,
}

impl PredicateBuilder {
    pub(crate) fn new(next: u32) -> Self {
        Self { next }
    }

    pub(crate) fn prepare(
        &mut self,
        collection_index: usize,
        filter: Option<&BoundExpr>,
        fts: Option<&BoundFts>,
        leaves: &mut Vec<PreparedLeaf>,
    ) -> (Option<PreparedExpr>, Option<TermId>) {
        let filter = filter.map(|expr| self.prepare_expr(collection_index, expr, leaves));
        let text = fts.map(|fts| {
            let id = self.allocate();
            leaves.push(PreparedLeaf {
                id,
                collection_index,
                span: (0, 0),
                display_path: "@fts".into(),
                logical_type: "Text".into(),
                binding: "fts".into(),
                properties: PredicateProperties {
                    reorderable: true,
                    may_be_unknown: false,
                    requires_source: true,
                },
                source: PreparedLeafSource::Text(fts.clone()),
            });
            id
        });
        (filter, text)
    }

    pub(crate) fn next_id(&self) -> u32 {
        self.next
    }

    fn prepare_expr(
        &mut self,
        collection_index: usize,
        expr: &BoundExpr,
        leaves: &mut Vec<PreparedLeaf>,
    ) -> PreparedExpr {
        match expr {
            BoundExpr::Term(term) => {
                PreparedExpr::Term(self.prepare_term(collection_index, term, leaves))
            }
            BoundExpr::And(parts) => PreparedExpr::And(
                parts
                    .iter()
                    .map(|part| self.prepare_expr(collection_index, part, leaves))
                    .collect(),
            ),
            BoundExpr::Or(parts) => PreparedExpr::Or(
                parts
                    .iter()
                    .map(|part| self.prepare_expr(collection_index, part, leaves))
                    .collect(),
            ),
            BoundExpr::Not(inner) => {
                PreparedExpr::Not(Box::new(self.prepare_expr(collection_index, inner, leaves)))
            }
        }
    }

    fn prepare_term(
        &mut self,
        collection_index: usize,
        term: &BoundTerm,
        leaves: &mut Vec<PreparedLeaf>,
    ) -> TermId {
        let id = self.allocate();
        leaves.push(PreparedLeaf {
            id,
            collection_index,
            span: term.span,
            display_path: term.query_path.display.clone(),
            logical_type: format!("{:?}", term.kind),
            binding: format!("{}@{}", term.binding_name, term.binding_version),
            properties: PredicateProperties {
                reorderable: term.operation.is_none()
                    && term.matcher.is_none()
                    && term.related.is_none()
                    && term.rhs.is_none()
                    && !term.query_path.has_position,
                may_be_unknown: term.related.is_some()
                    || term.rhs.is_some()
                    || term.path.root != crate::model::PathRoot::Data,
                requires_source: true,
            },
            source: PreparedLeafSource::Structured(term.clone()),
        });
        id
    }

    fn allocate(&mut self) -> TermId {
        let id = TermId(self.next);
        self.next = self.next.checked_add(1).expect("query term ID overflow");
        id
    }
}

#[derive(Clone)]
pub(crate) struct PreparedPredicates {
    leaves: Vec<PreparedLeaf>,
}

impl PreparedPredicates {
    pub(crate) fn new(leaves: Vec<PreparedLeaf>) -> Self {
        Self { leaves }
    }

    pub(crate) fn get(&self, id: TermId) -> Option<&PreparedLeaf> {
        self.leaves.iter().find(|leaf| leaf.id == id)
    }

    pub(crate) fn leaves(&self) -> &[PreparedLeaf] {
        &self.leaves
    }

    pub(crate) fn term_for_bound_term(
        &self,
        collection_index: usize,
        term: &BoundTerm,
    ) -> Option<TermId> {
        self.leaves
            .iter()
            .find(|leaf| {
                leaf.collection_index == collection_index
                    && leaf.display_path != "@fts"
                    && match &leaf.source {
                        PreparedLeafSource::Structured(candidate) => {
                            crate::reference::same_bound_term(candidate, term)
                        }
                        PreparedLeafSource::Text(_) => false,
                    }
            })
            .map(|leaf| leaf.id)
    }

    pub(crate) fn propose(
        &self,
        leaf: TermId,
        provider: &dyn IndexAccessProvider,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        let request = self.request(leaf)?;
        provider.propose(leaf, request, input, offers)
    }

    pub(crate) fn restrict<'a>(
        &self,
        leaf: TermId,
        provider: &dyn IndexAccessProvider,
        choice: AccessId,
        domain: SlotInput<'a>,
        execution: &AccessExecution<'_>,
    ) -> AccessResult<Restriction<'a>> {
        let request = self.request(leaf)?;
        provider.apply(leaf, request, choice, domain, execution)
    }

    pub(crate) fn verify(
        &self,
        leaf: TermId,
        record: &RecordInput<'_>,
        evidence: &mut EvidenceSink<'_>,
    ) -> Result<Truth, String> {
        let entry = self
            .get(leaf)
            .ok_or_else(|| format!("unknown prepared predicate {leaf}"))?;
        match &entry.source {
            PreparedLeafSource::Structured(term) => {
                #[cfg(test)]
                let alternate = alternate_term_result(term, record.context);
                #[cfg(not(test))]
                let alternate: Option<Result<bool, String>> = None;
                let matched = match alternate {
                    Some(result) => result?,
                    None => eval_term(term, record.context)?,
                };
                if matched && let Some(fields) = evidence_fields(term) {
                    evidence.fields.extend(fields);
                }
                Ok(if matched { Truth::True } else { Truth::False })
            }
            PreparedLeafSource::Text(fts) => {
                let mut fields = HashSet::new();
                let matched =
                    eval_bound_fts(fts, record.text_segments, record.context, &mut fields)?;
                if matched {
                    evidence.fields.extend(fields);
                }
                Ok(if matched { Truth::True } else { Truth::False })
            }
        }
    }

    pub(crate) fn request(&self, leaf: TermId) -> AccessResult<AccessRequest<'_>> {
        let entry = self
            .get(leaf)
            .ok_or(crate::query_access::contract::AccessError::StaleHandle)?;
        Ok(match &entry.source {
            PreparedLeafSource::Structured(term) => AccessRequest::Structured(term),
            PreparedLeafSource::Text(fts) => AccessRequest::Text(fts),
        })
    }
}

fn evidence_fields(term: &BoundTerm) -> Option<Vec<String>> {
    if matches!(
        term.operator,
        crate::query::Operator::Ne | crate::query::Operator::NotContains
    ) {
        return None;
    }
    Some(vec![term.query_path.display.clone()])
}

#[cfg(test)]
pub(crate) fn alternate_term_result(
    term: &BoundTerm,
    context: &RecordContext<'_>,
) -> Option<Result<bool, String>> {
    let mut registry = test_alternate_matcher()
        .lock()
        .expect("alternate matcher registry poisoned");
    let matcher = registry.as_mut()?;
    if term.path.root != crate::model::PathRoot::Data
        || term.path.segments.len() != 1
        || term.path.segments[0] != matcher.field
        || term.operator != crate::query::Operator::Contains
        || term.transform != crate::reference::Transform::Identity
        || !matches!(term.literal, crate::model::Value::String(_))
    {
        return None;
    }
    matcher
        .invocations
        .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let crate::model::Value::String(literal) = &term.literal else {
        unreachable!()
    };
    let selected = match context.value {
        crate::model::Value::Object(object) => object.get(&matcher.field),
        _ => None,
    };
    Some(Ok(match selected {
        Some(crate::model::Value::String(text)) => text
            .as_bytes()
            .windows(literal.len())
            .any(|window| window == literal.as_bytes()),
        _ => false,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bound_term(field: &str) -> BoundTerm {
        let filter = crate::query::parse_filter(&format!("{field} = 'value'")).unwrap();
        let BoundExpr::Term(term) =
            crate::reference::bind_filter(&filter, &crate::reference::Schema::default()).unwrap()
        else {
            unreachable!()
        };
        term
    }

    #[test]
    fn same_source_span_keeps_field_occurrences_distinct() {
        let mut left = bound_term("left");
        let mut right = bound_term("right");
        left.span = (4, 17);
        right.span = (4, 17);
        let expr = BoundExpr::And(vec![BoundExpr::Term(left), BoundExpr::Term(right)]);
        let mut leaves = Vec::new();
        PredicateBuilder::new(0).prepare(0, Some(&expr), None, &mut leaves);
        let predicates = PreparedPredicates::new(leaves);
        let left = predicates
            .term_for_bound_term(
                0,
                match &expr {
                    BoundExpr::And(parts) => match &parts[0] {
                        BoundExpr::Term(term) => term,
                        _ => unreachable!(),
                    },
                    _ => unreachable!(),
                },
            )
            .expect("left occurrence");
        let right = predicates
            .term_for_bound_term(
                0,
                match &expr {
                    BoundExpr::And(parts) => match &parts[1] {
                        BoundExpr::Term(term) => term,
                        _ => unreachable!(),
                    },
                    _ => unreachable!(),
                },
            )
            .expect("right occurrence");
        assert_ne!(left, right);
    }
}
