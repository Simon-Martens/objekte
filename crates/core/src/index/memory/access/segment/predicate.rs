//! Native structured predicate probes for one sealed segment.

use std::{collections::BTreeMap, sync::Arc};

use roaring::RoaringBitmap;

use super::super::catalog::{IndexCatalog, PostingProposal, self_comparison_shape};
use super::super::text::segment_raw_covers_identity_field;
use crate::index::memory::segment::IndexedCollectionSegment;
use crate::index::storage::file_catalog::{FileCatalogReader, FileScalarPredicate};
use crate::index::storage::order::ScalarOrderPredicate;
use crate::index::storage::segment::{SegmentPredicateAccess, SegmentReader};
use crate::index::storage::text::TextAccessPolicy;
use crate::inventory::{
    ByteRange, DiagnosticRef, FileEntry, FileStatus, RecordClass, RecordEntry, SharedPath,
    SourceRevision, relative_path_bounds,
};
use crate::model::{
    FileId, LogicalType, MetadataField, Number, PathRoot, RecordId, RecordSlot, Value,
};
use crate::query::Operator;
use crate::query::binding::filter::{BoundTerm, Derived};
use crate::query::semantics::record::RecordContext;
use crate::query_access::membership::MembershipView;
use crate::query_execution::restrictions::{
    AccessPathKind, MAX_INDEXED_PATH_SCAN, SegmentCandidateBounds, custom_bigram_compatible,
    empty_selection_preservation, file_path_matches, indexed_file_path_term, order_matches,
};
use crate::records::RecordIdSlot;
use crate::reference::filter::eval_term;
use crate::repository::state::CollectionState;
use crate::schema::operation::Transform;

pub(crate) fn segment_access_path_kind(
    term: &BoundTerm,
    segment: &IndexedCollectionSegment,
    policy: TextAccessPolicy<'_>,
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
    let index = IndexCatalog::segment_with_bindings(segment, policy.schema, policy.bindings);
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

fn segment_text_candidates(
    segment: &IndexedCollectionSegment,
    term: &BoundTerm,
    policy: TextAccessPolicy<'_>,
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
    let indexes = IndexCatalog::segment_with_bindings(segment, policy.schema, policy.bindings);
    let raw_compatible = matches!(term.transform, Transform::Identity | Transform::CaseFold)
        && term.operation.is_none()
        && term.matcher.is_none()
        && segment_raw_covers_identity_field(segment, policy);
    let Some(index) = indexes.field_text_for(
        policy.schema,
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
        && segment_raw_covers_identity_field(segment, collection.text_access_policy());
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
            object_id: None,
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
            object_id: None,
        },
        class: RecordClass::Logical,
        diagnostic_ref: DiagnosticRef(0),
    }
}

fn segment_file_size_matches(
    segment: &IndexedCollectionSegment,
    domain: &RoaringBitmap,
    term: &BoundTerm,
    catalog: Option<&dyn FileCatalogReader>,
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
    if let Some(catalog) = catalog
        && let Some(files) = catalog
            .scalar_candidates(FileScalarPredicate::Size(term.operator, literal), 4096)
            .map_err(|error| error.to_string())?
    {
        let mut matches = RoaringBitmap::new();
        for file in files.iter().copied() {
            if let Some(slots) = segment.file_records(file) {
                matches.extend(slots.iter().map(|slot| slot.0));
            }
        }
        matches &= domain;
        return Ok(Some(matches));
    }
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
    catalog: &dyn FileCatalogReader,
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
        let Some(entry) = catalog.file(id).map_err(|error| error.to_string())? else {
            continue;
        };
        let path = crate::model::path_representation::query_path(&entry.relative_path);
        if file_path_matches(&path, operator, literal) {
            extend_segment_file_slots(segment, index, &mut matches)?;
        }
    }
    Ok(matches & domain)
}

