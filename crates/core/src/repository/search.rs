//! Optimized, reference, explain and streaming repository search entry points.

use std::collections::HashMap;

use crate::inventory::ActiveRecords;
use crate::model::Value;
use crate::query::Query;
use crate::query_results::{SearchHit, SearchResult};

use super::Repository;

impl Repository {
    pub fn search(&self, query: &Query) -> Result<SearchResult, String> {
        let resolved = query.resolve_current_clock()?;
        let result = if self.has_native_segments() {
            let snapshot = self.native_snapshot();
            let cache = crate::query_execution::runner::LifecycleCacheScope {
                store: &self.verified,
                instance_id: self.instance_id,
            };
            crate::query_execution::runner::search_segmented_lifecycle(
                self,
                resolved.as_ref(),
                snapshot.active,
                snapshot.segments,
                snapshot.catalog,
                snapshot.revision,
                Some(cache),
            )?
        } else {
            crate::query_execution::runner::search(self, resolved.as_ref())?
        };
        Ok(self.stamp_native_query_result(query, result))
    }

    /// Count exact matches without constructing a result page or sort keys.
    /// Query parameters must already be bound; unresolved clock macros use
    /// the current clock, as in `search`.
    pub fn count_exact(&self, query: &Query) -> Result<usize, String> {
        let mut request = query.clone();
        request.options.count = true;
        request.options.limit = 0;
        request.options.offset = 0;
        self.search(&request)?
            .count
            .ok_or("count result missing".into())
    }

    /// Count with the same frozen clock, cancellation, and limits as
    /// `search_with_context`. The context belongs to this one invocation.
    pub fn count_exact_with_context(
        &self,
        query: &Query,
        context: &crate::streaming::ExecutionContext,
    ) -> Result<usize, crate::streaming::ExecutionError> {
        let mut request = query.clone();
        request.options.count = true;
        request.options.limit = 0;
        request.options.offset = 0;
        self.search_with_context(&request, context)?
            .count
            .ok_or_else(|| crate::streaming::ExecutionError {
                kind: crate::streaming::ExecutionErrorKind::Execution,
                message: "count result missing".into(),
            })
    }

    /// Search using one frozen clock and shared cancellation/resource limits.
    pub fn search_with_context(
        &self,
        query: &Query,
        context: &crate::streaming::ExecutionContext,
    ) -> Result<SearchResult, crate::streaming::ExecutionError> {
        context.enter()?;
        let resolved = query.with_clock(context.clock.now).map_err(|message| {
            context.error(crate::streaming::ExecutionErrorKind::InvalidQuery, message)
        })?;
        let result = if self.has_native_segments() {
            let cache = crate::query_execution::runner::LifecycleCacheScope {
                store: &self.verified,
                instance_id: self.instance_id,
            };
            crate::query_execution::runner::search_segmented_with_context_lifecycle(
                self,
                &resolved,
                &self.active,
                &self.segments,
                &self.catalog,
                self.revision,
                context,
                Some(cache),
            )?
        } else {
            crate::query_execution::runner::search_with_context(self, &resolved, context)?
        };
        Ok(self.stamp_native_result(result))
    }

    /// Deliver verified requested-page updates while the query is executing.
    pub fn search_stream<F>(
        &self,
        query: &Query,
        options: crate::streaming::StreamOptions,
        cancellation: &crate::streaming::CancellationToken,
        sink: &mut F,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        self.search_stream_scoped(
            query,
            crate::streaming::ResultScope::Page {
                offset: query.options.offset,
                limit: query.options.limit,
            },
            options,
            cancellation,
            sink,
        )
    }

