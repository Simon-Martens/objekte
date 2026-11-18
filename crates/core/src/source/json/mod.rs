//! JSON-backed source readers.

use crate::inventory::ByteRange;

use super::{
    ExtractedRecord, LogicalTextCoverage, ReaderFailure, ScanProjection, SourceReader,
    TextDerivation,
};

pub(crate) mod document;
pub(crate) mod scalars;

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
mod tests;
