//! Combined full-text and structured requests with bounded page delivery.

use std::path::PathBuf;
use std::time::Instant;

use objekte_search_core::lifecycle::{IndexRevision, IndexView};
use objekte_search_core::query::{
    CaseSensitivity, FilterExpr, FtsMode, FtsQuery, Query, QueryOptions, RegexOptions,
    parse_structured_filter,
};
use objekte_search_core::repository::SearchHit;
use objekte_search_core::streaming::{
    CancellationToken, StreamControl, StreamEvent, StreamMode, StreamOptions, StreamOutcome,
};
use objekte_search_core::workload::{ReturnMode, WorkCounters};

use super::{BackendError, BackendErrorKind, OpenedSession};

pub const MAX_PAGE_ROWS: usize = 100;

/// Search controls and the full requested range, independent of viewport pages.
#[derive(Clone, Debug)]
pub struct QuerySpec {
    pub full_text: String,
    pub filter: String,
    pub scope: Option<String>,
    pub directory: Option<String>,
    pub case_sensitive: bool,
    pub regex: bool,
    pub offset: usize,
    pub limit: Option<usize>,
    pub count: bool,
}

impl Default for QuerySpec {
    fn default() -> Self {
        Self {
            full_text: String::new(),
            filter: String::new(),
            scope: None,
            directory: None,
            case_sensitive: true,
            regex: false,
            offset: 0,
            limit: None,
            count: false,
        }
    }
}

/// One verified viewport page; `count` is exact or absent, never an estimate.
#[derive(Clone, Debug)]
pub struct QueryPage {
    /// Root captured with the result, for metadata projection without source IO.
    pub repository_root: PathBuf,
    /// The immutable publication used for this page and its continuation.
    pub instance_id: u64,
    pub revision: IndexRevision,
    pub hits: Vec<SearchHit>,
    pub page_start: usize,
    pub requested_offset: usize,
    pub next_page_start: Option<usize>,
    pub count: Option<usize>,
    pub counters: WorkCounters,
    pub elapsed_ns: u128,
}

impl QueryPage {
    /// A source range could not be verified, so delivered rows and count may
    /// cover only part of the requested domain.
    pub fn incomplete(&self) -> bool {
        self.counters.source_unavailable_events > 0
    }
}

/// Parse the full accepted structured syntax while keeping FTS independent.
///
/// `limit == None` is represented by zero in this unpaged template. Callers
/// must use `query_page` or a streaming `ResultScope::All` before executing it.
pub fn build_query(spec: &QuerySpec) -> Result<Query, String> {
    let filter = spec.filter.trim();
    let full_text = spec.full_text.trim();
    let parsed = (!filter.is_empty())
        .then(|| parse_structured_filter(filter))
        .transpose()?;
    let mut predicate = parsed.as_ref().and_then(|parsed| parsed.filter.clone());
    if let Some(directory) = &spec.directory {
        let prefix = format!("{directory}/");
        let upper = format!("{directory}0");
        let directory_filter = parse_structured_filter(&format!(
            "@file.path >= {} && @file.path < {}",
            serde_json::to_string(&prefix).map_err(|error| error.to_string())?,
            serde_json::to_string(&upper).map_err(|error| error.to_string())?,
        ))?
        .filter
        .ok_or("directory filter is empty")?;
        predicate = Some(match predicate {
            Some(existing) => FilterExpr::And(vec![existing, directory_filter]),
            None => directory_filter,
        });
    }
    Ok(Query {
        collections: spec.scope.iter().cloned().collect(),
        fts: (!full_text.is_empty()).then(|| FtsQuery {
            case_sensitivity: if spec.case_sensitive {
                CaseSensitivity::Sensitive
            } else {
                CaseSensitivity::Insensitive
            },
            mode: if spec.regex {
                FtsMode::Regex(RegexOptions::default())
            } else {
                FtsMode::Literal
            },
            literal: full_text.to_owned(),
            pattern_parameter: None,
            fields: Vec::new(),
        }),
        filter: predicate,
        options: QueryOptions {
            sort: parsed.and_then(|parsed| parsed.sort).unwrap_or_default(),
            limit: spec.limit.unwrap_or(0),
            offset: spec.offset,
            count: spec.count,
            return_mode: ReturnMode::Bytes,
            evidence: true,
        },
    })
}

/// Return at most 100 rows from the requested range with cooperative cancellation.
pub fn query_page(
    session: &OpenedSession,
    spec: &QuerySpec,
    page_start: usize,
    page_size: usize,
    cancellation: &CancellationToken,
) -> Result<QueryPage, BackendError> {
    let view = session
        .repository
        .index()
        .view()
        .map_err(|error| BackendError::new(BackendErrorKind::Query, error.to_string()))?;
    query_page_on_view(session, &view, spec, page_start, page_size, cancellation)
}

