//! Combined full-text and structured requests with bounded page delivery.

use std::time::Instant;

use objekte_search_core::query::{
    CaseSensitivity, FtsMode, FtsQuery, Query, QueryOptions, RegexOptions, parse_structured_filter,
};
use objekte_search_core::repository::SearchHit;
use objekte_search_core::streaming::{
    CancellationToken, StreamControl, StreamEvent, StreamMode, StreamOptions, StreamOutcome,
};
use objekte_search_core::workload::{ReturnMode, SortField, WorkCounters};

use super::{BackendError, BackendErrorKind, OpenedSession};

pub const MAX_PAGE_ROWS: usize = 100;


/// Search controls and the full requested range, independent of viewport pages.
#[derive(Clone, Debug)]
pub struct QuerySpec {
    pub full_text: String,
    pub filter: String,
    pub scope: Option<String>,
    pub case_sensitive: bool,
    pub regex: bool,
    pub sort: Vec<SortField>,
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
            case_sensitive: true,
            regex: false,
            sort: Vec::new(),
            offset: 0,
            limit: None,
            count: false,
        }
    }
}


/// One verified viewport page; `count` is exact or absent, never an estimate.
#[derive(Clone, Debug)]
pub struct QueryPage {
    pub hits: Vec<SearchHit>,
    pub page_start: usize,
    pub requested_offset: usize,
    pub next_page_start: Option<usize>,
    pub count: Option<usize>,
    pub counters: WorkCounters,
    pub elapsed_ns: u128,
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
    if parsed.as_ref().is_some_and(|parsed| parsed.sort.is_some()) && !spec.sort.is_empty() {
        return Err("inline SORT BY conflicts with independent sort options".into());
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
        filter: parsed.as_ref().and_then(|parsed| parsed.filter.clone()),
        options: QueryOptions {
            sort: parsed
                .and_then(|parsed| parsed.sort)
                .unwrap_or_else(|| spec.sort.clone()),
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
    let outcome = session.repository.search_stream(
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
            let visible_total = summary.count.map(|total| {
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
                hits: page,
                page_start,
                requested_offset,
                next_page_start,
                count: summary.count,
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
    match session.repository.search_stream(
        &query,
        StreamOptions {
            mode: StreamMode::PageReplacements,
            ..StreamOptions::default()
        },
        cancellation,
        &mut |_event| Ok(StreamControl::Continue),
    ) {
        Ok(StreamOutcome::Complete(summary)) => summary.count.ok_or_else(|| {
            BackendError::new(BackendErrorKind::Query, "search omitted exact count")
        }),
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
