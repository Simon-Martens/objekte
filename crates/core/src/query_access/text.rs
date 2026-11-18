//! Native FTS provider adapter.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use roaring::RoaringBitmap;

use crate::build::IndexedCollectionSegment;
use crate::executor::{
    native_estimate_fts_candidates, native_fts_candidates, native_segment_estimate_fts_candidates,
    native_segment_fts_candidates,
};
use crate::query_execution::domain::domain_id;
use crate::query_predicate::PreparedPredicates;
use crate::query_predicate::TermId;
use crate::repository::CollectionState;

use super::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength,
    ProviderId, Restriction,
};
use super::membership::{DomainId, SlotInput, SlotSet};
use super::registry::ProviderCatalog;

pub(crate) const NATIVE_TEXT_PROVIDER: ProviderId = ProviderId(2);

pub(crate) struct NativeTextProvider<'a> {
    collection: &'a CollectionState,
    domain: DomainId,
    instance_id: u64,
    operations: Mutex<TextProviderState>,
    proposals: AtomicU64,
    applications: AtomicU64,
}

#[derive(Default)]
struct TextProviderState {
    operations: Vec<TermId>,
    next_operation: u32,
}

pub(crate) struct NativeSegmentTextProvider<'a> {
    segment: &'a IndexedCollectionSegment,
    collection: &'a CollectionState,
    domain: DomainId,
    instance_id: u64,
    operations: Mutex<TextProviderState>,
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
                collection.repository_instance,
                collection.generation.collection.0,
                collection.generation.generation,
                Some(segment.id.get()),
            ),
            instance_id: collection.repository_instance,
            operations: Mutex::new(TextProviderState::default()),
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
        Self::with_instance(collection, segment_index, 0)
    }

    pub(crate) fn with_instance(
        collection: &'a CollectionState,
        segment_index: Option<u32>,
        instance_id: u64,
    ) -> Self {
        Self {
            collection,
            domain: domain_id(
                instance_id,
                collection.generation.collection.0,
                collection.generation.generation,
                segment_index.map(u64::from),
            ),
            instance_id,
            operations: Mutex::new(TextProviderState::default()),
            proposals: AtomicU64::new(0),
            applications: AtomicU64::new(0),
        }
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }

    pub(crate) fn input_for<'b>(&self, input: &'b RoaringBitmap) -> SlotInput<'b> {
        SlotInput::unrestricted(self.domain, input)
    }

    pub(crate) fn counter_snapshot(&self) -> (u64, u64) {
        (
            self.proposals.load(Ordering::Relaxed),
            self.applications.load(Ordering::Relaxed),
        )
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
        self.proposals.fetch_add(1, Ordering::Relaxed);
        if input.domain != self.domain {
            return Ok(());
        }
        let AccessRequest::Text(fts) = request else {
            return Ok(());
        };
        let Some((output, work)) = native_estimate_fts_candidates(self.collection, fts) else {
            return Ok(());
        };
        let operation = {
            let mut state = self
                .operations
                .lock()
                .expect("text provider state poisoned");
            let operation = state.next_operation;
            state.next_operation += 1;
            state.operations.push(term);
            operation
        };
        offers.push(AccessOffer::new(
            AccessId {
                provider: NATIVE_TEXT_PROVIDER,
                operation: OperationId(operation),
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
    ) -> AccessResult<Restriction<'a>> {
        execution.check()?;
        let input_count = input.len() as u64;
        if choice.provider != NATIVE_TEXT_PROVIDER
            || self
                .operations
                .lock()
                .expect("text provider state poisoned")
                .operations
                .get(choice.operation.0 as usize)
                != Some(&_term)
        {
            return Err(AccessError::StaleHandle);
        }
        if input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        if input.is_empty() {
            return Ok(Restriction::candidate(input));
        }
        let AccessRequest::Text(fts) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let slots = {
            let result = native_fts_candidates(self.collection, fts)
                .ok_or(AccessError::CapabilityUnavailable)?;
            let input_ref = input
                .effective_bitmap()
                .ok_or_else(|| AccessError::Internal("effective input missing".into()))?;
            SlotSet::from_owned(result & input_ref)
        };
        self.applications.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        crate::query_access::contract::note_test_application();
        let result = input.replace_slots(slots);
        let output_count = result.len() as u64;
        crate::query_execution::request::record_observation(
            self.instance_id,
            _term,
            self.domain,
            choice,
            self.collection.generation.collection.0,
            input_count,
            output_count,
            0,
        );
        Ok(Restriction::candidate(result))
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
        if input.domain != self.domain {
            return Ok(());
        }
        let AccessRequest::Text(fts) = request else {
            return Ok(());
        };
        let Some((output, work)) =
            native_segment_estimate_fts_candidates(self.segment, self.collection, fts)
        else {
            return Ok(());
        };
        let operation = {
            let mut state = self
                .operations
                .lock()
                .expect("text provider state poisoned");
            let operation = state.next_operation;
            state.next_operation += 1;
            state.operations.push(term);
            operation
        };
        offers.push(AccessOffer::new(
            AccessId {
                provider: NATIVE_TEXT_PROVIDER,
                operation: OperationId(operation),
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
    ) -> AccessResult<Restriction<'a>> {
        execution.check()?;
        if choice.provider != NATIVE_TEXT_PROVIDER
            || self
                .operations
                .lock()
                .expect("text provider state poisoned")
                .operations
                .get(choice.operation.0 as usize)
                != Some(&_term)
        {
            return Err(AccessError::StaleHandle);
        }
        if input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        if input.is_empty() {
            return Ok(Restriction::candidate(input));
        }
        let AccessRequest::Text(fts) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let slots = {
            let result = native_segment_fts_candidates(self.segment, self.collection, fts)
                .map_err(AccessError::Internal)?
                .ok_or(AccessError::CapabilityUnavailable)?;
            let input_ref = input
                .effective_bitmap()
                .ok_or_else(|| AccessError::Internal("effective input missing".into()))?;
            SlotSet::from_owned(result & input_ref)
        };
        let input_count = input.len() as u64;
        #[cfg(test)]
        crate::query_access::contract::note_test_application();
        let result = input.replace_slots(slots);
        let output_count = result.len() as u64;
        crate::query_execution::request::record_observation(
            self.instance_id,
            _term,
            self.domain,
            choice,
            self.collection.generation.collection.0,
            input_count,
            output_count,
            0,
        );
        Ok(Restriction::candidate(result))
    }
}

pub(crate) fn prepared_text_candidates(
    predicates: &PreparedPredicates,
    term: TermId,
    collection: &CollectionState,
    instance_id: u64,
    input: &RoaringBitmap,
) -> Result<Option<RoaringBitmap>, String> {
    let provider = NativeTextProvider::with_instance(collection, None, instance_id);
    #[cfg(test)]
    let test_providers = crate::query_execution::request::test_access_providers(instance_id);
    let mut providers: smallvec::SmallVec<[&dyn IndexAccessProvider; 4]> =
        smallvec::SmallVec::new();
    #[cfg(test)]
    let suppress_native =
        crate::query_execution::request::test_native_access_suppressed(instance_id);
    #[cfg(not(test))]
    let suppress_native = false;
    if !suppress_native {
        providers.push(&provider);
    }
    #[cfg(test)]
    for test_provider in &test_providers {
        providers.push(test_provider.as_ref());
    }
    let catalog = ProviderCatalog::new(providers);
    let input_estimate = InputEstimate {
        domain: provider.domain(),
        domain_len: collection.stats.record_count as u64,
        input_len: input.len(),
        mean_record_bytes: collection.stats.mean_record_bytes(),
        mean_records_per_file: collection.stats.mean_records_per_file(),
        file_count: collection.stats.file_count as u64,
    };
    let offers = catalog
        .propose_prepared(predicates, term, input_estimate)
        .map_err(|error| error.to_string())?;
    let Some(chosen) = crate::query_plan::selection::choose_offer(term, &offers, input.len())
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let applied = catalog
        .apply(
            term,
            predicates
                .request(term)
                .map_err(|error| error.to_string())?,
            chosen.id(),
            provider.input_for(input),
            &AccessExecution { context: None },
        )
        .map_err(|error| error.to_string())?
        .possible;
    match applied.into_slots() {
        SlotSet::Owned(bitmap) => Ok(Some(bitmap)),
        SlotSet::Unrestricted => Ok(None),
        SlotSet::Empty => Ok(Some(RoaringBitmap::new())),
        _ => Err("text predicate facade returned a non-owned bitmap".into()),
    }
}

pub(crate) fn prepared_text_estimate(
    predicates: &PreparedPredicates,
    term: TermId,
    collection: &CollectionState,
    instance_id: u64,
    input_len: u64,
) -> Result<Option<(u64, f64)>, String> {
    let provider = NativeTextProvider::with_instance(collection, None, instance_id);
    #[cfg(test)]
    let test_providers = crate::query_execution::request::test_access_providers(instance_id);
    let mut providers: smallvec::SmallVec<[&dyn IndexAccessProvider; 4]> =
        smallvec::SmallVec::new();
    #[cfg(test)]
    let suppress_native =
        crate::query_execution::request::test_native_access_suppressed(instance_id);
    #[cfg(not(test))]
    let suppress_native = false;
    if !suppress_native {
        providers.push(&provider);
    }
    #[cfg(test)]
    for test_provider in &test_providers {
        providers.push(test_provider.as_ref());
    }
    let catalog = ProviderCatalog::new(providers);
    let offers = catalog
        .propose_prepared(
            predicates,
            term,
            InputEstimate {
                domain: provider.domain(),
                domain_len: collection.stats.record_count as u64,
                input_len,
                mean_record_bytes: collection.stats.mean_record_bytes(),
                mean_records_per_file: collection.stats.mean_records_per_file(),
                file_count: collection.stats.file_count as u64,
            },
        )
        .map_err(|error| error.to_string())?;
    Ok(
        crate::query_plan::selection::choose_offer(term, &offers, input_len)
            .map_err(|error| error.to_string())?
            .map(|offer| (offer.estimated_output(), offer.estimated_work(input_len))),
    )
}
