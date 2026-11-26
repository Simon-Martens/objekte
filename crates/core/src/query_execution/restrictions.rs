//! Exact posting restrictions plus one residual scan over surviving records.

use crate::index::traits::segment::{
    SegmentPredicateAccess, SegmentReader, SegmentSlotFact, visit_checked_file_slots,
    visit_segment_files,
};
use crate::records::RecordIdSlot;
use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::sync::Arc;

use roaring::RoaringBitmap;
use smallvec::SmallVec;

#[cfg(test)]
use crate::index::memory::access::catalog::IndexCatalog;
use crate::index::segment::SegmentId;
use crate::index::traits::file_catalog::{FileCatalogReader, FileScalarPredicate};
use crate::index::traits::order::ScalarOrderPredicate;
use crate::index::traits::publication::PublishedSegment;
use crate::index::traits::text::SegmentTextAccess;
use crate::index::traits::transfer::StorageError;
use crate::inventory::{ActiveRecords, relative_path_bounds};
use crate::model::{FileId, LogicalType, MetadataField, Number, PathRoot, Value};
use crate::query::binding::fts::BoundFts;
use crate::query::semantics::record::RecordContext;
use crate::query::semantics::self_comparison_shape;
use crate::query::{Operator, Quantifier};
use crate::query::{
    binding::filter::BoundExpr, binding::filter::BoundTerm, binding::filter::Derived,
};
use crate::query_access::base::BaseCollectionProvider;
use crate::query_access::contract::{
    AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, Coverage,
    IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength, Proven, ProviderId,
    Restriction,
};
use crate::query_access::contract::{PostingId, PostingOperation};
use crate::query_access::membership::{
    DomainId, MembershipView, OperationInput, SlotInput, SlotSet,
};
use crate::query_access::registry::ProviderCatalog;
use crate::query_plan::selection::ordered_offer_ids;
use crate::query_predicate::{PreparedPredicates, TermId};
use crate::reference::filter::eval_term;
use crate::repository::Repository;
use crate::repository::state::CollectionState;
use crate::schema::operation::Transform;
use crate::source::LogicalTextCoverage;
use serde::Serialize;

pub(crate) const MAX_INDEXED_PATH_SCAN: usize = 8192;

#[cfg(test)]
thread_local! {
    static PREDICATE_BOUNDS_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PREDICATE_APPLICATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PREDICATE_APPLICATION_OBSERVATIONS: std::cell::RefCell<Vec<PredicateApplicationObservation>> =
        const { std::cell::RefCell::new(Vec::new()) };
    static OBSERVE_PREDICATE_APPLICATIONS: std::cell::Cell<bool> =
        const { std::cell::Cell::new(false) };
}

#[cfg(test)]
pub(crate) fn predicate_bounds_calls() -> usize {
    PREDICATE_BOUNDS_CALLS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn predicate_applications() -> usize {
    PREDICATE_APPLICATIONS.with(std::cell::Cell::get)
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct PredicateApplicationObservation {
    pub(crate) field: String,
    pub(crate) operator: crate::query::Operator,
    pub(crate) value: String,
    pub(crate) segment: u64,
    pub(crate) input: Vec<u32>,
}

#[cfg(test)]
pub(crate) fn start_predicate_application_observations() {
    PREDICATE_APPLICATION_OBSERVATIONS.with(|observations| observations.borrow_mut().clear());
    OBSERVE_PREDICATE_APPLICATIONS.with(|value| value.set(true));
}

#[cfg(test)]
pub(crate) fn take_predicate_application_observations() -> Vec<PredicateApplicationObservation> {
    OBSERVE_PREDICATE_APPLICATIONS.with(|value| value.set(false));
    PREDICATE_APPLICATION_OBSERVATIONS.with(std::cell::RefCell::take)
}

/// Conservative predicate bounds for one locally slotted delta. `may` is a
/// superset of true records; `must` is a subset. Carrying both through NOT
/// prevents an unavailable or unindexed value from becoming a false proof.
#[derive(Clone)]
pub(crate) struct SegmentCandidateBounds {
    pub may: Arc<RoaringBitmap>,
    pub must: Arc<RoaringBitmap>,
}

struct SourceVerificationProvider;

static SOURCE_VERIFICATION_PROVIDER: SourceVerificationProvider = SourceVerificationProvider;

impl IndexAccessProvider for SourceVerificationProvider {
    fn provider_id(&self) -> ProviderId {
        ProviderId(u32::MAX - 1)
    }

    fn propose(
        &self,
        term: TermId,
        _request: AccessRequest<'_>,
        input: InputEstimate,
        offers: &mut OfferList,
    ) -> crate::query_access::contract::AccessResult<()> {
        offers.push(AccessOffer::new(
            AccessId {
                provider: self.provider_id(),
                operation: OperationId(0),
            },
            term,
            AccessKind::SourceScan,
            Coverage::Input,
            ProofStrength::None,
            input.input_len,
            0.0,
            30.0,
            false,
            false,
        ));
        Ok(())
    }

    fn apply<'a>(
        &self,
        _term: TermId,
        _request: AccessRequest<'_>,
        _choice: AccessId,
        input: SlotInput<'a>,
        execution: &AccessExecution<'_>,
    ) -> crate::query_access::contract::AccessResult<Restriction<'a>> {
        execution.check()?;
        Ok(Restriction::candidate(input))
    }
}

pub(crate) fn access_path_kind_name(kind: AccessPathKind) -> &'static str {
    match kind {
        AccessPathKind::Metadata => "metadata",
        AccessPathKind::FilePathIndex => "file_path_index",
        AccessPathKind::Posting => "posting",
        AccessPathKind::SortColumn => "sort_column",
        AccessPathKind::CustomBigram => "custom_bigram",
        AccessPathKind::RawBigram => "raw_bigram",
        AccessPathKind::RelationTargetFirst => "relation_target_first",
        AccessPathKind::RelationSourceRestricted => "relation_source_restricted",
        AccessPathKind::Scanner => "scanner",
    }
}

pub(crate) fn access_path_kind_from_name(name: &str) -> Option<AccessPathKind> {
    Some(match name {
        "metadata" => AccessPathKind::Metadata,
        "file_path_index" => AccessPathKind::FilePathIndex,
        "posting" => AccessPathKind::Posting,
        "sort_column" => AccessPathKind::SortColumn,
        "custom_bigram" => AccessPathKind::CustomBigram,
        "raw_bigram" => AccessPathKind::RawBigram,
        "relation_target_first" => AccessPathKind::RelationTargetFirst,
        "relation_source_restricted" => AccessPathKind::RelationSourceRestricted,
        "scanner" => AccessPathKind::Scanner,
        _ => return None,
    })
}

#[derive(Default)]
pub(crate) struct RoutedFileRestrictions {
    terms: HashMap<TermId, Option<HashMap<SegmentId, RoaringBitmap>>>,
    pub(crate) resolved: u64,
    pub(crate) matching_files: u64,
    pub(crate) membership_probes: u64,
    pub(crate) broad_fallbacks: u64,
}

impl RoutedFileRestrictions {
    pub(crate) fn prepare(
        predicates: &PreparedPredicates,
        catalog: &dyn FileCatalogReader,
        routes: &[crate::index::traits::segment_catalog::SegmentCatalogHandle],
        segments: &[Arc<Vec<Arc<PublishedSegment>>>],
    ) -> Result<Self, String> {
        let mut prepared = Self::default();
        for leaf in predicates.leaves() {
            let Some(term) = leaf.bound_term() else {
                continue;
            };
            let Some(files) = routed_file_ids(catalog, term)? else {
                continue;
            };
            prepared.resolved += 1;
            let Some(files) = files else {
                prepared.broad_fallbacks += 1;
                prepared.terms.insert(leaf.id, None);
                continue;
            };
            prepared.matching_files += files.len() as u64;
            let by_segment = prepared
                .terms
                .entry(leaf.id)
                .or_default()
                .get_or_insert_with(HashMap::new);
            let routes = &routes[leaf.collection_index];
            for file in files.iter().copied() {
                for &id in routes
                    .segments_for_file(file)
                    .map_err(|error| error.to_string())?
                    .iter()
                {
                    let Some(position) = routes.position(id).map_err(|error| error.to_string())?
                    else {
                        continue;
                    };
                    let segment = segments[leaf.collection_index]
                        .get(position)
                        .filter(|segment| segment.id == id)
                        .ok_or("file route points to a missing segment")?;
                    prepared.membership_probes += 1;
                    visit_checked_file_slots(segment, file, &mut |slots| {
                        let selected = by_segment.entry(id).or_default();
                        for &slot in slots {
                            if segment.active.contains(slot)? {
                                selected.insert(slot.0);
                            }
                        }
                        Ok(())
                    })?;
                }
            }
        }
        Ok(prepared)
    }

    pub(crate) fn count_work(&self, counters: &mut crate::measurement::WorkCounters) {
        counters.file_restrictions_resolved = self.resolved;
        counters.file_restriction_matching_files = self.matching_files;
        counters.file_membership_lists_probed = self.membership_probes;
        counters.file_restrictions_broad = self.broad_fallbacks;
    }
}

fn routed_file_ids(
    catalog: &dyn FileCatalogReader,
    term: &BoundTerm,
) -> Result<Option<Option<Arc<[FileId]>>>, String> {
    if let Some((operator, literal)) = indexed_file_path_term(term) {
        let bounds =
            relative_path_bounds(operator, literal).ok_or("invalid indexed path comparison")?;
        let files = catalog
            .path_candidates(bounds, MAX_INDEXED_PATH_SCAN)
            .map_err(|error| error.to_string())?;
        return Ok(Some(files.map(Arc::from)));
    }
    if term.transform != Transform::Identity
        || term.period.is_some()
        || term.rhs.is_some()
        || term.related.is_some()
        || term.operation.is_some()
        || term.matcher.is_some()
        || term.multi
        || term.query_path.has_position
        || term.query_path.has_explicit_array_step
        || !matches!(
            term.operator,
            Operator::Eq | Operator::Ne | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
        )
    {
        return Ok(None);
    }
    const LIMIT: usize = 4096;
    let predicate = match (&term.path.root, term.derived.clone(), &term.literal) {
        (PathRoot::Metadata(MetadataField::FileSize), Derived::None, Value::Number(value)) => {
            FileScalarPredicate::Size(term.operator, value)
        }
        (
            PathRoot::Metadata(MetadataField::FileModified),
            Derived::None,
            Value::Datetime(value),
        ) => FileScalarPredicate::Modified(term.operator, value.unix_timestamp_nanos()),
        (PathRoot::Metadata(MetadataField::FileCreated), Derived::None, Value::Datetime(value)) => {
            FileScalarPredicate::Created(term.operator, value.unix_timestamp_nanos())
        }
        (PathRoot::Metadata(MetadataField::FileCreated), Derived::Null, Value::Bool(value))
            if matches!(
                (term.operator, value),
                (Operator::Eq, true) | (Operator::Ne, false)
            ) =>
        {
            FileScalarPredicate::UnknownCreated
        }
        _ => return Ok(None),
    };
    let candidates = catalog
        .scalar_candidates(predicate, LIMIT)
        .map_err(|error| error.to_string())?;
    Ok(Some(candidates))
}

struct SegmentPlanContext<'a> {
    segment: &'a dyn SegmentReader,
    collection: &'a CollectionState,
    access: SegmentPredicateAccess<'a>,
    provider: Option<&'a dyn IndexAccessProvider>,
    test_providers: Vec<Arc<dyn IndexAccessProvider>>,
    predicates: Option<&'a PreparedPredicates>,
    routed: Option<(&'a RoutedFileRestrictions, SegmentId)>,
    execution: Option<&'a crate::query_execution::streaming::ExecutionContext>,
    offers: HashMap<TermId, OfferList>,
    terms: Vec<(BoundTerm, TermId)>,
    next_term: u32,
}

fn with_segment_predicate_access<T>(
    segment: &dyn SegmentReader,
    access: SegmentPredicateAccess<'_>,
    select: impl FnOnce(Option<&dyn IndexAccessProvider>) -> Result<T, String>,
) -> Result<T, String> {
    let mut select = Some(select);
    let mut result = None;
    segment.with_predicate_access(access, &mut |provider| {
        let select = select.take().ok_or_else(|| {
            StorageError::invalid(
                "open segment predicate access",
                "provider callback repeated",
            )
        })?;
        result = Some(select(provider));
        Ok(())
    })?;
    result.ok_or_else(|| "segment predicate access omitted provider selection".to_owned())?
}

fn with_base_predicate_access<T>(
    repo: Option<&Repository>,
    collection: &CollectionState,
    read: impl FnOnce(Option<&dyn IndexAccessProvider>) -> Result<T, String>,
) -> Result<T, String> {
    let Some(base) = &collection.base_records else {
        return read(None);
    };
    let instance = collection.repository_instance;
    let id = collection.collection_id();
    let access = SegmentPredicateAccess {
        text: SegmentTextAccess {
            policy: collection.text_access_policy(),
            domain: crate::query_execution::domain::domain_id(
                instance,
                id.0,
                collection.base_generation(),
                None,
            ),
            instance,
            collection: id,
        },
        collection_name: &collection.definition.name,
        files: repo
            .filter(|repo| repo.repository_root().is_some())
            .map(|repo| repo.catalog.as_ref() as &dyn FileCatalogReader),
    };
    with_segment_predicate_access(base.as_ref(), access, read)
}

