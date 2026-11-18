//! Exact posting restrictions plus one residual scan over surviving records.

use std::collections::{BTreeMap, HashMap};
use std::ops::Bound;
use std::sync::{Arc, Mutex};

use roaring::RoaringBitmap;
use smallvec::SmallVec;

use crate::build::IndexedCollectionSegment;
use crate::index_catalog::{IndexCatalog, PostingId, PostingOperation, self_comparison_shape};
use crate::inventory::{
    ActiveRecords, ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry,
    SharedPath, SourceRevision, relative_path_bounds,
};
use crate::lifecycle_catalog::FileCatalog;
use crate::model::{
    FileId, LogicalType, MetadataField, Number, PathRoot, RecordId, RecordSlot, Value,
};
use crate::query::{FtsMode, Operator, Quantifier};
use crate::query_access::contract::{
    AccessExecution, AccessId, AccessKind, AccessOffer, AccessRequest, Coverage,
    IndexAccessProvider, InputEstimate, OfferList, OperationId, ProofStrength, Proven, ProviderId,
    Restriction,
};
use crate::query_access::membership::{DomainId, SlotInput, SlotSet};
use crate::query_access::native::NativeCollectionProvider;
use crate::query_access::registry::ProviderCatalog;
use crate::query_plan::selection::ordered_offer_ids;
use crate::query_predicate::{PreparedPredicates, TermId};
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, Derived, RecordContext, Transform, eval_term,
};
use crate::repository::{CollectionState, Repository};
use crate::sorting::SortKey;
use crate::source::LogicalTextCoverage;
use serde::Serialize;

const MAX_INDEXED_PATH_SCAN: usize = 8192;

/// Conservative predicate bounds for one locally slotted delta. `may` is a
/// superset of true records; `must` is a subset. Carrying both through NOT
/// prevents an unavailable or unindexed value from becoming a false proof.
pub(crate) struct SegmentCandidateBounds {
    pub may: RoaringBitmap,
    pub must: RoaringBitmap,
}

const NATIVE_SEGMENT_PROVIDER: ProviderId = ProviderId(3);

struct NativeSegmentProvider<'a> {
    segment: &'a IndexedCollectionSegment,
    collection: &'a CollectionState,
    file_catalog: Option<&'a FileCatalog>,
    domain: DomainId,
    operations: Mutex<Vec<TermId>>,
}

