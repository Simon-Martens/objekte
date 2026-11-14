//! Compatibility surface for frontends that still read repository
//! configuration metadata while routing live queries through index views.

use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use crate::inventory::{DiagnosticRef, SourceDiagnostic};
use crate::lifecycle::{Index, IndexError, IndexView};
use crate::model::{FileId, Value};
use crate::query::Query;
use crate::repository::{Repository, SearchHit, SearchResult, StartupStats};
use crate::streaming::{
    CancellationToken, ExecutionErrorKind, ResultScope, StreamControl, StreamError, StreamEvent,
    StreamOptions, StreamOutcome,
};
use crate::workload::ReturnMode;

/// A live index retaining lightweight repository metadata for frontends.
/// Search and inspection acquire a fresh view unless explicitly pinned for a
/// multi-request desktop operation such as a finite batch export.
#[derive(Clone)]
pub struct IndexedRepository {
    index: Index,
    metadata: Arc<Repository>,
    pinned: Option<IndexView>,
    startup: StartupStats,
}

impl IndexedRepository {
    pub fn new(index: Index) -> Result<Self, IndexError> {
        let view = index.view()?;
        let metadata = Arc::clone(&view.0.repository);
        let startup = metadata.startup().clone();
        Ok(Self {
            index,
            metadata,
            pinned: None,
            startup,
        })
    }

    pub fn index(&self) -> &Index {
        &self.index
    }

    /// Keep all subsequent reads on one immutable publication. This lets
    /// existing CLI-protocol batch/export code execute reproducibly while the
    /// live index continues to publish revisions in another worker.
    pub fn with_pinned_view(&self, view: IndexView) -> Result<Self, String> {
        if view.instance_id() != self.index.instance_id() {
            return Err("index view belongs to another repository instance".into());
        }
        let mut pinned = self.clone();
        pinned.pinned = Some(view);
        Ok(pinned)
    }

    fn read_view(&self) -> Result<IndexView, IndexError> {
        self.pinned.clone().map_or_else(|| self.index.view(), Ok)
    }

    pub fn startup(&self) -> &StartupStats {
        &self.startup
    }

    /// Resolve diagnostics from the current catalog, including files added
    /// after the base repository was built. Return an owned value because the
    /// publication pin is released when this method returns.
    pub fn diagnostic(&self, file: FileId, reference: DiagnosticRef) -> Option<SourceDiagnostic> {
        self.read_view().ok()?.diagnostic(file, reference).cloned()
    }

    pub fn search(&self, query: &Query) -> Result<SearchResult, String> {
        self.read_view()
            .map_err(|error| error.to_string())?
            .search(query)
    }

    /// Bind typed parameters and search the same live or pinned publication
    /// used by the ordinary adapter search path.
    pub fn search_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<SearchResult, String> {
        self.read_view()
            .map_err(|error| error.to_string())?
            .search_with_parameters(query, parameters)
    }

    pub fn get_record(
        &self,
        collection: &str,
        record_id: &str,
        return_mode: ReturnMode,
    ) -> Result<Option<SearchHit>, String> {
        self.read_view()
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
        self.read_view()
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
        self.read_view()
            .map_err(|error| error.to_string())?
            .relation_backlink_page(target_collection, reverse_alias, target_id, offset, limit)
    }

    pub fn collection_graph(&self) -> Result<crate::relation_inspection::CollectionGraph, String> {
        self.read_view()
            .map_err(|error| error.to_string())?
            .collection_graph()
    }

    pub fn posting_values(
        &self,
        request: &crate::posting_values::PostingValuesRequest,
    ) -> Result<crate::posting_values::PostingValuesPage, String> {
        self.read_view()
            .map_err(|error| error.to_string())?
            .posting_values(request)
    }

    fn view_for_stream(&self) -> Result<IndexView, StreamError> {
        self.read_view().map_err(|error| StreamError::Execution {
            kind: ExecutionErrorKind::Execution,
            message: error.to_string(),
            progress: Default::default(),
        })
    }
}

impl Deref for IndexedRepository {
    type Target = Repository;

    fn deref(&self) -> &Self::Target {
        &self.metadata
    }
}

#[cfg(test)]
#[path = "lifecycle_caller.test.rs"]
mod tests;
