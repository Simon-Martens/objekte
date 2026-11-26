//! Native FTS provider adapter.

use std::sync::Mutex;

use roaring::RoaringBitmap;

use super::catalog::IndexCatalog;
use crate::index::segment::IndexedCollectionSegment;
use crate::index::storage::segment::{RecordBacking, SegmentReader};
use crate::index::storage::text::SegmentTextAccess;
use crate::index::storage::text::TextAccessPolicy;
use crate::model::PathRoot;
use crate::query::{FtsMode, binding::fts::BoundFts};
use crate::query_execution::domain::domain_id;
use crate::query_execution::restrictions::raw_covers_identity_field;
use crate::query_predicate::PreparedPredicates;
use crate::query_predicate::TermId;
use crate::repository::state::CollectionState;
use smallvec::SmallVec;

use crate::query_access::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength,
    ProviderId, Restriction,
};
use crate::query_access::membership::{DomainId, SlotInput, SlotSet};
use crate::query_access::registry::ProviderCatalog;

pub(crate) const NATIVE_TEXT_PROVIDER: ProviderId = ProviderId(2);

#[cfg(test)]
thread_local! {
    static LAST_TEXT_INPUT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn test_text_input() -> usize {
    LAST_TEXT_INPUT.with(std::cell::Cell::get)
}

pub(crate) struct NativeTextProvider<'a> {
    collection: &'a CollectionState,
    domain: DomainId,
    instance_id: u64,
    operations: Mutex<TextProviderState>,
}

#[derive(Default)]
struct TextProviderState {
    operations: Vec<TermId>,
    next_operation: u32,
}

pub(crate) struct NativeSegmentTextProvider<'a> {
    segment: &'a IndexedCollectionSegment,
    access: SegmentTextAccess<'a>,
    operations: Mutex<TextProviderState>,
}

impl<'a> NativeSegmentTextProvider<'a> {
    pub(crate) fn for_access(
        segment: &'a IndexedCollectionSegment,
        access: SegmentTextAccess<'a>,
    ) -> Self {
        Self {
            segment,
            access,
            operations: Mutex::new(TextProviderState::default()),
        }
    }

