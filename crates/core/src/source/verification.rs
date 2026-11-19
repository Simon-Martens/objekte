//! Typed dispatch for the crate's borrowed scalar verification operations.

use crate::query::binding::filter::BoundTerm;
use crate::query::binding::fts::BoundFts;
use crate::query_execution::streaming::ExecutionContext;
use crate::schema::Schema;

use super::json::scalars;
use super::{ExtractedRecord, ScanProjection, registry::PreparedSource};

pub(crate) use super::json::scalars::BorrowedScanResult;
#[cfg(test)]
pub(crate) use super::json::scalars::{borrowed_scan_calls, combined_scan_calls};

/// The built-in scalar representation selected by the installed registration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ScalarVerification {
    ResolvedStrings,
    DecodedScalars,
}

/// Query inputs handled by one borrowed scalar scan.
pub(crate) struct BorrowedScanQuery<'a> {
    pub(crate) fts: Option<&'a BoundFts>,
    pub(crate) filter: Option<&'a BoundTerm>,
    pub(crate) retain_evidence: bool,
}

impl ScalarVerification {
    pub(crate) fn scan_borrowed_segments(
        self,
        bytes: &[u8],
        query: &BorrowedScanQuery<'_>,
    ) -> Result<BorrowedScanResult, String> {
        scalars::scan_borrowed_segments_worker(
            bytes,
            &scalars::BorrowedScanQuery {
                fts: query.fts,
                filter: query.filter,
                decoded_scalars: self == Self::DecodedScalars,
                retain_evidence: query.retain_evidence,
            },
        )
    }

    pub(crate) fn extract_decoded_record(
        self,
        bytes: &[u8],
        projection: &ScanProjection,
        fts: &BoundFts,
        retain_evidence: bool,
    ) -> Result<(crate::model::Value, BorrowedScanResult), String> {
        if self != Self::DecodedScalars {
            return Err("combined decoded-scalar verification requires decoded scalars".into());
        }
        scalars::extract_query_record_borrowed_fts_worker(bytes, projection, fts, retain_evidence)
    }
}

/// Trusted decoded-scalar JSON keeps its source corpus while producing only
/// the logical projection required by residual evaluation or output.
pub(crate) fn prepare_decoded_record_with_borrowed_fts(
    source: &PreparedSource,
    schema: &Schema,
    bytes: &[u8],
    projection: &ScanProjection,
    fts: &BoundFts,
    evidence: bool,
    execution: Option<&ExecutionContext>,
) -> Result<(ExtractedRecord, BorrowedScanResult), String> {
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    let verification = source
        .scalar_verification()
        .ok_or("combined decoded-scalar verification requires a built-in reader")?;
    let (mut value, matched) =
        verification.extract_decoded_record(bytes, projection, fts, evidence)?;
    schema.convert_indexed_record(&mut value, projection.top_level.as_ref())?;
    if let Some(execution) = execution {
        execution.check_string()?;
    }
    Ok((
        ExtractedRecord {
            value,
            text: Vec::new(),
        },
        matched,
    ))
}
