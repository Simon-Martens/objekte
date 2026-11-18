//! Native FTS provider adapter.

use std::cell::Cell;

use roaring::RoaringBitmap;

use crate::executor::{
    native_estimate_fts_candidates, native_fts_candidates, native_segment_estimate_fts_candidates,
    native_segment_fts_candidates,
};
use crate::build::IndexedCollectionSegment;
use crate::query_execution::domain::domain_id;
use crate::query_predicate::TermId;
use crate::query_predicate::PreparedPredicates;
use crate::repository::CollectionState;

use super::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength,
    ProviderId,
};
use super::membership::{DomainId, SlotInput, SlotSet};

pub(crate) const NATIVE_TEXT_PROVIDER: ProviderId = ProviderId(2);

pub(crate) struct NativeTextProvider<'a> {
    collection: &'a CollectionState,
    domain: DomainId,
    proposals: Cell<u64>,
    applications: Cell<u64>,
}

pub(crate) struct NativeSegmentTextProvider<'a> {
    segment: &'a IndexedCollectionSegment,
    collection: &'a CollectionState,
    domain: DomainId,
}

impl<'a> NativeSegmentTextProvider<'a> {
    pub(crate) fn new(
        segment: &'a IndexedCollectionSegment,
        collection: &'a CollectionState,
    ) -> Self {
        Self {
            segment,
            collection,
            domain: domain_id(
                collection.generation.collection.0,
                collection.generation.generation,
                segment.id.get() as u32,
            ),
        }
    }

    pub(crate) fn input_for<'b>(&self, input: &'b RoaringBitmap) -> SlotInput<'b> {
        SlotInput::unrestricted(self.domain, input)
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }
}

impl<'a> NativeTextProvider<'a> {
    pub(crate) fn new(collection: &'a CollectionState, segment_index: Option<u32>) -> Self {
        Self {
            collection,
            domain: domain_id(
                collection.generation.collection.0,
                collection.generation.generation,
                segment_index.unwrap_or(u32::MAX),
            ),
            proposals: Cell::new(0),
            applications: Cell::new(0),
        }
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }

    pub(crate) fn input_for<'b>(&self, input: &'b RoaringBitmap) -> SlotInput<'b> {
        SlotInput::unrestricted(self.domain, input)
    }

    pub(crate) fn counter_snapshot(&self) -> (u64, u64) {
        (self.proposals.get(), self.applications.get())
    }
}

impl IndexAccessProvider for NativeTextProvider<'_> {
    fn provider_id(&self) -> ProviderId {
        NATIVE_TEXT_PROVIDER
    }

    fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        self.proposals.set(self.proposals.get() + 1);
        let AccessRequest::Text(fts) = request else {
            return Ok(());
        };
        let Some((output, work)) = native_estimate_fts_candidates(self.collection, fts) else {
            return Ok(());
        };
        offers.push(AccessOffer::new(
            AccessId {
                provider: NATIVE_TEXT_PROVIDER,
                operation: OperationId(0),
            },
            term,
            AccessKind::CandidateMembership,
            if fts.fields.is_empty() || fts.fields.len() == 1 {
                Coverage::Complete
            } else {
                Coverage::CandidateOnly
            },
            ProofStrength::Candidate,
            output.min(input.input_len),
            work,
            0.0,
            true,
            false,
        ));
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
        if choice.provider != NATIVE_TEXT_PROVIDER || choice.operation != OperationId(0) {
            return Err(AccessError::StaleHandle);
        }
        if input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        let AccessRequest::Text(fts) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let result = native_fts_candidates(self.collection, fts)
            .ok_or(AccessError::CapabilityUnavailable)?;
        self.applications.set(self.applications.get() + 1);
        #[cfg(test)]
        crate::query_access::contract::note_test_application();
        let input_ref = input.input();
        Ok(SlotInput::new(
            self.domain,
            input_ref,
            SlotSet::from_owned(result & input_ref),
        ))
    }
}

impl IndexAccessProvider for NativeSegmentTextProvider<'_> {
    fn provider_id(&self) -> ProviderId {
        NATIVE_TEXT_PROVIDER
    }

    fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        let AccessRequest::Text(fts) = request else {
            return Ok(());
        };
        let Some((output, work)) =
            native_segment_estimate_fts_candidates(self.segment, self.collection, fts)
        else {
            return Ok(());
        };
        offers.push(AccessOffer::new(
            AccessId {
                provider: NATIVE_TEXT_PROVIDER,
                operation: OperationId(1),
            },
            term,
            AccessKind::CandidateMembership,
            Coverage::CandidateOnly,
            ProofStrength::Candidate,
            output.min(input.input_len),
            work,
            0.0,
            true,
            false,
        ));
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
        if choice.provider != NATIVE_TEXT_PROVIDER || choice.operation != OperationId(1) {
            return Err(AccessError::StaleHandle);
        }
        if input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        let AccessRequest::Text(fts) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let result =
            native_segment_fts_candidates(self.segment, self.collection, fts)
                .map_err(AccessError::Internal)?
                .ok_or(AccessError::CapabilityUnavailable)?;
        #[cfg(test)]
        crate::query_access::contract::note_test_application();
        let input_ref = input.input();
        Ok(SlotInput::new(
            self.domain,
            input_ref,
            SlotSet::from_owned(result & input_ref),
        ))
    }
}

pub(crate) fn prepared_text_candidates(
    predicates: &PreparedPredicates,
    term: TermId,
    collection: &CollectionState,
) -> Result<Option<RoaringBitmap>, String> {
    let provider = NativeTextProvider::new(collection, None);
    let mut offers = OfferList::new();
    predicates
        .propose(
            term,
            &provider,
            InputEstimate {
                domain_len: collection.stats.record_count as u64,
                input_len: collection.stats.record_count as u64,
                mean_record_bytes: collection.stats.mean_record_bytes(),
                mean_records_per_file: collection.stats.mean_records_per_file(),
                file_count: collection.stats.file_count as u64,
            },
            &mut offers,
        )
        .map_err(|error| error.to_string())?;
    let Some(chosen) = crate::query_plan::selection::choose_offer(
        term,
        &offers,
        collection.stats.record_count as u64,
    )
    .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let input = (0..collection.stats.record_count as u32).collect::<RoaringBitmap>();
    let applied = predicates
        .restrict(
            term,
            &provider,
            chosen.id(),
            provider.input_for(&input),
            &AccessExecution { context: None },
        )
        .map_err(|error| error.to_string())?;
    match applied.into_slots() {
        SlotSet::Owned(bitmap) => Ok(Some(bitmap)),
        SlotSet::Unrestricted => Ok(None),
        SlotSet::Empty => Ok(Some(RoaringBitmap::new())),
        _ => Err("text predicate facade returned a non-owned bitmap".into()),
    }
}