/// Return a page from the caller's pinned publication. Reuse the same view for
/// every continuation, projection, and exact count in one executed context.
pub fn query_page_on_view(
    session: &OpenedSession,
    view: &IndexView,
    spec: &QuerySpec,
    page_start: usize,
    page_size: usize,
    cancellation: &CancellationToken,
) -> Result<QueryPage, BackendError> {
    if !(1..=MAX_PAGE_ROWS).contains(&page_size) {
        return Err(BackendError::new(
            BackendErrorKind::Options,
            format!("page size must be between 1 and {MAX_PAGE_ROWS}"),
        ));
    }
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    if let Some(scope) = &spec.scope
        && !session.collections.contains(scope)
    {
        return Err(BackendError::new(
            BackendErrorKind::Options,
            format!("collection {scope} is not open"),
        ));
    }
    let requested_offset = spec
        .offset
        .checked_add(page_start)
        .ok_or_else(|| BackendError::new(BackendErrorKind::Options, "result offset overflow"))?;
    let requested_rows = spec.limit.map_or(page_size, |limit| {
        page_size.min(limit.saturating_sub(page_start))
    });
    let mut query =
        build_query(spec).map_err(|message| BackendError::new(BackendErrorKind::Query, message))?;
    query.options.offset = requested_offset;
    query.options.limit = requested_rows;
    let started = Instant::now();
    let mut page = Vec::new();
    let outcome = view.search_stream(
        &query,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            ..StreamOptions::default()
        },
        cancellation,
        &mut |event| {
            if let StreamEvent::PageReplacement { rows, .. } = event {
                page = rows.to_vec();
            }
            Ok(StreamControl::Continue)
        },
    );
    match outcome {
        Ok(StreamOutcome::Complete(summary)) => {
            let next = page_start.saturating_add(page.len());
            let count = (summary.counters.match_count_complete
                && summary.counters.source_unavailable_events == 0)
                .then_some(summary.count)
                .flatten();
            let visible_total = count.map(|total| {
                spec.limit
                    .map_or(total.saturating_sub(spec.offset), |limit| {
                        limit.min(total.saturating_sub(spec.offset))
                    })
            });
            let next_page_start = if requested_rows == 0
                || page.len() < requested_rows
                || visible_total.is_some_and(|total| next >= total)
            {
                None
            } else {
                Some(next)
            };
            Ok(QueryPage {
                repository_root: session.repository.files().root.clone(),
                instance_id: view.instance_id(),
                revision: view.revision(),
                hits: page,
                page_start,
                requested_offset,
                next_page_start,
                count,
                counters: summary.counters,
                elapsed_ns: started.elapsed().as_nanos(),
            })
        }
        Ok(StreamOutcome::Cancelled(_)) => Err(BackendError::cancelled()),
        Err(error) => Err(BackendError::new(
            BackendErrorKind::Query,
            error.to_string(),
        )),
    }
}

/// Count all matches without retaining result rows or sort keys.
pub fn count_exact(
    session: &OpenedSession,
    spec: &QuerySpec,
    cancellation: &CancellationToken,
) -> Result<usize, BackendError> {
    let view = session
        .repository
        .index()
        .view()
        .map_err(|error| BackendError::new(BackendErrorKind::Query, error.to_string()))?;
    count_exact_on_view(&view, spec, cancellation)
}

/// Count against the exact publication supplying the displayed result pages.
pub fn count_exact_on_view(
    view: &IndexView,
    spec: &QuerySpec,
    cancellation: &CancellationToken,
) -> Result<usize, BackendError> {
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let mut query =
        build_query(spec).map_err(|message| BackendError::new(BackendErrorKind::Query, message))?;
    query.options.offset = 0;
    query.options.limit = 0;
    query.options.count = true;
    query.options.sort.clear();
    query.options.return_mode = ReturnMode::Locator;
    match view.search_stream(
        &query,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            ..StreamOptions::default()
        },
        cancellation,
        &mut |_event| Ok(StreamControl::Continue),
    ) {
        Ok(StreamOutcome::Complete(summary)) => {
            if !summary.counters.match_count_complete
                || summary.counters.source_unavailable_events > 0
            {
                return Err(BackendError::new(
                    BackendErrorKind::Query,
                    "some source records could not be checked; exact count is unavailable",
                ));
            }
            summary.count.ok_or_else(|| {
                BackendError::new(BackendErrorKind::Query, "search omitted exact count")
            })
        }
        Ok(StreamOutcome::Cancelled(_)) => Err(BackendError::cancelled()),
        Err(error) => Err(BackendError::new(
            BackendErrorKind::Query,
            error.to_string(),
        )),
    }
}

#[cfg(test)]
#[path = "query.test.rs"]
mod tests;