fn segment_text_access(
    collection: &CollectionState,
    published_id: SegmentId,
) -> SegmentTextAccess<'_> {
    let id = collection.collection_id();
    SegmentTextAccess {
        policy: collection.text_access_policy(),
        domain: crate::query_execution::domain::domain_id(
            collection.repository_instance,
            id.0,
            collection.base_generation(),
            Some(published_id.get()),
        ),
        instance: collection.repository_instance,
        collection: id,
    }
}

impl<'a> SegmentPlanContext<'a> {
    fn new(
        segment: &'a dyn SegmentReader,
        collection: &'a CollectionState,
        access: SegmentPredicateAccess<'a>,
        provider: Option<&'a dyn IndexAccessProvider>,
    ) -> Self {
        #[cfg(test)]
        let test_providers = crate::query_execution::request::all_test_access_providers();
        #[cfg(not(test))]
        let test_providers = Vec::new();
        Self {
            segment,
            collection,
            access,
            provider,
            test_providers,
            predicates: None,
            routed: None,
            execution: None,
            offers: HashMap::new(),
            terms: Vec::new(),
            next_term: 0,
        }
    }

    fn estimate(
        &self,
        term_id: TermId,
        term: &BoundTerm,
        input_len: u64,
    ) -> Result<Option<(u64, f64)>, String> {
        let offers = self
            .catalog()
            .propose(
                term_id,
                AccessRequest::Structured(term),
                self.input_estimate(input_len),
            )
            .map_err(|error| error.to_string())?;
        Ok(
            crate::query_plan::selection::choose_offer(term_id, &offers, input_len)
                .map_err(|error| error.to_string())?
                .map(|offer| (offer.estimated_output(), offer.estimated_work(input_len))),
        )
    }

    fn input_estimate(&self, input_len: u64) -> InputEstimate {
        InputEstimate {
            domain: self.access.text.domain,
            domain_len: self.segment.descriptor().slots as u64,
            input_len,
            mean_record_bytes: self.collection.stats.mean_record_bytes(),
            mean_records_per_file: self.collection.stats.mean_records_per_file(),
            file_count: self.segment.file_count() as u64,
        }
    }

    fn catalog(&self) -> ProviderCatalog<'_> {
        let mut providers: SmallVec<[&dyn IndexAccessProvider; 4]> = SmallVec::new();
        #[cfg(test)]
        let suppress_native = crate::query_execution::request::test_native_access_suppressed(
            self.access.text.instance,
        );
        #[cfg(not(test))]
        let suppress_native = false;
        if let Some(provider) = self.provider.filter(|_| !suppress_native) {
            providers.push(provider);
        } else {
            providers.push(&SOURCE_VERIFICATION_PROVIDER);
        }
        for provider in &self.test_providers {
            providers.push(provider.as_ref());
        }
        ProviderCatalog::new(providers)
    }

    fn term_id(&mut self, term: &BoundTerm) -> TermId {
        if let Some(predicates) = self.predicates
            && let Some(id) =
                predicates.term_for_bound_term(self.access.text.collection.0 as usize, term)
        {
            return id;
        }
        if let Some((_, term_id)) = self
            .terms
            .iter()
            .find(|(candidate, _)| crate::query::binding::filter::same_bound_term(candidate, term))
        {
            return *term_id;
        }
        let term_id = TermId::new(self.next_term);
        self.next_term = self
            .next_term
            .checked_add(1)
            .expect("segment predicate ID overflow");
        self.terms.push((term.clone(), term_id));
        term_id
    }

    fn execute_term(
        &mut self,
        term: &BoundTerm,
        input: OperationInput<'_>,
    ) -> Result<SegmentCandidateBounds, String> {
        let term_id = self.term_id(term);
        if let Some((routed, published_id)) = self.routed
            && let Some(selection) = routed.terms.get(&term_id)
        {
            let input = input.bitmap().map_err(|error| error.to_string())?;
            let slots = match selection {
                Some(by_segment) => by_segment
                    .get(&published_id)
                    .map_or_else(RoaringBitmap::new, |slots| slots & input.as_ref()),
                None => {
                    let catalog = self
                        .access
                        .files
                        .expect("routed restriction uses a catalog");
                    scan_routed_files(self.segment, catalog, term, input.as_ref())?
                }
            };
            return Ok(SegmentCandidateBounds::exact(slots));
        }
        if !self.offers.contains_key(&term_id) {
            let estimate = self.input_estimate(input.len() as u64);
            let offers = self
                .catalog()
                .propose(term_id, AccessRequest::Structured(term), estimate)
                .map_err(|error| error.to_string())?;
            self.offers.insert(term_id, offers);
        }
        let choice = {
            let offers = self
                .offers
                .get(&term_id)
                .expect("segment offers disappeared");
            crate::query_plan::selection::choose_offer(term_id, offers, input.len() as u64)
                .map_err(|error| error.to_string())?
                .cloned()
        };
        let Some(choice) = choice else {
            return Ok(SegmentCandidateBounds::candidate(
                input
                    .bitmap()
                    .map_err(|error| error.to_string())?
                    .into_owned(),
            ));
        };
        let restriction = self
            .catalog()
            .apply(
                term_id,
                AccessRequest::Structured(term),
                choice.id(),
                SlotInput::unrestricted(self.access.text.domain, input),
                &AccessExecution {
                    context: self.execution,
                },
            )
            .map_err(|error| error.to_string())?;
        #[cfg(test)]
        {
            PREDICATE_APPLICATIONS.with(|applications| applications.set(applications.get() + 1));
            if OBSERVE_PREDICATE_APPLICATIONS.with(std::cell::Cell::get) {
                let observation = PredicateApplicationObservation {
                    field: term.path.display(),
                    operator: term.operator,
                    value: format!("{:?}", term.literal),
                    segment: self.segment.descriptor().id.get(),
                    input: input
                        .bitmap()
                        .map_err(|error| error.to_string())?
                        .iter()
                        .collect(),
                };
                PREDICATE_APPLICATION_OBSERVATIONS
                    .with(|observations| observations.borrow_mut().push(observation));
            }
        }
        let possible = restriction.possible;
        let may = shared_bitmap_from_slot_input(possible)?;
        let must = match restriction.proven {
            Proven::None => Arc::new(RoaringBitmap::new()),
            Proven::All => Arc::clone(&may),
            Proven::Subset(proven) => shared_bitmap_from_slot_input(proven)?,
        };
        Ok(SegmentCandidateBounds { may, must })
    }

    fn record_skipped_expr(&mut self, expr: &BoundExpr) {
        match expr {
            BoundExpr::Term(term) => {
                let term_id = self.term_id(term);
                crate::query_execution::request::record_skipped_observation(
                    self.access.text.instance,
                    term_id,
                    self.access.text.domain,
                    self.access.text.collection.0,
                    0,
                );
            }
            BoundExpr::And(parts) | BoundExpr::Or(parts) => {
                for part in parts {
                    self.record_skipped_expr(part);
                }
            }
            BoundExpr::Not(inner) => self.record_skipped_expr(inner),
        }
    }
}

fn shared_bitmap_from_slot_input(input: SlotInput<'_>) -> Result<Arc<RoaringBitmap>, String> {
    let source = input.operation_input();
    Ok(match input.into_slots() {
        SlotSet::Unrestricted => Arc::new(
            source
                .bitmap()
                .map_err(|error| error.to_string())?
                .into_owned(),
        ),
        SlotSet::Empty => Arc::new(RoaringBitmap::new()),
        SlotSet::Borrowed(slots) => Arc::new(slots.clone()),
        SlotSet::Owned(slots) => Arc::new(slots),
        SlotSet::Shared(slots) => slots,
        SlotSet::Sorted(slots) => Arc::new(slots.iter().copied().collect()),
    })
}

impl SegmentCandidateBounds {
    pub(crate) fn candidate(may: RoaringBitmap) -> Self {
        Self {
            may: Arc::new(may),
            must: Arc::new(RoaringBitmap::new()),
        }
    }

    pub(crate) fn exact(matches: RoaringBitmap) -> Self {
        let matches = Arc::new(matches);
        Self {
            may: Arc::clone(&matches),
            must: matches,
        }
    }

    pub(crate) fn bounds(may: RoaringBitmap, must: RoaringBitmap) -> Self {
        if may == must {
            Self::exact(may)
        } else {
            Self {
                may: Arc::new(may),
                must: Arc::new(must),
            }
        }
    }

    pub(crate) fn unknown(domain: &RoaringBitmap) -> Self {
        Self::candidate(domain.clone())
    }
}

pub(crate) fn segment_filter_bounds(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    active: &ActiveRecords,
    expr: &BoundExpr,
    collection: &CollectionState,
) -> Result<SegmentCandidateBounds, String> {
    segment_filter_bounds_with_catalog(published_id, segment, active, expr, collection, None)
}

pub(crate) fn segment_filter_explain_offers(
    segment: &dyn SegmentReader,
    collection: &CollectionState,
    term: &BoundTerm,
    input_len: u64,
) -> Result<crate::query_access::contract::OfferList, String> {
    let access = SegmentPredicateAccess {
        text: segment_text_access(collection, segment.descriptor().id),
        collection_name: &collection.definition.name,
        files: None,
    };
    with_segment_predicate_access(segment, access, |provider| {
        let mut offers = crate::query_access::contract::OfferList::new();
        if let Some(provider) = provider {
            let plan = SegmentPlanContext::new(segment, collection, access, Some(provider));
            provider
                .propose_explain(
                    TermId::new(0),
                    AccessRequest::Structured(term),
                    plan.input_estimate(input_len),
                    &mut offers,
                )
                .map_err(|error| error.to_string())?;
        }
        Ok(offers)
    })
}

pub(crate) fn segment_filter_estimate_with_term(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    collection: &CollectionState,
    catalog: Option<&dyn FileCatalogReader>,
    term: &BoundTerm,
    input_len: u64,
) -> Result<Option<(u64, f64)>, String> {
    let access = SegmentPredicateAccess {
        text: segment_text_access(collection, published_id),
        collection_name: &collection.definition.name,
        files: catalog,
    };
    with_segment_predicate_access(segment, access, |provider| {
        let mut context = SegmentPlanContext::new(segment, collection, access, provider);
        let term_id = context.term_id(term);
        context.estimate(term_id, term, input_len)
    })
}

pub(crate) fn segment_filter_estimate(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    collection: &CollectionState,
    catalog: Option<&dyn FileCatalogReader>,
    expr: &BoundExpr,
    input_len: u64,
) -> Result<Option<(u64, f64)>, String> {
    match expr {
        BoundExpr::Term(term) => segment_filter_estimate_with_term(
            published_id,
            segment,
            collection,
            catalog,
            term,
            input_len,
        ),
        BoundExpr::And(parts) => {
            let mut best = None;
            for part in parts {
                if let Some(estimate) = segment_filter_estimate(
                    published_id,
                    segment,
                    collection,
                    catalog,
                    part,
                    input_len,
                )? {
                    best = Some(best.map_or(estimate, |current: (u64, f64)| {
                        if estimate.1 < current.1 {
                            estimate
                        } else {
                            current
                        }
                    }));
                }
            }
            Ok(best)
        }
        BoundExpr::Or(_) | BoundExpr::Not(_) => Ok(None),
    }
}

pub(crate) fn segment_filter_bounds_with_catalog(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    active: &ActiveRecords,
    expr: &BoundExpr,
    collection: &CollectionState,
    catalog: Option<&dyn FileCatalogReader>,
) -> Result<SegmentCandidateBounds, String> {
    segment_filter_bounds_with_routes(
        published_id,
        segment,
        active,
        expr,
        collection,
        catalog,
        None,
        None,
        None,
        None,
    )
}

#[cfg(test)]
pub(crate) fn segment_filter_bounds_with_input_catalog_and_predicates(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    active: &ActiveRecords,
    expr: &BoundExpr,
    collection: &CollectionState,
    catalog: Option<&dyn FileCatalogReader>,
    input: &RoaringBitmap,
    predicates: Option<&PreparedPredicates>,
    execution: Option<&crate::query_execution::streaming::ExecutionContext>,
) -> Result<SegmentCandidateBounds, String> {
    segment_filter_bounds_with_routes(
        published_id,
        segment,
        active,
        expr,
        collection,
        catalog,
        Some(input),
        predicates,
        execution,
        None,
    )
}

pub(crate) fn segment_filter_bounds_with_routes(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    active: &ActiveRecords,
    expr: &BoundExpr,
    collection: &CollectionState,
    catalog: Option<&dyn FileCatalogReader>,
    input: Option<&RoaringBitmap>,
    predicates: Option<&PreparedPredicates>,
    execution: Option<&crate::query_execution::streaming::ExecutionContext>,
    routed: Option<(&RoutedFileRestrictions, SegmentId)>,
) -> Result<SegmentCandidateBounds, String> {
    #[cfg(test)]
    PREDICATE_BOUNDS_CALLS.with(|calls| calls.set(calls.get() + 1));
    if active.slot_count() as usize != segment.descriptor().slots {
        return Err("segment active domain length mismatch".into());
    }
    if let Some(input) = input
        && !active.includes(input)?
    {
        return Err("segment restriction exceeds its active domain".into());
    }
    let access = SegmentPredicateAccess {
        text: segment_text_access(collection, published_id),
        collection_name: &collection.definition.name,
        files: catalog,
    };
    with_segment_predicate_access(segment, access, |provider| {
        let mut context = SegmentPlanContext::new(segment, collection, access, provider);
        context.predicates = predicates;
        context.execution = execution;
        context.routed = routed;
        if let Some(input) = input {
            return segment_expr_bounds_with_context(expr, active.len(), input, &mut context);
        }
        if let BoundExpr::Term(term) = expr
            && active.borrowed_bitmap().is_none()
            && active.len() != u64::from(active.slot_count())
        {
            return context.execute_term(term, OperationInput::Pinned(active));
        }
        let members;
        let input = if let Some(bitmap) = active.borrowed_bitmap() {
            bitmap
        } else {
            members = active.bitmap()?;
            &members
        };
        segment_expr_bounds_with_context(expr, active.len(), input, &mut context)
    })
}

