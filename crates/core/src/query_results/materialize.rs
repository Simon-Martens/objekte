//! Payload materialization for selected result pages.

use std::collections::BTreeMap;

use roaring::RoaringBitmap;

use super::Payload;
use crate::inventory::ByteRange;
use crate::measurement::WorkCounters;
use crate::query::Query;
use crate::query::options::ReturnMode;
use crate::query_diagnostics::trace::QueryPlanTrace;
use crate::query_execution::streaming::{CancellationToken, ExecutionContext};
use crate::query_results::payload::payload_from_bytes_with_execution;
use crate::query_results::ranking::Winner;
use crate::repository::Repository;
use crate::source::access::SourceAccessSession;

/// Load only payloads needed by the currently visible requested page.
pub(crate) fn materialize_winner_page(
    repo: &Repository,
    query: &Query,
    page: &mut [Winner],
    counters: &mut WorkCounters,
    files_read: &mut Option<RoaringBitmap>,
    mut traces: Option<&mut Vec<QueryPlanTrace>>,
    cancellation: Option<&CancellationToken>,
    stream_limit: Option<usize>,
    execution: Option<&ExecutionContext>,
) -> Result<(), String> {
    if query.options.return_mode != ReturnMode::Locator {
        let mut files: BTreeMap<(usize, u32), Vec<(u32, ByteRange)>> = BTreeMap::new();
        for (index, winner) in page.iter_mut().enumerate() {
            if winner.hit.payload.is_none() {
                let record = &repo.collections[winner.collection_index].generation.records
                    [winner.slot as usize];
                if query.options.return_mode == ReturnMode::Logical
                    && record.class == crate::inventory::RecordClass::RawFallback
                {
                    winner.hit.payload = Some(Payload::LogicalUnavailable {
                        diagnostic_ref: record.diagnostic_ref,
                    });
                    continue;
                }
                files
                    .entry((winner.collection_index, record.file.0))
                    .or_default()
                    .push((index as u32, record.range));
            }
        }
        let mut source_access = SourceAccessSession::new(&repo.catalog);
        if let Some(execution) = execution {
            source_access.configure_execution(execution);
        }
        for ((collection_index, file_id), ranges) in files {
            if cancellation.is_some_and(CancellationToken::is_cancelled) {
                return Err("stream cancelled".into());
            }
            let collection = &repo.collections[collection_index];
            source_access.select_backend(&collection.required_file_reader());
            for batch in ranges.chunks(if stream_limit.is_some() { 1 } else { 128 }) {
                if cancellation.is_some_and(CancellationToken::is_cancelled) {
                    return Err("stream cancelled".into());
                }
                let read = if let Some(limit) = stream_limit {
                    source_access.read_bounded(crate::model::FileId(file_id), batch, limit)?
                } else {
                    source_access.read(crate::model::FileId(file_id), batch)?
                };
                counters.files_opened += read.opened;
                counters.bytes_read += read.bytes_read;
                if let Some(traces) = traces.as_deref_mut() {
                    if let Some(actual) = traces
                        .iter_mut()
                        .find(|trace| trace.collection == collection.definition.name)
                        .and_then(|trace| trace.actual.as_mut())
                    {
                        actual.files_opened += read.opened;
                        actual.bytes_read += read.bytes_read;
                        if query.options.return_mode == ReturnMode::Logical {
                            actual.record_parses += read.records.len() as u64;
                        }
                    }
                }
                for (index, bytes) in read.records {
                    if cancellation.is_some_and(CancellationToken::is_cancelled) {
                        return Err("stream cancelled".into());
                    }
                    let (payload, parses) = payload_from_bytes_with_execution(
                        page[index as usize].hit.record_class,
                        Some(collection),
                        &query.options.return_mode,
                        bytes,
                        execution,
                    )?;
                    counters.record_parses += parses;
                    page[index as usize].hit.payload = payload;
                }
            }
        }
        *files_read
            .as_mut()
            .expect("payload access needs file union") |= source_access.accessed_files();
    }
    Ok(())
}
