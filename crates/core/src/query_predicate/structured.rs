//! Structured predicate evidence reconstructed from exact membership proofs.

use std::collections::HashSet;

use crate::inventory::RecordClass;
use crate::model::Value;
use crate::query::Operator;
use crate::query::semantics::record::RecordContext;
use crate::query::{
    binding::filter::BoundExpr, binding::filter::Derived, binding::filter::same_bound_term,
};
use crate::query_execution::restrictions::Proof;
use crate::reference::filter::eval_unavailable_filter;
use crate::repository::Repository;
use crate::repository::state::CollectionState;

pub(crate) fn proof_evidence(
    expr: &BoundExpr,
    slot: u32,
    proofs: &[Proof],
) -> Option<HashSet<String>> {
    match expr {
        BoundExpr::Term(term) => {
            let proof = proofs
                .iter()
                .find(|proof| same_bound_term(&proof.term, term) && proof.input.contains(slot))?;
            if !proof.matches.contains(slot) {
                return None;
            }
            let mut evidence = HashSet::new();
            if term.derived == Derived::None
                && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
                if let Some(related) = &term.related {
                    evidence.insert(related.evidence_path.clone());
                }
            }
            Some(evidence)
        }
        BoundExpr::And(parts) => {
            let mut evidence = HashSet::new();
            for part in parts {
                evidence.extend(proof_evidence(part, slot, proofs)?);
            }
            Some(evidence)
        }
        BoundExpr::Or(parts) => {
            let mut evidence = HashSet::new();
            let mut matched = false;
            for part in parts {
                if let Some(branch) = proof_evidence(part, slot, proofs) {
                    matched = true;
                    evidence.extend(branch);
                }
            }
            matched.then_some(evidence)
        }
        BoundExpr::Not(inner) => proof_evidence(inner, slot, proofs)
            .is_none()
            .then_some(HashSet::new()),
    }
}

/// Fallback proofs establish truth from inventory metadata. Reconstruct their
/// evidence with the same three-valued evaluator as reference execution: an
/// unavailable logical predicate stays unavailable even under negation.
pub(crate) fn proven_evidence(
    repo: &Repository,
    collection: &CollectionState,
    record: &crate::inventory::RecordEntry,
    expr: &BoundExpr,
    proofs: &[Proof],
) -> Result<HashSet<String>, String> {
    if record.class != RecordClass::RawFallback {
        return Ok(proof_evidence(expr, record.slot.0, proofs).unwrap_or_default());
    }
    let empty = Value::Object(Default::default());
    let file = &repo.files.files[record.file.0 as usize];
    let ctx = RecordContext {
        value: &empty,
        source_bytes: None,
        record,
        file,
        collection_name: &collection.definition.name,
        repository_root: &repo.files.required_root(),
    };
    let mut evidence = HashSet::new();
    if eval_unavailable_filter(expr, &ctx, &mut evidence)? != Some(true) {
        return Err("index-proven fallback has non-true filter truth".into());
    }
    Ok(evidence)
}