fn append_file_members(
    segment: &dyn SegmentReader,
    file: FileId,
    matches: &mut RoaringBitmap,
) -> Result<(), StorageError> {
    visit_checked_file_slots(segment, file, &mut |slots| {
        matches.extend(slots.iter().map(|slot| slot.0));
        Ok(())
    })
}

fn scan_routed_files(
    segment: &dyn SegmentReader,
    catalog: &dyn FileCatalogReader,
    term: &BoundTerm,
    input: &RoaringBitmap,
) -> Result<RoaringBitmap, String> {
    let mut matches = RoaringBitmap::new();
    let path_term = indexed_file_path_term(term);
    visit_segment_files(segment, |file| {
        let selected = if let Some((operator, literal)) = path_term {
            catalog.file(file.id)?.is_some_and(|entry| {
                let key = crate::model::path_representation::query_path(&entry.relative_path);
                file_path_matches(&key, operator, literal)
            })
        } else {
            match (&term.path.root, &term.derived, &term.literal) {
                (PathRoot::Metadata(MetadataField::FileSize), _, Value::Number(limit)) => {
                    order_matches(
                        Number::integer(file.revision.size as i128).cmp(limit),
                        term.operator,
                    )
                }
                (PathRoot::Metadata(MetadataField::FileModified), _, Value::Datetime(limit)) => {
                    let modified = file.revision.modified_ns;
                    time::OffsetDateTime::from_unix_timestamp_nanos(modified)
                        .map_err(|error| error.to_string())?;
                    order_matches(modified.cmp(&limit.unix_timestamp_nanos()), term.operator)
                }
                (PathRoot::Metadata(MetadataField::FileCreated), derived, literal) => {
                    let Some(entry) = catalog.file(file.id)? else {
                        return Ok::<_, String>(());
                    };
                    match (derived, literal) {
                        (Derived::Null, _) => entry.created_ns.is_none(),
                        (_, Value::Datetime(limit)) => entry.created_ns.is_some_and(|created| {
                            order_matches(created.cmp(&limit.unix_timestamp_nanos()), term.operator)
                        }),
                        _ => false,
                    }
                }
                _ => return Err("unsupported routed file restriction".into()),
            }
        };
        if selected {
            append_file_members(segment, file.id, &mut matches)?;
        }
        Ok(())
    })?;
    Ok(matches & input)
}

pub(crate) fn indexed_file_path_term(term: &BoundTerm) -> Option<(Operator, &str)> {
    if term.path.root != PathRoot::Metadata(MetadataField::FilePath)
        || term.transform != Transform::Identity
        || term.derived != Derived::None
        || term.period.is_some()
        || term.rhs.is_some()
        || term.related.is_some()
        || term.operation.is_some()
        || term.matcher.is_some()
        || term.multi
        || term.query_path.has_position
        || term.query_path.has_explicit_array_step
    {
        return None;
    }
    let Value::String(literal) = &term.literal else {
        return None;
    };
    matches!(
        term.operator,
        Operator::Eq | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
    )
    .then_some((term.operator, literal.as_str()))
}

pub(crate) fn file_path_matches(value: &str, operator: Operator, literal: &str) -> bool {
    match operator {
        Operator::Eq => value == literal,
        Operator::Gt => value > literal,
        Operator::Ge => value >= literal,
        Operator::Lt => value < literal,
        Operator::Le => value <= literal,
        _ => unreachable!("indexed path comparison"),
    }
}

fn stronger_lower(candidate: &Bound<Arc<str>>, current: &Bound<Arc<str>>) -> bool {
    match (candidate, current) {
        (Bound::Unbounded, _) => false,
        (_, Bound::Unbounded) => true,
        (
            Bound::Included(new) | Bound::Excluded(new),
            Bound::Included(old) | Bound::Excluded(old),
        ) => {
            new > old
                || (new == old
                    && matches!(candidate, Bound::Excluded(_))
                    && matches!(current, Bound::Included(_)))
        }
    }
}

fn stronger_upper(candidate: &Bound<Arc<str>>, current: &Bound<Arc<str>>) -> bool {
    match (candidate, current) {
        (Bound::Unbounded, _) => false,
        (_, Bound::Unbounded) => true,
        (
            Bound::Included(new) | Bound::Excluded(new),
            Bound::Included(old) | Bound::Excluded(old),
        ) => {
            new < old
                || (new == old
                    && matches!(candidate, Bound::Excluded(_))
                    && matches!(current, Bound::Included(_)))
        }
    }
}

/// Merge direct path comparisons before touching the ordered index. Directory
/// filters commonly have a broad lower bound and a narrow upper bound.
fn combined_file_path_bounds(
    parts: &[&BoundExpr],
) -> Option<((Bound<Arc<str>>, Bound<Arc<str>>), Vec<usize>)> {
    let mut lower = Bound::Unbounded;
    let mut upper = Bound::Unbounded;
    let mut path_parts = Vec::new();
    for (index, part) in parts.iter().enumerate() {
        let BoundExpr::Term(term) = part else {
            continue;
        };
        let Some((operator, literal)) = indexed_file_path_term(term) else {
            continue;
        };
        let (next_lower, next_upper) = relative_path_bounds(operator, literal)?;
        if stronger_lower(&next_lower, &lower) {
            lower = next_lower;
        }
        if stronger_upper(&next_upper, &upper) {
            upper = next_upper;
        }
        path_parts.push(index);
    }
    (path_parts.len() >= 2).then_some(((lower, upper), path_parts))
}

pub(crate) fn segment_fts_candidates(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    collection: &CollectionState,
    fts: &BoundFts,
) -> Result<Option<RoaringBitmap>, String> {
    let input = (0..segment.descriptor().slots as u32).collect::<RoaringBitmap>();
    let bounds = segment_fts_bounds_with_term(
        published_id,
        segment,
        collection,
        fts,
        TermId::new(0),
        &input,
        None,
    )?;
    Ok(Some(bounds.may.as_ref().clone()))
}

pub(crate) fn segment_fts_bounds_with_term(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    collection: &CollectionState,
    fts: &BoundFts,
    term: TermId,
    input: &RoaringBitmap,
    context: Option<&crate::query_execution::streaming::ExecutionContext>,
) -> Result<SegmentCandidateBounds, String> {
    with_segment_text_offer(
        published_id,
        segment,
        collection,
        fts,
        term,
        input.len() as u64,
        |domain, catalog, chosen| {
            let Some(chosen) = chosen else {
                return Ok(SegmentCandidateBounds::candidate(input.clone()));
            };
            let applied = catalog
                .apply(
                    term,
                    AccessRequest::Text(fts),
                    chosen.id(),
                    SlotInput::unrestricted(domain, input),
                    &AccessExecution { context },
                )
                .map_err(|error| error.to_string())?;
            let possible = applied.possible;
            let may = shared_bitmap_from_slot_input(possible)?;
            let must = match applied.proven {
                Proven::None => Arc::new(RoaringBitmap::new()),
                Proven::All => Arc::clone(&may),
                Proven::Subset(proven) => shared_bitmap_from_slot_input(proven)?,
            };
            Ok(SegmentCandidateBounds { may, must })
        },
    )
}

pub(crate) fn segment_fts_estimate_with_term(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    collection: &CollectionState,
    fts: &BoundFts,
    term: TermId,
    input_len: u64,
) -> Result<Option<(u64, f64)>, String> {
    with_segment_text_offer(
        published_id,
        segment,
        collection,
        fts,
        term,
        input_len,
        |_, _, chosen| {
            Ok(chosen.map(|offer| (offer.estimated_output(), offer.estimated_work(input_len))))
        },
    )
}

