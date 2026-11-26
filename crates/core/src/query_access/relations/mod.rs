//! Provider-owned alternatives for forward relation semijoins.

use std::sync::{Arc, Mutex};

use roaring::RoaringBitmap;

use crate::index::storage::publication::PublishedSegment;
use crate::inventory::ActiveRecords;
use crate::model::CollectionId;
use crate::query_access::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength,
    ProviderId, Restriction,
};
use crate::query_access::membership::{DomainId, SlotInput, SlotSet};
use crate::query_access::relations::semijoin::{
    SemijoinWork, Strategy, candidates_with_strategy_with_execution,
    planning_estimate_with_segments, qualified_candidates_with_strategy,
};
use crate::query_execution::domain::domain_id;
use crate::query_execution::relations::view::MergedRelationView;
use crate::query_execution::restrictions::{AccessPathKind, access_path_kind_name};
use crate::query_predicate::TermId;
use crate::repository::Repository;

pub(crate) mod semijoin;

pub(crate) const NATIVE_RELATION_PROVIDER: ProviderId = ProviderId(4);

struct RelationOperation {
    term: TermId,
    strategy: Strategy,
    path: crate::query_execution::restrictions::AccessPathEstimate,
}

/// One pinned relation catalog. Offers carry the selected traversal strategy;
/// application performs that exact strategy and returns checked membership.
pub(crate) struct NativeRelationProvider<'a> {
    repo: &'a Repository,
    segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
    collection: CollectionId,
    domain: DomainId,
    operations: Mutex<Vec<RelationOperation>>,
    work: Mutex<SemijoinWork>,
}

impl<'a> NativeRelationProvider<'a> {
    pub(crate) fn new(
        repo: &'a Repository,
        segments: &'a [Arc<Vec<Arc<PublishedSegment>>>],
        collection: CollectionId,
    ) -> Result<Self, String> {
        let state = repo
            .collections
            .get(collection.0 as usize)
            .ok_or("relation provider source collection missing")?;
        Ok(Self {
            repo,
            segments,
            collection,
            domain: domain_id(
                repo.instance_id,
                state.collection_id().0,
                state.base_generation(),
                None,
            ),
            operations: Mutex::new(Vec::new()),
            work: Mutex::new(SemijoinWork::default()),
        })
    }

    pub(crate) fn domain(&self) -> DomainId {
        self.domain
    }

    pub(crate) fn take_work(&self) -> SemijoinWork {
        std::mem::take(
            &mut *self
                .work
                .lock()
                .expect("relation provider work state poisoned"),
        )
    }

    fn operation_for(&self, choice: AccessId, term: TermId) -> AccessResult<Strategy> {
        if choice.provider != NATIVE_RELATION_PROVIDER {
            return Err(AccessError::StaleHandle);
        }
        self.operations
            .lock()
            .expect("relation provider operation state poisoned")
            .get(choice.operation.0 as usize)
            .filter(|operation| operation.term == term)
            .map(|operation| operation.strategy)
            .ok_or(AccessError::StaleHandle)
    }

    pub(crate) fn path_for(
        &self,
        choice: AccessId,
    ) -> Option<crate::query_execution::restrictions::AccessPathEstimate> {
        (choice.provider == NATIVE_RELATION_PROVIDER)
            .then(|| {
                self.operations
                    .lock()
                    .expect("relation provider operation state poisoned")
                    .get(choice.operation.0 as usize)
                    .map(|operation| operation.path.clone())
            })
            .flatten()
    }

    pub(crate) fn alternatives_for(
        &self,
        choice: AccessId,
        term: TermId,
    ) -> Vec<crate::query_execution::restrictions::AccessPathEstimate> {
        let selected_kind = self.path_for(choice).map(|path| path.kind);
        self.operations
            .lock()
            .expect("relation provider operation state poisoned")
            .iter()
            .filter(|operation| {
                operation.term == term
                    && selected_kind.is_none_or(|selected| operation.path.kind != selected)
            })
            .map(|operation| operation.path.clone())
            .collect()
    }

