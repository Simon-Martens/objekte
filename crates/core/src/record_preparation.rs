//! Shared parse, conversion, extracted-text and payload boundary.

use crate::reference::Schema;
use crate::repository::Payload;
use crate::source::{ExtractedRecord, PreparedSource, ScanProjection, finalize_extracted_text};
use crate::workload::ReturnMode;

pub(crate) fn finish_build_record(
    schema: &Schema,
    record: &mut ExtractedRecord,
) -> Result<(), String> {
    schema.coerce_record(&mut record.value)?;
    Ok(())
}

/// Decode at most once for verification and the selected logical payload.
/// Query projections retain the build-validated logical contract; a winner
/// requesting logical data uses the complete projected record instead.
pub(crate) fn prepare_query_record(
    source: &PreparedSource,
    schema: &Schema,
    bytes: &[u8],
    projection: &ScanProjection,
    logical_output: bool,
) -> Result<ExtractedRecord, String> {
    let mut record = source.reader().parse_for_query(bytes, projection)?;
    if logical_output {
        schema.coerce_projected_record(&mut record.value, projection.top_level.as_ref())?;
    } else {
        schema.coerce_query_record(&mut record.value, projection.top_level.as_ref())?;
    }
    if projection.collect_text {
        finalize_extracted_text(source.reader(), &mut record, schema.has_temporal_fields());
    }
    Ok(record)
}

pub(crate) fn materialize(
    mode: &ReturnMode,
    bytes: Vec<u8>,
    logical: Option<ExtractedRecord>,
) -> Result<Option<Payload>, String> {
    Ok(match mode {
        ReturnMode::Locator => None,
        ReturnMode::Bytes => Some(Payload::Bytes(bytes)),
        ReturnMode::Logical => Some(Payload::Logical(
            logical.ok_or("logical payload was not prepared")?.value,
        )),
    })
}