fn segment_file_modified_matches(
    segment: &IndexedCollectionSegment,
    collection_name: &str,
    domain: &RoaringBitmap,
    term: &BoundTerm,
    catalog: Option<&dyn FileCatalogReader>,
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
    if let (Some(catalog), Some(literal)) = (catalog, direct_literal)
        && let Some(files) = catalog
            .scalar_candidates(FileScalarPredicate::Modified(term.operator, literal), 4096)
            .map_err(|error| error.to_string())?
    {
        let mut matches = RoaringBitmap::new();
        for file in files.iter().copied() {
            if let Some(slots) = segment.file_records(file) {
                matches.extend(slots.iter().map(|slot| slot.0));
            }
        }
        matches &= domain;
        return Ok(Some(matches));
    }
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
                range: (&record).record_range(),
                file: Some(&file),
                collection_name,
                repository_root: Some(std::path::Path::new("")),
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

fn segment_file_created_matches(
    segment: &IndexedCollectionSegment,
    domain: &RoaringBitmap,
    term: &BoundTerm,
    catalog: Option<&dyn FileCatalogReader>,
) -> Result<Option<RoaringBitmap>, String> {
    if term.path.root != PathRoot::Metadata(MetadataField::FileCreated)
        || term.transform != Transform::Identity
        || term.period.is_some()
        || term.rhs.is_some()
        || term.related.is_some()
        || term.matcher.is_some()
        || term.operation.is_some()
    {
        return Ok(None);
    }
    let Some(catalog) = catalog else {
        return Ok(None);
    };
    let predicate = if term.derived == Derived::Null {
        let Value::Bool(literal) = &term.literal else {
            return Ok(None);
        };
        let want_null = match term.operator {
            Operator::Eq => *literal,
            Operator::Ne => !*literal,
            _ => return Ok(None),
        };
        if !want_null {
            return Ok(None);
        }
        FileScalarPredicate::UnknownCreated
    } else {
        if term.derived != Derived::None {
            return Ok(None);
        }
        let Value::Datetime(literal) = &term.literal else {
            return Ok(None);
        };
        FileScalarPredicate::Created(term.operator, literal.unix_timestamp_nanos())
    };
    let Some(files) = catalog
        .scalar_candidates(predicate, 4096)
        .map_err(|error| error.to_string())?
    else {
        return Ok(None);
    };
    let mut matches = RoaringBitmap::new();
    for file in files.iter().copied() {
        if let Some(slots) = segment.file_records(file) {
            matches.extend(slots.iter().map(|slot| slot.0));
        }
    }
    matches &= domain;
    Ok(Some(matches))
}

fn segment_record_range_matches(
    segment: &IndexedCollectionSegment,
    collection_name: &str,
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
    let record = metadata_probe_record();
    let direct_literal = if term.transform == Transform::Identity
        && term.derived == Derived::None
        && term.period.is_none()
        && matches!(
            term.operator,
            Operator::Eq | Operator::Ne | Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
        ) {
        match &term.literal {
            Value::Number(value) => Some(value),
            _ => None,
        }
    } else {
        None
    };
    let mut matches = RoaringBitmap::new();
    for slot in domain.iter() {
        let range = segment
            .backing
            .file_locator(RecordSlot(slot))
            .and_then(|locator| locator.range);
        let selected = if let Some(literal) = direct_literal {
            range.is_some_and(|range| {
                let value = match term.path.root {
                    PathRoot::Metadata(MetadataField::RecordByteStart) => range.start,
                    PathRoot::Metadata(MetadataField::RecordByteEnd) => range.end,
                    PathRoot::Metadata(MetadataField::RecordByteLength) => range.len(),
                    _ => unreachable!("byte range field checked above"),
                };
                order_matches(Number::integer(value as i128).cmp(literal), term.operator)
            })
        } else {
            let ctx = RecordContext {
                value: &empty,
                source_bytes: None,
                record: &record,
                range,
                file: Some(&file),
                collection_name,
                repository_root: Some(std::path::Path::new("")),
            };
            eval_term(term, &ctx)?
        };
        if selected {
            matches.insert(slot);
        }
    }
    Ok(Some(matches))
}

fn segment_sort_column_matches(
    segment: &dyn SegmentReader,
    logical_active: &RoaringBitmap,
    term: &BoundTerm,
    ordered_only: bool,
) -> Result<Option<RoaringBitmap>, String> {
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
        return Ok(None);
    }
    let Some(predicate) = ScalarOrderPredicate::new(&term.literal, term.operator) else {
        return Ok(None);
    };
    let Some(column) = segment.order_column(&term.path)? else {
        return Ok(None);
    };
    if column.key_count() != segment.descriptor().slots {
        return Err("sort column does not cover the segment".into());
    }
    Ok(column.match_scalar(
        &predicate,
        MembershipView::Borrowed(logical_active),
        ordered_only,
    )?)
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
        .filter(|step| matches!(step, crate::query::path::PathStep::ArrayIndex(_)))
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

/// Metadata is resident in the published record and file catalog. Evaluate it
/// over the active slots, just as the base collection metadata access does,
/// without reopening source files.
fn segment_metadata_matches(
    segment: &IndexedCollectionSegment,
    collection_name: &str,
    domain: &RoaringBitmap,
    term: &BoundTerm,
    catalog: Option<&dyn FileCatalogReader>,
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
        let Some(file) = segment
            .backing
            .file_locator(crate::model::RecordSlot(slot))
            .map(|locator| catalog.file(locator.file))
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten()
        else {
            return Ok(None);
        };
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record,
            range: segment
                .backing
                .file_locator(record.slot)
                .and_then(|locator| locator.range),
            file: Some(file.as_ref()),
            collection_name,
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
        let Some(file) = segment
            .backing
            .file_locator(crate::model::RecordSlot(slot))
            .map(|locator| catalog.file(locator.file))
            .transpose()
            .map_err(|error| error.to_string())?
            .flatten()
        else {
            return Ok(None);
        };
        let ctx = RecordContext {
            value: &empty,
            source_bytes: None,
            record,
            range: segment
                .backing
                .file_locator(record.slot)
                .and_then(|locator| locator.range),
            file: Some(file.as_ref()),
            collection_name,
            repository_root: catalog.repository_root(),
        };
        if eval_term(term, &ctx)? {
            matches.insert(slot);
        }
    }
    Ok(Some(matches))
}

