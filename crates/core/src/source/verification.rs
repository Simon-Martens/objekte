//! Typed dispatch for the crate's borrowed scalar verification operations.

use crate::query::binding::filter::BoundTerm;
use crate::query::binding::fts::BoundFts;
use crate::query_execution::streaming::ExecutionContext;
use crate::schema::Schema;

use super::{ExtractedRecord, ScanProjection, registry::PreparedSource};

#[cfg(test)]
pub(crate) use super::test_support::{borrowed_scan_calls, combined_scan_calls};

/// Scope temporary matching scratch to this source record.
pub fn scalar_match_session() -> impl Drop {
    crate::query::semantics::folding::FoldRecordGuard::new()
}

/// The source text representation a backend can scan without allocating
/// an owned record. Core selects the matching semantics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScalarVerification {
    ResolvedStrings,
    DecodedScalars,
}

/// A source-local text match request. Query binding and predicate execution
/// stay inside core.
#[derive(Clone, Copy)]
pub struct ScalarText<'a>(&'a BoundFts);

impl ScalarText<'_> {
    pub fn is_unscoped(&self) -> bool {
        self.0.fields.is_empty()
    }

    pub fn paths(&self) -> impl Iterator<Item = &[String]> {
        self.0
            .fields
            .iter()
            .map(|field| field.path.segments.as_slice())
    }

    pub fn selects_path<T: AsRef<str>>(&self, path: &[T]) -> bool {
        self.is_unscoped()
            || self
                .paths()
                .any(|selected| crate::reference::fts::fts_field_selects_path(selected, path))
    }

    pub fn matches<T: AsRef<str>>(
        &self,
        path: &[T],
        text: &str,
        evidence: &mut Vec<String>,
        retain_evidence: bool,
    ) -> Result<bool, String> {
        crate::reference::fts::match_borrowed_source_segment(
            self.0,
            path,
            text,
            evidence,
            retain_evidence,
        )
    }

    pub fn matches_with_filter<T: AsRef<str>>(
        &self,
        path: &[T],
        text: &str,
        filter: ScalarFilter<'_>,
        evidence: &mut Vec<String>,
        retain_evidence: bool,
    ) -> Result<(bool, bool), String> {
        crate::reference::fts::match_borrowed_source_pair(
            self.0,
            path,
            text,
            filter.0,
            evidence,
            retain_evidence,
        )
    }
}

impl<'a> From<&'a BoundFts> for ScalarText<'a> {
    fn from(binding: &'a BoundFts) -> Self {
        Self(binding)
    }
}

/// A single eligible string predicate for projected source scanning.
#[derive(Clone, Copy)]
pub struct ScalarFilter<'a>(&'a BoundTerm);

impl ScalarFilter<'_> {
    pub fn path(&self) -> &[String] {
        &self.0.path.segments
    }
    pub fn is_multi(&self) -> bool {
        self.0.multi
    }
    pub fn matches_any(&self) -> bool {
        self.0.quantifier == crate::query::Quantifier::Any
    }
    pub fn matches(&self, value: &str) -> bool {
        crate::reference::fts::match_borrowed_source_filter(self.0, value)
    }
}

impl<'a> From<&'a BoundTerm> for ScalarFilter<'a> {
    fn from(binding: &'a BoundTerm) -> Self {
        Self(binding)
    }
}

/// Inputs and outcomes at the optional borrowed-scan boundary.
#[derive(Clone, Copy)]
pub struct ScalarQuery<'a> {
    pub text: Option<ScalarText<'a>>,
    pub filter: Option<ScalarFilter<'a>>,
    pub retain_evidence: bool,
}

pub struct BorrowedScanResult {
    pub fts_match: bool,
    pub filter_match: bool,
    pub evidence: std::collections::HashSet<String>,
}

/// Query inputs handled by one borrowed scalar scan.
pub(crate) struct BorrowedScanQuery<'a> {
    pub(crate) fts: Option<&'a BoundFts>,
    pub(crate) filter: Option<&'a BoundTerm>,
    pub(crate) retain_evidence: bool,
}

pub(crate) fn scan_borrowed_segments(
    source: &PreparedSource,
    bytes: &[u8],
    query: &BorrowedScanQuery<'_>,
) -> Result<BorrowedScanResult, String> {
    source.reader().scan_scalars(
        bytes,
        &ScalarQuery {
            text: query.fts.map(Into::into),
            filter: query.filter.map(Into::into),
            retain_evidence: query.retain_evidence,
        },
    )
}

/// A reader with combined scalar scanning can match its text corpus while producing only
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
    let (mut value, matched) =
        source
            .reader()
            .project_and_scan_scalars(bytes, projection, fts.into(), evidence)?;
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
