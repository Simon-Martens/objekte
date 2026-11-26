//! Executable native offers for one published segment domain.

use std::sync::{Arc, Mutex};

use super::catalog::IndexCatalog;
use crate::index::memory::segment::IndexedCollectionSegment;
#[cfg(test)]
use crate::index::storage::file_catalog::FileCatalogReader;
use crate::index::storage::segment::SegmentPredicateAccess;
#[cfg(test)]
use crate::index::storage::text::SegmentTextAccess;
use crate::query::binding::filter::BoundTerm;
use crate::query_access::contract::{PostingId, PostingOperation};
#[cfg(test)]
use crate::query_execution::domain::domain_id;
use crate::query_execution::restrictions::AccessPathKind;
use crate::query_predicate::TermId;
#[cfg(test)]
use crate::repository::state::CollectionState;

use crate::query_access::contract::{
    AccessError, AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, AccessResult,
    Coverage, ExplainAccess, IndexAccessProvider, InputEstimate, OfferList, OperationId,
    ProofStrength, ProviderId, Restriction,
};
#[cfg(test)]
use crate::query_access::membership::DomainId;
use crate::query_access::membership::{SlotInput, SlotSet};

mod predicate;

use predicate::{native_segment_term_bounds, segment_access_path_kind, segment_text_estimate};

pub(crate) const NATIVE_SEGMENT_PROVIDER: ProviderId = ProviderId(3);

#[cfg(test)]
thread_local! {
    static LAST_STRUCTURED_OUTPUT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn record_test_structured_output(pointer: usize) {
    LAST_STRUCTURED_OUTPUT.with(|value| value.set(pointer));
}

#[cfg(test)]
pub(crate) fn test_structured_output() -> usize {
    LAST_STRUCTURED_OUTPUT.with(std::cell::Cell::get)
}

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
    segment: &'a IndexedCollectionSegment,
    access: SegmentPredicateAccess<'a>,
    operations: Mutex<Vec<PreparedSegmentOperation>>,
}

impl<'a> NativeSegmentProvider<'a> {
    pub(crate) fn for_access(
        segment: &'a IndexedCollectionSegment,
        access: SegmentPredicateAccess<'a>,
    ) -> Self {
        Self {
            segment,
            access,
            operations: Mutex::new(Vec::new()),
        }
    }

    #[cfg(test)]
    pub(crate) fn new(
        published_id: crate::index::memory::segment::SegmentId,
        segment: &'a IndexedCollectionSegment,
        collection: &'a CollectionState,
        file_catalog: Option<&'a dyn FileCatalogReader>,
    ) -> Self {
        Self::for_access(
            segment,
            SegmentPredicateAccess {
                text: SegmentTextAccess {
                    domain: domain_id(
                        collection.repository_instance,
                        collection.generation.collection.0,
                        collection.generation.generation,
                        Some(published_id.get()),
                    ),
                    instance: collection.repository_instance,
                    collection: collection.generation.collection,
                    policy: collection.text_access_policy(),
                },
                collection_name: &collection.definition.name,
                files: file_catalog,
            },
        )
    }