pub(crate) fn native_segment_term_bounds(
    segment: &IndexedCollectionSegment,
    term: &BoundTerm,
    access: SegmentPredicateAccess<'_>,
    domain: &RoaringBitmap,
    logical_active: &RoaringBitmap,
    fallback_active: &RoaringBitmap,
) -> Result<SegmentCandidateBounds, String> {
    let catalog = access.files;
    if term.query_path.has_position {
        if let Some(matches) = segment_positional_eq_candidates(segment, logical_active, term)? {
            return Ok(SegmentCandidateBounds::candidate(
                &matches | fallback_active,
            ));
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
        && term.transform == crate::schema::operation::Transform::Identity
        && let Value::Id(id) = &term.literal
    {
        let mut matches = RoaringBitmap::new();
        if let Some(slot) = segment.id_lookup.get(id.as_str()).copied()
            && domain.contains(slot.0)
        {
            matches.insert(slot.0);
        }
        return Ok(SegmentCandidateBounds::exact(matches));
    }
    if let Some(matches) = segment_file_size_matches(segment, domain, term, catalog)? {
        return Ok(SegmentCandidateBounds::exact(matches));
    }
    if let Some((operator, literal)) = indexed_file_path_term(term)
        && let Some(catalog) = catalog
    {
        let bounds =
            relative_path_bounds(operator, literal).ok_or("invalid indexed path comparison")?;
        let mut matches = if let Some(files) = catalog
            .path_candidates(bounds, MAX_INDEXED_PATH_SCAN)
            .map_err(|error| error.to_string())?
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
        return Ok(SegmentCandidateBounds::exact(matches));
    }
    if let Some(matches) =
        segment_file_modified_matches(segment, access.collection_name, domain, term, catalog)?
    {
        return Ok(SegmentCandidateBounds::exact(matches));
    }
    if let Some(matches) = segment_file_created_matches(segment, domain, term, catalog)? {
        return Ok(SegmentCandidateBounds::exact(matches));
    }
    if let Some(matches) =
        segment_record_range_matches(segment, access.collection_name, domain, term)?
    {
        return Ok(SegmentCandidateBounds::exact(matches));
    }
    if let Some(matches) =
        segment_metadata_matches(segment, access.collection_name, domain, term, catalog)?
    {
        return Ok(SegmentCandidateBounds::exact(matches));
    }
    if term.path.root != PathRoot::Data {
        return Ok(SegmentCandidateBounds::unknown(domain));
    }
    if let Some(matches) = segment_sort_column_matches(segment, logical_active, term, true)? {
        return Ok(SegmentCandidateBounds::bounds(
            &matches | fallback_active,
            matches,
        ));
    }
    let index_catalog = IndexCatalog::segment_with_bindings(
        segment,
        access.text.policy.schema,
        access.text.policy.bindings,
    );
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
            let work = |proposal: &PostingProposal| {
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
        return Ok(SegmentCandidateBounds::bounds(
            &matches | fallback_active,
            if proposal.exact {
                matches
            } else {
                RoaringBitmap::new()
            },
        ));
    }
    if let Some(matches) = segment_sort_column_matches(segment, logical_active, term, false)? {
        return Ok(SegmentCandidateBounds::bounds(
            &matches | fallback_active,
            matches,
        ));
    }
    if let Some(mut grams) = segment_text_candidates(segment, term, access.text.policy)? {
        if empty_selection_preservation(term) {
            grams |= &segment.empty_array_records;
        }
        Ok(SegmentCandidateBounds::candidate(
            (&grams & domain) | fallback_active,
        ))
    } else {
        Ok(SegmentCandidateBounds::unknown(domain))
    }
}
