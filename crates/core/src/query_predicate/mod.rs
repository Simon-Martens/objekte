//! Opaque prepared predicate identity and planning properties.

use std::fmt;

use crate::reference::{BoundExpr, BoundTerm};

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
        has_text: bool,
        leaves: &mut Vec<PreparedLeaf>,
    ) -> (Option<PreparedExpr>, Option<TermId>) {
        let filter = filter.map(|expr| self.prepare_expr(collection_index, expr, leaves));
        let text = has_text.then(|| {
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

    pub(crate) fn term_for_span(
        &self,
        collection_index: usize,
        span: (usize, usize),
    ) -> Option<TermId> {
        self.leaves
            .iter()
            .find(|leaf| {
                leaf.collection_index == collection_index
                    && leaf.span == span
                    && leaf.display_path != "@fts"
            })
            .map(|leaf| leaf.id)
    }
}