    #[cfg(test)]
    pub(crate) fn domain(&self) -> DomainId {
        self.access.text.domain
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
                && crate::query::binding::filter::same_bound_term(&candidate.bound, bound)
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
        if input.domain != self.access.text.domain {
            return Ok(());
        }
        let AccessRequest::Structured(bound) = request else {
            return Ok(());
        };
        let direct_posting_compatible = bound.path.root == crate::model::PathRoot::Data
            && bound.derived == crate::query::binding::filter::Derived::None
            && bound.related.is_none()
            && bound.rhs.is_none()
            && bound.operation.is_none()
            && bound.matcher.is_none()
            && !bound.query_path.has_position
            && !bound.query_path.has_explicit_array_step
            && matches!(
                bound.operator,
                crate::query::Operator::Eq
                    | crate::query::Operator::Ne
                    | crate::query::Operator::Gt
                    | crate::query::Operator::Ge
                    | crate::query::Operator::Lt
                    | crate::query::Operator::Le
            );
        let index = IndexCatalog::segment_with_bindings(
            self.segment,
            self.access.text.policy.schema,
            self.access.text.policy.bindings,
        );
        let domain = &self.segment.logical_records;
        let mut direct_output = None;
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
            let access_kind =
                segment_access_path_kind(bound, self.segment, self.access.text.policy);
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

        let source_kind = segment_access_path_kind(bound, self.segment, self.access.text.policy);
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

    fn propose_explain(
        &self,
        term: TermId,
        request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> AccessResult<()> {
        if input.domain != self.access.text.domain {
            return Ok(());
        }
        let AccessRequest::Structured(bound) = request else {
            return Ok(());
        };
        let catalog = IndexCatalog::segment_with_bindings(
            self.segment,
            self.access.text.policy.schema,
            self.access.text.policy.bindings,
        );
        for access in catalog
            .field_postings(
                &bound.path.segments,
                &bound.binding_name,
                bound.operation.is_some(),
            )
            .into_iter()
            .flatten()
        {
            let Some(proposal) = access
                .assess(bound, catalog.logical, &mut 0)
                .map_err(AccessError::Internal)?
            else {
                continue;
            };
            if !proposal.exact {
                continue;
            }
            let operation = self.operation_id(
                term,
                bound,
                SegmentPhysicalOperation::Posting {
                    access: proposal.id,
                    operation: proposal.operation,
                    exact: proposal.exact,
                },
                AccessPathKind::Posting,
            );
            offers.push(
                AccessOffer::new(
                    AccessId {
                        provider: NATIVE_SEGMENT_PROVIDER,
                        operation,
                    },
                    term,
                    AccessKind::ExactMembership,
                    Coverage::Partial {
                        covered: proposal.coverage,
                    },
                    ProofStrength::Exact,
                    proposal.output.min(input.input_len),
                    proposal.lookup_work,
                    0.0,
                    true,
                    false,
                )
                .with_explanation(ExplainAccess::Posting(proposal.id, proposal.operation)),
            );
        }
        if offers.is_empty()
            && let Some(output) =
                segment_text_estimate(self.segment, bound, self.access.text.policy)
        {
            let operation = self.operation_id(
                term,
                bound,
                SegmentPhysicalOperation::NativeFallback,
                AccessPathKind::RawBigram,
            );
            offers.push(
                AccessOffer::new(
                    AccessId {
                        provider: NATIVE_SEGMENT_PROVIDER,
                        operation,
                    },
                    term,
                    AccessKind::CandidateMembership,
                    Coverage::CandidateOnly,
                    ProofStrength::Candidate,
                    output.min(input.input_len),
                    output as f64,
                    0.0,
                    false,
                    false,
                )
                .with_explanation(ExplainAccess::Bigram),
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
        if choice.provider != NATIVE_SEGMENT_PROVIDER || input.domain() != self.access.text.domain {
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
            if prepared.term != term
                || !crate::query::binding::filter::same_bound_term(&prepared.bound, bound)
            {
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
                let effective = input.bitmap_for_native()?;
                let logical = effective.as_ref() & &self.segment.logical_records;
                let fallback = effective.as_ref() - &self.segment.logical_records;
                let bounds = native_segment_term_bounds(
                    self.segment,
                    bound,
                    self.access,
                    effective.as_ref(),
                    &logical,
                    &fallback,
                )
                .map_err(AccessError::Internal)?;
                let possible = input.replace_slots(SlotSet::from_shared(bounds.may));
                let proven = SlotInput::new(
                    self.access.text.domain,
                    possible.operation_input(),
                    SlotSet::from_shared(bounds.must),
                );
                crate::query_access::contract::Restriction::bounds(possible, proven)?
            }
            SegmentPhysicalOperation::Posting {
                access,
                operation,
                exact,
            } => {
                let effective = input.bitmap_for_native()?;
                let index = IndexCatalog::segment_with_bindings(
                    self.segment,
                    self.access.text.policy.schema,
                    self.access.text.policy.bindings,
                );
                let access = index
                    .posting(
                        &bound.path.segments,
                        &bound.binding_name,
                        bound.operation.is_some(),
                        access,
                    )
                    .ok_or(AccessError::StaleHandle)?;
                let effective_logical = effective.as_ref() & &self.segment.logical_records;
                let matches = access
                    .execute(
                        bound,
                        &self.segment.logical_records,
                        &effective_logical,
                        operation,
                    )
                    .map_err(AccessError::Internal)?
                    .ok_or(AccessError::CapabilityUnavailable)?;
                let fallback = effective.as_ref() - &self.segment.logical_records;
                if fallback.is_empty() {
                    let matches = Arc::new(matches);
                    let possible = input.replace_slots(SlotSet::from_shared(Arc::clone(&matches)));
                    Restriction {
                        possible,
                        proven: if exact && !matches.is_empty() {
                            crate::query_access::contract::Proven::All
                        } else {
                            crate::query_access::contract::Proven::None
                        },
                    }
                } else {
                    let operation_input = input.operation_input();
                    let proven = Arc::new(matches.clone());
                    let possible = input.replace_slots(SlotSet::from_owned(&matches | &fallback));
                    Restriction {
                        possible,
                        proven: if exact && !matches.is_empty() {
                            crate::query_access::contract::Proven::Subset(SlotInput::new(
                                self.access.text.domain,
                                operation_input,
                                SlotSet::from_shared(proven),
                            ))
                        } else {
                            crate::query_access::contract::Proven::None
                        },
                    }
                }
            }
        };
        crate::query_execution::request::record_observation_for_execution(
            execution,
            self.access.text.instance,
            term,
            self.access.text.domain,
            choice,
            self.access.text.collection.0,
            input_len,
            restriction.possible.len() as u64,
            match &restriction.proven {
                crate::query_access::contract::Proven::None => 0,
                crate::query_access::contract::Proven::All => restriction.possible.len() as u64,
                crate::query_access::contract::Proven::Subset(proven) => proven.len() as u64,
            },
            Some(crate::query_execution::restrictions::access_path_kind_name(
                access_kind,
            )),
        );
        #[cfg(test)]
        record_test_structured_output(
            restriction
                .possible
                .effective_bitmap()
                .map_or(0, |bitmap| bitmap as *const _ as usize),
        );
        Ok(restriction)
    }
}

#[cfg(test)]
mod tests {
    use roaring::RoaringBitmap;

    use super::*;
    use crate::query::binding::filter::BoundExpr;
    use crate::query_access::contract::{
        AccessExecution, AccessKind, AccessRequest, InputEstimate, OfferList, ProofStrength, Proven,
    };
    use crate::repository::configuration::{CollectionDefinition, RepositoryOptions};

    #[test]
    fn numeric_range_posting_excludes_retired_logical_slots_before_fallback() {
        let root = std::env::temp_dir().join(format!(
            "objekte-segment-range-restriction-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("data")).unwrap();
        std::fs::write(
            root.join("data/records.rows"),
            b"(\"id\"=\"0\",\"value\"=10)\n(\"id\"=\"1\",\"value\"=20)\n(bad)\n",
        )
        .unwrap();
        let mut definition = CollectionDefinition::folder(
            "data",
            "data",
            crate::source::test_support::line_format(),
        );
        definition.identity =
            crate::repository::configuration::IdentityPolicy::Intrinsic(vec!["id".into()]);
        definition
            .schema
            .register_field(crate::schema::FieldDefinition {
                path: vec!["value".into()],
                kind: crate::model::LogicalType::Number,
                required: false,
                nullable: true,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            })
            .unwrap();
        let index = crate::lifecycle::Index::start_build(
            &root,
            vec![definition.clone()],
            RepositoryOptions {
                verified_cache_bytes: 0,
                ..Default::default()
            },
            crate::source::test_support::fixture_registry(),
        )
        .and_then(|task| task.wait_complete())
        .unwrap();
        let view = index.view().unwrap();
        let collection = &view.repository().collections()[0];
        let published = &view.segments(crate::model::CollectionId(0)).unwrap()[0];
        let filter = crate::query::parse_filter("value >= 20").unwrap();
        let BoundExpr::Term(term) =
            crate::query::binding::filter::bind_filter(&filter, &definition.schema).unwrap()
        else {
            unreachable!()
        };
        let provider = NativeSegmentProvider::new(
            published.id,
            published.native_payload(),
            collection,
            Some(&view.0.catalog),
        );
        let domain = provider.domain();
        let term_id = TermId::new(7);
        let mut offers = OfferList::new();
        provider
            .propose(
                term_id,
                AccessRequest::Structured(&term),
                InputEstimate {
                    domain,
                    domain_len: 3,
                    input_len: 2,
                    mean_record_bytes: 20.0,
                    mean_records_per_file: 3.0,
                    file_count: 1,
                },
                &mut offers,
            )
            .unwrap();
        let chosen = offers
            .iter()
            .find(|offer| {
                offer.kind() == AccessKind::ExactMembership && offer.proof() == ProofStrength::Exact
            })
            .unwrap();
        assert_eq!(chosen.kind(), AccessKind::ExactMembership);
        assert_eq!(chosen.proof(), ProofStrength::Exact);
        let input_values: RoaringBitmap = [0, 2].into_iter().collect();
        let input = SlotInput::new(
            domain,
            &input_values,
            SlotSet::from_owned(input_values.clone()),
        );
        let restriction = provider
            .apply(
                term_id,
                AccessRequest::Structured(&term),
                chosen.id(),
                input,
                &AccessExecution { context: None },
            )
            .unwrap();
        assert_eq!(
            restriction.possible.iter().unwrap().collect::<Vec<_>>(),
            [2]
        );
        assert!(matches!(&restriction.proven, Proven::None));

        drop(view);
        index.shutdown().unwrap();
        std::fs::remove_dir_all(root).unwrap();
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

#[cfg(test)]
#[test]
fn managed_queries_and_components_use_distinct_published_domains() {
    use crate::index::memory::access::text::NativeSegmentTextProvider;
    use crate::lifecycle::{Index, IndexView};
    use crate::query::binding::{filter::BoundExpr, fts::bind_fts};
    use crate::query::options::ReturnMode;
    use crate::query::{CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, parse_filter};
    use crate::query_diagnostics::trace::ObservationStatus;
    use crate::repository::configuration::{IdentityPolicy, RepositoryOptions};

    use std::collections::BTreeSet;

    let root = std::env::temp_dir().join(format!(
        "objekte-review-query-domains-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let mut definition = crate::source::test_support::document_folder("entries", "data");
    definition.identity = IdentityPolicy::Intrinsic(vec!["id".into()]);
    definition
        .schema
        .register_field(crate::schema::FieldDefinition {
            path: vec!["year".into()],
            kind: crate::model::LogicalType::Number,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: vec![],
        })
        .unwrap();
    let schema = definition.schema.clone();
    let index = Index::start_build(
        &root,
        vec![definition],
        RepositoryOptions {
            verified_cache_bytes: 0,
            ..Default::default()
        },
        crate::source::test_support::fixture_registry(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let path = root.join("data/first.row");
    std::fs::write(
        &path,
        br#"("id"="first","year"=2000,"title"="domainneedle")"#,
    )
    .unwrap();
    let first = index.add_file(&path).unwrap();
    let second = root.join("data/second.row");
    std::fs::write(
        &second,
        br#"("id"="second","year"=2000,"title"="domainneedle")"#,
    )
    .unwrap();
    index.add_file(&second).unwrap();
    let pinned = index.view().unwrap();
    let filter = parse_filter("year = 2000").unwrap();
    let fts = FtsQuery {
        case_sensitivity: CaseSensitivity::Sensitive,
        mode: FtsMode::Literal,
        literal: "domainneedle".into(),
        pattern_parameter: None,
        fields: Vec::new(),
    };
    let assert_view = |view: &IndexView, ids: [&str; 2]| {
        let repo = view.repository();
        let collection = &repo.collections()[0];
        let expected = view
            .segment_summaries()
            .into_iter()
            .filter(|summary| summary.active)
            .map(|summary| {
                format!(
                    "{:?}",
                    domain_id(
                        collection.repository_instance,
                        summary.collection.0,
                        collection.generation.generation,
                        Some(summary.id.get())
                    )
                )
            })
            .collect::<BTreeSet<_>>();
        assert_eq!(expected.len(), 2);
        for query in [(Some(filter.clone()), None), (None, Some(fts.clone()))] {
            let (result, trace) = view
                .explain_analyze(&Query {
                    collections: vec!["entries".into()],
                    filter: query.0,
                    fts: query.1,
                    options: QueryOptions {
                        sort: Vec::new(),
                        limit: 10,
                        offset: 0,
                        count: true,
                        return_mode: ReturnMode::Locator,
                        evidence: false,
                    },
                })
                .unwrap();
            let hits: BTreeSet<_> = result
                .hits
                .iter()
                .map(|hit| hit.record_id.as_str())
                .collect();
            assert_eq!(hits, ids.into_iter().collect());
            let actual = trace
                .iter()
                .flat_map(|trace| &trace.observations)
                .filter(|event| {
                    event.status == ObservationStatus::Executed && event.input_records > 0
                })
                .map(|event| event.domain.clone())
                .collect::<BTreeSet<_>>();
            assert_eq!(actual, expected);
        }
        let segments = view.segments(crate::model::CollectionId(0)).unwrap();
        for segment in segments.iter() {
            assert!(
                crate::index::storage::segment::SegmentReader::components(segment)
                    .unwrap()
                    .iter()
                    .all(|desc| desc.segment == segment.id)
            );
        }
    };
    assert_view(&pinned, ["first", "second"]);
    let collection = &pinned.repository().collections()[0];
    let segments = pinned.segments(crate::model::CollectionId(0)).unwrap();
    let [left, right] = segments.as_slice() else {
        panic!("two independent placements")
    };
    let left_structured = NativeSegmentProvider::new(
        left.id,
        left.native_payload(),
        collection,
        Some(&pinned.0.catalog),
    );
    let right_structured = NativeSegmentProvider::new(
        right.id,
        right.native_payload(),
        collection,
        Some(&pinned.0.catalog),
    );
    let left_text = NativeSegmentTextProvider::new(left.id, left.native_payload(), collection);
    let right_text = NativeSegmentTextProvider::new(right.id, right.native_payload(), collection);
    let mask = left.active.bitmap().unwrap();
    let BoundExpr::Term(term) =
        crate::query::binding::filter::bind_filter(&filter, &schema).unwrap()
    else {
        panic!("expected structured term");
    };
    let bound_fts = bind_fts(&fts, &schema).unwrap();
    let term_id = TermId::new(0);
    for (provider, domain, request, foreign) in [
        (
            &right_structured as &dyn IndexAccessProvider,
            right_structured.domain(),
            AccessRequest::Structured(&term),
            SlotInput::new(
                left_structured.domain(),
                &mask,
                SlotSet::from_owned(mask.clone()),
            ),
        ),
        (
            &right_text as &dyn IndexAccessProvider,
            right_text.domain(),
            AccessRequest::Text(&bound_fts),
            left_text.input_for(&mask),
        ),
    ] {
        let mut offers = OfferList::new();
        provider
            .propose(
                term_id,
                request,
                InputEstimate {
                    domain,
                    domain_len: 1,
                    input_len: 1,
                    mean_record_bytes: 40.0,
                    mean_records_per_file: 1.0,
                    file_count: 1,
                },
                &mut offers,
            )
            .unwrap();
        let offer = offers.iter().next().expect("native offer");
        assert!(
            provider
                .apply(
                    term_id,
                    request,
                    offer.id(),
                    foreign,
                    &AccessExecution { context: None }
                )
                .is_err()
        );
    }
    let replacement = root.join("data/replacement.row");
    std::fs::write(
        &replacement,
        br#"("id"="updated","year"=2000,"title"="domainneedle")"#,
    )
    .unwrap();
    index.update_file(first.file, &replacement).unwrap();
    assert_view(&index.view().unwrap(), ["updated", "second"]);
    assert_view(&pinned, ["first", "second"]);
    std::fs::remove_dir_all(root).unwrap();
}
