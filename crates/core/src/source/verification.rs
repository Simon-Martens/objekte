//! Typed dispatch for the crate's borrowed scalar verification operations.

use crate::query::binding::filter::BoundTerm;
use crate::query::binding::fts::BoundFts;

use super::ScanProjection;
use super::json::scalars;

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