fn with_segment_text_offer<T>(
    published_id: SegmentId,
    segment: &dyn SegmentReader,
    collection: &CollectionState,
    fts: &BoundFts,
    term: TermId,
    input_len: u64,
    selected: impl FnOnce(DomainId, &ProviderCatalog<'_>, Option<&AccessOffer>) -> Result<T, String>,
) -> Result<T, String> {
    let access = segment_text_access(collection, published_id);
    let domain = access.domain;
    #[cfg(test)]
    let test_providers = crate::query_execution::request::all_test_access_providers();
    #[cfg(test)]
    let suppress_native = crate::query_execution::request::test_native_access_suppressed(
        collection.repository_instance,
    );
    #[cfg(not(test))]
    let suppress_native = false;
    let estimate = InputEstimate {
        domain,
        domain_len: segment.descriptor().slots as u64,
        input_len,
        mean_record_bytes: collection.stats.mean_record_bytes(),
        mean_records_per_file: collection.stats.mean_records_per_file(),
        file_count: segment.file_count() as u64,
    };
    let mut selected = Some(selected);
    let mut result = None;
    segment.with_text_access(access, &mut |provider| {
        let selected = selected.take().ok_or_else(|| {
            StorageError::invalid("open segment text access", "provider callback repeated")
        })?;
        let mut providers: SmallVec<[&dyn IndexAccessProvider; 4]> = SmallVec::new();
        if let Some(provider) = provider.filter(|_| !suppress_native) {
            providers.push(provider);
        }
        #[cfg(test)]
        for test_provider in &test_providers {
            providers.push(test_provider.as_ref());
        }
        let catalog = ProviderCatalog::new(providers);
        result = Some((|| {
            let offers = catalog
                .propose(term, AccessRequest::Text(fts), estimate)
                .map_err(|error| error.to_string())?;
            let chosen = crate::query_plan::selection::choose_offer(term, &offers, input_len)
                .map_err(|error| error.to_string())?;
            selected(domain, &catalog, chosen)
        })());
        Ok(())
    })?;
    result.ok_or_else(|| "segment text access omitted provider selection".to_owned())?
}

fn segment_expr_bounds_with_context(
    expr: &BoundExpr,
    domain_len: u64,
    effective_input: &RoaringBitmap,
    context: &mut SegmentPlanContext<'_>,
) -> Result<SegmentCandidateBounds, String> {
    let catalog = context.access.files;
    Ok(match expr {
        BoundExpr::Term(term) => context.execute_term(term, effective_input.into())?,
        BoundExpr::And(parts) => {
            let mut flattened = Vec::new();
            for part in parts {
                flatten_and(part, &mut flattened);
            }
            let use_fused = context.routed.is_none_or(|(routes, _)| {
                !flattened.iter().any(|expr| match expr {
                    BoundExpr::Term(term) if indexed_file_path_term(term).is_some() => context
                        .predicates
                        .and_then(|predicates| {
                            predicates.term_for_bound_term(
                                context.access.text.collection.0 as usize,
                                term,
                            )
                        })
                        .is_some_and(|id| routes.terms.get(&id).is_some_and(Option::is_some)),
                    _ => false,
                })
            });
            let path_range = catalog
                .filter(|_| use_fused)
                .and_then(|_| combined_file_path_bounds(&flattened));
            if path_range.is_none() && flattened.len() > 1 {
                let mut ordered = Vec::with_capacity(flattened.len());
                for part in flattened {
                    let work = if let BoundExpr::Term(term) = part {
                        let id = context.term_id(term);
                        context
                            .estimate(id, term, effective_input.len())?
                            .map_or(f64::INFINITY, |(_, work)| work)
                    } else {
                        f64::INFINITY
                    };
                    ordered.push((work, part));
                }
                ordered.sort_by(|left, right| left.0.total_cmp(&right.0));
                flattened = ordered.into_iter().map(|(_, part)| part).collect();
            }
            let mut may = effective_input.clone();
            let mut must = effective_input.clone();
            if let (Some(catalog), Some((bounds, indices))) = (catalog, &path_range) {
                let mut matches = RoaringBitmap::new();
                catalog
                    .visit_paths(bounds.clone(), &mut |files| {
                        for &file in files {
                            append_file_members(context.segment, file, &mut matches)?;
                        }
                        Ok(())
                    })
                    .map_err(|error| error.to_string())?;
                matches &= &may;
                may &= &matches;
                must &= matches;
                let fused_ids = indices
                    .iter()
                    .filter_map(|index| match flattened[*index] {
                        BoundExpr::Term(term) => Some(context.term_id(&term)),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                crate::query_execution::request::record_fused_observation(
                    0,
                    &fused_ids,
                    context.access.text.domain,
                    context.access.text.collection.0,
                    domain_len,
                    may.len() as u64,
                    must.len() as u64,
                    "fused_file_path_range",
                );
            }
            for index in 0..flattened.len() {
                if path_range
                    .as_ref()
                    .is_some_and(|(_, indices)| indices.contains(&index))
                {
                    continue;
                }
                if may.is_empty() {
                    must.clear();
                    for skipped in &flattened[index..] {
                        context.record_skipped_expr(skipped);
                    }
                    break;
                }
                let part = flattened[index];
                let child = segment_expr_bounds_with_context(part, domain_len, &may, context)?;
                must &= &*child.must;
                may = child.may.as_ref().clone();
            }
            SegmentCandidateBounds::bounds(may, must)
        }
        BoundExpr::Or(parts) => {
            let mut may = RoaringBitmap::new();
            let mut must = RoaringBitmap::new();
            for part in parts {
                let child =
                    segment_expr_bounds_with_context(part, domain_len, effective_input, context)?;
                may |= &*child.may;
                must |= &*child.must;
            }
            may &= effective_input;
            must &= effective_input;
            SegmentCandidateBounds::bounds(may, must)
        }
        BoundExpr::Not(inner) => {
            let child =
                segment_expr_bounds_with_context(inner, domain_len, effective_input, context)?;
            let may = effective_input - &*child.must;
            let must = effective_input - &*child.may;
            SegmentCandidateBounds::bounds(may, must)
        }
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessPathKind {
    Metadata,
    FilePathIndex,
    Posting,
    SortColumn,
    CustomBigram,
    RawBigram,
    RelationTargetFirst,
    RelationSourceRestricted,
    Scanner,
}

/// Estimated cost and coverage of one physical query path.
#[derive(Clone, Debug, Serialize)]
pub struct AccessPathEstimate {
    pub kind: AccessPathKind,
    pub exact: bool,
    pub coverage: u32,
    pub estimated_output: u64,
    pub estimated_work: f64,
    pub estimated_files: f64,
    pub estimated_bytes: f64,
    #[serde(skip)]
    pub(crate) posting: Option<(PostingId, PostingOperation, u32)>,
}

/// Planner decision and alternatives for one bound term.
#[derive(Clone, Debug, Serialize)]
pub struct TermChoice {
    /// Request-local opaque leaf identity. Omitted for legacy fallback-only
    /// choices that are not produced by the prepared predicate table.
    pub term_id: Option<u32>,
    pub span: (usize, usize),
    pub field: String,
    pub logical_type: String,
    pub binding: String,
    pub exact_case_sensitivity: Option<crate::query::CaseSensitivity>,
    pub selected: AccessPathEstimate,
    pub alternatives: Vec<AccessPathEstimate>,
    pub actual_input: u64,
    /// Set only by estimate-only EXPLAIN; `actual_input` is then unavailable.
    pub estimated_input: Option<u64>,
    pub actual_output: Option<u64>,
    pub reason: String,
    pub dynamic_binding: bool,
    pub gram_candidates: Option<u64>,
    pub structural_candidates: Option<u64>,
    pub residual_verification: bool,
    pub executed: bool,
}

/// Work done to choose and execute access paths for one filter expression.
#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct PlanningCounters {
    pub term_estimation_calls: u64,
    pub dictionary_keys_visited: u64,
    pub candidate_bitmap_builds: u64,
    pub reused_term_estimates: u64,
    pub reused_order_plans: u64,
}

struct PlanContext<'a> {
    provider: BaseCollectionProvider<'a>,
    selected: Option<&'a dyn IndexAccessProvider>,
    execution: Option<&'a crate::query_execution::streaming::ExecutionContext>,
    #[cfg(test)]
    instance_id: u64,
    offers: HashMap<TermId, OfferList>,
    test_providers: Vec<Arc<dyn IndexAccessProvider>>,
    term_ids: Vec<(u32, BoundTerm, TermId)>,
    next_term_id: u32,
    counters: PlanningCounters,
    diagnostics: bool,
    predicates: Option<Arc<PreparedPredicates>>,
}

impl<'a> PlanContext<'a> {
    fn new(
        repo: &'a Repository,
        collection: &'a CollectionState,
        diagnostics: bool,
        predicates: Option<Arc<PreparedPredicates>>,
        selected: Option<&'a dyn IndexAccessProvider>,
        execution: Option<&'a crate::query_execution::streaming::ExecutionContext>,
    ) -> Self {
        let test_providers = {
            #[cfg(test)]
            {
                crate::query_execution::request::test_access_providers(repo.instance_id)
            }
            #[cfg(not(test))]
            {
                Vec::new()
            }
        };
        Self {
            provider: BaseCollectionProvider::new(
                repo,
                collection,
                collection.stats.record_count as u64,
                None,
            ),
            selected,
            execution,
            #[cfg(test)]
            instance_id: repo.instance_id,
            offers: HashMap::new(),
            test_providers,
            term_ids: Vec::new(),
            next_term_id: 0,
            counters: PlanningCounters::default(),
            diagnostics,
            predicates,
        }
    }

    fn for_estimation(
        collection: &'a CollectionState,
        input_len: u64,
        diagnostics: bool,
        selected: Option<&'a dyn IndexAccessProvider>,
    ) -> Self {
        Self {
            provider: BaseCollectionProvider::for_estimation(collection, input_len),
            selected,
            execution: None,
            #[cfg(test)]
            instance_id: collection.repository_instance,
            offers: HashMap::new(),
            test_providers: Vec::new(),
            term_ids: Vec::new(),
            next_term_id: 0,
            counters: PlanningCounters::default(),
            diagnostics,
            predicates: None,
        }
    }

    fn proposal_input(&self, collection: &CollectionState, input_len: u64) -> InputEstimate {
        InputEstimate {
            domain: self.provider.domain(),
            domain_len: collection.stats.record_count as u64,
            input_len,
            mean_record_bytes: collection.stats.mean_record_bytes(),
            mean_records_per_file: collection.stats.mean_records_per_file(),
            file_count: collection.stats.file_count as u64,
        }
    }

    fn catalog(&self) -> ProviderCatalog<'_> {
        let mut providers: SmallVec<[&dyn IndexAccessProvider; 4]> = SmallVec::new();
        #[cfg(test)]
        let suppress_native =
            crate::query_execution::request::test_native_access_suppressed(self.instance_id);
        #[cfg(not(test))]
        let suppress_native = false;
        match (self.selected, suppress_native) {
            (Some(provider), false) => {
                providers.push(provider);
                providers.push(&SOURCE_VERIFICATION_PROVIDER);
            }
            (None, false) => providers.push(&self.provider),
            (_, true) => providers.push(&SOURCE_VERIFICATION_PROVIDER),
        }
        for provider in &self.test_providers {
            providers.push(provider.as_ref());
        }
        ProviderCatalog::new(providers)
    }

    fn offers(
        &mut self,
        collection: &CollectionState,
        term: &BoundTerm,
        input_len: u64,
    ) -> Result<&OfferList, String> {
        let key = self.term_id(collection, term);
        if self.offers.contains_key(&key) {
            self.counters.reused_term_estimates += 1;
            return Ok(self.offers.get(&key).expect("cached offers disappeared"));
        }
        self.counters.term_estimation_calls += 1;
        let input_estimate = self.proposal_input(collection, input_len);
        let catalog = self.catalog();
        let proposal = match &self.predicates {
            Some(predicates) => catalog.propose_prepared(predicates, key, input_estimate),
            None => catalog.propose(key, AccessRequest::Structured(term), input_estimate),
        };
        drop(catalog);
        let offers = proposal.map_err(|error| error.to_string())?;
        self.counters.dictionary_keys_visited += self.provider.dictionary_keys_visited();
        self.offers.insert(key, offers);
        Ok(self
            .offers
            .get(&key)
            .expect("newly cached offers disappeared"))
    }

    fn paths(
        &mut self,
        collection: &CollectionState,
        term: &BoundTerm,
        input_len: u64,
    ) -> Result<Vec<AccessPathEstimate>, String> {
        let ordered = {
            let offers = self.offers(collection, term, input_len)?;
            ordered_offer_ids(offers, input_len)
        };
        Ok(ordered
            .into_iter()
            .map(|offer| {
                self.provider
                    .path_for(offer.id())
                    .unwrap_or_else(|| estimate_from_offer(&offer, input_len))
            })
            .collect())
    }

    fn term_id(&mut self, collection: &CollectionState, term: &BoundTerm) -> TermId {
        if let Some(predicates) = &self.predicates
            && let Some(id) =
                predicates.term_for_bound_term(collection.collection_id().0 as usize, term)
        {
            return id;
        }
        let collection_id = collection.collection_id().0;
        if let Some((_, _, id)) =
            self.term_ids
                .iter()
                .find(|(candidate_collection, candidate, _)| {
                    *candidate_collection == collection_id
                        && crate::query::binding::filter::same_bound_term(candidate, term)
                })
        {
            return *id;
        }
        let id = TermId::new(self.next_term_id);
        self.next_term_id = self
            .next_term_id
            .checked_add(1)
            .expect("query term ID overflow");
        self.term_ids.push((collection_id, term.clone(), id));
        id
    }

    fn execute_base_term(
        &mut self,
        collection: &CollectionState,
        term: &BoundTerm,
        input: &RoaringBitmap,
    ) -> Result<(Option<RoaringBitmap>, Option<AccessOffer>, Vec<AccessOffer>), String> {
        let term_id = self.term_id(collection, term);
        let diagnostics = self.diagnostics;
        let (chosen, alternatives) = {
            let offers = self.offers(collection, term, input.len())?;
            let chosen =
                crate::query_plan::selection::choose_offer(term_id, offers, input.len() as u64)
                    .map_err(|error| error.to_string())?
                    .cloned();
            let alternatives = if diagnostics {
                ordered_offer_ids(offers, input.len() as u64)
                    .into_iter()
                    .filter(|offer| {
                        chosen
                            .as_ref()
                            .is_none_or(|chosen| offer.id() != chosen.id())
                    })
                    .collect()
            } else {
                Vec::new()
            };
            (chosen, alternatives)
        };
        let Some(chosen) = chosen else {
            return Ok((None, None, Vec::new()));
        };
        let catalog = self.catalog();
        let restricted = catalog
            .apply(
                term_id,
                match &self.predicates {
                    Some(predicates) => predicates
                        .request(term_id)
                        .map_err(|error| error.to_string())?,
                    None => AccessRequest::Structured(term),
                },
                chosen.id(),
                self.provider.input_for(input),
                &AccessExecution {
                    context: self.execution,
                },
            )
            .map_err(|error| error.to_string())?;
        let exact = chosen.proof() == ProofStrength::Exact;
        if exact && !matches!(restricted.proven, Proven::All) {
            return Err("selected exact base access did not return a proof".into());
        }
        let slots = match restricted.possible.into_slots() {
            crate::query_access::membership::SlotSet::Unrestricted => exact.then(|| input.clone()),
            crate::query_access::membership::SlotSet::Empty => Some(RoaringBitmap::new()),
            crate::query_access::membership::SlotSet::Owned(bitmap) => Some(bitmap),
            crate::query_access::membership::SlotSet::Borrowed(bitmap) => Some(bitmap & input),
            crate::query_access::membership::SlotSet::Shared(bitmap) => {
                Some(bitmap.as_ref() & input)
            }
            crate::query_access::membership::SlotSet::Sorted(slots) => Some(
                slots
                    .iter()
                    .copied()
                    .filter(|&slot| input.contains(slot))
                    .collect(),
            ),
        };
        Ok((slots, Some(chosen), alternatives))
    }

    fn estimate_prepared_term(
        &self,
        collection: &CollectionState,
        predicates: &PreparedPredicates,
        term_id: TermId,
        input_len: u64,
    ) -> Result<Option<(u64, f64)>, String> {
        let catalog = self.catalog();
        let offers = catalog
            .propose_prepared(
                predicates,
                term_id,
                self.proposal_input(collection, input_len),
            )
            .map_err(|error| error.to_string())?;
        Ok(
            crate::query_plan::selection::choose_offer(term_id, &offers, input_len)
                .map_err(|error| error.to_string())?
                .map(|offer| (offer.estimated_output(), offer.estimated_work(input_len))),
        )
    }
}

fn term_case_sensitivity(term: &BoundTerm) -> Option<crate::query::CaseSensitivity> {
    (matches!(term.kind, LogicalType::String | LogicalType::Unknown)
        && matches!(term.literal, Value::String(_))
        && term.operation.is_none())
    .then_some(if term.transform == Transform::CaseFold {
        crate::query::CaseSensitivity::Insensitive
    } else {
        crate::query::CaseSensitivity::Sensitive
    })
}

fn skipped_choices(
    collection: &CollectionState,
    expr: &BoundExpr,
    output: &mut Vec<TermChoice>,
) -> Result<(), String> {
    match expr {
        BoundExpr::Term(term) => {
            let mut paths = enumerate_paths(collection, term, 0, &mut 0)?;
            paths.sort_by(|a, b| a.estimated_work.total_cmp(&b.estimated_work));
            let selected = paths.remove(0);
            output.push(TermChoice {
                term_id: None,
                span: term.span,
                field: term.query_path.display.clone(),
                logical_type: format!("{:?}", term.kind),
                binding: format!("{}@{}", term.binding_name, term.binding_version),
                exact_case_sensitivity: term_case_sensitivity(term),
                selected,
                alternatives: paths,
                actual_input: 0,
                estimated_input: None,
                actual_output: Some(0),
                reason: "skipped after an exact empty conjunct".into(),
                dynamic_binding: term.kind == LogicalType::Unknown,
                gram_candidates: None,
                structural_candidates: None,
                residual_verification: false,
                executed: false,
            });
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            for part in parts {
                skipped_choices(collection, part, output)?;
            }
        }
        BoundExpr::Not(inner) => skipped_choices(collection, inner, output)?,
    }
    Ok(())
}

/// Known true and false fallback slots; all other slots are unavailable.
struct FallbackTruth {
    yes: RoaringBitmap,
    no: RoaringBitmap,
    proofs: Vec<Proof>,
    choices: Vec<TermChoice>,
}

fn fallback_truth(
    repo: &Repository,
    collection: &CollectionState,
    expr: &BoundExpr,
    input: &RoaringBitmap,
) -> Result<FallbackTruth, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let metadata = term.related.is_none()
                && term.path.root != PathRoot::Data
                && term.path.root != PathRoot::Backlinks
                && term
                    .rhs
                    .as_ref()
                    .is_none_or(|rhs| rhs.path.root != PathRoot::Data);
            let mut yes = RoaringBitmap::new();
            if metadata {
                let empty = Value::Object(BTreeMap::new());
                for slot in input.iter() {
                    let record = &*collection.base_record(crate::model::RecordSlot(slot))?;
                    let file = repo.base_file(record.file)?;
                    let ctx = RecordContext {
                        value: &empty,
                        source_bytes: None,
                        record,
                        range: (record).record_range(),
                        file: Some(&file),
                        collection_name: &collection.definition.name,
                        repository_root: Some(repo.required_file_root()),
                    };
                    if eval_term(term, &ctx)? {
                        yes.insert(slot);
                    }
                }
            }
            let no = if metadata {
                input - &yes
            } else {
                RoaringBitmap::new()
            };
            let proofs = if metadata {
                vec![Proof {
                    term: term.clone(),
                    input: input.clone(),
                    matches: yes.clone(),
                }]
            } else {
                Vec::new()
            };
            let choices = vec![TermChoice {
                term_id: None,
                span: term.span,
                field: term.query_path.display.clone(),
                logical_type: format!("{:?}", term.kind),
                binding: format!("{}@{}", term.binding_name, term.binding_version),
                exact_case_sensitivity: term_case_sensitivity(term),
                selected: AccessPathEstimate {
                    kind: if metadata {
                        AccessPathKind::Metadata
                    } else {
                        AccessPathKind::Scanner
                    },
                    exact: metadata,
                    coverage: input.len() as u32,
                    estimated_output: yes.len(),
                    estimated_work: if metadata { input.len() as f64 } else { 0.0 },
                    estimated_files: 0.0,
                    estimated_bytes: 0.0,
                    posting: None,
                },
                alternatives: Vec::new(),
                actual_input: input.len(),
                estimated_input: None,
                actual_output: Some(yes.len()),
                reason: if metadata {
                    "fallback metadata evaluated from inventory"
                } else {
                    "logical value unavailable on raw fallback"
                }
                .into(),
                dynamic_binding: term.kind == LogicalType::Unknown,
                gram_candidates: None,
                structural_candidates: None,
                residual_verification: false,
                executed: metadata,
            }];
            FallbackTruth {
                yes,
                no,
                proofs,
                choices,
            }
        }
        BoundExpr::And(parts) => {
            let mut yes = input.clone();
            let mut no = RoaringBitmap::new();
            let mut proofs = Vec::new();
            let mut choices = Vec::new();
            for part in parts {
                let child = fallback_truth(repo, collection, part, input)?;
                yes &= child.yes;
                no |= child.no;
                proofs.extend(child.proofs);
                choices.extend(child.choices);
            }
            FallbackTruth {
                yes,
                no,
                proofs,
                choices,
            }
        }
        BoundExpr::Or(parts) => {
            let mut yes = RoaringBitmap::new();
            let mut no = input.clone();
            let mut proofs = Vec::new();
            let mut choices = Vec::new();
            for part in parts {
                let child = fallback_truth(repo, collection, part, input)?;
                yes |= child.yes;
                no &= child.no;
                proofs.extend(child.proofs);
                choices.extend(child.choices);
            }
            FallbackTruth {
                yes,
                no,
                proofs,
                choices,
            }
        }
        BoundExpr::Not(inner) => {
            let child = fallback_truth(repo, collection, inner, input)?;
            FallbackTruth {
                yes: child.no,
                no: child.yes,
                proofs: child.proofs,
                choices: child.choices,
            }
        }
    })
}

