//! Whole-file JSON Lines source framing and parsing.

use crate::inventory::ByteRange;

use super::json::document::{
    json_reader_failure, parse_json, parse_json_for_query, parse_json_for_query_diagnosed,
    parse_json_for_query_trusted,
};
use super::{
    ExtractedRecord, LogicalTextCoverage, ReaderFailure, ScanProjection, SourceReader,
    TextDerivation,
};

/// Reader for independently parseable JSON Lines records.
pub struct JsonLinesReader;

impl SourceReader for JsonLinesReader {
    fn name(&self) -> &'static str {
        "jsonl"
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

    fn supports_bounded_json_lines_build(&self) -> bool {
        true
    }

    fn frame(&self, file: &[u8]) -> Result<Vec<ByteRange>, String> {
        let mut result = Vec::new();
        let mut start = 0usize;
        for (i, byte) in file.iter().enumerate() {
            if *byte == b'\n' {
                if !file[start..i].iter().all(u8::is_ascii_whitespace) {
                    result.push(ByteRange::checked(
                        start as u64,
                        i as u64,
                        file.len() as u64,
                    )?);
                }
                start = i + 1;
            }
        }
        if start < file.len() && !file[start..].iter().all(u8::is_ascii_whitespace) {
            result.push(ByteRange::checked(
                start as u64,
                file.len() as u64,
                file.len() as u64,
            )?);
        }
        Ok(result)
    }

    fn parse(&self, bytes: &[u8]) -> Result<ExtractedRecord, String> {
        parse_json(bytes)
    }

    fn parse_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_for_query_diagnosed(bytes, &ScanProjection::full(true))
            .map_err(json_reader_failure)
    }

    fn parse_for_build_outcome(&self, bytes: &[u8]) -> Result<ExtractedRecord, ReaderFailure> {
        parse_json_for_query_diagnosed(bytes, &ScanProjection::full(false))
            .map_err(json_reader_failure)
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
