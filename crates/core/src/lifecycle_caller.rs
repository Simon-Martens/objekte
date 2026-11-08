//! Compatibility surface for frontends that still read base repository
//! metadata while routing live queries through pinned index publications.

use std::ops::Deref;

use crate::inventory::{DiagnosticRef, SourceDiagnostic};
use crate::lifecycle::{Index, IndexError, IndexView};
use crate::model::FileId;
use crate::query::Query;
use crate::repository::{Repository, SearchHit, SearchResult, SnapshotAction, StartupStats};
use crate::streaming::{
    CancellationToken, ExecutionErrorKind, ResultScope, StreamControl, StreamError, StreamEvent,
    StreamOptions, StreamOutcome,
};
use crate::workload::ReturnMode;

/// A live index with a pinned base repository for existing frontend metadata.
/// Search and inspection methods always acquire a fresh published view.
#[derive(Clone)]
pub struct IndexedRepository {
    index: Index,
    base: IndexView,
    startup: StartupStats,
}

impl IndexedRepository {
    pub fn new(index: Index) -> Result<Self, IndexError> {
        let base = index.view()?;
        let mut startup = base.repository().startup().clone();
        if index.status()?.saved_revision.is_some() {
            startup.restored = true;
            startup.snapshot_action = SnapshotAction::Restored;
        }
        Ok(Self {
            index,
            base,
            startup,
        })
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    pub fn startup(&self) -> &StartupStats {
        &self.startup
    }

    /// Resolve diagnostics from the current catalog, including files added
    /// after the base repository was built. Return an owned value because the
    /// publication pin is released when this method returns.
    pub fn diagnostic(&self, file: FileId, reference: DiagnosticRef) -> Option<SourceDiagnostic> {
        self.index.view().ok()?.diagnostic(file, reference).cloned()
    }

    pub fn search(&self, query: &Query) -> Result<SearchResult, String> {
        self.index
            .view()
            .map_err(|error| error.to_string())?
            .search(query)
    }

    pub fn get_record(
        &self,
        collection: &str,
        record_id: &str,
        return_mode: ReturnMode,
    ) -> Result<Option<SearchHit>, String> {
        self.index
            .view()
            .map_err(|error| error.to_string())?
            .get_record(collection, record_id, return_mode)
    }

    pub fn search_stream<F>(
        &self,
        query: &Query,
        options: StreamOptions,
        cancellation: &CancellationToken,
        sink: &mut F,
    ) -> Result<StreamOutcome, StreamError>
    where
        F: for<'event> FnMut(StreamEvent<'event>) -> Result<StreamControl, String>,
    {
        self.view_for_stream()?
            .search_stream(query, options, cancellation, sink)
    }

    pub fn search_stream_scoped<F>(
        &self,
        query: &Query,
        scope: ResultScope,
        options: StreamOptions,
        cancellation: &CancellationToken,
        sink: &mut F,
    ) -> Result<StreamOutcome, StreamError>
    where
        F: for<'event> FnMut(StreamEvent<'event>) -> Result<StreamControl, String>,
    {
        self.view_for_stream()?
            .search_stream_scoped(query, scope, options, cancellation, sink)
    }

    pub fn relation_outgoing_page(
        &self,
        source_collection: &str,
        relation_path: &[String],
        source_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::relation_inspection::OutgoingReferencePage, String> {
        self.index
            .view()
            .map_err(|error| error.to_string())?
            .relation_outgoing_page(source_collection, relation_path, source_id, offset, limit)
    }

    pub fn relation_backlink_page(
        &self,
        target_collection: &str,
        reverse_alias: &str,
        target_id: &str,
        offset: usize,
        limit: usize,
    ) -> Result<crate::relation_inspection::BacklinkPage, String> {
        self.index
            .view()
            .map_err(|error| error.to_string())?
            .relation_backlink_page(target_collection, reverse_alias, target_id, offset, limit)
    }

    pub fn collection_graph(&self) -> Result<crate::relation_inspection::CollectionGraph, String> {
        self.index
            .view()
            .map_err(|error| error.to_string())?
            .collection_graph()
    }

    pub fn posting_values(
        &self,
        request: &crate::posting_values::PostingValuesRequest,
    ) -> Result<crate::posting_values::PostingValuesPage, String> {
        self.index
            .view()
            .map_err(|error| error.to_string())?
            .posting_values(request)
    }

    fn view_for_stream(&self) -> Result<IndexView, StreamError> {
        self.index.view().map_err(|error| StreamError::Execution {
            kind: ExecutionErrorKind::Execution,
            message: error.to_string(),
            progress: Default::default(),
        })
    }
}

impl Deref for IndexedRepository {
    type Target = Repository;

    fn deref(&self) -> &Self::Target {
        self.base.repository()
    }
}

#[cfg(test)]
#[path = "lifecycle_caller.test.rs"]
mod tests;