/// Exact membership evidence produced by an index path.
pub(crate) struct Proof {
    pub term: BoundTerm,
    pub input: RoaringBitmap,
    pub matches: RoaringBitmap,
}

/// Candidate set, exactness, and diagnostics for a planned expression.
pub(crate) struct Plan {
    pub candidates: Option<RoaringBitmap>,
    pub exact: bool,
    pub proven_hits: RoaringBitmap,
    pub proofs: Vec<Proof>,
    pub choices: Vec<TermChoice>,
    pub semijoin_work: crate::query_access::relations::semijoin::SemijoinWork,
    pub planning: PlanningCounters,
}

/// A whole-record raw index is a field superset only when its source reader
/// supplies every logical string and the indexed representation is identity.
/// Resolve this at planning time, before any record is visited. The source
/// registration and raw binding versions are fixed by the collection's
/// immutable generation and snapshot configuration.
pub(crate) fn raw_covers_identity_field(collection: &CollectionState) -> Result<(), String> {
    let Some(source) = &collection.source else {
        return Err("collection has no file text representation".into());
    };
    if source.reader().logical_text_coverage() != LogicalTextCoverage::AllStringFields {
        return Err(format!(
            "source {}@{} does not cover every logical string field",
            source.id(),
            source.version()
        ));
    }
    if let Some(binding) = &collection.definition.schema.raw {
        return Err(format!(
            "raw representation {}@{} differs from identity field text",
            binding.name, binding.version
        ));
    }
    #[cfg(test)]
    if collection.base_records.is_none()
        && collection.raw.record_count as usize == collection.base_slot_count()
    {
        return Ok(());
    }
    Err("base raw coverage was not advertised for this predicate".into())
}

pub(crate) fn empty_selection_preservation(term: &BoundTerm) -> bool {
    term.multi && term.quantifier != Quantifier::Any
}

#[cfg(test)]
fn raw_bigram_candidates(
    collection: &CollectionState,
    term: &BoundTerm,
) -> Result<Option<RoaringBitmap>, String> {
    let Some(mut grams) = term
        .gram_request
        .as_ref()
        .and_then(|request| collection.raw.candidates_prepared(request))
    else {
        return Ok(None);
    };
    if empty_selection_preservation(term) {
        // The universal comparison may be true on a present empty array with
        // no string witness. This bitmap is deliberately collection-wide.
        collection
            .base_slot_members(SegmentSlotFact::EmptyArray)?
            .view()
            .union_into_bitmap(&mut grams);
    }
    Ok(Some(grams))
}

fn term_set(
    repo: &Repository,
    collection: &CollectionState,
    term: &BoundTerm,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
) -> Result<Option<RoaringBitmap>, String> {
    if term.related.is_some() {
        return Ok(None);
    }
    if let PathRoot::Metadata(field) = term.path.root {
        if term.rhs.is_some() {
            let empty = Value::Object(BTreeMap::new());
            let mut result = RoaringBitmap::new();
            for slot in domain.iter() {
                let record = &*collection.base_record(crate::model::RecordSlot(slot))?;
                let file = repo.base_file(record.file)?;
                let ctx = RecordContext {
                    value: &empty,
                    source_bytes: None,
                    record,
                    range: (record).record_range(),
                    file: Some(&file),
                    collection_name: &collection.definition.name,
                    repository_root: Some(repo.required_file_root()),
                };
                if eval_term(term, &ctx)? {
                    result.insert(slot);
                }
            }
            return Ok(Some(result));
        }
        if field == MetadataField::RecordId
            && term.operator == Operator::Eq
            && term.transform == Transform::Identity
        {
            if let Value::Id(id) = &term.literal {
                let mut result = RoaringBitmap::new();
                if let Some(slot) = collection.base_slot_for_record(id.as_str())? {
                    result.insert(slot.0);
                }
                return Ok(Some(result));
            }
        }
        let scalar = if field == MetadataField::FileCreated
            && term.derived == Derived::Null
            && term.transform == Transform::Identity
            && let Value::Bool(literal) = &term.literal
            && matches!(term.operator, Operator::Eq | Operator::Ne)
            && ((*literal && term.operator == Operator::Eq)
                || (!*literal && term.operator == Operator::Ne))
        {
            Some(FileScalarPredicate::UnknownCreated)
        } else if term.transform == Transform::Identity
            && term.derived == Derived::None
            && term.period.is_none()
            && term.matcher.is_none()
            && term.operation.is_none()
        {
            match (field, &term.literal) {
                (MetadataField::FileSize, Value::Number(literal)) => {
                    Some(FileScalarPredicate::Size(term.operator, literal))
                }
                (MetadataField::FileModified, Value::Datetime(literal)) => Some(
                    FileScalarPredicate::Modified(term.operator, literal.unix_timestamp_nanos()),
                ),
                (MetadataField::FileCreated, Value::Datetime(literal)) => Some(
                    FileScalarPredicate::Created(term.operator, literal.unix_timestamp_nanos()),
                ),
                _ => None,
            }
        } else {
            None
        };
        if let Some(predicate) = scalar
            && let Some(files) = repo.catalog.scalar_candidates(predicate, 4096)?
        {
            let mut result = RoaringBitmap::new();
            for &file in files.iter() {
                collection.visit_base_file_slots(file, &mut |slots| {
                    result.extend(slots.iter().map(|slot| slot.0));
                    Ok(())
                })?;
            }
            return Ok(Some(result & domain));
        }
        if let Some((operator, literal)) = indexed_file_path_term(term) {
            let mut result = RoaringBitmap::new();
            if input.len() <= 4096 {
                let mut matched_files = HashMap::new();
                for slot in input.iter() {
                    let file_id = collection.base_record(crate::model::RecordSlot(slot))?.file;
                    let matches = match matched_files.entry(file_id) {
                        std::collections::hash_map::Entry::Occupied(entry) => *entry.get(),
                        std::collections::hash_map::Entry::Vacant(entry) => {
                            let file = repo.base_file(file_id)?;
                            let path =
                                crate::model::path_representation::query_path(&file.relative_path);
                            *entry.insert(file_path_matches(&path, operator, literal))
                        }
                    };
                    if matches {
                        result.insert(slot);
                    }
                }
                return Ok(Some(result));
            }
            let bounds =
                relative_path_bounds(operator, literal).ok_or("invalid indexed path comparison")?;
            if let Some(files) = repo
                .catalog
                .path_candidates(bounds, MAX_INDEXED_PATH_SCAN)?
            {
                for file in files {
                    collection.visit_base_file_slots(file, &mut |slots| {
                        result.extend(slots.iter().map(|slot| slot.0));
                        Ok(())
                    })?;
                }
                return Ok(Some(result & domain));
            }
        }
        let empty = Value::Object(BTreeMap::new());
        let mut result = RoaringBitmap::new();
        let file_level = matches!(
            field,
            MetadataField::FileId
                | MetadataField::FilePath
                | MetadataField::FileSize
                | MetadataField::FileCreated
                | MetadataField::FileModified
        );
        if file_level {
            collection.any_base_file::<String>(|file_id| {
                let mut matched = None;
                collection.visit_base_file_slots(file_id, &mut |slots| {
                    let keep = if let Some(keep) = matched {
                        keep
                    } else {
                        let record = collection.base_record(slots[0])?;
                        let file = repo.base_file(file_id).map_err(|error| {
                            StorageError::invalid("read base filter file", error)
                        })?;
                        let ctx = RecordContext {
                            value: &empty,
                            source_bytes: None,
                            record: record.as_ref(),
                            range: record.record_range(),
                            file: Some(&file),
                            collection_name: &collection.definition.name,
                            repository_root: Some(repo.required_file_root()),
                        };
                        let keep = eval_term(term, &ctx).map_err(|error| {
                            StorageError::invalid("match base file metadata", error)
                        })?;
                        matched = Some(keep);
                        keep
                    };
                    if keep {
                        result.extend(slots.iter().map(|slot| slot.0));
                    }
                    Ok(())
                })?;
                Ok(false)
            })?;
            return Ok(Some(result));
        }
        if matches!(
            field,
            MetadataField::CollectionName | MetadataField::RepositoryRoot
        ) {
            if collection.base_slot_count() != 0 {
                let record = &*collection.base_record(crate::model::RecordSlot(0))?;
                let file = repo.base_file(record.file)?;
                let ctx = RecordContext {
                    value: &empty,
                    source_bytes: None,
                    record,
                    range: (record).record_range(),
                    file: Some(&file),
                    collection_name: &collection.definition.name,
                    repository_root: Some(repo.required_file_root()),
                };
                if eval_term(term, &ctx)? {
                    return Ok(Some(domain.clone()));
                }
            }
            return Ok(Some(result));
        }
        for slot in 0..collection.base_slot_count() {
            let record = &*collection.base_record(crate::model::RecordSlot(slot as u32))?;
            let file = repo.base_file(record.file)?;
            let ctx = RecordContext {
                value: &empty,
                source_bytes: None,
                record,
                range: (record).record_range(),
                file: Some(&file),
                collection_name: &collection.definition.name,
                repository_root: Some(repo.required_file_root()),
            };
            if eval_term(term, &ctx)? {
                result.insert(record.slot.0);
            }
        }
        return Ok(Some(result));
    }
    Ok(None)
}

pub(crate) fn custom_bigram_compatible(term: &BoundTerm) -> bool {
    if term.operation.is_none() {
        // Registered built-in transforms map indexed text and scanned values
        // with the same Transform; they do not install an exact matcher.
        return true;
    }
    term.matcher
        .as_ref()
        .is_some_and(|matcher| matcher.bigram_compatible())
}