    /// Stream a finite page or every match from one frozen execution.
    pub fn search_stream_scoped<F>(
        &self,
        query: &Query,
        scope: crate::streaming::ResultScope,
        options: crate::streaming::StreamOptions,
        cancellation: &crate::streaming::CancellationToken,
        sink: &mut F,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        let context = crate::streaming::ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            crate::streaming::ExecutionLimits::default(),
            Some(cancellation.clone()),
        );
        self.search_stream_scoped_with_context(query, scope, options, &context, sink)
    }

    /// Stream with a caller-supplied frozen clock and shared request limits.
    pub fn search_stream_scoped_with_context<F>(
        &self,
        query: &Query,
        scope: crate::streaming::ResultScope,
        options: crate::streaming::StreamOptions,
        context: &crate::streaming::ExecutionContext,
        sink: &mut F,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        self.search_stream_scoped_with_context_active(query, scope, options, context, sink, None)
    }

    pub(crate) fn search_stream_scoped_with_context_active<F>(
        &self,
        query: &Query,
        scope: crate::streaming::ResultScope,
        options: crate::streaming::StreamOptions,
        context: &crate::streaming::ExecutionContext,
        sink: &mut F,
        active: Option<&[ActiveRecords]>,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        let cancellation = context.cancellation().cloned().unwrap_or_default();
        if active.is_none() && self.has_native_segments() {
            let cache = crate::query_execution::runner::LifecycleCacheScope {
                store: &self.verified,
                instance_id: self.instance_id,
            };
            let needs_remap = |hit: &SearchHit| {
                hit.diagnostic_ref.stored_index().is_some()
                    || self
                        .collections
                        .iter()
                        .find(|collection| collection.definition.name == hit.collection)
                        .is_none_or(|collection| collection.generation.generation != hit.generation)
            };
            let mut remap_rows = |event: crate::streaming::StreamEvent<'_>| -> Result<
                crate::streaming::StreamControl,
                String,
            > {
                match event {
                    crate::streaming::StreamEvent::OrderedRows {
                        sequence,
                        start,
                        rows,
                        progress,
                    } if rows.iter().any(needs_remap) => {
                        let mut remapped = rows.to_vec();
                        for hit in &mut remapped {
                            self.stamp_native_hit(hit);
                        }
                        sink(crate::streaming::StreamEvent::OrderedRows {
                            sequence,
                            start,
                            rows: &remapped,
                            progress,
                        })
                    }
                    crate::streaming::StreamEvent::PageReplacement {
                        sequence,
                        rows,
                        progress,
                    } if rows.iter().any(needs_remap) => {
                        let mut remapped = rows.to_vec();
                        for hit in &mut remapped {
                            self.stamp_native_hit(hit);
                        }
                        sink(crate::streaming::StreamEvent::PageReplacement {
                            sequence,
                            rows: &remapped,
                            progress,
                        })
                    }
                    event => sink(event),
                }
            };
            return crate::query_execution::runner::search_stream_segmented_with_context_lifecycle(
                self,
                query,
                scope,
                options,
                context,
                &self.active,
                &self.segments,
                &self.catalog,
                self.revision,
                &mut remap_rows,
                Some(cache),
            );
        }
        if let Err(error) = context.enter() {
            if error.kind == crate::streaming::ExecutionErrorKind::Cancelled {
                return Ok(crate::streaming::StreamOutcome::Cancelled(
                    Default::default(),
                ));
            }
            return Err(crate::streaming::StreamError::Execution {
                kind: error.kind,
                message: error.message,
                progress: Default::default(),
            });
        }
        let resolved = query.with_clock(context.clock.now).map_err(|message| {
            crate::streaming::StreamError::Execution {
                kind: crate::streaming::ExecutionErrorKind::InvalidQuery,
                message,
                progress: Default::default(),
            }
        })?;
        let mut request = resolved;
        request.options.offset = scope.offset();
        match scope {
            crate::streaming::ResultScope::Page { limit, .. } => request.options.limit = limit,
            crate::streaming::ResultScope::All { .. } => {
                // Per-record residual verification needs one retained match.
                // The scope, not this local page size, controls delivery.
                request.options.limit = 1;
                request.options.count = true;
            }
        }
        let needs_remap = |hit: &SearchHit| {
            hit.diagnostic_ref.stored_index().is_some()
                || self
                    .collections
                    .iter()
                    .find(|collection| collection.definition.name == hit.collection)
                    .is_none_or(|collection| collection.generation.generation != hit.generation)
        };
        let mut remap_rows = |event: crate::streaming::StreamEvent<'_>| -> Result<
            crate::streaming::StreamControl,
            String,
        > {
            match event {
                crate::streaming::StreamEvent::OrderedRows {
                    sequence,
                    start,
                    rows,
                    progress,
                } if rows.iter().any(needs_remap) => {
                    let mut remapped = rows.to_vec();
                    for hit in &mut remapped {
                        self.stamp_native_hit(hit);
                    }
                    sink(crate::streaming::StreamEvent::OrderedRows {
                        sequence,
                        start,
                        rows: &remapped,
                        progress,
                    })
                }
                crate::streaming::StreamEvent::PageReplacement {
                    sequence,
                    rows,
                    progress,
                } if rows.iter().any(needs_remap) => {
                    let mut remapped = rows.to_vec();
                    for hit in &mut remapped {
                        self.stamp_native_hit(hit);
                    }
                    sink(crate::streaming::StreamEvent::PageReplacement {
                        sequence,
                        rows: &remapped,
                        progress,
                    })
                }
                event => sink(event),
            }
        };
        crate::query_execution::runner::search_stream_scoped_with_context_active(
            self,
            &request,
            scope,
            options,
            &cancellation,
            context,
            &mut remap_rows,
            active,
        )
    }

    /// Bind typed parameters once, then stream the resulting frozen request.
    pub fn search_stream_with_parameters<F>(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
        options: crate::streaming::StreamOptions,
        cancellation: &crate::streaming::CancellationToken,
        sink: &mut F,
    ) -> Result<crate::streaming::StreamOutcome, crate::streaming::StreamError>
    where
        F: for<'e> FnMut(
            crate::streaming::StreamEvent<'e>,
        ) -> Result<crate::streaming::StreamControl, String>,
    {
        if cancellation.is_cancelled() {
            return Ok(crate::streaming::StreamOutcome::Cancelled(
                Default::default(),
            ));
        }
        let bound = query.with_parameters(parameters).map_err(|message| {
            crate::streaming::StreamError::Execution {
                kind: crate::streaming::ExecutionErrorKind::InvalidQuery,
                message,
                progress: Default::default(),
            }
        })?;
        self.search_stream(&bound, options, cancellation, sink)
    }

    /// Execute a parsed query with immutable, typed parameter values.
    pub fn search_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<SearchResult, String> {
        self.search(&query.with_parameters(parameters)?)
    }

    pub fn explain(
        &self,
        query: &Query,
    ) -> Result<Vec<crate::query_diagnostics::trace::QueryPlanTrace>, String> {
        let resolved = query.resolve_current_clock()?;
        if self.has_native_segments() {
            crate::query_execution::runner::explain_segmented(
                self,
                resolved.as_ref(),
                &self.active,
                &self.segments,
                self.revision,
            )
        } else {
            crate::query_execution::runner::explain(self, resolved.as_ref())
        }
    }

    /// Explain one execution-bound parameter choice.
    pub fn explain_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<Vec<crate::query_diagnostics::trace::QueryPlanTrace>, String> {
        self.explain(&query.with_parameters(parameters)?)
    }

    pub fn explain_analyze(
        &self,
        query: &Query,
    ) -> Result<
        (
            SearchResult,
            Vec<crate::query_diagnostics::trace::QueryPlanTrace>,
        ),
        String,
    > {
        let resolved = query.resolve_current_clock()?;
        let result = if self.has_native_segments() {
            crate::query_execution::runner::search_analyze_segmented(
                self,
                resolved.as_ref(),
                &self.active,
                &self.segments,
                &self.catalog,
                self.revision,
            )?
        } else {
            crate::query_execution::runner::search_analyze(self, resolved.as_ref())?
        };
        Ok((self.stamp_native_result(result.0), result.1))
    }

    pub fn search_reference(&self, query: &Query) -> Result<SearchResult, String> {
        self.search_reference_with_active(query, None)
    }

    pub(crate) fn search_reference_with_active(
        &self,
        query: &Query,
        active: Option<&[ActiveRecords]>,
    ) -> Result<SearchResult, String> {
        let resolved = query.resolve_current_clock()?;
        let query = resolved.as_ref();
        let domains = active.unwrap_or(&self.active);
        let result = crate::reference::relations::search_segmented(
            self,
            query,
            domains,
            &self.segments,
            &self.catalog,
            self.revision,
        )?;
        Ok(self.stamp_native_result(result))
    }

    /// Evaluate a parameter choice through the independent reference route.
    pub fn search_reference_with_parameters(
        &self,
        query: &Query,
        parameters: &HashMap<String, Value>,
    ) -> Result<SearchResult, String> {
        let bound = query.with_parameters(parameters)?;
        self.search_reference(&bound)
    }
}
