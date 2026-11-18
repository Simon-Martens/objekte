//! Executable native offers for one published segment domain.

use std::sync::{Arc, Mutex};

use crate::build::IndexedCollectionSegment;
use crate::executor::{AccessPathKind, segment_access_path_kind};
use crate::index_catalog::{IndexCatalog, PostingId, PostingOperation};
use crate::lifecycle_catalog::FileCatalog;
use crate::query_execution::domain::domain_id;
use crate::query_predicate::TermId;
use crate::reference::BoundTerm;
use crate::repository::CollectionState;

use super::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength,
    ProviderId, Restriction,
};
use super::membership::{DomainId, SlotInput, SlotSet};

pub(crate) const NATIVE_SEGMENT_PROVIDER: ProviderId = ProviderId(3);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SegmentPhysicalOperation {
    Posting {
        access: PostingId,
        operation: PostingOperation,
        exact: bool,
    },
    NativeFallback,
}

struct PreparedSegmentOperation {
    term: TermId,
    bound: BoundTerm,
    physical: SegmentPhysicalOperation,
    access_kind: AccessPathKind,
}

pub(crate) struct NativeSegmentProvider<'a> {
    pub(crate) segment: &'a IndexedCollectionSegment,
    pub(crate) collection: &'a CollectionState,
    file_catalog: Option<&'a FileCatalog>,
    pub(crate) domain: DomainId,
    pub(crate) instance_id: u64,
    operations: Mutex<Vec<PreparedSegmentOperation>>,
}