#[cfg(test)]
fn custom_text_index<'a>(
    collection: &'a CollectionState,
    path: &[String],
    name: &str,
) -> Option<&'a crate::index::bigram::BigramIndex> {
    match IndexCatalog::collection(collection).field_text_for(
        &collection.definition.schema,
        path,
        name,
        !matches!(name, "default" | "lower"),
        true,
        false,
    )? {
        crate::index::memory::bigram::placement::GramAccess::Native(index) => Some(index),
        crate::index::memory::bigram::placement::GramAccess::Placed(_) => None,
    }
}

pub(crate) fn enumerate_paths(
    collection: &CollectionState,
    term: &BoundTerm,
    input_len: u64,
    _dictionary_keys_visited: &mut u64,
) -> Result<Vec<AccessPathEstimate>, String> {
    let stats = collection.stats;
    let source_unit = 30.0 + stats.mean_record_bytes() / 128.0;
    let mut paths = Vec::new();
    let logical_count = collection.base_logical_count()? as u32;
    let mut add = |kind,
                   exact,
                   lookup: f64,
                   output: u64,
                   posting: Option<(PostingId, PostingOperation, u32)>| {
        let output = output.min(input_len);
        let files =
            (output as f64 / stats.mean_records_per_file().max(1.0)).min(stats.file_count as f64);
        paths.push(AccessPathEstimate {
            kind,
            exact,
            coverage: if matches!(
                kind,
                AccessPathKind::Posting
                    | AccessPathKind::SortColumn
                    | AccessPathKind::RawBigram
                    | AccessPathKind::CustomBigram
            ) {
                posting.map_or(logical_count, |(_, _, coverage)| coverage)
            } else {
                stats.record_count as u32
            },
            estimated_output: output,
            estimated_work: lookup
                + if exact {
                    output as f64 * 0.02
                } else {
                    output as f64 * source_unit
                },
            estimated_files: files,
            estimated_bytes: output as f64 * stats.mean_record_bytes(),
            posting,
        });
    };
    if term.related.is_some() {
        // Relation terms retain a three-valued residual until the dedicated
        // semijoin planner can prove target and source memberships together.
        add(AccessPathKind::Scanner, false, 0.0, input_len, None);
        return Ok(paths);
    }
    if term
        .rhs
        .as_ref()
        .is_some_and(|rhs| term.path.root != PathRoot::Data && rhs.path.root != PathRoot::Data)
    {
        add(
            AccessPathKind::Metadata,
            true,
            stats.record_count as f64,
            input_len / 2,
            None,
        );
        return Ok(paths);
    }
    if term.rhs.is_some() && !self_comparison_shape(term) {
        // Ordinary postings have record membership, not array occurrence
        // positions. A positional predicate always keeps a residual scan.
        add(AccessPathKind::Scanner, false, 0.0, input_len, None);
        return Ok(paths);
    }
    if let PathRoot::Metadata(field) = term.path.root {
        let direct_id_lookup = field == MetadataField::RecordId
            && term.operator == Operator::Eq
            && term.transform == Transform::Identity;
        let path_lookup = indexed_file_path_term(term).is_some();
        let work = if direct_id_lookup {
            1.0
        } else if path_lookup {
            (stats.file_count as f64 + 1.0).log2() + 1.0
        } else if matches!(
            field,
            MetadataField::CollectionName | MetadataField::RepositoryRoot
        ) {
            1.0
        } else if matches!(
            field,
            MetadataField::RecordByteStart
                | MetadataField::RecordByteEnd
                | MetadataField::RecordByteLength
        ) {
            stats.record_count as f64
        } else {
            stats.file_count as f64
        };
        let output = if direct_id_lookup { 1 } else { input_len / 2 };
        add(
            if path_lookup {
                AccessPathKind::FilePathIndex
            } else {
                AccessPathKind::Metadata
            },
            true,
            work,
            output,
            None,
        );
    } else {
        #[cfg(test)]
        if collection.base_records.is_none() {
            let catalog = IndexCatalog::collection(collection);
            for access in catalog
                .field_postings(
                    &term.path.segments,
                    &term.binding_name,
                    term.operation.is_some(),
                )
                .into_iter()
                .flatten()
            {
                if let Some(proposal) =
                    access.assess(term, catalog.logical, _dictionary_keys_visited)?
                {
                    add(
                        AccessPathKind::Posting,
                        proposal.exact,
                        proposal.lookup_work,
                        proposal.output,
                        Some((proposal.id, proposal.operation, proposal.coverage)),
                    );
                }
            }
        }
        if term.derived == Derived::None
            && term.period.is_none()
            && !term.multi
            && !term.query_path.has_position
            && term.transform == Transform::Identity
            && term.operation.is_none()
            && matches!(
                term.kind,
                LogicalType::Number
                    | LogicalType::Date
                    | LogicalType::Datetime
                    | LogicalType::Unixtime
            )
            && matches!(
                term.operator,
                Operator::Eq
                    | Operator::Ne
                    | Operator::Gt
                    | Operator::Ge
                    | Operator::Lt
                    | Operator::Le
            )
            && !matches!(term.literal, Value::Null)
            && collection.order_column(&term.path)?.is_some()
        {
            add(
                AccessPathKind::SortColumn,
                true,
                stats.record_count as f64,
                input_len / 4,
                None,
            );
        }
        #[cfg(test)]
        if collection.base_records.is_none()
            && term.derived == Derived::None
            && matches!(term.operator, Operator::Contains | Operator::Eq)
            && matches!(term.kind, LogicalType::String | LogicalType::Unknown)
        {
            if let Value::String(literal) = &term.literal {
                if !literal.contains(['%', '\\']) {
                    if matches!(term.binding_name.as_str(), "default" | "lower")
                        && matches!(term.transform, Transform::Identity | Transform::CaseFold)
                        && term.operation.is_none()
                        && term.matcher.is_none()
                        && raw_covers_identity_field(collection).is_ok()
                    {
                        if let Some(gram_count) = term
                            .gram_request
                            .as_ref()
                            .and_then(|request| collection.raw.estimate_prepared(request))
                        {
                            let structural_count = if empty_selection_preservation(term) {
                                Some(
                                    collection
                                        .base_slot_members(SegmentSlotFact::EmptyArray)?
                                        .view()
                                        .len() as u64,
                                )
                            } else {
                                None
                            };
                            let output = gram_count
                                .saturating_add(structural_count.unwrap_or(0))
                                .min(input_len);
                            add(
                                AccessPathKind::RawBigram,
                                false,
                                literal.len() as f64,
                                output,
                                None,
                            );
                        }
                    }
                    if !term.query_path.has_position
                        && (!term.multi || term.quantifier == Quantifier::Any)
                        && custom_bigram_compatible(term)
                    {
                        if let Some(index) =
                            custom_text_index(collection, &term.path.segments, &term.binding_name)
                        {
                            if let Some(output) = term
                                .gram_request
                                .as_ref()
                                .and_then(|request| index.estimate_prepared(request))
                            {
                                add(
                                    AccessPathKind::CustomBigram,
                                    false,
                                    literal.len() as f64,
                                    output,
                                    None,
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    add(AccessPathKind::Scanner, false, 0.0, input_len, None);
    Ok(paths)
}

pub(crate) fn execute_path(
    repo: &Repository,
    collection: &CollectionState,
    term: &BoundTerm,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    selected: &AccessPathEstimate,
) -> Result<Option<RoaringBitmap>, String> {
    Ok(match selected.kind {
        AccessPathKind::Metadata | AccessPathKind::FilePathIndex => {
            term_set(repo, collection, term, domain, input)?.map(|set| &set & input)
        }
        #[cfg(test)]
        AccessPathKind::Posting => {
            let (id, operation, _) = selected.posting.ok_or("missing prepared posting access")?;
            let catalog = IndexCatalog::collection(collection);
            match catalog.posting(
                &term.path.segments,
                &term.binding_name,
                term.operation.is_some(),
                id,
            ) {
                Some(access) => access.execute(term, catalog.logical, input, operation)?,
                None => None,
            }
        }
        #[cfg(test)]
        AccessPathKind::CustomBigram => custom_candidates(collection, term),
        #[cfg(test)]
        AccessPathKind::RawBigram => raw_bigram_candidates(collection, term)?,
        #[cfg(not(test))]
        AccessPathKind::Posting | AccessPathKind::CustomBigram | AccessPathKind::RawBigram => {
            return Err("selected access did not originate from a base provider".into());
        }
        AccessPathKind::SortColumn => {
            let column = collection
                .order_column(&term.path)?
                .ok_or("selected sort column missing")?;
            let predicate = ScalarOrderPredicate::new(&term.literal, term.operator)
                .ok_or("sort-column predicate is not scalar")?;
            column.match_scalar(&predicate, MembershipView::Borrowed(input), false)?
        }
        AccessPathKind::RelationTargetFirst | AccessPathKind::RelationSourceRestricted => {
            return Err("relation strategy is selected after local planning".into());
        }
        AccessPathKind::Scanner => None,
    })
}

fn estimate_from_offer(offer: &AccessOffer, input_len: u64) -> AccessPathEstimate {
    use crate::query_access::contract::{AccessKind, ProofStrength};

    AccessPathEstimate {
        kind: match offer.kind() {
            AccessKind::ExactMembership | AccessKind::CandidateMembership => {
                AccessPathKind::Posting
            }
            AccessKind::Metadata => AccessPathKind::Metadata,
            AccessKind::OrderedColumn => AccessPathKind::SortColumn,
            AccessKind::Relation => AccessPathKind::RelationSourceRestricted,
            AccessKind::SourceScan => AccessPathKind::Scanner,
        },
        exact: offer.proof() == ProofStrength::Exact,
        coverage: offer.estimated_output().min(u32::MAX as u64) as u32,
        estimated_output: offer.estimated_output(),
        estimated_work: offer.estimated_work(input_len),
        estimated_files: 0.0,
        estimated_bytes: 0.0,
        posting: None,
    }
}

pub(crate) fn order_matches(order: std::cmp::Ordering, op: Operator) -> bool {
    match op {
        Operator::Eq => order.is_eq(),
        Operator::Ne => !order.is_eq(),
        Operator::Gt => order.is_gt(),
        Operator::Ge => order.is_ge(),
        Operator::Lt => order.is_lt(),
        Operator::Le => order.is_le(),
        _ => false,
    }
}

pub(crate) fn estimate_expr(
    collection: &CollectionState,
    expr: &BoundExpr,
    input_len: u64,
) -> Result<(f64, u64), String> {
    with_base_predicate_access(None, collection, |selected| {
        estimate_expr_cached(
            collection,
            expr,
            input_len,
            &mut PlanContext::for_estimation(collection, input_len, false, selected),
        )
    })
}

/// An estimate-only physical decision. It has no candidate membership or
/// exact proof and is safe to use for non-analyzing EXPLAIN.
pub(crate) struct EstimatedPlan {
    pub candidates: u64,
    pub exact: bool,
    pub choices: Vec<TermChoice>,
    pub planning: PlanningCounters,
}

struct EstimatedNode {
    candidates: u64,
    exact: bool,
    work: f64,
    choices: Vec<TermChoice>,
}

pub(crate) fn estimate_plan_expr(
    collection: &CollectionState,
    expr: &BoundExpr,
    input_len: u64,
) -> Result<EstimatedPlan, String> {
    with_base_predicate_access(None, collection, |selected| {
        let mut context = PlanContext::for_estimation(collection, input_len, true, selected);
        let node = estimate_plan_node(collection, expr, input_len, &mut context)?;
        Ok(EstimatedPlan {
            candidates: node.candidates,
            exact: node.exact,
            choices: node.choices,
            planning: context.counters,
        })
    })
}

pub(crate) fn estimate_prepared_filter_term(
    repo: &Repository,
    collection: &CollectionState,
    predicates: &PreparedPredicates,
    term_id: TermId,
    input_len: u64,
) -> Result<Option<(u64, f64)>, String> {
    with_base_predicate_access(Some(repo), collection, |selected| {
        PlanContext::new(repo, collection, false, None, selected, None)
            .estimate_prepared_term(collection, predicates, term_id, input_len)
    })
}

fn base_raw_gram_estimate(_collection: &CollectionState, _term: &BoundTerm) -> Option<u64> {
    #[cfg(test)]
    if _collection.base_records.is_none() {
        return _term
            .gram_request
            .as_ref()
            .and_then(|request| _collection.raw.estimate_prepared(request));
    }
    None
}

fn estimate_plan_node<'a>(
    collection: &'a CollectionState,
    expr: &BoundExpr,
    input_len: u64,
    context: &mut PlanContext<'a>,
) -> Result<EstimatedNode, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let mut paths = context.paths(collection, term, input_len)?;
            let selected = paths.remove(0);
            let gram_candidates = (selected.kind == AccessPathKind::RawBigram)
                .then(|| base_raw_gram_estimate(collection, term))
                .flatten();
            let structural_candidates = if selected.kind == AccessPathKind::RawBigram
                && empty_selection_preservation(term)
            {
                Some(
                    collection
                        .base_slot_members(SegmentSlotFact::EmptyArray)?
                        .view()
                        .len() as u64,
                )
            } else {
                None
            };
            let reason = if matches!(
                selected.posting,
                Some((_, PostingOperation::SelfComparePresence, _))
            ) {
                "self-comparison reduced to path presence; estimate only".into()
            } else if selected.kind == AccessPathKind::Scanner
                && term.path.root == PathRoot::Data
                && matches!(term.kind, LogicalType::String | LogicalType::Unknown)
                && matches!(term.binding_name.as_str(), "default" | "lower")
                && term.operation.is_none()
                && matches!(term.operator, Operator::Contains | Operator::Eq)
            {
                raw_covers_identity_field(collection)
                    .err()
                    .map(|coverage| format!("raw bigram rejected: {coverage}; estimate only"))
                    .unwrap_or_else(|| "estimate only; scanner selected".into())
            } else {
                "estimate only; selected path and candidates were not executed".into()
            };
            EstimatedNode {
                candidates: selected.estimated_output,
                exact: selected.exact,
                work: selected.estimated_work,
                choices: vec![TermChoice {
                    term_id: Some(context.term_id(collection, term).get()),
                    span: term.span,
                    field: term.query_path.display.clone(),
                    logical_type: format!("{:?}", term.kind),
                    binding: format!("{}@{}", term.binding_name, term.binding_version),
                    exact_case_sensitivity: term_case_sensitivity(term),
                    residual_verification: !selected.exact,
                    selected,
                    alternatives: paths,
                    actual_input: 0,
                    estimated_input: Some(input_len),
                    actual_output: None,
                    reason,
                    dynamic_binding: term.kind == LogicalType::Unknown,
                    gram_candidates,
                    structural_candidates,
                    executed: false,
                }],
            }
        }
        BoundExpr::And(parts) => {
            let mut remaining = Vec::new();
            for part in parts {
                flatten_and(part, &mut remaining);
            }
            let mut current = input_len;
            let mut exact = true;
            let mut work = 0.0;
            let mut choices = Vec::new();
            while !remaining.is_empty() {
                let next = choose_and_next(collection, &remaining, current, context)?;
                let child =
                    estimate_plan_node(collection, remaining.remove(next), current, context)?;
                current = current.min(child.candidates);
                exact &= child.exact;
                work += child.work;
                choices.extend(child.choices);
            }
            EstimatedNode {
                candidates: current,
                exact,
                work,
                choices,
            }
        }
        BoundExpr::Or(parts) => {
            let mut remaining = parts.iter().collect::<Vec<_>>();
            let mut candidates = 0u64;
            let mut exact = true;
            let mut work = 0.0;
            let mut choices = Vec::new();
            while !remaining.is_empty() {
                // Estimate-only has no overlap proof. Do not subtract summed
                // counts or skip a branch as though estimates were matches.
                let next = choose_or_next(collection, &remaining, input_len, context)?;
                let child =
                    estimate_plan_node(collection, remaining.remove(next), input_len, context)?;
                candidates = candidates.saturating_add(child.candidates).min(input_len);
                exact &= child.exact;
                work += child.work;
                choices.extend(child.choices);
            }
            EstimatedNode {
                candidates,
                exact,
                work,
                choices,
            }
        }
        BoundExpr::Not(inner) => {
            let mut child = estimate_plan_node(collection, inner, input_len, context)?;
            child.candidates = if child.exact {
                input_len.saturating_sub(child.candidates)
            } else {
                input_len
            };
            child
        }
    })
}

fn estimate_expr_cached<'a>(
    collection: &'a CollectionState,
    expr: &BoundExpr,
    input_len: u64,
    context: &mut PlanContext<'a>,
) -> Result<(f64, u64), String> {
    Ok(match expr {
        BoundExpr::Term(term) => context
            .paths(collection, term, input_len)?
            .into_iter()
            .min_by(|a, b| a.estimated_work.total_cmp(&b.estimated_work))
            .map(|p| (p.estimated_work, p.estimated_output))
            .unwrap_or((input_len as f64, input_len)),
        BoundExpr::And(parts) => parts
            .iter()
            .map(|p| estimate_expr_cached(collection, p, input_len, context))
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .unwrap_or((0.0, input_len)),
        BoundExpr::Or(parts) => {
            let estimates: Vec<_> = parts
                .iter()
                .map(|p| estimate_expr_cached(collection, p, input_len, context))
                .collect::<Result<_, _>>()?;
            (
                estimates.iter().map(|e| e.0).sum(),
                estimates.iter().map(|e| e.1).sum::<u64>().min(input_len),
            )
        }
        BoundExpr::Not(inner) => {
            let (work, _) = estimate_expr_cached(collection, inner, input_len, context)?;
            (work, input_len / 2)
        }
    })
}

/// Keep opaque operations and correlated/positional comparisons in their
/// existing short-circuit order. Built-in local predicates are pure and can
/// be moved within the contiguous prefix before such a barrier.
fn reorderable(expr: &BoundExpr) -> bool {
    match expr {
        BoundExpr::Term(term) => {
            term.operation.is_none()
                && term.matcher.is_none()
                && term.related.is_none()
                && term.rhs.is_none()
                && !term.query_path.has_position
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => parts.iter().all(reorderable),
        BoundExpr::Not(inner) => reorderable(inner),
    }
}

fn reorderable_prefix(parts: &[&BoundExpr]) -> usize {
    parts
        .iter()
        .position(|part| !reorderable(part))
        .unwrap_or(parts.len())
}

/// Compare a restriction with the cost of carrying its survivors through the
/// other pending predicates. Correlation is unknown, so survivor count is a
/// conservative upper estimate and is reassessed after each real restriction.
fn choose_and_next<'a>(
    collection: &'a CollectionState,
    remaining: &[&BoundExpr],
    input_len: u64,
    context: &mut PlanContext<'a>,
) -> Result<usize, String> {
    let prefix = reorderable_prefix(remaining);
    if prefix == 0 {
        return Ok(0);
    }
    let estimates = remaining[..prefix]
        .iter()
        .map(|part| estimate_expr_cached(collection, part, input_len, context))
        .collect::<Result<Vec<_>, _>>()?;
    let total = estimates.iter().map(|(work, _)| *work).sum::<f64>();
    Ok(estimates
        .iter()
        .enumerate()
        .map(|(index, &(work, output))| {
            let survivors = output.min(input_len) as f64 / input_len.max(1) as f64;
            (index, work + survivors * (total - work).max(0.0))
        })
        .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        .map(|(index, _)| index)
        .unwrap_or(0))
}

/// Only exact bounds can be counted as definite OR matches. Candidate-only
/// branches keep their residual and cannot exclude later membership work.
fn estimate_definite<'a>(
    collection: &'a CollectionState,
    expr: &BoundExpr,
    input_len: u64,
    context: &mut PlanContext<'a>,
) -> Result<(f64, u64), String> {
    Ok(match expr {
        BoundExpr::Term(term) => context
            .paths(collection, term, input_len)?
            .into_iter()
            .min_by(|a, b| a.estimated_work.total_cmp(&b.estimated_work))
            .map(|path| {
                (
                    path.estimated_work,
                    if path.exact { path.estimated_output } else { 0 },
                )
            })
            .unwrap_or((input_len as f64, 0)),
        _ => {
            let (work, _) = estimate_expr_cached(collection, expr, input_len, context)?;
            (work, 0)
        }
    })
}

fn choose_or_next<'a>(
    collection: &'a CollectionState,
    remaining: &[&BoundExpr],
    input_len: u64,
    context: &mut PlanContext<'a>,
) -> Result<usize, String> {
    let prefix = reorderable_prefix(remaining);
    if prefix == 0 {
        return Ok(0);
    }
    let choice = remaining[..prefix]
        .iter()
        .enumerate()
        .map(|(index, part)| {
            let (work, definite) = estimate_definite(collection, part, input_len, context)?;
            // A definite-match discount is bounded by one branch's own work.
            // Candidate residuals are verified after Boolean composition, so
            // a costly fixed dictionary walk cannot be justified solely by
            // its broad match count.
            let proved_fraction = definite.min(input_len) as f64 / input_len.max(1) as f64;
            Ok((index, work / (1.0 + proved_fraction)))
        })
        .collect::<Result<Vec<_>, String>>()?
        .into_iter()
        .min_by(|a, b| a.1.total_cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
        .map(|(index, _)| index)
        .unwrap_or(0);
    Ok(choice)
}

fn flatten_and<'a>(expr: &'a BoundExpr, output: &mut Vec<&'a BoundExpr>) {
    if let BoundExpr::And(parts) = expr {
        for part in parts {
            flatten_and(part, output);
        }
    } else {
        output.push(expr);
    }
}

