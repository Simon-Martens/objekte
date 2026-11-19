//! Filter-leaf evidence shared by every exact evaluation route.

use std::collections::HashSet;

use crate::query::Operator;
use crate::query::binding::filter::{BoundTerm, Derived};

pub(crate) fn emit_bound_filter_evidence(
    term: &BoundTerm,
    result: Option<bool>,
    evidence: Option<&mut HashSet<String>>,
) {
    let Some(evidence) = evidence else {
        return;
    };
    if result != Some(true)
        || term.derived != Derived::None
        || matches!(term.operator, Operator::Ne | Operator::NotContains)
    {
        return;
    }
    evidence.insert(term.query_path.display.clone());
    if let Some(rhs) = &term.rhs {
        evidence.insert(rhs.query_path.display.clone());
    }
    if let Some(related) = &term.related {
        if let Some(source_operand) = &related.source_operand {
            evidence.insert(source_operand.query_path.display.clone());
        }
        evidence.insert(related.evidence_path.clone());
    }
}
