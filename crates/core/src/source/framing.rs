//! Generic source range validation and bounded JSON Lines framing.

use crate::inventory::ByteRange;

pub(crate) fn validate_reader_ranges(ranges: &[ByteRange], file_size: usize) -> Result<(), String> {
    let size = u64::try_from(file_size).map_err(|_| "source size overflow")?;
    let mut previous_end = 0u64;
    for range in ranges {
        ByteRange::checked(range.start, range.end, size)
            .map_err(|error| format!("reader contract: {error}"))?;
        usize::try_from(range.start).map_err(|_| "reader contract: range start overflow")?;
        usize::try_from(range.end).map_err(|_| "reader contract: range end overflow")?;
        if range.start < previous_end {
            return Err("reader contract: record ranges overlap or are out of order".into());
        }
        previous_end = range.end;
    }
    Ok(())
}

/// Frame a source once for independent construction jobs. Errors that the
/// ordinary extraction path records as file outcomes use that path unchanged.
pub struct BoundedBuildChunk {
    pub bytes: Vec<u8>,
    pub ranges: Vec<ByteRange>,
    pub file_offset: u64,
    pub preceding_lines: u64,
}

#[cfg(test)]
pub(crate) use super::jsonl_framing::BoundedJsonLines;