impl<'a> NativeSegmentProvider<'a> {
    pub(crate) fn new(
        segment: &'a IndexedCollectionSegment,
        collection: &'a CollectionState,
        file_catalog: Option<&'a FileCatalog>,
    ) -> Self {
        Self {
            segment,
            collection,
            file_catalog,
            domain: domain_id(
                collection.repository_instance,
                collection.generation.collection.0,
                collection.generation.generation,
                Some(segment.id.get()),
            ),
            instance_id: collection.repository_instance,
            operations: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }

    pub(crate) fn instance_id(&self) -> u64 {
        self.instance_id
    }

    fn operation_id(
        &self,
        term: TermId,
        bound: &BoundTerm,
        physical: SegmentPhysicalOperation,
        access_kind: AccessPathKind,
    ) -> OperationId {
        let mut operations = self
            .operations
            .lock()
            .expect("segment provider state poisoned");
        if let Some((index, _)) = operations.iter().enumerate().find(|(_, candidate)| {
            candidate.term == term
                && candidate.physical == physical
                && crate::reference::same_bound_term(&candidate.bound, bound)
        }) {
            return OperationId(index as u32);
        }
        let index = operations.len();
        operations.push(PreparedSegmentOperation {
            term,
            bound: bound.clone(),
            physical,
            access_kind,
        });
        OperationId(index as u32)
    }
}

impl IndexAccessProvider for NativeSegmentProvider<'_> {
    fn provider_id(&self) -> ProviderId {
        NATIVE_SEGMENT_PROVIDER
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
        let AccessRequest::Structured(bound) = request else {
            return Ok(());
        };
        let direct_posting_compatible = bound.path.root == crate::model::PathRoot::Data
            && bound.derived == crate::reference::Derived::None
            && bound.related.is_none()
            && bound.rhs.is_none()
            && bound.operation.is_none()
            && bound.matcher.is_none()
            && !bound.query_path.has_position
            && !bound.query_path.has_explicit_array_step
            && self.segment.sorting.column(&bound.path.display()).is_err()
            && matches!(
                bound.operator,
                crate::query::Operator::Eq
                    | crate::query::Operator::Ne
                    | crate::query::Operator::Gt
                    | crate::query::Operator::Ge
                    | crate::query::Operator::Lt
                    | crate::query::Operator::Le
            );
        let index = IndexCatalog::segment(self.segment, self.collection);
        let domain = &self.segment.logical_records;
        let mut direct_output = None;
        let mut direct_exact = false;
        for access in direct_posting_compatible
            .then_some(())
            .into_iter()
            .flat_map(|_| {
                index
                    .field_postings(
                        &bound.path.segments,
                        &bound.binding_name,
                        bound.operation.is_some(),
                    )
                    .into_iter()
                    .flatten()
            })
        {
            let mut visits = 0u64;
            let Some(proposal) = access
                .assess(bound, domain, &mut visits)
                .map_err(AccessError::Internal)?
            else {
                continue;
            };
            let physical = SegmentPhysicalOperation::Posting {
                access: proposal.id,
                operation: proposal.operation,
                exact: proposal.exact,
            };
            direct_output = Some(
                direct_output.map_or(proposal.output, |current: u64| current.min(proposal.output)),
            );
            direct_exact |= proposal.exact;
            let access_kind = segment_access_path_kind(bound, self.segment, self.collection);
            let operation = self.operation_id(term, bound, physical, access_kind);
            offers.push(AccessOffer::new(
                AccessId {
                    provider: NATIVE_SEGMENT_PROVIDER,
                    operation,
                },
                term,
                if proposal.exact {
                    AccessKind::ExactMembership
                } else {
                    AccessKind::CandidateMembership
                },
                if proposal.coverage >= self.segment.record_count() as u32 {
                    Coverage::Complete
                } else {
                    Coverage::Partial {
                        covered: proposal.coverage,
                    }
                },
                if proposal.exact {
                    ProofStrength::Exact
                } else {
                    ProofStrength::Candidate
                },
                proposal.output.min(input.input_len),
                proposal.lookup_work.max(0.0),
                0.0,
                proposal.exact,
                access_kind == AccessPathKind::SortColumn,
            ));
        }

        if direct_exact {
            return Ok(());
        }

        let source_kind = segment_access_path_kind(bound, self.segment, self.collection);
        let fallback_operation = self.operation_id(
            term,
            bound,
            SegmentPhysicalOperation::NativeFallback,
            source_kind,
        );
        offers.push(AccessOffer::new(
            AccessId {
                provider: NATIVE_SEGMENT_PROVIDER,
                operation: fallback_operation,
            },
            term,
            access_kind_to_contract(source_kind),
            Coverage::Input,
            ProofStrength::Candidate,
            input.input_len,
            direct_output.map_or_else(
                || source_kind_cost(source_kind),
                |output| 1.0 + output as f64 * 0.01,
            ),
            0.0,
            false,
            source_kind == AccessPathKind::SortColumn,
        ));
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
        execution.check()?;
        if choice.provider != NATIVE_SEGMENT_PROVIDER || input.domain() != self.domain {
            return Err(AccessError::StaleHandle);
        }
        let AccessRequest::Structured(bound) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let (physical, access_kind) = {
            let operations = self
                .operations
                .lock()
                .expect("segment provider state poisoned");
            let prepared = operations
                .get(choice.operation.0 as usize)
                .ok_or(AccessError::StaleHandle)?;
            if prepared.term != term || !crate::reference::same_bound_term(&prepared.bound, bound) {
                return Err(AccessError::StaleHandle);
            }
            (prepared.physical, prepared.access_kind)
        };
        let input_len = input.len() as u64;
        if input.is_empty() {
            return Ok(Restriction::exact(input));
        }
        let restriction = match physical {
            SegmentPhysicalOperation::NativeFallback => {
                let effective = input
                    .effective_bitmap()
                    .ok_or_else(|| AccessError::Internal("effective input missing".into()))?;
                let logical = effective & &self.segment.logical_records;
                let fallback = effective - &self.segment.logical_records;
                let bounds = crate::executor::segment_expr_bounds(
                    self.segment,
                    &crate::reference::BoundExpr::Term(bound.clone()),
                    self.collection,
                    effective,
                    &logical,
                    &fallback,
                    self.file_catalog,
                )
                .map_err(AccessError::Internal)?;
                let possible = input.replace_slots(SlotSet::from_shared(bounds.may));
                let proven = SlotInput::new(
                    self.domain,
                    possible.operation_input(),
                    SlotSet::from_shared(bounds.must),
                );
                super::contract::Restriction::bounds(possible, proven)
            }
            SegmentPhysicalOperation::Posting {
                access,
                operation,
                exact,
            } => {
                let effective = input
                    .effective_bitmap()
                    .ok_or_else(|| AccessError::Internal("effective input missing".into()))?;
                let index = IndexCatalog::segment(self.segment, self.collection);
                let access = index
                    .posting(
                        &bound.path.segments,
                        &bound.binding_name,
                        bound.operation.is_some(),
                        access,
                    )
                    .ok_or(AccessError::StaleHandle)?;
                let matches = access
                    .execute(bound, &self.segment.logical_records, effective, operation)
                    .map_err(AccessError::Internal)?
                    .ok_or(AccessError::CapabilityUnavailable)?;
                let fallback = effective - &self.segment.logical_records;
                if fallback.is_empty() {
                    let matches = Arc::new(matches);
                    let possible = input.replace_slots(SlotSet::from_shared(Arc::clone(&matches)));
                    Restriction {
                        possible,
                        proven: if exact {
                            super::contract::Proven::All
                        } else {
                            super::contract::Proven::None
                        },
                    }
                } else {
                    let operation_input = input.operation_input();
                    let proven = Arc::new(matches.clone());
                    let possible = input.replace_slots(SlotSet::from_owned(&matches | &fallback));
                    Restriction {
                        possible,
                        proven: if exact {
                            super::contract::Proven::Subset(SlotInput::new(
                                self.domain,
                                operation_input,
                                SlotSet::from_shared(proven),
                            ))
                        } else {
                            super::contract::Proven::None
                        },
                    }
                }
            }
        };
        crate::query_execution::request::record_observation_for_execution(
            execution,
            self.instance_id,
            term,
            self.domain,
            choice,
            self.collection.generation.collection.0,
            input_len,
            restriction.possible.len() as u64,
            match &restriction.proven {
                super::contract::Proven::None => 0,
                super::contract::Proven::All => restriction.possible.len() as u64,
                super::contract::Proven::Subset(proven) => proven.len() as u64,
            },
            Some(crate::executor::access_path_kind_name(access_kind)),
        );
        Ok(restriction)
    }
}

fn access_kind_to_contract(kind: AccessPathKind) -> AccessKind {
    match kind {
        AccessPathKind::Metadata
        | AccessPathKind::FilePathIndex
        | AccessPathKind::Posting
        | AccessPathKind::SortColumn => AccessKind::ExactMembership,
        AccessPathKind::RelationTargetFirst | AccessPathKind::RelationSourceRestricted => {
            AccessKind::Relation
        }
        AccessPathKind::CustomBigram | AccessPathKind::RawBigram => AccessKind::CandidateMembership,
        AccessPathKind::Scanner => AccessKind::SourceScan,
    }
}

fn source_kind_cost(kind: AccessPathKind) -> f64 {
    match kind {
        AccessPathKind::Metadata
        | AccessPathKind::FilePathIndex
        | AccessPathKind::Posting
        | AccessPathKind::SortColumn => 1.0,
        AccessPathKind::CustomBigram | AccessPathKind::RawBigram => 2.0,
        AccessPathKind::RelationTargetFirst | AccessPathKind::RelationSourceRestricted => 4.0,
        AccessPathKind::Scanner => 8.0,
    }
}
