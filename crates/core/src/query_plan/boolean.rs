//! Boolean composition over opaque prepared predicate leaves.

use crate::query_predicate::PreparedExpr;

pub(crate) fn legal_and_order(expr: &PreparedExpr) -> Vec<usize> {
    let PreparedExpr::And(parts) = expr else {
        return Vec::new();
    };
    (0..parts.len()).collect()
}

pub(crate) fn legal_or_order(expr: &PreparedExpr) -> Vec<usize> {
    let PreparedExpr::Or(parts) = expr else {
        return Vec::new();
    };
    (0..parts.len()).collect()
}

pub(crate) fn preserves_unknown(expr: &PreparedExpr) -> bool {
    match expr {
        PreparedExpr::Term(_) => true,
        PreparedExpr::And(parts) | PreparedExpr::Or(parts) => parts.iter().any(preserves_unknown),
        PreparedExpr::Not(inner) => preserves_unknown(inner),
    }
}
