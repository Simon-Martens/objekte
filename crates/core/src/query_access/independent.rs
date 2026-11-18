//! Independent sorted-table backing used by conformance tests.

use std::sync::Mutex;

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

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct BackingObservation {
    pub(crate) kind: u32,
    pub(crate) input: Vec<u32>,
    pub(crate) output: Vec<u32>,
}

pub(crate) struct SortedBackingProvider {
    id: ProviderId,
    domain: DomainId,
    field: String,
    supports_text: bool,
    rows: Vec<(String, Vec<u32>)>,
    grams: Vec<((bool, u64), Vec<u32>)>,
    state: Mutex<IndependentState>,
}

#[derive(Default)]
struct IndependentState {
    scalar_work: f64,
    gram_work: f64,
    operations: Vec<(TermId, u32)>,
    next_operation: u32,
    observations: Vec<BackingObservation>,
    proposals: u64,
    applications: u64,
}

impl SortedBackingProvider {
    pub(crate) fn from_fixture(domain: DomainId, records: &[&str]) -> Self {
        Self::from_field_fixture(domain, "text", records)
    }

    pub(crate) fn from_field_fixture(
        domain: DomainId,
        field: &str,
        records: &[&str],
    ) -> Self {
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
            id: INDEPENDENT_PROVIDER,
            domain,
            field: field.into(),
            supports_text: true,
            rows: merged,
            grams,
            state: Mutex::new(IndependentState {
                scalar_work: 10.0,
                ..IndependentState::default()
            }),
        }
    }

    pub(crate) fn input_for<'a>(&self, input: &'a RoaringBitmap) -> SlotInput<'a> {
        SlotInput::unrestricted(self.domain, input)
    }

    pub(crate) fn counters(&self) -> (u64, u64) {
        let state = self.state.lock().expect("backing state poisoned");
        (state.proposals, state.applications)
    }

    pub(crate) fn set_costs(&self, scalar_work: f64, gram_work: f64) {
        let mut state = self.state.lock().expect("backing state poisoned");
        state.scalar_work = scalar_work;
        state.gram_work = gram_work;
    }

    pub(crate) fn with_provider_id(mut self, id: ProviderId) -> Self {
        self.id = id;
        self
    }

    pub(crate) fn scalar_only(self) -> Self {
        Self {
            supports_text: false,
            ..self
        }
    }

    pub(crate) fn observations(&self) -> Vec<BackingObservation> {
        self.state
            .lock()
            .expect("backing state poisoned")
            .observations
            .clone()
    }

    pub(crate) fn clear_observations(&self) {
        self.state
            .lock()
            .expect("backing state poisoned")
            .observations
            .clear();
    }

    fn scalar(&self, term: &BoundTerm) -> Option<Vec<u32>> {
        if term.path.segments.len() != 1 || term.path.segments[0] != self.field {
            return None;
        }
        if !matches!(
            term.operator,
            Operator::Eq | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
        ) || term.rhs.is_some()
            || term.related.is_some()
            || term.operation.is_some()
            || term.matcher.is_some()
            || !matches!(
                term.literal,
                crate::model::Value::String(_) | crate::model::Value::Number(_)
            )
        {
            return None;
        }
        let literal = match &term.literal {
            crate::model::Value::String(literal) => literal.clone(),
            crate::model::Value::Number(literal) => match literal {
                crate::model::Number::Integer(value) => value.to_string(),
                crate::model::Number::Float(value) => format!("{value:?}"),
                crate::model::Number::ExactDecimal(value) => format!("{value:?}"),
            },
            _ => unreachable!(),
        };
        let mut slots = Vec::new();
        for (value, members) in &self.rows {
            let matches = match term.operator {
                Operator::Eq => value == &literal,
                Operator::Gt => value > &literal,
                Operator::Ge => value >= &literal,
                Operator::Lt => value < &literal,
                Operator::Le => value <= &literal,
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

    fn prepare_operation(&self, term: TermId, kind: u32) -> OperationId {
        let mut state = self.state.lock().expect("backing state poisoned");
        let operation = state.next_operation;
        state.next_operation += 1;
        state.operations.push((term, kind));
        OperationId(operation)
    }

    fn operation(&self, id: OperationId) -> Option<(TermId, u32)> {
        self.state
            .lock()
            .expect("backing state poisoned")
            .operations
            .get(id.0 as usize)
            .copied()
    }
}

impl IndexAccessProvider for SortedBackingProvider {
    fn provider_id(&self) -> ProviderId {
        self.id
    }

    fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        let (scalar_work, gram_work) = {
            let mut state = self.state.lock().expect("backing state poisoned");
            state.proposals += 1;
            (state.scalar_work, state.gram_work)
        };
        match request {
            AccessRequest::Structured(bound) => {
                if let Some(slots) = self.scalar(bound) {
                    offers.push(AccessOffer::new(
                        AccessId {
                            provider: self.id,
                            operation: self.prepare_operation(term, 0),
                        },
                        term,
                        AccessKind::ExactMembership,
                        Coverage::Complete,
                        ProofStrength::Exact,
                        slots.len() as u64,
                        scalar_work + (self.rows.len() as f64).log2(),
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
                            provider: self.id,
                            operation: self.prepare_operation(term, 1),
                        },
                        term,
                        AccessKind::CandidateMembership,
                        Coverage::CandidateOnly,
                        ProofStrength::Candidate,
                        slots.len() as u64,
                        gram_work + request.gram_count(BigramAlphabet::Unicode) as f64,
                        0.0,
                        true,
                        false,
                    ));
                }
            }
            AccessRequest::Text(fts) => {
                if !self.supports_text {
                    return Ok(());
                }
                if let Some(slots) = self.grams(&fts.gram_request) {
                    offers.push(AccessOffer::new(
                        AccessId {
                            provider: self.id,
                            operation: self.prepare_operation(term, 2),
                        },
                        term,
                        AccessKind::CandidateMembership,
                        Coverage::CandidateOnly,
                        ProofStrength::Candidate,
                        slots.len().min(input.input_len as usize) as u64,
                        gram_work + fts.gram_request.gram_count(BigramAlphabet::Unicode) as f64,
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
        term: TermId,
        request: AccessRequest<'_>,
        choice: AccessId,
        input: SlotInput<'a>,
        execution: &AccessExecution<'_>,
    ) -> AccessResult<SlotInput<'a>> {
        execution.check()?;
        if choice.provider != self.id || input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        if input.is_empty() {
            return Ok(input);
        }
        let Some((operation_term, operation)) = self.operation(choice.operation) else {
            return Err(AccessError::StaleHandle);
        };
        if operation_term != term {
            return Err(AccessError::StaleHandle);
        }
        let slots = match (operation, request) {
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
        self.state
            .lock()
            .expect("backing state poisoned")
            .applications += 1;
        let observation_input = input.iter().collect::<Vec<_>>();
        let input_ref = input
            .effective_bitmap()
            .ok_or_else(|| AccessError::Internal("effective input missing".into()))?;
        let mut bitmap = RoaringBitmap::new();
        for slot in slots {
            if input_ref.contains(slot) {
                bitmap.insert(slot);
            }
        }
        self.state
            .lock()
            .expect("backing state poisoned")
            .observations
            .push(BackingObservation {
                kind: operation,
                input: observation_input,
                output: bitmap.iter().collect(),
            });
        Ok(input.replace_slots(SlotSet::from_owned(bitmap)))
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
        let restricted_len = restricted.len();
        let result = provider
            .apply(
                structured_term,
                AccessRequest::Structured(&term),
                structured_choice.id(),
                restricted,
                &AccessExecution { context: None },
            )
            .unwrap();
        assert_eq!(result.iter().collect::<Vec<_>>(), [3]);
        assert_eq!(restricted_len, 1);
        assert_eq!(result.len(), 1);
    }
}