#[cfg(test)]
pub(crate) fn plan_expr_restricted(
    repo: &Repository,
    collection: &CollectionState,
    segments: &[Arc<Vec<Arc<crate::index::traits::publication::PublishedSegment>>>],
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    evidence: bool,
) -> Result<Plan, String> {
    plan_expr_restricted_with_execution(
        repo, collection, segments, expr, domain, input, evidence, true, None, None,
    )
}

pub(crate) fn plan_expr_restricted_with_execution(
    repo: &Repository,
    collection: &CollectionState,
    segments: &[Arc<Vec<Arc<crate::index::traits::publication::PublishedSegment>>>],
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    evidence: bool,
    diagnostics: bool,
    execution: Option<&crate::query_execution::streaming::ExecutionContext>,
    predicates: Option<Arc<PreparedPredicates>>,
) -> Result<Plan, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    with_base_predicate_access(Some(repo), collection, |selected| {
        let mut context = PlanContext::new(
            repo,
            collection,
            diagnostics,
            predicates,
            selected,
            execution,
        );
        let mut plan = plan_expr_restricted_inner(
            repo,
            collection,
            segments,
            expr,
            domain,
            input,
            evidence,
            &mut context,
            execution,
        )?;
        plan.planning = context.counters;
        Ok(plan)
    })
}

fn plan_expr_restricted_inner<'a>(
    repo: &'a Repository,
    collection: &'a CollectionState,
    segments: &[Arc<Vec<Arc<crate::index::traits::publication::PublishedSegment>>>],
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    evidence: bool,
    context: &mut PlanContext<'a>,
    execution: Option<&crate::query_execution::streaming::ExecutionContext>,
) -> Result<Plan, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    if input.is_empty() {
        return Ok(Plan {
            candidates: Some(RoaringBitmap::new()),
            exact: true,
            proven_hits: RoaringBitmap::new(),
            proofs: Vec::new(),
            choices: Vec::new(),
            semijoin_work: Default::default(),
            planning: PlanningCounters::default(),
        });
    }
    let logical_members = collection.base_slot_members(SegmentSlotFact::Logical)?;
    let logical_members = logical_members.view();
    if logical_members.len() == collection.base_slot_count() {
        let mut plan = plan_logical_expr(repo, collection, expr, domain, input, evidence, context)?;
        if !collection.definition.schema.relations().is_empty() {
            let mut term_id = |term: &BoundTerm| context.term_id(collection, term);
            crate::query_access::relations::semijoin::restrict(
                repo,
                segments,
                expr,
                input,
                &mut plan,
                &mut term_id,
                execution,
            )?;
        }
        return Ok(plan);
    }
    let logical_domain = logical_members.intersect_bitmap(domain);
    let logical_input = input & &logical_domain;
    let fallback_input = logical_members.subtract_from(input);
    let mut logical = if logical_input.is_empty() {
        Plan {
            candidates: Some(RoaringBitmap::new()),
            exact: true,
            proven_hits: RoaringBitmap::new(),
            proofs: Vec::new(),
            choices: Vec::new(),
            semijoin_work: Default::default(),
            planning: PlanningCounters::default(),
        }
    } else {
        plan_logical_expr(
            repo,
            collection,
            expr,
            &logical_domain,
            &logical_input,
            evidence,
            context,
        )?
    };
    let mut candidates = logical.candidates.take().unwrap_or(logical_input);
    if !fallback_input.is_empty() {
        let fallback = fallback_truth(repo, collection, expr, &fallback_input)?;
        candidates |= &fallback.yes;
        logical.proven_hits |= &fallback.yes;
        logical.proofs.extend(fallback.proofs);
        logical.choices.extend(fallback.choices);
    }
    logical.candidates = Some(candidates);
    if !collection.definition.schema.relations().is_empty() {
        let mut term_id = |term: &BoundTerm| context.term_id(collection, term);
        crate::query_access::relations::semijoin::restrict(
            repo,
            segments,
            expr,
            input,
            &mut logical,
            &mut term_id,
            execution,
        )?;
    }
    Ok(logical)
}

