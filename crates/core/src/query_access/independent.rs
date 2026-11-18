//! Independent sorted-table backing used by conformance tests.

use std::cell::Cell;

use roaring::RoaringBitmap;

use crate::bigram::BigramRequest;
use crate::execution_options::BigramAlphabet;
use crate::query::Operator;
use crate::query_predicate::TermId;
use crate::reference::BoundTerm;

use super::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength,
    ProviderId,
};
use super::membership::{DomainId, SlotInput, SlotSet};

const INDEPENDENT_PROVIDER: ProviderId = ProviderId(90);

pub(crate) struct SortedBackingProvider {
    domain: DomainId,
    rows: Vec<(String, Vec<u32>)>,
    grams: Vec<((bool, u64), Vec<u32>)>,
    proposals: Cell<u64>,
    applications: Cell<u64>,
}

impl SortedBackingProvider {
    pub(crate) fn from_fixture(domain: DomainId, records: &[&str]) -> Self {
        let mut rows: Vec<(String, Vec<u32>)> = records
            .iter()
            .enumerate()
            .map(|(slot, value)| ((*value).to_owned(), vec![slot as u32]))
            .collect();
        rows.sort_by(|left, right| left.0.cmp(&right.0));
        let mut merged: Vec<(String, Vec<u32>)> = Vec::new();
        for (value, slots) in rows {
            if let Some((last_value, last_slots)) = merged.last_mut()
                && last_value == &value
            {
                last_slots.extend(slots);
                continue;
            }
            merged.push((value, slots));
        }
        let mut grams: Vec<((bool, u64), Vec<u32>)> = Vec::new();
        for (slot, value) in records.iter().enumerate() {
            let request = BigramRequest::new(value);
            for key in request.grams(BigramAlphabet::Unicode) {
                match grams.binary_search_by_key(key, |(candidate, _)| *candidate) {
                    Ok(index) => grams[index].1.push(slot as u32),
                    Err(index) => grams.insert(index, (*key, vec![slot as u32])),
                }
            }
        }
        Self {
            domain,
            rows: merged,
            grams,
            proposals: Cell::new(0),
            applications: Cell::new(0),
        }
    }

    pub(crate) fn input_for<'a>(&self, input: &'a RoaringBitmap) -> SlotInput<'a> {
        SlotInput::unrestricted(self.domain, input)
    }

    pub(crate) fn counters(&self) -> (u64, u64) {
        (self.proposals.get(), self.applications.get())
    }

    fn scalar(&self, term: &BoundTerm) -> Option<Vec<u32>> {
        if !matches!(
            term.operator,
            Operator::Eq | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
        ) || term.rhs.is_some()
            || term.related.is_some()
            || term.operation.is_some()
            || term.matcher.is_some()
            || !matches!(term.literal, crate::model::Value::String(_))
        {
            return None;
        }
        let crate::model::Value::String(literal) = &term.literal else {
            unreachable!()
        };
        let mut slots = Vec::new();
        for (value, members) in &self.rows {
            let matches = match term.operator {
                Operator::Eq => value == literal,
                Operator::Gt => value > literal,
                Operator::Ge => value >= literal,
                Operator::Lt => value < literal,
                Operator::Le => value <= literal,
                _ => unreachable!(),
            };
            if matches {
                slots.extend(members);
            }
        }
        slots.sort_unstable();
        Some(slots)
    }

    fn grams(&self, request: &BigramRequest) -> Option<Vec<u32>> {
        let mut keys = request.grams(BigramAlphabet::Unicode).iter().peekable();
        let first = keys.next()?;
        let mut result = self.lookup_gram(*first)?.to_vec();
        for key in keys {
            result.retain(|slot| self.lookup_gram(*key).is_some_and(|other| other.contains(slot)));
            if result.is_empty() {
                break;
            }
        }
        result.sort_unstable();
        result.dedup();
        Some(result)
    }

    fn lookup_gram(&self, key: (bool, u64)) -> Option<&[u32]> {
        self.grams
            .binary_search_by_key(&key, |(candidate, _)| *candidate)
            .ok()
            .map(|index| self.grams[index].1.as_slice())
    }
}

impl IndexAccessProvider for SortedBackingProvider {
    fn provider_id(&self) -> ProviderId {
        INDEPENDENT_PROVIDER
    }

    fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        self.proposals.set(self.proposals.get() + 1);
        match request {
            AccessRequest::Structured(bound) => {
                if let Some(slots) = self.scalar(bound) {
                    offers.push(AccessOffer::new(
                        AccessId {
                            provider: INDEPENDENT_PROVIDER,
                            operation: OperationId(0),
                        },
                        term,
                        AccessKind::ExactMembership,
                        Coverage::Complete,
                        ProofStrength::Exact,
                        slots.len() as u64,
                        10.0 + (self.rows.len() as f64).log2(),
                        0.0,
                        false,
                        false,
                    ));
                }
                if let Some(request) = bound.gram_request.as_ref()
                    && let Some(slots) = self.grams(request)
                {
                    offers.push(AccessOffer::new(
                        AccessId {
                            provider: INDEPENDENT_PROVIDER,
                            operation: OperationId(1),
                        },
                        term,
                        AccessKind::CandidateMembership,
                        Coverage::CandidateOnly,
                        ProofStrength::Candidate,
                        slots.len() as u64,
                        request.gram_count(BigramAlphabet::Unicode) as f64,
                        0.0,
                        true,
                        false,
                    ));
                }
            }
            AccessRequest::Text(fts) => {
                if let Some(slots) = self.grams(&fts.gram_request) {
                    offers.push(AccessOffer::new(
                        AccessId {
                            provider: INDEPENDENT_PROVIDER,
                            operation: OperationId(2),
                        },
                        term,
                        AccessKind::CandidateMembership,
                        Coverage::CandidateOnly,
                        ProofStrength::Candidate,
                        slots.len().min(input.input_len as usize) as u64,
                        fts.gram_request.gram_count(BigramAlphabet::Unicode) as f64,
                        0.0,
                        true,
                        false,
                    ));
                }
            }
        }
        Ok(())
    }

    fn apply<'a>(
        &self,
        _term: TermId,
        request: AccessRequest<'_>,
        choice: AccessId,
        input: SlotInput<'a>,
        execution: &AccessExecution<'_>,
    ) -> AccessResult<SlotInput<'a>> {
        execution.check()?;
        if choice.provider != INDEPENDENT_PROVIDER || input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        let slots = match (choice.operation.0, request) {
            (0, AccessRequest::Structured(bound)) => {
                self.scalar(bound).ok_or(AccessError::CapabilityUnavailable)?
            }
            (1, AccessRequest::Structured(bound)) => self
                .grams(
                    bound
                        .gram_request
                        .as_ref()
                        .ok_or(AccessError::CapabilityUnavailable)?,
                )
                .ok_or(AccessError::CapabilityUnavailable)?,
            (2, AccessRequest::Text(fts)) => self
                .grams(&fts.gram_request)
                .ok_or(AccessError::CapabilityUnavailable)?,
            _ => return Err(AccessError::StaleHandle),
        };
        self.applications.set(self.applications.get() + 1);
        let input_ref = input.input();
        let bitmap = slots.into_iter().collect::<RoaringBitmap>() & input_ref;
        Ok(SlotInput::new(
            self.domain,
            input_ref,
            SlotSet::from_owned(bitmap),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::FtsMode;
    use crate::query_plan::selection::choose_offer;
    use crate::reference::{BoundExpr, Schema};

    #[test]
    fn sorted_scalar_and_gram_backing_match_reference_membership() {
        let records = ["alpha", "beta", "alphabet", "gamma"];
        let provider = SortedBackingProvider::from_fixture(DomainId::new(11), &records);
        let schema = Schema::default();
        let filter = crate::query::parse_filter("text = 'alpha'").unwrap();
        let BoundExpr::Term(term) = crate::reference::bind_filter(&filter, &schema).unwrap() else {
            unreachable!()
        };
        let term_id = TermId::new(1);
        let input_values: RoaringBitmap = (0..4).collect();
        let mut offers = OfferList::new();
        provider
            .propose(
                term_id,
                AccessRequest::Structured(&term),
                InputEstimate {
                    domain_len: 4,
                    input_len: 4,
                    ..Default::default()
                },
                &mut offers,
            )
            .unwrap();
        let chosen = offers
            .iter()
            .find(|offer| offer.proof() == ProofStrength::Exact)
            .unwrap();
        let applied = provider
            .apply(
                term_id,
                AccessRequest::Structured(&term),
                chosen.id(),
                provider.input_for(&input_values),
                &AccessExecution { context: None },
            )
            .unwrap();
        assert_eq!(applied.iter().collect::<Vec<_>>(), [0]);

        let fts = crate::query::FtsQuery {
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "alpha".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        };
        let bound_fts = crate::reference::bind_fts(&fts, &schema).unwrap();
        let mut offers = OfferList::new();
        provider
            .propose(
                TermId::new(2),
                AccessRequest::Text(&bound_fts),
                InputEstimate {
                    domain_len: 4,
                    input_len: 4,
                    ..Default::default()
                },
                &mut offers,
            )
            .unwrap();
        let chosen = choose_offer(TermId::new(2), &offers, 4)
            .unwrap()
            .unwrap();
        let candidates = provider
            .apply(
                TermId::new(2),
                AccessRequest::Text(&bound_fts),
                chosen.id(),
                provider.input_for(&input_values),
                &AccessExecution { context: None },
            )
            .unwrap();
        assert_eq!(candidates.iter().collect::<Vec<_>>(), [0, 2]);
        let exact = candidates
            .iter()
            .filter(|slot| records[*slot as usize].contains("alpha"))
            .collect::<Vec<_>>();
        assert_eq!(exact, [0, 2]);
        assert_eq!(provider.counters(), (2, 2));
    }

    #[test]
    fn selective_text_reduces_the_input_to_the_structured_lookup() {
        let records = ["common", "common", "common", "rare"];
        let provider = SortedBackingProvider::from_fixture(DomainId::new(12), &records);
        let input_values: RoaringBitmap = (0..4).collect();
        let schema = Schema::default();

        let fts = crate::query::FtsQuery {
            case_sensitivity: crate::query::CaseSensitivity::Sensitive,
            mode: FtsMode::Literal,
            literal: "rare".into(),
            pattern_parameter: None,
            fields: Vec::new(),
        };
        let bound_fts = crate::reference::bind_fts(&fts, &schema).unwrap();
        let text_term = TermId::new(10);
        let mut text_offers = OfferList::new();
        provider
            .propose(
                text_term,
                AccessRequest::Text(&bound_fts),
                InputEstimate {
                    domain_len: 4,
                    input_len: 4,
                    ..Default::default()
                },
                &mut text_offers,
            )
            .unwrap();

        let filter = crate::query::parse_filter("text = 'rare'").unwrap();
        let BoundExpr::Term(term) = crate::reference::bind_filter(&filter, &schema).unwrap() else {
            unreachable!()
        };
        let structured_term = TermId::new(11);
        let mut structured_offers = OfferList::new();
        provider
            .propose(
                structured_term,
                AccessRequest::Structured(&term),
                InputEstimate {
                    domain_len: 4,
                    input_len: 4,
                    ..Default::default()
                },
                &mut structured_offers,
            )
            .unwrap();
        let structured_exact = structured_offers
            .iter()
            .find(|offer| offer.proof() == ProofStrength::Exact)
            .unwrap();
        assert!(
            choose_offer(text_term, &text_offers, 4)
                .unwrap()
                .unwrap()
                .estimated_work(4)
                < structured_exact.estimated_work(4)
        );

        let text_choice = choose_offer(text_term, &text_offers, 4)
            .unwrap()
            .unwrap()
            .clone();
        let restricted = provider
            .apply(
                text_term,
                AccessRequest::Text(&bound_fts),
                text_choice.id(),
                provider.input_for(&input_values),
                &AccessExecution { context: None },
            )
            .unwrap();
        assert_eq!(restricted.iter().collect::<Vec<_>>(), [3]);

        let mut reduced_offers = OfferList::new();
        provider
            .propose(
                structured_term,
                AccessRequest::Structured(&term),
                InputEstimate {
                    domain_len: 4,
                    input_len: restricted.len() as u64,
                    ..Default::default()
                },
                &mut reduced_offers,
            )
            .unwrap();
        let structured_choice = reduced_offers
            .iter()
            .find(|offer| offer.proof() == ProofStrength::Exact)
            .unwrap()
            .clone();
        let input = restricted.input();
        let result = provider
            .apply(
                structured_term,
                AccessRequest::Structured(&term),
                structured_choice.id(),
                SlotInput::new(
                    restricted.domain(),
                    input,
                    SlotSet::Owned(restricted.iter().collect()),
                ),
                &AccessExecution { context: None },
            )
            .unwrap();
        assert_eq!(result.iter().collect::<Vec<_>>(), [3]);
        assert_eq!(result.len(), 1);
    }
}
