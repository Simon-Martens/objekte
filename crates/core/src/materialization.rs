//! Conversion of source bytes into a requested page payload.

use crate::execution_options::ReturnMode;
use crate::record_preparation::{materialize, prepare_query_record_with_execution};
use crate::repository::{CollectionState, Payload};
use crate::source::ScanProjection;
use crate::streaming::ExecutionContext;

/// Returns the payload and the number of logical record parses to account for.
/// Source access remains with each caller so grouped and bounded reads keep
/// their distinct access patterns. The collection is only needed for logical
/// payloads; bytes results avoid an otherwise unnecessary collection lookup.
pub(crate) fn payload_from_bytes(
    collection: Option<&CollectionState>,
    return_mode: &ReturnMode,
    bytes: Vec<u8>,
) -> Result<(Option<Payload>, u64), String> {
    payload_from_bytes_with_execution(collection, return_mode, bytes, None)
}

pub(crate) fn payload_from_bytes_with_execution(
    collection: Option<&CollectionState>,
    return_mode: &ReturnMode,
    bytes: Vec<u8>,
    execution: Option<&ExecutionContext>,
) -> Result<(Option<Payload>, u64), String> {
    let logical = if *return_mode == ReturnMode::Logical {
        let collection = collection.ok_or("result collection missing")?;
        Some(prepare_query_record_with_execution(
            &collection.source,
            &collection.definition.schema,
            &bytes,
            &ScanProjection::full(false),
            true,
            execution,
        )?)
    } else {
        None
    };
    let parsed = u64::from(logical.is_some());
    Ok((materialize(return_mode, bytes, logical)?, parsed))
}
