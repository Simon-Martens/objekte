//! Native in-memory adapters for structured and text access.

use std::cell::{Cell, RefCell};

use crate::executor::{
    AccessPathEstimate, AccessPathKind, enumerate_paths, execute_path,
};
use crate::query_execution::domain::domain_id;
use crate::query_predicate::TermId;
use crate::repository::{CollectionState, Repository};

use super::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength,
    ProviderId,
};
use super::membership::{DomainId, SlotInput, SlotSet};

pub(crate) const NATIVE_COLLECTION_PROVIDER: ProviderId = ProviderId(1);

#[derive(Default)]
pub(crate) struct ProviderCounters {
    pub(crate) proposals: Cell<u64>,
    pub(crate) applications: Cell<u64>,
    pub(crate) dictionary_keys_visited: Cell<u64>,
}

/// Adapter around the existing monomorphic native lookup kernels. The planner
/// receives only `AccessOffer` values; concrete postings, bigrams and columns
/// remain behind `apply`.
pub(crate) struct NativeCollectionProvider<'a> {
    repo: Option<&'a Repository>,
    collection: &'a CollectionState,
    domain: DomainId,
    input_len: u64,
    paths: RefCell<Vec<AccessPathEstimate>>,
    counters: ProviderCounters,
}

impl<'a> NativeCollectionProvider<'a> {
    pub(crate) fn new(
        repo: &'a Repository,
        collection: &'a CollectionState,
        domain_len: u64,
        segment_index: Option<u32>,
    ) -> Self {
        Self {
            repo: Some(repo),
            collection,
            domain: domain_id(
                collection.generation.collection.0,
                collection.generation.generation,
                segment_index.unwrap_or(u32::MAX),
            ),
            input_len: domain_len,
            paths: RefCell::new(Vec::new()),
            counters: ProviderCounters::default(),
        }
    }

    pub(crate) fn for_estimation(collection: &'a CollectionState, domain_len: u64) -> Self {
        Self {
            repo: None,
            collection,
            domain: domain_id(
                collection.generation.collection.0,
                collection.generation.generation,
                u32::MAX,
            ),
            input_len: domain_len,
            paths: RefCell::new(Vec::new()),
            counters: ProviderCounters::default(),
        }
    }

    pub(crate) fn counter_snapshot(&self) -> (u64, u64) {
        (
            self.counters.proposals.get(),
            self.counters.applications.get(),
        )
    }

    pub(crate) fn path_for(&self, id: AccessId) -> Option<AccessPathEstimate> {
        (id.provider == NATIVE_COLLECTION_PROVIDER)
            .then(|| self.paths.borrow().get(id.operation.0 as usize).cloned())
            .flatten()
    }

    pub(crate) fn prepared_paths(&self) -> Vec<AccessPathEstimate> {
        self.paths.borrow().clone()
    }

    pub(crate) fn dictionary_keys_visited(&self) -> u64 {
        self.counters.dictionary_keys_visited.get()
    }

    pub(crate) fn input_for<'b>(&self, input: &'b roaring::RoaringBitmap) -> SlotInput<'b> {
        SlotInput::unrestricted(self.domain, input)
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }
}

impl IndexAccessProvider for NativeCollectionProvider<'_> {
    fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        self.counters.proposals.set(self.counters.proposals.get() + 1);
        let AccessRequest::Structured(bound) = request else {
            return Ok(());
        };
        let mut visits = 0u64;
        let paths = enumerate_paths(
            self.collection,
            bound,
            input.input_len.min(self.input_len),
            &mut visits,
        )
        .map_err(AccessError::Internal)?;
        self.counters
            .dictionary_keys_visited
            .set(self.counters.dictionary_keys_visited.get() + visits);
        let coverage_len = self.collection.stats.record_count as u64;
        let mut prepared = self.paths.borrow_mut();
        prepared.clear();
        prepared.extend(paths.iter().cloned());
        for (index, path) in paths.iter().enumerate() {
            offers.push(AccessOffer::new(
                AccessId {
                    provider: NATIVE_COLLECTION_PROVIDER,
                    operation: OperationId(index as u32),
                },
                term,
                access_kind(path.kind),
                coverage(path, coverage_len),
                if path.exact {
                    ProofStrength::Exact
                } else {
                    ProofStrength::Candidate
                },
                path.estimated_output,
                path.estimated_work,
                0.0,
                false,
                path.kind == AccessPathKind::SortColumn,
            ));
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
        if choice.provider != NATIVE_COLLECTION_PROVIDER {
            return Err(AccessError::StaleHandle);
        }
        if input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        execution.check()?;
        let AccessRequest::Structured(bound) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let path = self
            .path_for(choice)
            .ok_or(AccessError::StaleHandle)?;
        self.counters
            .applications
            .set(self.counters.applications.get() + 1);
        let result = execute_path(
            self.repo.ok_or(AccessError::CapabilityUnavailable)?,
            self.collection,
            bound,
            input.input(),
            input.input(),
            &path,
        )
        .map_err(AccessError::Internal)?;
        let _ = term;
        let input_ref = input.input();
        let slots = result.map_or(SlotSet::Unrestricted, SlotSet::from_owned);
        Ok(SlotInput::new(self.domain, input_ref, slots))
    }
}

fn access_kind(kind: AccessPathKind) -> AccessKind {
    match kind {
        AccessPathKind::Posting => AccessKind::ExactMembership,
        AccessPathKind::CustomBigram | AccessPathKind::RawBigram => {
            AccessKind::CandidateMembership
        }
        AccessPathKind::Metadata | AccessPathKind::FilePathIndex => AccessKind::Metadata,
        AccessPathKind::SortColumn => AccessKind::OrderedColumn,
        AccessPathKind::RelationTargetFirst | AccessPathKind::RelationSourceRestricted => {
            AccessKind::Relation
        }
        AccessPathKind::Scanner => AccessKind::SourceScan,
    }
}

fn coverage(path: &AccessPathEstimate, domain_len: u64) -> Coverage {
    if u64::from(path.coverage) >= domain_len {
        Coverage::Complete
    } else if path.exact {
        Coverage::Partial {
            covered: path.coverage,
        }
    } else {
        Coverage::CandidateOnly
    }
}
