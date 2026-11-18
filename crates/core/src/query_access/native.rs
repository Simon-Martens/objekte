//! Native in-memory adapters for structured and text access.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::executor::{AccessPathEstimate, AccessPathKind, enumerate_paths, execute_path};
use crate::query_execution::domain::domain_id;
use crate::query_predicate::TermId;
use crate::repository::{CollectionState, Repository};

use super::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength,
    ProviderId, Restriction,
};
use super::membership::{DomainId, SlotInput, SlotSet};

pub(crate) const NATIVE_COLLECTION_PROVIDER: ProviderId = ProviderId(1);

#[derive(Default)]
pub(crate) struct ProviderCounters {
    pub(crate) proposals: AtomicU64,
    pub(crate) applications: AtomicU64,
    pub(crate) dictionary_keys_visited: AtomicU64,
}

/// Adapter around the existing monomorphic native lookup kernels. The planner
/// receives only `AccessOffer` values; concrete postings, bigrams and columns
/// remain behind `apply`.
pub(crate) struct NativeCollectionProvider<'a> {
    repo: Option<&'a Repository>,
    collection: &'a CollectionState,
    domain: DomainId,
    input_len: u64,
    prepared: Mutex<PreparedNativePaths>,
    counters: ProviderCounters,
}

#[derive(Default)]
struct PreparedNativePaths {
    paths: Vec<AccessPathEstimate>,
    terms: Vec<TermId>,
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
                repo.instance_id,
                collection.generation.collection.0,
                collection.generation.generation,
                segment_index.map(u64::from),
            ),
            input_len: domain_len,
            prepared: Mutex::new(PreparedNativePaths::default()),
            counters: ProviderCounters::default(),
        }
    }

    pub(crate) fn for_estimation(collection: &'a CollectionState, domain_len: u64) -> Self {
        Self {
            repo: None,
            collection,
            domain: domain_id(
                0,
                collection.generation.collection.0,
                collection.generation.generation,
                None,
            ),
            input_len: domain_len,
            prepared: Mutex::new(PreparedNativePaths::default()),
            counters: ProviderCounters::default(),
        }
    }

    pub(crate) fn counter_snapshot(&self) -> (u64, u64) {
        (
            self.counters.proposals.load(Ordering::Relaxed),
            self.counters.applications.load(Ordering::Relaxed),
        )
    }

    pub(crate) fn path_for(&self, id: AccessId) -> Option<AccessPathEstimate> {
        (id.provider == NATIVE_COLLECTION_PROVIDER)
            .then(|| {
                self.prepared
                    .lock()
                    .expect("native provider state poisoned")
                    .paths
                    .get(id.operation.0 as usize)
                    .cloned()
            })
            .flatten()
    }

    fn path_for_term(&self, id: AccessId, term: TermId) -> Option<AccessPathEstimate> {
        let prepared = self
            .prepared
            .lock()
            .expect("native provider state poisoned");
        if id.provider != NATIVE_COLLECTION_PROVIDER
            || prepared.terms.get(id.operation.0 as usize) != Some(&term)
        {
            return None;
        }
        prepared.paths.get(id.operation.0 as usize).cloned()
    }

    pub(crate) fn prepared_paths(&self) -> Vec<AccessPathEstimate> {
        self.prepared
            .lock()
            .expect("native provider state poisoned")
            .paths
            .clone()
    }

    pub(crate) fn dictionary_keys_visited(&self) -> u64 {
        self.counters
            .dictionary_keys_visited
            .load(Ordering::Relaxed)
    }

    pub(crate) fn input_for<'b>(&self, input: &'b roaring::RoaringBitmap) -> SlotInput<'b> {
        SlotInput::unrestricted(self.domain, input)
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }
}

impl IndexAccessProvider for NativeCollectionProvider<'_> {
    fn provider_id(&self) -> ProviderId {
        NATIVE_COLLECTION_PROVIDER
    }

    fn propose(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        self.counters.proposals.fetch_add(1, Ordering::Relaxed);
        if input.domain != self.domain {
            return Ok(());
        }
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
            .fetch_add(visits, Ordering::Relaxed);
        let coverage_len = self.collection.stats.record_count as u64;
        let start = {
            let mut prepared = self
                .prepared
                .lock()
                .expect("native provider state poisoned");
            let start = prepared.paths.len();
            prepared.paths.extend(paths.iter().cloned());
            prepared
                .terms
                .extend(std::iter::repeat_n(term, paths.len()));
            start
        };
        for (index, path) in paths.iter().enumerate() {
            let (startup_work, input_work) =
                crate::query_plan::selection::split_input_dependent_cost(
                    path.estimated_work,
                    input.input_len,
                );
            offers.push(AccessOffer::new(
                AccessId {
                    provider: NATIVE_COLLECTION_PROVIDER,
                    operation: OperationId((start + index) as u32),
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
                startup_work,
                input_work,
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
    ) -> AccessResult<Restriction<'a>> {
        if choice.provider != NATIVE_COLLECTION_PROVIDER {
            return Err(AccessError::StaleHandle);
        }
        if input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        execution.check()?;
        let input_count = input.len() as u64;
        if input.is_empty() {
            return Ok(Restriction::exact(input));
        }
        let AccessRequest::Structured(bound) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let path = self
            .path_for_term(choice, term)
            .ok_or(AccessError::StaleHandle)?;
        self.counters.applications.fetch_add(1, Ordering::Relaxed);
        #[cfg(test)]
        crate::query_access::contract::note_test_application();
        let slots = {
            let input_ref = input
                .effective_bitmap()
                .ok_or_else(|| AccessError::Internal("effective input missing".into()))?;
            let result = execute_path(
                self.repo.ok_or(AccessError::CapabilityUnavailable)?,
                self.collection,
                bound,
                &self.collection.logical_records,
                input_ref,
                &path,
            )
            .map_err(AccessError::Internal)?;
            result.map_or(SlotSet::Unrestricted, SlotSet::from_owned)
        };
        let result = input.replace_slots(slots);
        let output_count = result.len() as u64;
        crate::query_execution::request::record_observation(
            self.repo.map_or(0, |repo| repo.instance_id),
            term,
            self.domain,
            choice,
            self.collection.generation.collection.0,
            input_count,
            output_count,
            if path.exact { output_count } else { 0 },
        );
        Ok(if path.exact {
            Restriction::exact(result)
        } else {
            Restriction::candidate(result)
        })
    }
}

fn access_kind(kind: AccessPathKind) -> AccessKind {
    match kind {
        AccessPathKind::Posting => AccessKind::ExactMembership,
        AccessPathKind::CustomBigram | AccessPathKind::RawBigram => AccessKind::CandidateMembership,
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