fn plan_logical_expr<'a>(
    repo: &'a Repository,
    collection: &'a CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    evidence: bool,
    context: &mut PlanContext<'a>,
) -> Result<Plan, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let term_id = context.term_id(collection, term);
            let (candidates, chosen, offered_alternatives) =
                context.execute_base_term(collection, term, input)?;
            if chosen.as_ref().is_some_and(|choice| {
                choice.kind() != crate::query_access::contract::AccessKind::SourceScan
            }) {
                context.counters.candidate_bitmap_builds += 1;
            }
            let exact = chosen.as_ref().is_some_and(|choice| {
                choice.proof() == crate::query_access::contract::ProofStrength::Exact
            });
            if exact && candidates.is_none() {
                return Err("selected exact access path did not return a proof".into());
            }
            let exact_candidates = if exact { candidates.as_ref() } else { None };
            let proofs = exact_candidates
                .filter(|_| evidence)
                .map(|set| {
                    vec![Proof {
                        term: term.clone(),
                        input: input.clone(),
                        matches: set.clone(),
                    }]
                })
                .unwrap_or_default();
            let dynamic_binding = term.kind == LogicalType::Unknown;
            let choices = if context.diagnostics {
                let selected = chosen.as_ref().map_or_else(
                    || AccessPathEstimate {
                        kind: AccessPathKind::Scanner,
                        exact: false,
                        coverage: input.len().min(u32::MAX as u64) as u32,
                        estimated_output: input.len(),
                        estimated_work: input.len() as f64,
                        estimated_files: 0.0,
                        estimated_bytes: 0.0,
                        posting: None,
                    },
                    |offer| {
                        context
                            .provider
                            .path_for(offer.id())
                            .unwrap_or_else(|| estimate_from_offer(offer, input.len() as u64))
                    },
                );
                let alternatives: Vec<AccessPathEstimate> = offered_alternatives
                    .iter()
                    .map(|offer| {
                        context
                            .provider
                            .path_for(offer.id())
                            .unwrap_or_else(|| estimate_from_offer(offer, input.len() as u64))
                    })
                    .collect();
                let gram_candidates = (selected.kind == AccessPathKind::RawBigram)
                    .then(|| base_raw_gram_estimate(collection, term))
                    .flatten();
                let structural_candidates = if selected.kind == AccessPathKind::RawBigram
                    && empty_selection_preservation(term)
                {
                    Some(
                        collection
                            .base_slot_members(SegmentSlotFact::EmptyArray)?
                            .view()
                            .len() as u64,
                    )
                } else {
                    None
                };
                let reason = if selected.kind == AccessPathKind::Posting {
                    if matches!(
                        selected.posting,
                        Some((_, PostingOperation::SelfComparePresence, _))
                    ) {
                        "self-comparison reduced to path presence".into()
                    } else if selected.exact {
                        "compatible posting capability proves this bound predicate".into()
                    } else {
                        "posting capability restricts candidates; residual verification retains selector and quantifier semantics".into()
                    }
                } else if selected.kind == AccessPathKind::RawBigram && dynamic_binding {
                    format!(
                        "undeclared dynamic string-witness restriction via shared raw index {}@{}; empty-selection preservation: {}; exact field residual retained",
                        collection.required_file_reader().id(),
                        collection.required_file_reader().version(),
                        structural_candidates.is_some()
                    )
                } else if selected.kind == AccessPathKind::Scanner
                    && term.kind == LogicalType::Unknown
                    && term.path.root == PathRoot::Data
                {
                    let posting_reason =
                        "no lower-cost compatible posting capability; scanner selected";
                    if matches!(term.binding_name.as_str(), "default" | "lower")
                        && term.operation.is_none()
                        && matches!(term.operator, Operator::Contains | Operator::Eq)
                    {
                        if let Err(coverage) = raw_covers_identity_field(collection) {
                            format!("raw bigram rejected: {coverage}; {posting_reason}")
                        } else {
                            posting_reason.into()
                        }
                    } else {
                        posting_reason.into()
                    }
                } else if selected.kind == AccessPathKind::Scanner
                    && term.path.root == PathRoot::Data
                    && matches!(term.kind, LogicalType::String | LogicalType::Unknown)
                    && matches!(term.binding_name.as_str(), "default" | "lower")
                    && term.operation.is_none()
                    && matches!(term.operator, Operator::Contains | Operator::Eq)
                {
                    raw_covers_identity_field(collection)
                    .err()
                    .map(|reason| {
                        format!(
                            "raw bigram rejected: {reason}; dynamic binding: {dynamic_binding}; scanner selected"
                        )
                    })
                    .unwrap_or_else(|| {
                        if base_raw_gram_estimate(collection, term).is_none()
                        {
                            "raw bigram has no usable necessary grams; scanner selected".into()
                        } else {
                            "lowest estimated lookup plus remaining source work for current input"
                                .into()
                        }
                    })
                } else {
                    "lowest estimated lookup plus remaining source work for current input".into()
                };
                vec![TermChoice {
                    term_id: Some(term_id.get()),
                    span: term.span,
                    field: term.query_path.display.clone(),
                    logical_type: format!("{:?}", term.kind),
                    binding: format!("{}@{}", term.binding_name, term.binding_version),
                    exact_case_sensitivity: term_case_sensitivity(term),
                    selected,
                    alternatives,
                    actual_input: input.len(),
                    estimated_input: None,
                    actual_output: candidates.as_ref().map(RoaringBitmap::len),
                    reason,
                    dynamic_binding,
                    gram_candidates,
                    structural_candidates,
                    residual_verification: !exact,
                    executed: true,
                }]
            } else {
                Vec::new()
            };
            Plan {
                proven_hits: exact_candidates.cloned().unwrap_or_default(),
                candidates,
                exact,
                proofs,
                choices,
                semijoin_work: Default::default(),
                planning: PlanningCounters::default(),
            }
        }
        BoundExpr::And(parts) => {
            let mut remaining = Vec::new();
            for part in parts {
                flatten_and(part, &mut remaining);
            }
            let mut current = input.clone();
            let mut choices = Vec::new();
            if let Some((bounds, fused_indices)) = combined_file_path_bounds(&remaining) {
                let mut matches = RoaringBitmap::new();
                repo.catalog.visit_paths(bounds, &mut |files| {
                    for &file in files {
                        collection.visit_base_file_slots(file, &mut |slots| {
                            matches.extend(slots.iter().map(|slot| slot.0));
                            Ok(())
                        })?;
                    }
                    Ok(())
                })?;
                current &= matches;
                let fused_terms = fused_indices
                    .iter()
                    .filter_map(|index| match remaining[*index] {
                        BoundExpr::Term(term) => Some(term),
                        _ => None,
                    })
                    .collect::<Vec<_>>();
                let fused_ids = fused_terms
                    .iter()
                    .map(|term| context.term_id(collection, term))
                    .collect::<Vec<_>>();
                crate::query_execution::request::record_fused_observation(
                    repo.instance_id,
                    &fused_ids,
                    context.provider.domain(),
                    collection.collection_id().0,
                    input.len() as u64,
                    current.len() as u64,
                    current.len() as u64,
                    "fused_file_path_range",
                );
                if context.diagnostics {
                    choices.extend(fused_terms.into_iter().map(|term| TermChoice {
                        term_id: Some(context.term_id(collection, term).get()),
                        span: term.span,
                        field: term.query_path.display.clone(),
                        logical_type: format!("{:?}", term.kind),
                        binding: format!("{}@{}", term.binding_name, term.binding_version),
                        exact_case_sensitivity: term_case_sensitivity(term),
                        selected: AccessPathEstimate {
                            kind: AccessPathKind::FilePathIndex,
                            exact: true,
                            coverage: current.len().min(u32::MAX as u64) as u32,
                            estimated_output: current.len(),
                            estimated_work: current.len() as f64,
                            estimated_files: 0.0,
                            estimated_bytes: 0.0,
                            posting: None,
                        },
                        alternatives: Vec::new(),
                        actual_input: input.len(),
                        estimated_input: None,
                        actual_output: Some(current.len()),
                        reason: "fused file-path range traversal".into(),
                        dynamic_binding: false,
                        gram_candidates: None,
                        structural_candidates: None,
                        residual_verification: false,
                        executed: true,
                    }));
                }
                for index in fused_indices.into_iter().rev() {
                    remaining.remove(index);
                }
            }
            let mut exact = true;
            let mut proofs = Vec::new();
            while !remaining.is_empty() {
                let next = choose_and_next(collection, &remaining, current.len(), context)?;
                let child = plan_logical_expr(
                    repo,
                    collection,
                    remaining.remove(next),
                    domain,
                    &current,
                    evidence,
                    context,
                )?;
                if let Some(set) = child.candidates {
                    current &= set;
                }
                exact &= child.exact;
                proofs.extend(child.proofs);
                choices.extend(child.choices);
                if current.is_empty() {
                    exact = true;
                    for part in &remaining {
                        record_skipped_expr(repo, collection, part, domain, context);
                    }
                    if context.diagnostics {
                        for part in remaining {
                            skipped_choices(collection, part, &mut choices)?;
                        }
                    }
                    break;
                }
            }
            Plan {
                proven_hits: if exact {
                    current.clone()
                } else {
                    RoaringBitmap::new()
                },
                candidates: Some(current),
                exact,
                proofs,
                choices,
                semijoin_work: Default::default(),
                planning: PlanningCounters::default(),
            }
        }
        BoundExpr::Or(parts) => {
            let mut remaining = parts.iter().collect::<Vec<_>>();
            let mut candidates = Some(RoaringBitmap::new());
            let mut proven_hits = RoaringBitmap::new();
            let mut proofs = Vec::new();
            let mut choices = Vec::new();
            let mut all_exact = true;
            while !remaining.is_empty() {
                let active = if evidence {
                    input.clone()
                } else {
                    input - &proven_hits
                };
                if active.is_empty() {
                    for skipped in &remaining {
                        record_skipped_expr(repo, collection, skipped, domain, context);
                    }
                    if context.diagnostics {
                        for skipped in &remaining {
                            skipped_choices(collection, skipped, &mut choices)?;
                        }
                    }
                    break;
                }
                let next = choose_or_next(collection, &remaining, active.len(), context)?;
                let part = remaining.remove(next);
                let child =
                    plan_logical_expr(repo, collection, part, domain, &active, evidence, context)?;
                all_exact &= child.exact;
                proven_hits |= &child.proven_hits;
                candidates = match (candidates, child.candidates) {
                    (Some(mut union), Some(set)) => {
                        union |= set;

                        Some(union)
                    }
                    _ => None,
                };
                proofs.extend(child.proofs);
                choices.extend(child.choices);
            }
            let exact = all_exact || (!evidence && proven_hits.len() == input.len());
            if !evidence && proven_hits.len() == input.len() {
                candidates = Some(proven_hits.clone());
            }
            Plan {
                candidates,
                exact,
                proven_hits,
                proofs,
                choices,
                semijoin_work: Default::default(),
                planning: PlanningCounters::default(),
            }
        }
        BoundExpr::Not(inner) => {
            let child =
                plan_logical_expr(repo, collection, inner, domain, input, evidence, context)?;
            if child.exact {
                let matches = input - &child.candidates.unwrap();
                Plan {
                    candidates: Some(matches.clone()),
                    exact: true,
                    proven_hits: matches,
                    proofs: child.proofs,
                    choices: child.choices,
                    semijoin_work: Default::default(),
                    planning: PlanningCounters::default(),
                }
            } else {
                Plan {
                    candidates: None,
                    exact: false,
                    proven_hits: RoaringBitmap::new(),
                    proofs: Vec::new(),
                    choices: child.choices,
                    semijoin_work: Default::default(),
                    planning: PlanningCounters::default(),
                }
            }
        }
    })
}

fn record_skipped_expr<'a>(
    repo: &Repository,
    collection: &'a CollectionState,
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    context: &mut PlanContext<'a>,
) {
    match expr {
        BoundExpr::Term(term) => {
            let term_id = context.term_id(collection, term);
            crate::query_execution::request::record_skipped_observation(
                repo.instance_id,
                term_id,
                context.provider.domain(),
                collection.collection_id().0,
                0,
            );
        }
        BoundExpr::And(parts) | BoundExpr::Or(parts) => {
            for part in parts {
                record_skipped_expr(repo, collection, part, domain, context);
            }
        }
        BoundExpr::Not(inner) => {
            record_skipped_expr(repo, collection, inner, domain, context);
        }
    }
    let _ = domain;
}

#[cfg(test)]
fn custom_candidates(collection: &CollectionState, term: &BoundTerm) -> Option<RoaringBitmap> {
    if term.derived != Derived::None || !matches!(term.operator, Operator::Contains | Operator::Eq)
    {
        return None;
    }
    if !custom_bigram_compatible(term) {
        return None;
    }
    let Value::String(literal) = &term.literal else {
        return None;
    };
    if literal.contains(['%', '\\']) {
        return None;
    }
    custom_text_index(collection, &term.path.segments, &term.binding_name)?
        .candidates_prepared(term.gram_request.as_ref()?)
}

pub(crate) fn estimate_fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Result<Option<(u64, f64)>, String> {
    crate::query_access::text::base_text_estimate(collection, fts)
}

#[cfg(test)]
pub(crate) fn fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<RoaringBitmap> {
    use crate::index::memory::access::text::NativeTextProvider;
    use crate::query_access::contract::{AccessExecution, IndexAccessProvider, InputEstimate};

    let provider = NativeTextProvider::new(collection, None);
    let mut offers = crate::query_access::contract::OfferList::new();
    provider
        .propose(
            TermId::new(0),
            AccessRequest::Text(fts),
            InputEstimate {
                domain: provider.domain(),
                domain_len: collection.stats.record_count as u64,
                input_len: collection.stats.record_count as u64,
                mean_record_bytes: collection.stats.mean_record_bytes(),
                mean_records_per_file: collection.stats.mean_records_per_file(),
                file_count: collection.stats.file_count as u64,
            },
            &mut offers,
        )
        .ok()?;
    let chosen = crate::query_plan::selection::choose_offer(
        TermId::new(0),
        &offers,
        collection.stats.record_count as u64,
    )
    .ok()??
    .id();
    let input = (0..collection.stats.record_count as u32).collect::<RoaringBitmap>();
    let applied = provider
        .apply(
            TermId::new(0),
            AccessRequest::Text(fts),
            chosen,
            provider.input_for(&input),
            &AccessExecution { context: None },
        )
        .ok()?;
    let slots = applied.possible.into_slots();
    match slots {
        crate::query_access::membership::SlotSet::Owned(bitmap) => Some(bitmap),
        crate::query_access::membership::SlotSet::Unrestricted => Some(input),
        crate::query_access::membership::SlotSet::Empty => Some(RoaringBitmap::new()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "restrictions/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "restrictions/dynamic_filter_tests.rs"]
mod dynamic_filter_tests;