    fn apply_qualified(
        &self,
        term: &crate::query::binding::filter::BoundTerm,
        input: &RoaringBitmap,
        strategy: Strategy,
        execution: &AccessExecution<'_>,
    ) -> Result<Option<(RoaringBitmap, SemijoinWork)>, String> {
        let source = term
            .related
            .as_ref()
            .and_then(|related| related.hops.first())
            .map(|hop| hop.from)
            .ok_or("relation provider term has no source hop")?;
        if source != self.collection {
            return Err("relation provider source domain mismatch".into());
        }
        let empty_active = (0..self.repo.collections.len())
            .map(|_| ActiveRecords::empty())
            .collect::<Vec<_>>();
        let view =
            MergedRelationView::new(self.repo, &empty_active, self.segments, &self.repo.catalog)?;
        let mut native_input = crate::index::segment::SegmentRecordSet::default();
        for slot in input.iter() {
            match view.native_address_for_flat_slot(source, crate::model::RecordSlot(slot))? {
                Some(address) => {
                    native_input.insert(address);
                }
                None => {
                    let state = &self.repo.collections[source.0 as usize];
                    if !state.logical_records.contains(slot) {
                        continue;
                    }
                    return Ok(None);
                }
            }
        }
        let (native_matches, work) = qualified_candidates_with_strategy(
            &view,
            term,
            &native_input,
            strategy,
            execution.context,
        )?;
        let mut matches = RoaringBitmap::new();
        for address in native_matches.iter() {
            let Some(record) = view.record(view.view_address(address)?)? else {
                continue;
            };
            let collection = &self.repo.collections[address.collection.0 as usize];
            if let Some(slot) = collection.id_lookup.get(&record.record.id) {
                matches.insert(slot.0);
            }
        }
        Ok(Some((matches, work)))
    }
}

impl IndexAccessProvider for NativeRelationProvider<'_> {
    fn provider_id(&self) -> ProviderId {
        NATIVE_RELATION_PROVIDER
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
        if bound.related.is_none() {
            return Ok(());
        }
        let (selected, selected_path, alternatives) =
            planning_estimate_with_segments(self.repo, self.segments, bound, input.input_len)
                .map_err(AccessError::Internal)?;
        let mut operations = self
            .operations
            .lock()
            .expect("relation provider operation state poisoned");
        let mut push =
            |strategy: Strategy, path: crate::query_execution::restrictions::AccessPathEstimate| {
                let operation = operations.len();
                operations.push(RelationOperation {
                    term,
                    strategy,
                    path: path.clone(),
                });
                offers.push(AccessOffer::new(
                    AccessId {
                        provider: NATIVE_RELATION_PROVIDER,
                        operation: OperationId(operation as u32),
                    },
                    term,
                    AccessKind::Relation,
                    Coverage::Complete,
                    ProofStrength::Exact,
                    path.estimated_output,
                    path.estimated_work,
                    0.0,
                    false,
                    false,
                ));
                debug_assert!(matches!(
                    path.kind,
                    AccessPathKind::RelationTargetFirst | AccessPathKind::RelationSourceRestricted
                ));
            };
        push(selected, selected_path);
        for path in alternatives {
            push(
                if selected == Strategy::TargetFirst {
                    Strategy::SourceRestricted
                } else {
                    Strategy::TargetFirst
                },
                path,
            );
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
        execution.check()?;
        if input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        let strategy = self.operation_for(choice, term)?;
        let AccessRequest::Structured(bound) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let current = input.bitmap_for_native();
        let current_len = current.len() as u64;
        if current.is_empty() {
            return Ok(Restriction::exact(input));
        }
        let (matches, work) = match self
            .apply_qualified(bound, current.as_ref(), strategy, execution)
            .map_err(AccessError::Internal)?
        {
            Some(result) => result,
            None => candidates_with_strategy_with_execution(
                self.repo,
                self.segments,
                bound,
                current.as_ref(),
                strategy,
                execution.context,
            )
            .map_err(AccessError::Internal)?,
        };
        self.work
            .lock()
            .expect("relation provider work state poisoned")
            .add(work);
        let result = input.replace_slots(SlotSet::from_owned(matches));
        let output_len = result.len() as u64;
        crate::query_execution::request::record_observation_with_kind(
            self.repo.instance_id,
            term,
            self.domain,
            choice,
            self.collection.0,
            current_len,
            output_len,
            output_len,
            Some(access_path_kind_name(match strategy {
                Strategy::TargetFirst => AccessPathKind::RelationTargetFirst,
                Strategy::SourceRestricted => AccessPathKind::RelationSourceRestricted,
            })),
        );
        Ok(Restriction::exact(result))
    }
}
