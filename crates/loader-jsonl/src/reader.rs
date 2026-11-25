//! Whole-file JSON Lines source framing and parsing.

use objekte_search_core::inventory::ByteRange;

use objekte_loader_json::records::{
    json_reader_failure, parse_json, parse_json_for_query, parse_json_for_query_diagnosed,
    parse_json_for_query_trusted,
};
use objekte_search_core::source::{
    ExtractedRecord, LogicalTextCoverage, ReaderFailure, ScanProjection, SourceReader,
    TextDerivation,
};

/// Reader for independently parseable JSON Lines records.
pub struct JsonLinesReader;

impl SourceReader for JsonLinesReader {
    fn scalar_verification(
        &self,
    ) -> Option<objekte_search_core::source::verification::ScalarVerification> {
        Some(objekte_search_core::source::verification::ScalarVerification::ResolvedStrings)
    }
    fn scan_scalars(
        &self,
        bytes: &[u8],
        query: &objekte_search_core::source::verification::ScalarQuery<'_>,
    ) -> Result<objekte_search_core::source::verification::BorrowedScanResult, String> {
        objekte_loader_json::records::scan_record(
            bytes,
            query,
            objekte_search_core::source::verification::ScalarVerification::ResolvedStrings,
        )
    }
    fn uses_extension_query_limits(&self) -> bool {
        false
    }
    fn supports_verified_layout(&self) -> bool {
        true
    }

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

    fn uses_pull_framing(&self) -> bool {
        true
    }

    fn maximum_records(&self, file_bytes: u64) -> u64 {
        // A nonempty record consumes at least one byte, and every record
        // other than the last also needs a separator.
        file_bytes.div_ceil(2)
    }

    fn build_framer<'a>(
        &self,
        input: &'a mut dyn std::io::Read,
        max_record_bytes: Option<usize>,
    ) -> Option<Box<dyn objekte_search_core::source::BuildFramer + 'a>> {
        Some(super::jsonl_framing::build_framer(input, max_record_bytes))
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

/// Install only JSON Lines. Linking the decoder does not register JSON.
pub fn register(
    registry: &mut objekte_search_core::source::registry::SourceRegistry,
) -> Result<(), String> {
    registry.register_file_batched("jsonl", 1, "jsonl", std::sync::Arc::new(JsonLinesReader))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standalone_jsonl_ranges_preserve_bytes_and_boundaries() {
        let bytes = "ü\n{\"x\":\"ä\"}\n\n{\"x\":\"b\"}\n".as_bytes();
        // A non-ASCII prefix is deliberately outside the valid records.
        let start = "ü\n".len();
        let valid = &bytes[start..];
        let ranges = JsonLinesReader.frame(valid).unwrap();
        assert_eq!(ranges.len(), 2);
        assert_eq!(
            &valid[ranges[0].start as usize..ranges[0].end as usize],
            "{\"x\":\"ä\"}".as_bytes()
        );
        for range in ranges {
            JsonLinesReader
                .parse(&valid[range.start as usize..range.end as usize])
                .unwrap();
        }
        assert!(JsonLinesReader.frame(b"").unwrap().is_empty());
    }
}
