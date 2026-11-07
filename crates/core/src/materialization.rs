//! Conversion of source bytes into a requested page payload.

use crate::record_preparation::{materialize, prepare_query_record};
use crate::repository::{CollectionState, Payload};
use crate::source::ScanProjection;
use crate::workload::ReturnMode;

/// Returns the payload and the number of logical record parses to account for.
/// Source access remains with each caller so grouped and bounded reads keep
/// their distinct access patterns.
pub(crate) fn payload_from_bytes(
    collection: &CollectionState,
    return_mode: &ReturnMode,
    bytes: Vec<u8>,
) -> Result<(Option<Payload>, u64), String> {
    let logical = if *return_mode == ReturnMode::Logical {
        Some(prepare_query_record(
            &collection.source,
            &collection.definition.schema,
            &bytes,
            &ScanProjection::full(false),
            true,
        )?)
    } else {
        None
    };
    let parsed = u64::from(logical.is_some());
    Ok((materialize(return_mode, bytes, logical)?, parsed))
}
