//! Complete NDJSON export for the visible desktop query controls.

use std::path::Path;

use objekte_cli::execution::{PreparedQuery, run_prepared_on_repository};
use objekte_search_core::streaming::CancellationToken;
use serde_json::json;

use super::batch::{
    BatchReport, BatchSource, BatchSpec, create_output_file, existing_run, finish_output, run_batch,
};
use super::{BackendError, OpenedSession, QuerySpec, build_query};

/// Export every requested match with the CLI's run/query/record event sequence.
///
/// Exact CLI direct modes are used when the query needs no desktop-only controls.
/// Combined or customized queries carry their controls in a JSON `input` string
/// and use `input_mode` and `kind` values of `combined`.
pub fn export_query_ndjson(
    session: &OpenedSession,
    spec: &QuerySpec,
    output_path: &Path,
    cancellation: &CancellationToken,
) -> Result<BatchReport, BackendError> {
    let filter_empty = spec.filter.trim().is_empty();
    let full_text_empty = spec.full_text.trim().is_empty();
    if spec.scope.is_none() {
        let source = if full_text_empty && !filter_empty {
            Some(BatchSource::DirectStructured(spec.filter.clone()))
        } else if filter_empty
            && !full_text_empty
            && spec.case_sensitive
            && !spec.regex
            && spec.full_text == spec.full_text.trim()
        {
            Some(BatchSource::DirectFullText(spec.full_text.clone()))
        } else {
            None
        };
        if let Some(source) = source {
            return run_batch(
                session,
                &BatchSpec {
                    source,
                    offset: spec.offset,
                    limit: spec.limit,
                    count: spec.count,
                },
                output_path,
                cancellation,
            );
        }
    }
    if cancellation.is_cancelled() {
        return Err(BackendError::cancelled());
    }
    let mut args = session.args.clone();
    args.tui = false;
    args.query = None;
    args.fts = None;
    args.query_file = None;
    args.offset = spec.offset;
    args.limit = spec.limit;
    args.count = spec.count;
    let input = json!({
        "full_text": spec.full_text,
        "filter": spec.filter,
        "scope": spec.scope,
        "case_sensitive": spec.case_sensitive,
        "regex": spec.regex,
    })
    .to_string();
    let prepared = PreparedQuery {
        input,
        query: build_query(spec),
        decoded_fts: (!full_text_empty).then(|| spec.full_text.trim().to_owned()),
    };
    let file = create_output_file(session, output_path)?;
    let exit =
        run_prepared_on_repository(existing_run(session, &args), prepared, file, cancellation);
    finish_output(exit, output_path)
}

#[cfg(test)]
#[path = "ndjson.test.rs"]
mod tests;