impl<'a> NativeSegmentProvider<'a> {
    fn new(
        segment: &'a IndexedCollectionSegment,
        collection: &'a CollectionState,
        file_catalog: Option<&'a FileCatalog>,
    ) -> Self {
        Self {
            segment,
            collection,
            file_catalog,
            domain: DomainId::from_parts(
                0,
                collection.generation.collection.0,
                collection.generation.generation,
                Some(segment.id.get()),
            ),
            operations: Mutex::new(Vec::new()),
        }
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
    ) -> crate::query_access::contract::AccessResult<()> {
        if input.domain != self.domain || !matches!(request, AccessRequest::Structured(_)) {
            return Ok(());
        }
        let operation = {
            let mut operations = self
                .operations
                .lock()
                .expect("segment provider state poisoned");
            let operation = operations.len();
            operations.push(term);
            operation
        };
        // Native application reads a prepared posting/column or resident
        // metadata. Its cardinality is only known after application, so the
        // offer stays conservative and never claims proof up front.
        offers.push(AccessOffer::new(
            AccessId {
                provider: NATIVE_SEGMENT_PROVIDER,
                operation: OperationId(operation as u32),
            },
            term,
            AccessKind::SourceScan,
            Coverage::Input,
            ProofStrength::None,
            input.input_len,
            0.5,
            0.0001,
            false,
            false,
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
    ) -> crate::query_access::contract::AccessResult<Restriction<'a>> {
        use crate::query_access::contract::AccessError;

        execution.check()?;
        if choice.provider != NATIVE_SEGMENT_PROVIDER {
            return Err(AccessError::StaleHandle);
        }
        if input.domain() != self.domain {
            return Err(AccessError::DomainMismatch);
        }
        if self
            .operations
            .lock()
            .expect("segment provider state poisoned")
            .get(choice.operation.0 as usize)
            != Some(&term)
        {
            return Err(AccessError::StaleHandle);
        }
        let AccessRequest::Structured(bound) = request else {
            return Err(AccessError::CapabilityUnavailable);
        };
        let input_len = input.len() as u64;
        if input.is_empty() {
            return Ok(Restriction::exact(input));
        }
        let input_bitmap = input.operation_input();
        let logical_active = input_bitmap & &self.segment.logical_records;
        let fallback_active = input_bitmap - &self.segment.logical_records;
        let bounds = segment_expr_bounds(
            self.segment,
            &BoundExpr::Term(bound.clone()),
            self.collection,
            input_bitmap,
            &logical_active,
            &fallback_active,
            self.file_catalog,
        )
        .map_err(AccessError::Internal)?;
        let possible = input.replace_slots(SlotSet::from_owned(bounds.may));
        let proven = SlotInput::new(self.domain, input_bitmap, SlotSet::from_owned(bounds.must));
        let output_len = possible.len() as u64;
        let proven_len = proven.len() as u64;
        let access_kind = segment_access_path_kind(bound, self.segment, self.collection);
        crate::query_execution::request::record_observation_with_kind(
            0,
            term,
            self.domain,
            choice,
            self.collection.generation.collection.0,
            input_len,
            output_len,
            proven_len,
            Some(access_path_kind_name(access_kind)),
        );
        Ok(Restriction::bounds(possible, proven))
    }
}

fn segment_access_path_kind(
    term: &BoundTerm,
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
) -> AccessPathKind {
    if term.related.is_some() {
        return AccessPathKind::RelationSourceRestricted;
    }
    if term.query_path.has_position {
        return AccessPathKind::Posting;
    }
    match term.path.root {
        PathRoot::Metadata(MetadataField::FilePath) => {
            return AccessPathKind::FilePathIndex;
        }
        PathRoot::Metadata(_) => return AccessPathKind::Metadata,
        PathRoot::Backlinks => return AccessPathKind::Scanner,
        PathRoot::Data => {}
    }
    if term.path.root == PathRoot::Data
        && matches!(
            term.kind,
            LogicalType::Number | LogicalType::Date | LogicalType::Datetime | LogicalType::Unixtime
        )
        && segment.sorting.column(&term.path.display()).is_ok()
    {
        return AccessPathKind::SortColumn;
    }
    if term.path.root == PathRoot::Data
        && matches!(term.kind, LogicalType::String | LogicalType::Unknown)
        && matches!(term.operator, Operator::Contains | Operator::Eq)
        && term.gram_request.is_some()
        && custom_bigram_compatible(term)
    {
        return if matches!(term.binding_name.as_str(), "default" | "lower") {
            AccessPathKind::RawBigram
        } else {
            AccessPathKind::CustomBigram
        };
    }
    let index = IndexCatalog::segment(segment, collection);
    if index
        .field_postings(
            &term.path.segments,
            &term.binding_name,
            term.operation.is_some(),
        )
        .into_iter()
        .flatten()
        .next()
        .is_some()
    {
        AccessPathKind::Posting
    } else {
        AccessPathKind::Scanner
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

struct SegmentPlanContext<'a> {
    provider: NativeSegmentProvider<'a>,
    test_providers: Vec<Arc<dyn IndexAccessProvider>>,
    offers: HashMap<TermId, OfferList>,
    terms: HashMap<(usize, usize, String), TermId>,
    next_term: u32,
}

impl<'a> SegmentPlanContext<'a> {
    fn new(
        segment: &'a IndexedCollectionSegment,
        collection: &'a CollectionState,
        file_catalog: Option<&'a FileCatalog>,
    ) -> Self {
        let test_providers = {
            #[cfg(test)]
            {
                crate::query_execution::request::all_test_access_providers()
            }
            #[cfg(not(test))]
            {
                Vec::new()
            }
        };
        Self {
            provider: NativeSegmentProvider::new(segment, collection, file_catalog),
            test_providers,
            offers: HashMap::new(),
            terms: HashMap::new(),
            next_term: 0,
        }
    }

    fn catalog(&self) -> ProviderCatalog<'_> {
        let mut providers: SmallVec<[&dyn IndexAccessProvider; 4]> = SmallVec::new();
        providers.push(&self.provider);
        for provider in &self.test_providers {
            providers.push(provider.as_ref());
        }
        ProviderCatalog::new(providers)
    }

    fn term_id(&mut self, term: &BoundTerm) -> TermId {
        let key = (term.span.0, term.span.1, term.query_path.display.clone());
        if let Some(term_id) = self.terms.get(&key) {
            return *term_id;
        }
        let term_id = TermId::new(self.next_term);
        self.next_term = self
            .next_term
            .checked_add(1)
            .expect("segment predicate ID overflow");
        self.terms.insert(key, term_id);
        term_id
    }

    fn execute_term(
        &mut self,
        term: &BoundTerm,
        input: &RoaringBitmap,
    ) -> Result<SegmentCandidateBounds, String> {
        let term_id = self.term_id(term);
        if !self.offers.contains_key(&term_id) {
            let estimate = InputEstimate {
                domain: self.provider.domain,
                domain_len: self.provider.segment.record_count() as u64,
                input_len: input.len() as u64,
                mean_record_bytes: self.provider.collection.stats.mean_record_bytes(),
                mean_records_per_file: self.provider.collection.stats.mean_records_per_file(),
                file_count: self.provider.segment.file_count() as u64,
            };
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
            return Ok(SegmentCandidateBounds {
                may: input.clone(),
                must: RoaringBitmap::new(),
            });
        };
        let restriction = self
            .catalog()
            .apply(
                term_id,
                AccessRequest::Structured(term),
                choice.id(),
                SlotInput::unrestricted(self.provider.domain, input),
                &AccessExecution { context: None },
            )
            .map_err(|error| error.to_string())?;
        let possible = restriction.possible;
        let may = bitmap_from_slot_input(&possible)?;
        let must = match restriction.proven {
            Proven::None => RoaringBitmap::new(),
            Proven::All => may.clone(),
            Proven::Subset(proven) => bitmap_from_slot_input(&proven)?,
        };
        Ok(SegmentCandidateBounds { may, must })
    }

    fn record_skipped_expr(&mut self, expr: &BoundExpr) {
        match expr {
            BoundExpr::Term(term) => {
                let term_id = self.term_id(term);
                crate::query_execution::request::record_skipped_observation(
                    0,
                    term_id,
                    self.provider.domain,
                    self.provider.collection.generation.collection.0,
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

fn bitmap_from_slot_input(input: &SlotInput<'_>) -> Result<RoaringBitmap, String> {
    match input.slots() {
        SlotSet::Unrestricted => input
            .effective_bitmap()
            .map(|slots| slots.clone())
            .ok_or_else(|| "unrestricted membership lost its input".into()),
        SlotSet::Empty => Ok(RoaringBitmap::new()),
        SlotSet::Borrowed(slots) => Ok((*slots).clone()),
        SlotSet::Owned(slots) => Ok(slots.clone()),
        SlotSet::Shared(slots) => Ok(slots.as_ref().clone()),
    }
}

impl SegmentCandidateBounds {
    fn unknown(domain: &RoaringBitmap) -> Self {
        Self {
            may: domain.clone(),
            must: RoaringBitmap::new(),
        }
    }
}

pub(crate) fn segment_filter_bounds(
    segment: &IndexedCollectionSegment,
    active: &ActiveRecords,
    expr: &BoundExpr,
    collection: &CollectionState,
) -> Result<SegmentCandidateBounds, String> {
    segment_filter_bounds_with_catalog(segment, active, expr, collection, None)
}

pub(crate) fn segment_filter_bounds_with_catalog(
    segment: &IndexedCollectionSegment,
    active: &ActiveRecords,
    expr: &BoundExpr,
    collection: &CollectionState,
    catalog: Option<&FileCatalog>,
) -> Result<SegmentCandidateBounds, String> {
    if active.slot_count() as usize != segment.record_count() {
        return Err("segment active domain length mismatch".into());
    }
    let domain = active.bitmap();
    let logical_active = &domain & &segment.logical_records;
    let fallback_active = &domain - &segment.logical_records;
    let mut context = SegmentPlanContext::new(segment, collection, catalog);
    segment_expr_bounds_with_context(
        segment,
        expr,
        collection,
        &domain,
        &logical_active,
        &fallback_active,
        catalog,
        &mut context,
    )
}

fn indexed_file_path_term(term: &BoundTerm) -> Option<(Operator, &str)> {
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

fn file_path_matches(value: &str, operator: Operator, literal: &str) -> bool {
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

/// Conservative local bigram candidates for a lifecycle segment. `None`
/// means the caller must keep the whole active domain.
pub(crate) fn native_segment_fts_candidates(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    fts: &BoundFts,
) -> Result<Option<RoaringBitmap>, String> {
    let indexes = IndexCatalog::segment(segment, collection);
    let Some(prepared) = prepare_fts_accesses(
        &indexes,
        collection,
        fts,
        segment.raw.record_count as usize == segment.record_count(),
        segment.logical_records.len() != segment.record_count() as u64,
        segment_raw_covers_identity_field(segment, collection),
        segment.record_count() as u64,
    ) else {
        return Ok(None);
    };
    prepared.candidates_checked()
}

pub(crate) fn native_segment_estimate_fts_candidates(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<(u64, f64)> {
    let indexes = IndexCatalog::segment(segment, collection);
    prepare_fts_accesses(
        &indexes,
        collection,
        fts,
        segment.raw.record_count as usize == segment.record_count(),
        segment.logical_records.len() != segment.record_count() as u64,
        segment_raw_covers_identity_field(segment, collection),
        segment.record_count() as u64,
    )?
    .estimate()
}

pub(crate) fn segment_fts_candidates(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    fts: &BoundFts,
) -> Result<Option<RoaringBitmap>, String> {
    use crate::query_access::contract::{AccessExecution, IndexAccessProvider, InputEstimate};
    use crate::query_access::text::NativeSegmentTextProvider;

    let provider = NativeSegmentTextProvider::new(segment, collection);
    #[cfg(test)]
    let test_providers = crate::query_execution::request::all_test_access_providers();
    let mut providers: SmallVec<[&dyn IndexAccessProvider; 4]> = SmallVec::new();
    providers.push(&provider);
    #[cfg(test)]
    for test_provider in &test_providers {
        providers.push(test_provider.as_ref());
    }
    let catalog = ProviderCatalog::new(providers);
    let offers = catalog
        .propose(
            TermId::new(0),
            AccessRequest::Text(fts),
            InputEstimate {
                domain: provider.domain(),
                domain_len: segment.record_count() as u64,
                input_len: segment.record_count() as u64,
                mean_record_bytes: collection.stats.mean_record_bytes(),
                mean_records_per_file: collection.stats.mean_records_per_file(),
                file_count: segment.file_count() as u64,
            },
        )
        .map_err(|error| error.to_string())?;
    let Some(chosen) = crate::query_plan::selection::choose_offer(
        TermId::new(0),
        &offers,
        segment.record_count() as u64,
    )
    .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let input = (0..segment.record_count() as u32).collect::<RoaringBitmap>();
    let applied = catalog
        .apply(
            TermId::new(0),
            AccessRequest::Text(fts),
            chosen.id(),
            provider.input_for(&input),
            &AccessExecution { context: None },
        )
        .map_err(|error| error.to_string())?;
    match applied.possible.into_slots() {
        crate::query_access::membership::SlotSet::Owned(bitmap) => Ok(Some(bitmap)),
        crate::query_access::membership::SlotSet::Unrestricted => Ok(None),
        crate::query_access::membership::SlotSet::Empty => Ok(Some(RoaringBitmap::new())),
        _ => Err("segment text provider returned a non-owned bitmap".into()),
    }
}

fn segment_text_candidates(
    segment: &IndexedCollectionSegment,
    term: &BoundTerm,
    collection: &CollectionState,
) -> Result<Option<RoaringBitmap>, String> {
    if term.derived != Derived::None
        || !matches!(term.operator, Operator::Contains | Operator::Eq)
        || !custom_bigram_compatible(term)
    {
        return Ok(None);
    }
    let Value::String(literal) = &term.literal else {
        return Ok(None);
    };
    if literal.contains(['%', '\\']) {
        return Ok(None);
    }
    let indexes = IndexCatalog::segment(segment, collection);
    let raw_compatible = matches!(term.transform, Transform::Identity | Transform::CaseFold)
        && term.operation.is_none()
        && term.matcher.is_none()
        && segment_raw_covers_identity_field(segment, collection);
    let Some(index) = indexes.field_text_for(
        &collection.definition.schema,
        &term.path.segments,
        &term.binding_name,
        term.operation.is_some(),
        true,
        raw_compatible,
    ) else {
        return Ok(None);
    };
    let Some(request) = term.gram_request.as_ref() else {
        return Ok(None);
    };
    Ok(index.candidates_prepared(request))
}

pub(crate) fn segment_text_estimate(
    segment: &IndexedCollectionSegment,
    term: &BoundTerm,
    collection: &CollectionState,
) -> Option<u64> {
    if term.derived != Derived::None
        || !matches!(term.operator, Operator::Contains | Operator::Eq)
        || !custom_bigram_compatible(term)
    {
        return None;
    }
    let Value::String(literal) = &term.literal else {
        return None;
    };
    if literal.contains(['%', '\\']) {
        return None;
    }
    let indexes = IndexCatalog::segment(segment, collection);
    let raw_compatible = matches!(term.transform, Transform::Identity | Transform::CaseFold)
        && term.operation.is_none()
        && term.matcher.is_none()
        && segment_raw_covers_identity_field(segment, collection);
    let index = indexes.field_text_for(
        &collection.definition.schema,
        &term.path.segments,
        &term.binding_name,
        term.operation.is_some(),
        true,
        raw_compatible,
    )?;
    index.estimate_prepared(term.gram_request.as_ref()?)
}

fn extend_segment_file_slots(
    segment: &IndexedCollectionSegment,
    index: usize,
    matches: &mut RoaringBitmap,
) -> Result<(), String> {
    segment.extend_file_slots(index, matches)
}

/// Only the selected scalar metadata field is read by these two fast paths.
/// Reuse one context object while direct table access supplies its value.
fn metadata_probe_file() -> FileEntry {
    FileEntry {
        id: FileId(0),
        relative_path: SharedPath::from(""),
        revision: SourceRevision {
            modified_ns: 0,
            size: 0,
        },
        created_ns: None,
        collections: Vec::new(),
        status: FileStatus::Valid,
        diagnostics: Vec::new(),
        omitted_diagnostics: 0,
    }
}

fn metadata_probe_record() -> RecordEntry {
    RecordEntry {
        slot: RecordSlot(0),
        id: RecordId(Arc::from("")),
        file: FileId(0),
        range: ByteRange { start: 0, end: 0 },
        revision: SourceRevision {
            modified_ns: 0,
            size: 0,
        },
        class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
    }
}

fn segment_file_size_matches(
    segment: &IndexedCollectionSegment,
    domain: &RoaringBitmap,
    term: &BoundTerm,
) -> Result<Option<RoaringBitmap>, String> {
    if term.path.root != PathRoot::Metadata(MetadataField::FileSize)
        || term.transform != Transform::Identity
        || term.derived != Derived::None
        || term.period.is_some()
        || term.rhs.is_some()
    {
        return Ok(None);
    }
    let Value::Number(literal) = &term.literal else {
        return Ok(None);
    };
    let mut matches = RoaringBitmap::new();
    for index in 0..segment.file_count() {
        let revision = segment
            .files
            .get(index)
            .map(|file| file.entry.revision)
            .ok_or("segment file row missing")?;
        let order = Number::integer(revision.size as i128).cmp(literal);
        if order_matches(order, term.operator) {
            extend_segment_file_slots(segment, index, &mut matches)?;
        }
    }
    matches &= domain;
    Ok(Some(matches))
}

fn segment_file_path_matches(
    segment: &IndexedCollectionSegment,
    domain: &RoaringBitmap,
    catalog: &FileCatalog,
    operator: Operator,
    literal: &str,
) -> Result<RoaringBitmap, String> {
    let mut matches = RoaringBitmap::new();
    for index in 0..segment.file_count() {
        let id = segment
            .files
            .get(index)
            .map(|file| file.entry.id)
            .ok_or("segment file row missing")?;
        let Some(entry) = catalog.entry(id) else {
            continue;
        };
        let path = crate::path_repr::query_path(&entry.relative_path);
        if file_path_matches(&path, operator, literal) {
            extend_segment_file_slots(segment, index, &mut matches)?;
        }
    }
    Ok(matches & domain)
}

fn segment_file_modified_matches(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    domain: &RoaringBitmap,
    term: &BoundTerm,
) -> Result<Option<RoaringBitmap>, String> {
    if term.path.root != PathRoot::Metadata(MetadataField::FileModified)
        || term.rhs.is_some()
        || term.related.is_some()
        || term.operation.is_some()
    {
        return Ok(None);
    }
    // Compatible moves retain the indexed revision; changed metadata takes
    // the replacement route. The segment's immutable file revision is exact.
    let empty = Value::Object(BTreeMap::new());
    let mut file = metadata_probe_file();
    let record = metadata_probe_record();
    let direct_literal = if term.transform == Transform::Identity
        && term.derived == Derived::None
        && term.period.is_none()
        && term.matcher.is_none()
        && matches!(
            term.operator,
            Operator::Eq | Operator::Ne | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
        ) {
        match &term.literal {
            Value::Datetime(value) => Some(value.unix_timestamp_nanos()),
            _ => None,
        }
    } else {
        None
    };
    let mut matches = RoaringBitmap::new();
    for index in 0..segment.file_count() {
        if segment
            .files
            .get(index)
            .map(|file| file.records.len())
            .ok_or("segment file row missing")?
            == 0
        {
            continue;
        }
        file.revision = segment
            .files
            .get(index)
            .map(|file| file.entry.revision)
            .ok_or("segment file row missing")?;
        let selected = if let Some(literal) = direct_literal {
            let modified = file.revision.modified_ns;
            time::OffsetDateTime::from_unix_timestamp_nanos(modified)
                .map_err(|error| error.to_string())?;
            order_matches(modified.cmp(&literal), term.operator)
        } else {
            let ctx = RecordContext {
                value: &empty,
                source_bytes: None,
                record: &record,
                file: &file,
                collection_name: &collection.definition.name,
                repository_root: std::path::Path::new(""),
            };
            eval_term(term, &ctx)?
        };
        if selected {
            extend_segment_file_slots(segment, index, &mut matches)?;
        }
    }
    matches &= domain;
    Ok(Some(matches))
}

fn segment_record_range_matches(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    domain: &RoaringBitmap,
    term: &BoundTerm,
) -> Result<Option<RoaringBitmap>, String> {
    if !matches!(
        term.path.root,
        PathRoot::Metadata(
            MetadataField::RecordByteStart
                | MetadataField::RecordByteEnd
                | MetadataField::RecordByteLength
        )
    ) || term.rhs.is_some()
        || term.related.is_some()
        || term.operation.is_some()
        || term.matcher.is_some()
    {
        return Ok(None);
    }
    let empty = Value::Object(BTreeMap::new());
    let file = metadata_probe_file();
    let mut record = metadata_probe_record();
    let mut matches = RoaringBitmap::new();
    for slot in domain.iter() {
        record.range = segment
            .record(RecordSlot(slot))
            .map(|record| record.range)
            .ok_or("segment record row missing")?;
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record: &record,
            file: &file,
            collection_name: &collection.definition.name,
            repository_root: std::path::Path::new(""),
        };
        if eval_term(term, &ctx)? {
            matches.insert(slot);
        }
    }
    Ok(Some(matches))
}

fn segment_sort_column_matches(
    segment: &IndexedCollectionSegment,
    logical_active: &RoaringBitmap,
    term: &BoundTerm,
    ordered_only: bool,
) -> Option<RoaringBitmap> {
    if term.derived != Derived::None
        || term.period.is_some()
        || term.multi
        || term.transform != Transform::Identity
        || term.operation.is_some()
        || term.rhs.is_some()
        || !matches!(
            term.kind,
            LogicalType::Number | LogicalType::Date | LogicalType::Datetime | LogicalType::Unixtime
        )
        || !matches!(
            term.operator,
            Operator::Eq | Operator::Ne | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
        )
    {
        return None;
    }
    let literal = sort_key(&term.literal)?;
    let column = segment.sorting.column(&term.path.display()).ok()?;
    if column.keys.len() != segment.record_count() {
        return None;
    }
    // A configured numeric/temporal order is already sorted by these keys.
    // When every key has the literal's type, a range predicate is one slice
    // of that order. Null/missing or mixed-type shards keep the slower proof.
    if matches!(
        term.operator,
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
    ) && column.ascending.len() == column.keys.len()
        && let (Some(first), Some(last)) = (
            column
                .ascending
                .first()
                .and_then(|slot| column.keys.get(slot.0 as usize)),
            column
                .ascending
                .last()
                .and_then(|slot| column.keys.get(slot.0 as usize)),
        )
        && std::mem::discriminant(first) == std::mem::discriminant(&literal)
        && std::mem::discriminant(last) == std::mem::discriminant(&literal)
    {
        if order_matches(first.compare(&literal, false), term.operator)
            && order_matches(last.compare(&literal, false), term.operator)
        {
            return Some(logical_active.clone());
        }
        let lower = column.ascending.partition_point(|slot| {
            column.keys[slot.0 as usize].compare(&literal, false) == std::cmp::Ordering::Less
        });
        let upper = column.ascending.partition_point(|slot| {
            column.keys[slot.0 as usize].compare(&literal, false) != std::cmp::Ordering::Greater
        });
        let selected = match term.operator {
            Operator::Gt => &column.ascending[upper..],
            Operator::Ge => &column.ascending[lower..],
            Operator::Lt => &column.ascending[..lower],
            Operator::Le => &column.ascending[..upper],
            _ => unreachable!(),
        };
        let mut matches = RoaringBitmap::new();
        matches.extend(selected.iter().map(|slot| slot.0));
        matches &= logical_active;
        return Some(matches);
    }
    if ordered_only {
        return None;
    }
    let mut matches = RoaringBitmap::new();
    for slot in logical_active.iter() {
        let key = &column.keys[slot as usize];
        if !matches!(key, SortKey::Missing | SortKey::Null)
            && order_matches(key.compare(&literal, false), term.operator)
        {
            matches.insert(slot);
        }
    }
    Some(matches)
}

fn segment_positional_eq_candidates(
    segment: &IndexedCollectionSegment,
    logical_active: &RoaringBitmap,
    term: &BoundTerm,
) -> Result<Option<RoaringBitmap>, String> {
    if !term.query_path.has_position
        || term.path.root != PathRoot::Data
        || term.related.is_some()
        || term.rhs.is_some()
        || term.operation.is_some()
        || term.derived != Derived::None
        || term.period.is_some()
        || term.transform != Transform::Identity
        || term.operator != Operator::Eq
    {
        return Ok(None);
    }
    if term
        .query_path
        .steps
        .iter()
        .filter(|step| matches!(step, crate::query_path::PathStep::ArrayIndex(_)))
        .take(2)
        .count()
        > 1
    {
        return Ok(None);
    }
    // A match at a particular array position must also occur in the
    // unpositioned value posting. Position still requires source verification.
    let Some(index) = segment.postings.fields.get(&term.path.segments) else {
        return Ok(None);
    };
    Ok(index.exact_set_restricted(term, &segment.logical_records, logical_active))
}

fn segment_raw_covers_identity_field(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
) -> bool {
    collection.source.reader().logical_text_coverage() == LogicalTextCoverage::AllStringFields
        && collection.definition.schema.raw.is_none()
        && segment.raw.record_count as usize == segment.record_count()
}

/// Metadata is resident in the published record and file catalog. Evaluate it
/// over the active slots, just as the base collection metadata access does,
/// without reopening source files.
fn segment_metadata_matches(
    segment: &IndexedCollectionSegment,
    collection: &CollectionState,
    domain: &RoaringBitmap,
    term: &BoundTerm,
    catalog: Option<&FileCatalog>,
) -> Result<Option<RoaringBitmap>, String> {
    let PathRoot::Metadata(_) = term.path.root else {
        return Ok(None);
    };
    if term.related.is_some()
        || term
            .rhs
            .as_ref()
            .is_some_and(|rhs| !matches!(rhs.path.root, PathRoot::Metadata(_)))
        || term.matcher.is_some()
        || term.operation.is_some()
    {
        return Ok(None);
    }
    let Some(catalog) = catalog else {
        return Ok(None);
    };
    let empty = Value::Object(BTreeMap::new());
    if term.rhs.is_none()
        && term.transform == Transform::Identity
        && matches!(
            term.path.root,
            PathRoot::Metadata(MetadataField::CollectionName | MetadataField::RepositoryRoot)
        )
    {
        let Some(slot) = domain.iter().next() else {
            return Ok(Some(RoaringBitmap::new()));
        };
        let record = segment
            .record(crate::model::RecordSlot(slot))
            .ok_or("segment record row missing")?;
        let Some(file) = catalog.entry(record.file) else {
            return Ok(None);
        };
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record,
            file,
            collection_name: &collection.definition.name,
            repository_root: catalog.repository_root(),
        };
        return Ok(Some(if eval_term(term, &ctx)? {
            domain.clone()
        } else {
            RoaringBitmap::new()
        }));
    }
    let mut matches = RoaringBitmap::new();
    for slot in domain.iter() {
        let record = segment
            .record(crate::model::RecordSlot(slot))
            .ok_or("segment record row missing")?;
        let Some(file) = catalog.entry(record.file) else {
            return Ok(None);
        };
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record,
            file,
            collection_name: &collection.definition.name,
            repository_root: catalog.repository_root(),
        };
        if eval_term(term, &ctx)? {
            matches.insert(slot);
        }
    }
    Ok(Some(matches))
}

#[allow(clippy::too_many_arguments)]
fn segment_expr_bounds_with_context(
    segment: &IndexedCollectionSegment,
    expr: &BoundExpr,
    collection: &CollectionState,
    domain: &RoaringBitmap,
    logical_active: &RoaringBitmap,
    fallback_active: &RoaringBitmap,
    catalog: Option<&FileCatalog>,
    context: &mut SegmentPlanContext<'_>,
) -> Result<SegmentCandidateBounds, String> {
    let _ = (logical_active, fallback_active);
    Ok(match expr {
        BoundExpr::Term(term) => context.execute_term(term, domain)?,
        BoundExpr::And(parts) => {
            let mut flattened = Vec::new();
            for part in parts {
                flatten_and(part, &mut flattened);
            }
            let path_range = catalog.and_then(|_| combined_file_path_bounds(&flattened));
            let mut may = domain.clone();
            let mut must = domain.clone();
            if let (Some(catalog), Some((bounds, _))) = (catalog, &path_range) {
                let mut matches = RoaringBitmap::new();
                for file in catalog.relative_path_ids_in_bounds(bounds.clone()) {
                    if let Some(slots) = segment.file_records(file) {
                        matches.extend(slots.iter().map(|slot| slot.0));
                    }
                }
                matches &= domain;
                may &= &matches;
                must &= matches;
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
                let child_logical = &may & &segment.logical_records;
                let child_fallback = &may - &segment.logical_records;
                let child = segment_expr_bounds_with_context(
                    segment,
                    part,
                    collection,
                    &may,
                    &child_logical,
                    &child_fallback,
                    catalog,
                    context,
                )?;
                must &= &child.must;
                may = child.may;
            }
            SegmentCandidateBounds { may, must }
        }
        BoundExpr::Or(parts) => {
            let mut may = RoaringBitmap::new();
            let mut must = RoaringBitmap::new();
            for part in parts {
                let child_logical = domain & &segment.logical_records;
                let child_fallback = domain - &segment.logical_records;
                let child = segment_expr_bounds_with_context(
                    segment,
                    part,
                    collection,
                    domain,
                    &child_logical,
                    &child_fallback,
                    catalog,
                    context,
                )?;
                may |= child.may;
                must |= child.must;
            }
            may &= domain;
            must &= domain;
            SegmentCandidateBounds { may, must }
        }
        BoundExpr::Not(inner) => {
            let child_logical = domain & &segment.logical_records;
            let child_fallback = domain - &segment.logical_records;
            let child = segment_expr_bounds_with_context(
                segment,
                inner,
                collection,
                domain,
                &child_logical,
                &child_fallback,
                catalog,
                context,
            )?;
            SegmentCandidateBounds {
                may: domain - &child.must,
                must: domain - &child.may,
            }
        }
    })
}

fn segment_expr_bounds(
    segment: &IndexedCollectionSegment,
    expr: &BoundExpr,
    collection: &CollectionState,
    domain: &RoaringBitmap,
    logical_active: &RoaringBitmap,
    fallback_active: &RoaringBitmap,
    catalog: Option<&FileCatalog>,
) -> Result<SegmentCandidateBounds, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            if term.query_path.has_position {
                if let Some(matches) =
                    segment_positional_eq_candidates(segment, logical_active, term)?
                {
                    return Ok(SegmentCandidateBounds {
                        may: &matches | fallback_active,
                        must: RoaringBitmap::new(),
                    });
                }
                return Ok(SegmentCandidateBounds::unknown(domain));
            }
            if term.related.is_some()
                || (term.rhs.is_some()
                    && !self_comparison_shape(term)
                    && !term.rhs.as_ref().is_some_and(|rhs| {
                        matches!(term.path.root, PathRoot::Metadata(_))
                            && matches!(rhs.path.root, PathRoot::Metadata(_))
                    }))
                || (term.query_path.has_explicit_array_step
                    && term.derived.state_predicate().is_none()
                    && !self_comparison_shape(term))
            {
                return Ok(SegmentCandidateBounds::unknown(domain));
            }
            if term.path.root == PathRoot::Metadata(MetadataField::RecordId)
                && term.operator == Operator::Eq
                && term.transform == crate::reference::Transform::Identity
                && let Value::Id(id) = &term.literal
            {
                let mut matches = RoaringBitmap::new();
                if let Some(slot) = segment.id_lookup.get(id.as_str()).copied()
                    && domain.contains(slot.0)
                {
                    matches.insert(slot.0);
                }
                return Ok(SegmentCandidateBounds {
                    may: matches.clone(),
                    must: matches,
                });
            }
            if let Some(matches) = segment_file_size_matches(segment, domain, term)? {
                return Ok(SegmentCandidateBounds {
                    may: matches.clone(),
                    must: matches,
                });
            }
            if let Some((operator, literal)) = indexed_file_path_term(term)
                && let Some(catalog) = catalog
            {
                let mut matches = if let Some(files) =
                    catalog.relative_path_ids_up_to(operator, literal, MAX_INDEXED_PATH_SCAN)
                {
                    let mut matches = RoaringBitmap::new();
                    for file in files {
                        if let Some(slots) = segment.file_records(file) {
                            matches.extend(slots.iter().map(|slot| slot.0));
                        }
                    }
                    matches
                } else {
                    segment_file_path_matches(segment, domain, catalog, operator, literal)?
                };
                matches &= domain;
                return Ok(SegmentCandidateBounds {
                    may: matches.clone(),
                    must: matches,
                });
            }
            if let Some(matches) = segment_file_modified_matches(segment, collection, domain, term)?
            {
                return Ok(SegmentCandidateBounds {
                    may: matches.clone(),
                    must: matches,
                });
            }
            if let Some(matches) = segment_record_range_matches(segment, collection, domain, term)?
            {
                return Ok(SegmentCandidateBounds {
                    may: matches.clone(),
                    must: matches,
                });
            }
            if let Some(matches) =
                segment_metadata_matches(segment, collection, domain, term, catalog)?
            {
                return Ok(SegmentCandidateBounds {
                    may: matches.clone(),
                    must: matches,
                });
            }
            if term.path.root != PathRoot::Data {
                return Ok(SegmentCandidateBounds::unknown(domain));
            }
            if let Some(matches) = segment_sort_column_matches(segment, logical_active, term, true)
            {
                return Ok(SegmentCandidateBounds {
                    may: &matches | fallback_active,
                    must: matches,
                });
            }
            let index_catalog = IndexCatalog::segment(segment, collection);
            let selected = index_catalog
                .field_postings(
                    &term.path.segments,
                    &term.binding_name,
                    term.operation.is_some(),
                )
                .into_iter()
                .flatten()
                .map(|access| {
                    access
                        .assess(term, index_catalog.logical, &mut 0)
                        .map(|proposal| proposal.map(|proposal| (access, proposal)))
                })
                .collect::<Result<Vec<_>, String>>()?
                .into_iter()
                .flatten()
                .min_by(|(_, left), (_, right)| {
                    let work = |proposal: &crate::index_catalog::PostingProposal| {
                        proposal.lookup_work
                            + proposal.output as f64 * if proposal.exact { 0.02 } else { 50.0 }
                    };
                    work(left).total_cmp(&work(right))
                });
            if let Some((access, proposal)) = selected
                && let Some(matches) = access.execute(
                    term,
                    index_catalog.logical,
                    logical_active,
                    proposal.operation,
                )?
            {
                return Ok(SegmentCandidateBounds {
                    may: &matches | fallback_active,
                    must: if proposal.exact {
                        matches
                    } else {
                        RoaringBitmap::new()
                    },
                });
            }
            {
                if let Some(matches) =
                    segment_sort_column_matches(segment, logical_active, term, false)
                {
                    return Ok(SegmentCandidateBounds {
                        may: &matches | fallback_active,
                        must: matches,
                    });
                }
                if let Some(mut grams) = segment_text_candidates(segment, term, collection)? {
                    if empty_selection_preservation(term) {
                        grams |= &segment.empty_array_records;
                    }
                    SegmentCandidateBounds {
                        may: (&grams & domain) | fallback_active,
                        must: RoaringBitmap::new(),
                    }
                } else {
                    SegmentCandidateBounds::unknown(domain)
                }
            }
        }
        BoundExpr::And(parts) => {
            let mut flattened = Vec::new();
            for part in parts {
                flatten_and(part, &mut flattened);
            }
            let path_range = catalog.and_then(|_| combined_file_path_bounds(&flattened));
            let mut may = domain.clone();
            let mut must = domain.clone();
            if let (Some(catalog), Some((bounds, _))) = (catalog, &path_range) {
                let mut matches = RoaringBitmap::new();
                for file in catalog.relative_path_ids_in_bounds(bounds.clone()) {
                    if let Some(slots) = segment.file_records(file) {
                        matches.extend(slots.iter().map(|slot| slot.0));
                    }
                }
                matches &= domain;
                may &= &matches;
                must &= matches;
            }
            for (index, part) in flattened.into_iter().enumerate() {
                if path_range
                    .as_ref()
                    .is_some_and(|(_, indices)| indices.contains(&index))
                {
                    continue;
                }
                let child = segment_expr_bounds(
                    segment,
                    part,
                    collection,
                    domain,
                    logical_active,
                    fallback_active,
                    catalog,
                )?;
                may &= child.may;
                must &= child.must;
            }
            SegmentCandidateBounds { may, must }
        }
        BoundExpr::Or(parts) => {
            let mut may = RoaringBitmap::new();
            let mut must = RoaringBitmap::new();
            for part in parts {
                let child = segment_expr_bounds(
                    segment,
                    part,
                    collection,
                    domain,
                    logical_active,
                    fallback_active,
                    catalog,
                )?;
                may |= child.may;
                must |= child.must;
            }
            SegmentCandidateBounds { may, must }
        }
        BoundExpr::Not(inner) => {
            let child = segment_expr_bounds(
                segment,
                inner,
                collection,
                domain,
                logical_active,
                fallback_active,
                catalog,
            )?;
            SegmentCandidateBounds {
                may: domain - &child.must,
                must: domain - &child.may,
            }
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
    provider: NativeCollectionProvider<'a>,
    offers: HashMap<TermId, OfferList>,
    test_providers: Vec<Arc<dyn IndexAccessProvider>>,
    term_ids: HashMap<(u32, (usize, usize)), TermId>,
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
            provider: NativeCollectionProvider::new(
                repo,
                collection,
                collection.stats.record_count as u64,
                None,
            ),
            offers: HashMap::new(),
            test_providers,
            term_ids: HashMap::new(),
            next_term_id: 0,
            counters: PlanningCounters::default(),
            diagnostics,
            predicates,
        }
    }

    fn for_estimation(collection: &'a CollectionState, input_len: u64, diagnostics: bool) -> Self {
        Self {
            provider: NativeCollectionProvider::for_estimation(collection, input_len),
            offers: HashMap::new(),
            test_providers: Vec::new(),
            term_ids: HashMap::new(),
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
        providers.push(&self.provider);
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
        let key = (collection.generation.collection.0, term.span);
        if let Some(id) = self.term_ids.get(&key) {
            return *id;
        }
        if let Some(predicates) = &self.predicates
            && let Some(id) =
                predicates.term_for_span(collection.generation.collection.0 as usize, term.span)
        {
            self.term_ids.insert(key, id);
            return id;
        }
        let id = TermId::new(self.next_term_id);
        self.next_term_id = self
            .next_term_id
            .checked_add(1)
            .expect("query term ID overflow");
        self.term_ids.insert(key, id);
        id
    }

    fn execute_native_term(
        &mut self,
        repo: &Repository,
        collection: &CollectionState,
        term: &BoundTerm,
        input: &RoaringBitmap,
    ) -> Result<(Option<RoaringBitmap>, Option<AccessOffer>, Vec<AccessOffer>), String> {
        let term_id = self.term_id(collection, term);
        let _ = repo;
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
                &AccessExecution { context: None },
            )
            .map_err(|error| error.to_string())?
            .possible;
        let slots = match restricted.into_slots() {
            crate::query_access::membership::SlotSet::Unrestricted => None,
            crate::query_access::membership::SlotSet::Empty => Some(RoaringBitmap::new()),
            crate::query_access::membership::SlotSet::Owned(bitmap) => Some(bitmap),
            crate::query_access::membership::SlotSet::Borrowed(bitmap) => Some(bitmap & input),
            crate::query_access::membership::SlotSet::Shared(bitmap) => {
                Some(bitmap.as_ref() & input)
            }
        };
        Ok((slots, Some(chosen), alternatives))
    }

    fn estimate_prepared_term(
        &self,
        repo: &Repository,
        collection: &CollectionState,
        predicates: &PreparedPredicates,
        term_id: TermId,
        term: &BoundTerm,
        input_len: u64,
    ) -> Result<Option<(u64, f64)>, String> {
        let provider = NativeCollectionProvider::new(repo, collection, input_len, None);
        #[cfg(test)]
        let test_providers =
            crate::query_execution::request::test_access_providers(repo.instance_id);
        let mut providers: SmallVec<[&dyn IndexAccessProvider; 4]> = SmallVec::new();
        providers.push(&provider);
        #[cfg(test)]
        for test_provider in &test_providers {
            providers.push(test_provider.as_ref());
        }
        let catalog = ProviderCatalog::new(providers);
        let offers = catalog
            .propose_prepared(
                predicates,
                term_id,
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
        let _ = term;
        Ok(
            crate::query_plan::selection::choose_offer(term_id, &offers, input_len)
                .map_err(|error| error.to_string())?
                .map(|offer| (offer.estimated_output(), offer.estimated_work(input_len))),
        )
    }
}

fn rescore_paths(
    collection: &CollectionState,
    term: &BoundTerm,
    paths: &[AccessPathEstimate],
    input_len: u64,
) -> Vec<AccessPathEstimate> {
    let stats = collection.stats;
    let source_unit = 30.0 + stats.mean_record_bytes() / 128.0;
    paths
        .iter()
        .map(|original| {
            let mut path = original.clone();
            let original_unit = if path.exact { 0.02 } else { source_unit };
            let lookup = path.estimated_work - path.estimated_output as f64 * original_unit;
            path.estimated_output = match path.kind {
                AccessPathKind::Scanner => input_len,
                AccessPathKind::SortColumn => input_len / 4,
                AccessPathKind::Metadata
                    if term.path.root == PathRoot::Metadata(MetadataField::RecordId)
                        && term.operator == Operator::Eq =>
                {
                    input_len.min(1)
                }
                AccessPathKind::Metadata | AccessPathKind::FilePathIndex => input_len / 2,
                _ => path.estimated_output.min(input_len),
            };
            path.estimated_work = lookup + path.estimated_output as f64 * original_unit;
            path.estimated_files = (path.estimated_output as f64
                / stats.mean_records_per_file().max(1.0))
            .min(stats.file_count as f64);
            path.estimated_bytes = path.estimated_output as f64 * stats.mean_record_bytes();
            path
        })
        .collect()
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
                    let record = &collection.generation.records[slot as usize];
                    let file = &repo.files.files[record.file.0 as usize];
                    let ctx = RecordContext {
                        value: &empty,
                        source_bytes: None,
                        record,
                        file,
                        collection_name: &collection.definition.name,
                        repository_root: &repo.files.root,
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
    pub semijoin_work: crate::relation_semijoin::SemijoinWork,
    pub planning: PlanningCounters,
}

/// A whole-record raw index is a field superset only when its source reader
/// supplies every logical string and the indexed representation is identity.
/// Resolve this at planning time, before any record is visited. The source
/// registration and raw binding versions are fixed by the collection's
/// immutable generation and snapshot configuration.
fn raw_covers_identity_field(collection: &CollectionState) -> Result<(), String> {
    if collection.source.reader().logical_text_coverage() != LogicalTextCoverage::AllStringFields {
        return Err(format!(
            "source {}@{} does not cover every logical string field",
            collection.source.id(),
            collection.source.version()
        ));
    }
    if let Some(binding) = &collection.definition.schema.raw {
        return Err(format!(
            "raw representation {}@{} differs from identity field text",
            binding.name, binding.version
        ));
    }
    if collection.raw.record_count as usize != collection.generation.records.len() {
        return Err("raw index has incomplete record coverage".into());
    }
    Ok(())
}

fn empty_selection_preservation(term: &BoundTerm) -> bool {
    term.multi && term.quantifier != Quantifier::Any
}

fn raw_bigram_candidates(collection: &CollectionState, term: &BoundTerm) -> Option<RoaringBitmap> {
    let request = term.gram_request.as_ref()?;
    let mut grams = collection.raw.candidates_prepared(request)?;
    if empty_selection_preservation(term) {
        // The universal comparison may be true on a present empty array with
        // no string witness. This bitmap is deliberately collection-wide.
        grams |= &collection.empty_array_records;
    }
    Some(grams)
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
                let record = &collection.generation.records[slot as usize];
                let file = &repo.files.files[record.file.0 as usize];
                let ctx = RecordContext {
                    value: &empty,
                    source_bytes: None,
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &repo.files.root,
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
                if let Some(slot) = collection.id_lookup.get(id.as_str()) {
                    result.insert(slot.0);
                }
                return Ok(Some(result));
            }
        }
        if let Some((operator, literal)) = indexed_file_path_term(term) {
            let mut result = RoaringBitmap::new();
            if input.len() <= 4096 {
                let mut matched_files = HashMap::new();
                for slot in input.iter() {
                    let file_id = collection.generation.records[slot as usize].file;
                    let matches = *matched_files.entry(file_id).or_insert_with(|| {
                        let file = &repo.files.files[file_id.0 as usize];
                        let path = crate::path_repr::query_path(&file.relative_path);
                        file_path_matches(&path, operator, literal)
                    });
                    if matches {
                        result.insert(slot);
                    }
                }
                return Ok(Some(result));
            }
            if let Some(files) =
                repo.files
                    .relative_path_ids_up_to(operator, literal, MAX_INDEXED_PATH_SCAN)
            {
                for file in files {
                    if let Some(slots) = collection.generation.file_records.records_for(file) {
                        result.extend(slots.iter().map(|slot| slot.0));
                    }
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
            for file_id in &collection.generation.files {
                let Some(slots) = collection.generation.file_records.records_for(*file_id) else {
                    continue;
                };
                let Some(first) = slots.first() else {
                    continue;
                };
                let record = &collection.generation.records[first.0 as usize];
                let file = &repo.files.files[file_id.0 as usize];
                let ctx = RecordContext {
                    value: &empty,
                    source_bytes: None,
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &repo.files.root,
                };
                if eval_term(term, &ctx)? {
                    result.extend(slots.iter().map(|s| s.0));
                }
            }
            return Ok(Some(result));
        }
        if matches!(
            field,
            MetadataField::CollectionName | MetadataField::RepositoryRoot
        ) {
            if let Some(record) = collection.generation.records.first() {
                let file = &repo.files.files[record.file.0 as usize];
                let ctx = RecordContext {
                    value: &empty,
                    source_bytes: None,
                    record,
                    file,
                    collection_name: &collection.definition.name,
                    repository_root: &repo.files.root,
                };
                if eval_term(term, &ctx)? {
                    return Ok(Some(domain.clone()));
                }
            }
            return Ok(Some(result));
        }
        for record in &collection.generation.records {
            let file = &repo.files.files[record.file.0 as usize];
            let ctx = RecordContext {
                value: &empty,
                source_bytes: None,
                record,
                file,
                collection_name: &collection.definition.name,
                repository_root: &repo.files.root,
            };
            if eval_term(term, &ctx)? {
                result.insert(record.slot.0);
            }
        }
        return Ok(Some(result));
    }
    if term.operation.is_some() {
        Ok(collection
            .custom_postings
            .get(&(term.path.segments.clone(), term.binding_name.clone()))
            .filter(|posting| posting.complete)
            .and_then(|posting| posting.index.exact_set(term, domain)))
    } else {
        Ok(collection
            .postings
            .fields
            .get(&term.path.segments)
            .and_then(|index| index.exact_set(term, domain)))
    }
}

fn custom_bigram_compatible(term: &BoundTerm) -> bool {
    if term.operation.is_none() {
        // Registered built-in transforms map indexed text and scanned values
        // with the same Transform; they do not install an exact matcher.
        return true;
    }
    term.matcher
        .as_ref()
        .is_some_and(|matcher| matcher.bigram_compatible())
}

fn custom_text_index<'a>(
    collection: &'a CollectionState,
    path: &[String],
    name: &str,
) -> Option<&'a crate::bigram::BigramIndex> {
    IndexCatalog::collection(collection).field_text_for(
        &collection.definition.schema,
        path,
        name,
        !matches!(name, "default" | "lower"),
        true,
        false,
    )
}

pub(crate) fn enumerate_paths(
    collection: &CollectionState,
    term: &BoundTerm,
    input_len: u64,
    dictionary_keys_visited: &mut u64,
) -> Result<Vec<AccessPathEstimate>, String> {
    let stats = collection.stats;
    let source_unit = 30.0 + stats.mean_record_bytes() / 128.0;
    let mut paths = Vec::new();
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
                posting.map_or(
                    collection.logical_records.len() as u32,
                    |(_, _, coverage)| coverage,
                )
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
            if let Some(proposal) = access.assess(term, catalog.logical, dictionary_keys_visited)? {
                add(
                    AccessPathKind::Posting,
                    proposal.exact,
                    proposal.lookup_work,
                    proposal.output,
                    Some((proposal.id, proposal.operation, proposal.coverage)),
                );
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
            && collection.sorting.column(&term.path.display()).is_ok()
        {
            add(
                AccessPathKind::SortColumn,
                true,
                stats.record_count as f64,
                input_len / 4,
                None,
            );
        }
        if term.derived == Derived::None
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
                            let structural_count = empty_selection_preservation(term)
                                .then_some(collection.empty_array_records.len());
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
        AccessPathKind::CustomBigram => custom_candidates(collection, term),
        AccessPathKind::RawBigram => raw_bigram_candidates(collection, term),
        AccessPathKind::SortColumn => {
            let column = collection.sorting.column(&term.path.display())?;
            let literal = sort_key(&term.literal).ok_or("sort-column predicate is not scalar")?;
            let mut result = RoaringBitmap::new();
            for (slot, key) in column.keys.iter().enumerate() {
                if !matches!(key, SortKey::Missing | SortKey::Null)
                    && order_matches(key.compare(&literal, false), term.operator)
                {
                    result.insert(slot as u32);
                }
            }
            Some(result)
        }
        AccessPathKind::RelationTargetFirst | AccessPathKind::RelationSourceRestricted => {
            return Err("relation strategy is selected after local planning".into());
        }
        AccessPathKind::Scanner => None,
    })
}

fn sort_key(value: &Value) -> Option<SortKey> {
    Some(match value {
        Value::Number(n) => SortKey::Number(n.clone()),
        Value::Date(d) => SortKey::Date(*d),
        Value::Datetime(d) => SortKey::Datetime(*d),
        Value::Unixtime(t) => SortKey::Unixtime(*t),
        _ => return None,
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

fn order_matches(order: std::cmp::Ordering, op: Operator) -> bool {
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
    estimate_expr_cached(
        collection,
        expr,
        input_len,
        &mut PlanContext::for_estimation(collection, input_len, false),
    )
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
    let mut context = PlanContext::for_estimation(collection, input_len, true);
    let node = estimate_plan_node(collection, expr, input_len, &mut context)?;
    Ok(EstimatedPlan {
        candidates: node.candidates,
        exact: node.exact,
        choices: node.choices,
        planning: context.counters,
    })
}

pub(crate) fn estimate_prepared_filter_term(
    repo: &Repository,
    collection: &CollectionState,
    predicates: &PreparedPredicates,
    term_id: TermId,
    term: &BoundTerm,
    input_len: u64,
) -> Result<Option<(u64, f64)>, String> {
    PlanContext::new(repo, collection, false, None)
        .estimate_prepared_term(repo, collection, predicates, term_id, term, input_len)
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
                .then(|| {
                    term.gram_request
                        .as_ref()
                        .and_then(|request| collection.raw.estimate_prepared(request))
                })
                .flatten();
            let structural_candidates = (selected.kind == AccessPathKind::RawBigram
                && empty_selection_preservation(term))
            .then_some(collection.empty_array_records.len());
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
    segments: &[Arc<Vec<Arc<crate::lifecycle::PublishedSegment>>>],
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
    segments: &[Arc<Vec<Arc<crate::lifecycle::PublishedSegment>>>],
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    evidence: bool,
    diagnostics: bool,
    execution: Option<&crate::streaming::ExecutionContext>,
    predicates: Option<Arc<PreparedPredicates>>,
) -> Result<Plan, String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    let mut context = PlanContext::new(repo, collection, diagnostics, predicates);
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
}

fn plan_expr_restricted_inner<'a>(
    repo: &'a Repository,
    collection: &'a CollectionState,
    segments: &[Arc<Vec<Arc<crate::lifecycle::PublishedSegment>>>],
    expr: &BoundExpr,
    domain: &RoaringBitmap,
    input: &RoaringBitmap,
    evidence: bool,
    context: &mut PlanContext<'a>,
    execution: Option<&crate::streaming::ExecutionContext>,
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
    if collection.logical_records.len() == collection.generation.records.len() as u64 {
        let mut plan = plan_logical_expr(repo, collection, expr, domain, input, evidence, context)?;
        if !collection.relations.is_empty() {
            crate::relation_semijoin::restrict(repo, segments, expr, input, &mut plan, execution)?;
        }
        return Ok(plan);
    }
    let logical_domain = domain & &collection.logical_records;
    let logical_input = input & &logical_domain;
    let fallback_input = input - &collection.logical_records;
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
    if !collection.relations.is_empty() {
        crate::relation_semijoin::restrict(repo, segments, expr, input, &mut logical, execution)?;
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
                context.execute_native_term(repo, collection, term, input)?;
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
                    .then(|| {
                        term.gram_request
                            .as_ref()
                            .and_then(|request| collection.raw.estimate_prepared(request))
                    })
                    .flatten();
                let structural_candidates = (selected.kind == AccessPathKind::RawBigram
                    && empty_selection_preservation(term))
                .then_some(collection.empty_array_records.len());
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
                        collection.source.id(),
                        collection.source.version(),
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
                        if term.gram_request.is_none()
                            || term.gram_request.as_ref().is_some_and(|request| {
                                collection.raw.estimate_prepared(request).is_none()
                            })
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
            if let Some((bounds, _)) = combined_file_path_bounds(&remaining) {
                let mut matches = RoaringBitmap::new();
                for file in repo.files.relative_path_ids_in_bounds(bounds) {
                    if let Some(slots) = collection.generation.file_records.records_for(file) {
                        matches.extend(slots.iter().map(|slot| slot.0));
                    }
                }
                current &= matches;
            }
            let mut exact = true;
            let mut proofs = Vec::new();
            let mut choices = Vec::new();
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
                collection.generation.collection.0,
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

/// A prepared text access is a set of borrowed candidate providers. The same
/// compatibility and coverage assessment feeds estimation and execution;
/// grams and regex conditions never claim positive field proof.
struct TextProbe<'a, 'b> {
    index: &'a crate::bigram::BigramIndex,
    request: &'b crate::bigram::BigramRequest,
}

struct PreparedFtsAccess<'a, 'b> {
    probes: SmallVec<[TextProbe<'a, 'b>; 2]>,
    regex: Option<&'b crate::regex_query::RegexCondition>,
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

    fn candidates_checked(&self) -> Result<Option<RoaringBitmap>, String> {
        let mut may = RoaringBitmap::new();
        for probe in &self.probes {
            let next = if let Some(condition) = self.regex {
                condition.candidates_checked(probe.index)?
            } else {
                probe.index.candidates_prepared(probe.request)
            };
            let Some(next) = next else {
                return Ok(None);
            };
            may |= next;
        }
        Ok(Some(may))
    }
}

fn prepare_fts_accesses<'a, 'b>(
    indexes: &IndexCatalog<'a>,
    collection: &CollectionState,
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
            || collection
                .definition
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
        let logical_text_complete = collection.source.reader().logical_text_coverage()
            == LogicalTextCoverage::AllStringFields;
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
                &collection.definition.schema,
                &field.path.segments,
                &field.binding_name,
                field.operation.is_some(),
                logical_text_complete,
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

pub(crate) fn native_fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<RoaringBitmap> {
    let indexes = IndexCatalog::collection(collection);
    prepare_fts_accesses(
        &indexes,
        collection,
        fts,
        true,
        collection.logical_records.len() != collection.generation.records.len() as u64,
        raw_covers_identity_field(collection).is_ok(),
        collection.stats.record_count as u64,
    )?
    .candidates()
}

pub(crate) fn native_estimate_fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<(u64, f64)> {
    let indexes = IndexCatalog::collection(collection);
    prepare_fts_accesses(
        &indexes,
        collection,
        fts,
        true,
        collection.logical_records.len() != collection.generation.records.len() as u64,
        raw_covers_identity_field(collection).is_ok(),
        collection.stats.record_count as u64,
    )?
    .estimate()
}

pub(crate) fn estimate_fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<(u64, f64)> {
    native_estimate_fts_candidates(collection, fts)
}

pub(crate) fn fts_candidates(
    collection: &CollectionState,
    fts: &BoundFts,
) -> Option<RoaringBitmap> {
    use crate::query_access::contract::{AccessExecution, IndexAccessProvider, InputEstimate};
    use crate::query_access::text::NativeTextProvider;

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
#[path = "executor.test.rs"]
mod tests;

#[cfg(test)]
#[path = "dynamic_filter.test.rs"]
mod dynamic_filter_tests;
