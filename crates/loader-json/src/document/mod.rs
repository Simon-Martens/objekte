//! JSON-backed source readers.

use objekte_search_core::inventory::ByteRange;

use objekte_search_core::source::{
    ExtractedRecord, LogicalTextCoverage, ReaderFailure, ScanProjection, SourceReader,
    TextDerivation,
};

pub(crate) mod document;
pub(crate) mod scalars;
pub(crate) mod value;

use objekte_search_core::source::verification::{
    BorrowedScanResult, ScalarQuery, ScalarText, ScalarVerification,
};

use document::{
    json_reader_failure, parse_json, parse_json_decoded_for_build_diagnosed,
    parse_json_decoded_for_query_diagnosed, parse_json_decoded_for_query_trusted,
    parse_json_for_build_diagnosed, parse_json_for_query, parse_json_for_query_diagnosed,
    parse_json_for_query_trusted,
};

/// Reader for a single standalone JSON record per file.
pub struct JsonReader;

/// Built-in one-document JSON reader with decoded scalar-value FTS text.
pub struct JsonDecodedScalarsReader;

impl SourceReader for JsonReader {
    fn scalar_verification(&self) -> Option<ScalarVerification> {
        Some(ScalarVerification::ResolvedStrings)
    }

    fn scan_scalars(
        &self,
        bytes: &[u8],
        query: &ScalarQuery<'_>,
    ) -> Result<BorrowedScanResult, String> {
        scan_record(bytes, query, ScalarVerification::ResolvedStrings)
    }

    fn supports_relative_path_identity(&self) -> bool {
        true
    }
    fn uses_extension_query_limits(&self) -> bool {
        false
    }
    fn supports_verified_layout(&self) -> bool {
        true
    }
    fn prefers_file_batches(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "json"
    }

    fn supports_trusted_query_extraction(&self) -> bool {
        true
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        LogicalTextCoverage::AllStringFields
    }

    fn text_derivation(&self) -> TextDerivation {
        TextDerivation::LogicalStringsAfterConversion
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        Ok(vec![ByteRange::checked(
            0,
            file.len() as u64,
            file.len() as u64,
        )?])
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        parse_json(bytes)
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_for_query_diagnosed(bytes, &ScanProjection::full(true))
            .map_err(json_reader_failure)
    }

    fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_for_build_diagnosed(bytes).map_err(json_reader_failure)
    }

    fn parse_for_query(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        parse_json_for_query(bytes, projection)
    }

    fn parse_for_query_trusted(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        parse_json_for_query_trusted(bytes, projection)
    }
}

impl SourceReader for JsonDecodedScalarsReader {
    fn project_and_scan_scalars(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
        query: ScalarText<'_>,
        evidence: bool,
    ) -> Result<(objekte_search_core::model::Value, BorrowedScanResult), String> {
        scalars::extract_query_record_borrowed_fts_worker(bytes, projection, query, evidence)
    }

    fn scalar_verification(&self) -> Option<ScalarVerification> {
        Some(ScalarVerification::DecodedScalars)
    }

    fn scan_scalars(
        &self,
        bytes: &[u8],
        query: &ScalarQuery<'_>,
    ) -> Result<BorrowedScanResult, String> {
        scan_record(bytes, query, ScalarVerification::DecodedScalars)
    }

    fn supports_relative_path_identity(&self) -> bool {
        true
    }
    fn uses_extension_query_limits(&self) -> bool {
        false
    }
    fn supports_verified_layout(&self) -> bool {
        true
    }
    fn prefers_file_batches(&self) -> bool {
        true
    }

    fn name(&self) -> &'static str {
        "json_decoded_scalars"
    }

    fn supports_trusted_query_extraction(&self) -> bool {
        true
    }

    fn logical_text_coverage(&self) -> LogicalTextCoverage {
        LogicalTextCoverage::AllStringFields
    }

    fn text_derivation(&self) -> TextDerivation {
        TextDerivation::ReaderSegments
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        JsonReader.frame(file)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        parse_json_decoded_for_query_diagnosed(bytes, &ScanProjection::full(true))
            .map_err(|issue| issue.message)
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_decoded_for_query_diagnosed(bytes, &ScanProjection::full(true))
            .map_err(json_reader_failure)
    }

    fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_decoded_for_build_diagnosed(bytes).map_err(json_reader_failure)
    }

    fn parse_for_query(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        parse_json_decoded_for_query_diagnosed(bytes, projection).map_err(|issue| issue.message)
    }

    fn parse_for_query_trusted(
        &self,
        bytes: &[u8],
        projection: &ScanProjection,
    ) -> Result<ExtractedRecord, String> {
        parse_json_decoded_for_query_trusted(bytes, projection)
    }
}

#[cfg(test)]
crate::objekte_json_unit_tests! { mod tests; }

/// Install both JSON document text representations in a source registry.
pub fn register(
    registry: &mut objekte_search_core::source::registry::SourceRegistry,
) -> Result<(), String> {
    use std::sync::Arc;
    registry.register("json", 1, "json", Arc::new(JsonReader))?;
    registry.register(
        "json_decoded_scalars",
        1,
        "json",
        Arc::new(JsonDecodedScalarsReader),
    )
}

pub fn scan_record(
    bytes: &[u8],
    query: &ScalarQuery<'_>,
    mode: ScalarVerification,
) -> Result<BorrowedScanResult, String> {
    scalars::scan_borrowed_segments_worker(
        bytes,
        &scalars::BorrowedScanQuery {
            fts: query.text,
            filter: query.filter,
            decoded_scalars: mode == ScalarVerification::DecodedScalars,
            retain_evidence: query.retain_evidence,
        },
    )
}
