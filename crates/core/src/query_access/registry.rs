//! Heterogeneous provider registry used by one request/domain plan.

use smallvec::SmallVec;

use crate::query_predicate::{PreparedPredicates, TermId};

use super::contract::{
    AccessError, AccessExecution, AccessId, AccessRequest, AccessResult, IndexAccessProvider,
    InputEstimate, OfferList, ProviderId,
};
use super::membership::SlotInput;

pub(crate) struct ProviderCatalog<'a> {
    providers: SmallVec<[&'a dyn IndexAccessProvider; 4]>,
}

impl<'a> ProviderCatalog<'a> {
    pub(crate) fn new(providers: impl IntoIterator<Item = &'a dyn IndexAccessProvider>) -> Self {
        Self {
            providers: providers.into_iter().collect(),
        }
    }

    pub(crate) fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
    ) -> AccessResult<OfferList> {
        let mut offers = OfferList::new();
        for provider in &self.providers {
            provider.propose(term, request, input, &mut offers)?;
        }
        Ok(offers)
    }

    pub(crate) fn propose_prepared(
        &self,
        predicates: &PreparedPredicates,
        term: TermId,
        input: InputEstimate,
    ) -> AccessResult<OfferList> {
        let mut offers = OfferList::new();
        for provider in &self.providers {
            predicates.propose(term, *provider, input, &mut offers)?;
        }
        Ok(offers)
    }

    pub(crate) fn apply<'b>(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        choice: AccessId,
        input: SlotInput<'b>,
        execution: &AccessExecution<'_>,
    ) -> AccessResult<SlotInput<'b>> {
        let provider = self
            .providers
            .iter()
            .copied()
            .find(|provider| provider.provider_id() == choice.provider)
            .ok_or(AccessError::StaleHandle)?;
        provider.apply(term, request, choice, input, execution)
    }
}

pub(crate) fn provider_ids(offers: &[super::contract::AccessOffer]) -> Vec<ProviderId> {
    offers.iter().map(|offer| offer.id().provider).collect()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use roaring::RoaringBitmap;

    use super::*;
    use crate::query_access::contract::{
        AccessKind, AccessOffer, Coverage, OperationId, ProofStrength,
    };
    use crate::query_access::membership::{DomainId, SlotSet};
    use crate::reference::BoundTerm;
    use crate::query_plan::selection::choose_offer;

    struct TestProvider {
        id: ProviderId,
        operation: OperationId,
        work: f64,
        output: u32,
        advertised_term: TermId,
        proposals: AtomicU64,
        applications: AtomicU64,
        fail_apply: bool,
    }

    impl IndexAccessProvider for TestProvider {
        fn provider_id(&self) -> ProviderId {
            self.id
        }

        fn propose(
            &self,
            _term: TermId,
            _request: AccessRequest<'_>,
            _input: InputEstimate,
            offers: &mut OfferList,
        ) -> AccessResult<()> {
            self.proposals.fetch_add(1, Ordering::Relaxed);
            offers.push(AccessOffer::new(
                AccessId {
                    provider: self.id,
                    operation: self.operation,
                },
                self.advertised_term,
                AccessKind::ExactMembership,
                Coverage::Complete,
                ProofStrength::Exact,
                u64::from(self.output),
                self.work,
                0.0,
                false,
                false,
            ));
            Ok(())
        }

        fn apply<'a>(
            &self,
            _term: TermId,
            _request: AccessRequest<'_>,
            _choice: AccessId,
            input: SlotInput<'a>,
            _execution: &AccessExecution<'_>,
        ) -> AccessResult<SlotInput<'a>> {
            self.applications.fetch_add(1, Ordering::Relaxed);
            if self.fail_apply {
                return Err(AccessError::Corrupt("injected failure".into()));
            }
            let slots = {
                let input_ref = input
                    .effective_bitmap()
                    .ok_or_else(|| AccessError::Internal("effective input missing".into()))?;
                (0..self.output)
                    .filter(|slot| input_ref.contains(*slot))
                    .collect::<RoaringBitmap>()
            };
            Ok(input.replace_slots(SlotSet::from_owned(slots)))
        }
    }

    fn provider(
        id: u32,
        work: f64,
        advertised_term: TermId,
    ) -> TestProvider {
        TestProvider {
            id: ProviderId(id),
            operation: OperationId(0),
            work,
            output: 2,
            advertised_term,
            proposals: AtomicU64::new(0),
            applications: AtomicU64::new(0),
            fail_apply: false,
        }
    }

    #[test]
    fn catalog_selects_cost_wins_independently_of_registration_order() {
        let term = TermId::new(5);
        let cheap = provider(1, 2.0, term);
        let expensive = provider(2, 20.0, term);
        for providers in [
            vec![&cheap as &dyn IndexAccessProvider, &expensive],
            vec![&expensive as &dyn IndexAccessProvider, &cheap],
        ] {
            let catalog = ProviderCatalog::new(providers);
            let offers = catalog
                .propose(
                    term,
                    AccessRequest::Structured(empty_term_ref()),
                    InputEstimate {
                        domain_len: 10,
                        input_len: 10,
                        ..Default::default()
                    },
                )
                .unwrap();
            assert_eq!(
                choose_offer(term, &offers, 10).unwrap().unwrap().id().provider,
                ProviderId(1)
            );
        }
        assert_eq!(cheap.proposals.load(Ordering::Relaxed), 2);
        assert_eq!(expensive.proposals.load(Ordering::Relaxed), 2);
        assert_eq!(cheap.applications.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn cheap_offer_for_another_term_is_not_eligible() {
        let term = TermId::new(6);
        let incompatible = provider(1, 0.0, TermId::new(99));
        let compatible = provider(2, 5.0, term);
        let catalog = ProviderCatalog::new([
            &incompatible as &dyn IndexAccessProvider,
            &compatible,
        ]);
        let offers = catalog
            .propose(
                term,
                AccessRequest::Structured(empty_term_ref()),
                InputEstimate {
                    domain_len: 10,
                    input_len: 10,
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(
            choose_offer(term, &offers, 10).unwrap().unwrap().id().provider,
            ProviderId(2)
        );
    }

    #[test]
    fn provider_application_failure_is_surfaced() {
        let term = TermId::new(7);
        let mut failing = provider(1, 1.0, term);
        failing.fail_apply = true;
        let failing = failing;
        let catalog = ProviderCatalog::new([&failing as &dyn IndexAccessProvider]);
        let input = (1..=3).collect::<RoaringBitmap>();
        let slots = SlotInput::unrestricted(DomainId::new(1), &input);
        let error = match catalog
            .apply(
                term,
                AccessRequest::Structured(empty_term_ref()),
                AccessId {
                    provider: ProviderId(1),
                    operation: OperationId(0),
                },
                slots,
                &AccessExecution { context: None },
            )
        {
            Ok(_) => panic!("expected injected provider failure"),
            Err(error) => error,
        };
        assert!(matches!(error, AccessError::Corrupt(_)));
    }

    fn empty_term_ref() -> &'static BoundTerm {
        static TERM: std::sync::OnceLock<BoundTerm> = std::sync::OnceLock::new();
        TERM.get_or_init(|| {
            let schema = crate::reference::Schema::default();
            let filter = crate::query::parse_filter("field = 'x'").unwrap();
            let crate::reference::BoundExpr::Term(term) =
                crate::reference::bind_filter(&filter, &schema).unwrap()
            else {
                unreachable!()
            };
            term
        })
    }
}