    #[cfg(test)]
    pub(crate) fn new(
        published_id: crate::index::segment::SegmentId,
        segment: &'a IndexedCollectionSegment,
        collection: &'a CollectionState,
    ) -> Self {
        let instance = collection.repository_instance;
        let collection_id = collection.collection_id();
        Self::for_access(
            segment,
            SegmentTextAccess {
                domain: domain_id(
                    instance,
                    collection_id.0,
                    collection.base_generation(),
                    Some(published_id.get()),
                ),
                instance,
                collection: collection_id,
                policy: collection.text_access_policy(),
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn input_for<'b>(&self, input: &'b RoaringBitmap) -> SlotInput<'b> {
        SlotInput::unrestricted(self.access.domain, input)
    }

    #[cfg(test)]
    pub(crate) fn domain(&self) -> DomainId {
        self.access.domain
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
                collection.collection_id().0,
                collection.base_generation(),
                segment_index.map(u64::from),
            ),
            instance_id,
            operations: Mutex::new(TextProviderState::default()),
        }
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }

    pub(crate) fn input_for<'b>(&self, input: &'b RoaringBitmap) -> SlotInput<'b> {
        SlotInput::unrestricted(self.domain, input)
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
            let input_ref = input.bitmap_for_native();
            SlotSet::from_owned(result & input_ref.as_ref())
        };
        #[cfg(test)]
        crate::query_access::contract::note_test_application();
        let result = input.replace_slots(slots);
        let output_count = result.len() as u64;
        crate::query_execution::request::record_observation_for_execution(
            execution,
            self.instance_id,
            _term,
            self.domain,
            choice,
            self.collection.collection_id().0,
            input_count,
            output_count,
            0,
            None,
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
        if input.domain != self.access.domain {
            return Ok(());
        }
        let AccessRequest::Text(fts) = request else {
            return Ok(());
        };
        let Some((output, work)) =
            native_segment_estimate_fts_candidates(self.segment, self.access.policy, fts)
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
        if input.domain() != self.access.domain {
            return Err(AccessError::DomainMismatch);
        }
        if input.is_empty() {
            return Ok(Restriction::candidate(input));
        }
        let AccessRequest::Text(fts) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let input_ref = input.bitmap_for_native();
        #[cfg(test)]
        LAST_TEXT_INPUT.with(|value| value.set(input_ref.as_ref() as *const _ as usize));
        let slots = native_segment_fts_candidates_restricted(
            self.segment,
            self.access.policy,
            fts,
            input_ref.as_ref(),
        )
        .map_err(AccessError::Internal)?
        .map_or(SlotSet::Unrestricted, SlotSet::from_owned);
        let input_count = input.len() as u64;
        #[cfg(test)]
        crate::query_access::contract::note_test_application();
        let result = input.replace_slots(slots);
        let output_count = result.len() as u64;
        crate::query_execution::request::record_observation_for_execution(
            execution,
            self.access.instance,
            _term,
            self.access.domain,
            choice,
            self.access.collection.0,
            input_count,
            output_count,
            0,
            None,
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
    if matches!(applied.slots(), SlotSet::Unrestricted) {
        return Ok(None);
    }
    Ok(Some(applied.into_owned_members()))
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

fn native_segment_fts_candidates_restricted(
    segment: &IndexedCollectionSegment,
    policy: TextAccessPolicy<'_>,
    fts: &BoundFts,
    input: &RoaringBitmap,
) -> Result<Option<RoaringBitmap>, String> {
    let indexes = IndexCatalog::segment_with_bindings(segment, policy.schema, policy.bindings);
    let Some(prepared) = prepare_fts_accesses(
        &indexes,
        policy,
        fts,
        segment.raw.record_count as usize == segment.record_count(),
        segment.logical_records.len() != segment.record_count() as u64,
        segment_raw_covers_identity_field(segment, policy),
        segment.record_count() as u64,
    ) else {
        return Ok(None);
    };
    prepared.candidates_checked_restricted(input)
}

fn native_segment_estimate_fts_candidates(
    segment: &IndexedCollectionSegment,
    policy: TextAccessPolicy<'_>,
    fts: &BoundFts,
) -> Option<(u64, f64)> {
    let indexes = IndexCatalog::segment_with_bindings(segment, policy.schema, policy.bindings);
    prepare_fts_accesses(
        &indexes,
        policy,
        fts,
        segment.raw.record_count as usize == segment.record_count(),
        segment.logical_records.len() != segment.record_count() as u64,
        segment_raw_covers_identity_field(segment, policy),
        segment.record_count() as u64,
    )?
    .estimate()
}

pub(crate) fn segment_raw_covers_identity_field(
    segment: &IndexedCollectionSegment,
    policy: TextAccessPolicy<'_>,
) -> bool {
    let representation_covers = match segment.backing_kind() {
        RecordBacking::FileRange => policy.logical_text_complete,
        RecordBacking::Retained | RecordBacking::Provider => segment.raw_covers_logical_fields,
    };
    representation_covers
        && policy.schema.raw.is_none()
        && segment.raw.record_count as usize == segment.record_count()
}

/// A prepared text access is a set of borrowed candidate providers. The same
/// compatibility and coverage assessment feeds estimation and execution;
/// grams and regex conditions never claim positive field proof.
struct TextProbe<'a, 'b> {
    index: &'a crate::index::bigram::BigramIndex,
    request: &'b crate::index::bigram::BigramRequest,
}

struct PreparedFtsAccess<'a, 'b> {
    probes: SmallVec<[TextProbe<'a, 'b>; 2]>,
    regex: Option<&'b crate::query::regex::RegexCondition>,
    literal_work: f64,
    max_count: u64,
}

impl PreparedFtsAccess<'_, '_> {
    fn estimate(&self) -> Option<(u64, f64)> {
        let mut count = 0u64;
        let mut work = self.literal_work;
        for probe in &self.probes {
            if let Some(condition) = self.regex {
                let (next_count, next_work) = condition.estimate(probe.index)?;
                count = count.saturating_add(next_count);
                work += next_work;
            } else {
                count = count.saturating_add(probe.index.estimate_prepared(probe.request)?);
            }
        }
        Some((count.min(self.max_count), work))
    }

    fn candidates(&self) -> Option<RoaringBitmap> {
        let mut may = RoaringBitmap::new();
        for probe in &self.probes {
            may |= if let Some(condition) = self.regex {
                condition.candidates(probe.index)?
            } else {
                probe.index.candidates_prepared(probe.request)?
            };
        }
        Some(may)
    }

    fn candidates_checked_restricted(
        &self,
        input: &RoaringBitmap,
    ) -> Result<Option<RoaringBitmap>, String> {
        let mut may = RoaringBitmap::new();
        for probe in &self.probes {
            let next = if let Some(condition) = self.regex {
                condition
                    .candidates_checked(probe.index)?
                    .map(|candidates| candidates & input)
            } else {
                probe
                    .index
                    .candidates_prepared_restricted(probe.request, Some(input))
            };
            let Some(next) = next else {
                return Ok(None);
            };
            may |= next;
        }
        Ok(Some(may & input))
    }
}

fn prepare_fts_accesses<'a, 'b>(
    indexes: &IndexCatalog<'a>,
    policy: TextAccessPolicy<'_>,
    fts: &'b BoundFts,
    raw_complete: bool,
    has_fallback: bool,
    raw_field_compatible: bool,
    max_count: u64,
) -> Option<PreparedFtsAccess<'a, 'b>> {
    if fts.fields.iter().any(|field| field.related.is_some()) {
        return None;
    }
    let regex = if matches!(fts.mode, FtsMode::Regex(_)) {
        Some(fts.fallback_regex.as_ref()?.condition()?)
    } else {
        None
    };
    let mut probes = SmallVec::new();
    let literal_work = if fts.fields.is_empty() {
        if !raw_complete
            || (regex.is_none()
                && fts
                    .raw_matcher
                    .as_ref()
                    .is_some_and(|matcher| !matcher.bigram_compatible()))
            || policy
                .schema
                .raw
                .as_ref()
                .is_some_and(|binding| !binding.operation.complete_coverage())
        {
            return None;
        }
        probes.push(TextProbe {
            index: indexes.raw_text,
            request: &fts.gram_request,
        });
        if has_fallback {
            probes.push(TextProbe {
                index: indexes.fallback_text,
                request: &fts.fallback_gram_request,
            });
        }
        if regex.is_none() {
            fts.literal.len() as f64
        } else {
            0.0
        }
    } else {
        for field in &fts.fields {
            if field.path.root != PathRoot::Data
                || field
                    .matcher
                    .as_ref()
                    .is_some_and(|matcher| !matcher.bigram_compatible())
            {
                return None;
            }
            let index = indexes.field_text_for(
                policy.schema,
                &field.path.segments,
                &field.binding_name,
                field.operation.is_some(),
                policy.logical_text_complete,
                raw_field_compatible,
            )?;
            probes.push(TextProbe {
                index,
                request: &field.gram_request,
            });
        }
        if regex.is_none() {
            fts.fields
                .iter()
                .map(|field| field.literal.len() as f64)
                .sum()
        } else {
            0.0
        }
    };
    Some(PreparedFtsAccess {
        probes,
        regex,
        literal_work,
        max_count,
    })
}

fn prepare_native_base_fts<'a>(
    collection: &'a CollectionState,
    fts: &'a BoundFts,
) -> Option<PreparedFtsAccess<'a, 'a>> {
    if collection.base_records.is_some() {
        return None;
    }
    let indexes = IndexCatalog::collection(collection);
    #[cfg(test)]
    let contains_fallback = collection.logical_records.len() != collection.base_slot_count() as u64;
    #[cfg(not(test))]
    let contains_fallback = false;
    prepare_fts_accesses(
        &indexes,
        collection.text_access_policy(),
        fts,
        true,
        contains_fallback,
        raw_covers_identity_field(collection).is_ok(),
        collection.stats.record_count as u64,
    )
}

pub(crate) fn native_fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<RoaringBitmap> {
    prepare_native_base_fts(collection, fts)?.candidates()
}

pub(crate) fn native_estimate_fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<(u64, f64)> {
    prepare_native_base_fts(collection, fts)?.estimate()
}
