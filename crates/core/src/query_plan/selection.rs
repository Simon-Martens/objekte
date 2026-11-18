//! Shared offer selection policy.

#[cfg(test)]
use crate::query_access::contract::OfferList;
use crate::query_access::contract::{
    AccessError, AccessKind, AccessOffer, AccessResult, Coverage, ProofStrength,
};
use crate::query_predicate::TermId;

/// Deterministically choose the least estimated work. Exact proof and
/// complete coverage win semantic ties before provider registration order.
pub(crate) fn choose_offer<'a>(
    term: TermId,
    offers: &'a [AccessOffer],
    input_len: u64,
) -> AccessResult<Option<&'a AccessOffer>> {
    let mut selected = None;
    for offer in offers.iter().filter(|offer| offer.term() == term) {
        let Some(current) = selected else {
            selected = Some(offer);
            continue;
        };
        let candidate_key = selection_key(offer, input_len);
        let current_key = selection_key(current, input_len);
        if candidate_key < current_key {
            selected = Some(offer);
        }
    }
    Ok(selected)
}

pub(crate) fn ordered_offer_ids(offers: &[AccessOffer], input_len: u64) -> Vec<AccessOffer> {
    let mut ordered = offers.to_vec();
    ordered.sort_by(|left, right| {
        selection_key(left, input_len)
            .partial_cmp(&selection_key(right, input_len))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    ordered
}

pub(crate) fn prefer_text_first(text: Option<(u64, f64)>, filter: Option<(u64, f64)>) -> bool {
    match (text, filter) {
        (Some((_, text_work)), Some((_, filter_work))) => text_work < filter_work,
        (Some(_), None) => true,
        _ => false,
    }
}

pub(crate) fn split_input_dependent_cost(total: f64, input_len: u64) -> (f64, f64) {
    (total * 0.25, total * 0.75 / input_len.max(1) as f64)
}

fn selection_key(offer: &AccessOffer, input_len: u64) -> (f64, u8, u8, u8, u32) {
    let broad_candidate = offer.proof() == ProofStrength::Candidate
        && offer.estimated_output() >= input_len
        && matches!(
            offer.kind(),
            AccessKind::CandidateMembership | AccessKind::Relation
        );
    (
        offer.estimated_work(input_len),
        match offer.proof() {
            ProofStrength::Exact => 0,
            ProofStrength::Candidate => 1,
            ProofStrength::None => 2,
        },
        match offer.coverage() {
            Coverage::Complete => 0,
            Coverage::Partial { .. } => 1,
            Coverage::CandidateOnly => 2,
            Coverage::Input => 3,
        },
        u8::from(broad_candidate),
        offer.id().provider.0,
    )
}

pub(crate) fn validate_offer_scope(offer: &AccessOffer) -> AccessResult<()> {
    if offer.term().get() == u32::MAX {
        return Err(AccessError::InvalidDomain(u64::MAX));
    }
    if offer.kind() == AccessKind::SourceScan && offer.proof() == ProofStrength::Exact {
        return Err(AccessError::Internal(
            "source scans cannot claim index proof".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use smallvec::smallvec;

    use super::*;
    use crate::query_access::contract::{AccessId, OperationId, ProviderId};

    fn offer(
        provider: u32,
        kind: AccessKind,
        proof: ProofStrength,
        output: u64,
        startup: f64,
        input_work: f64,
    ) -> AccessOffer {
        AccessOffer::new(
            AccessId {
                provider: ProviderId(provider),
                operation: OperationId(0),
            },
            TermId::new(1),
            kind,
            Coverage::Complete,
            proof,
            output,
            startup,
            input_work,
            false,
            false,
        )
    }

    #[test]
    fn lower_work_wins_and_cost_reversal_changes_the_choice() {
        let first = offer(
            1,
            AccessKind::ExactMembership,
            ProofStrength::Exact,
            3,
            1.0,
            0.0,
        );
        let second = offer(
            2,
            AccessKind::ExactMembership,
            ProofStrength::Exact,
            100,
            40.0,
            0.0,
        );
        let offers: OfferList = smallvec![first.clone(), second.clone()];
        assert_eq!(
            choose_offer(TermId::new(1), &offers, 100)
                .unwrap()
                .unwrap()
                .id()
                .provider,
            ProviderId(1)
        );
        let offers: OfferList = smallvec![second, first];
        assert_eq!(
            choose_offer(TermId::new(1), &offers, 100)
                .unwrap()
                .unwrap()
                .id()
                .provider,
            ProviderId(1)
        );
    }

    #[test]
    fn exact_proof_wins_an_equal_cost_tie() {
        let scan = offer(
            1,
            AccessKind::SourceScan,
            ProofStrength::None,
            100,
            5.0,
            0.0,
        );
        let posting = offer(
            2,
            AccessKind::ExactMembership,
            ProofStrength::Exact,
            100,
            5.0,
            0.0,
        );
        let offers: OfferList = smallvec![scan, posting];
        assert_eq!(
            choose_offer(TermId::new(1), &offers, 100)
                .unwrap()
                .unwrap()
                .kind(),
            AccessKind::ExactMembership
        );
    }

    #[test]
    fn same_cost_pair_can_reverse_text_first_without_registration_order() {
        assert!(prefer_text_first(Some((2, 4.0)), Some((8, 20.0))));
        assert!(!prefer_text_first(Some((2, 20.0)), Some((8, 4.0))));
        assert!(prefer_text_first(Some((2, 4.0)), None));
        assert!(!prefer_text_first(None, Some((8, 4.0))));
    }
}
