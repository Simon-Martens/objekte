//! Bounded record and resident relation access for the detail panel.

use objekte_search_core::relation_inspection::{
    BacklinkPage, CollectionGraph, OutgoingReferencePage,
};
use objekte_search_core::repository::SearchHit;
use objekte_search_core::streaming::CancellationToken;
use objekte_search_core::workload::ReturnMode;

use super::{BackendError, BackendErrorKind, OpenedSession};

/// Load original source bytes for one stable collection-local record identity.
pub fn record_by_id(
    session: &OpenedSession,
    collection: &str,
    id: &str,
    cancellation: &CancellationToken,
) -> Result<Option<SearchHit>, BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let hit = session
        .repository
        .get_record(collection, id, ReturnMode::Bytes)
        .map_err(|message| BackendError::new(BackendErrorKind::Query, message))?;
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    Ok(hit)
}

/// Inspect declared relations without reading all source documents.
pub fn collection_graph(
    session: &OpenedSession,
    cancellation: &CancellationToken,
) -> Result<CollectionGraph, BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    session
        .repository
        .collection_graph()
        .map_err(|message| BackendError::new(BackendErrorKind::Query, message))
}

/// Page source relation occurrences, including unresolved and duplicate targets.
pub fn outgoing_page(
    session: &OpenedSession,
    source_collection: &str,
    path: &[String],
    source_id: &str,
    offset: usize,
    limit: usize,
    cancellation: &CancellationToken,
) -> Result<OutgoingReferencePage, BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    session
        .repository
        .relation_outgoing_page(source_collection, path, source_id, offset, limit)
        .map_err(|message| BackendError::new(BackendErrorKind::Query, message))
}

/// Page backlink sources from the resident reverse index.
pub fn backlink_page(
    session: &OpenedSession,
    target_collection: &str,
    reverse_alias: &str,
    target_id: &str,
    offset: usize,
    limit: usize,
    cancellation: &CancellationToken,
) -> Result<BacklinkPage, BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    session
        .repository
        .relation_backlink_page(target_collection, reverse_alias, target_id, offset, limit)
        .map_err(|message| BackendError::new(BackendErrorKind::Query, message))
}
