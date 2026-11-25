//! Generic source range validation and bounded JSON Lines framing.

use std::io::{BufRead, BufReader};
#[cfg(test)]
use std::path::Path;

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

struct PendingBuildLine {
    bytes: Vec<u8>,
    file_offset: u64,
    preceding_lines: u64,
}

/// Build-only line framing. It holds one chunk and, when a boundary is
/// reached, at most one pending line. A single oversized line is admitted as
/// its own chunk rather than retaining the complete source file.
pub(crate) struct BoundedJsonLines<R: BufRead = BufReader<std::fs::File>> {
    reader: R,
    file_offset: u64,
    preceding_lines: u64,
    scratch: Vec<u8>,
    pending: Option<PendingBuildLine>,
    eof: bool,
}

#[cfg(test)]
impl BoundedJsonLines<BufReader<std::fs::File>> {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        Ok(Self::from_reader(BufReader::new(file)))
    }
}

impl<R: BufRead> BoundedJsonLines<R> {
    pub(crate) fn from_reader(reader: R) -> Self {
        Self {
            reader,
            file_offset: 0,
            preceding_lines: 0,
            scratch: Vec::new(),
            pending: None,
            eof: false,
        }
    }

    pub fn next_chunk(
        &mut self,
        max_bytes: usize,
        max_ranges: usize,
    ) -> Result<Option<BoundedBuildChunk>, String> {
        if max_bytes == 0 || max_ranges == 0 {
            return Err("bounded framing requires positive limits".into());
        }
        let mut chunk: Option<BoundedBuildChunk> = None;
        loop {
            let line = if let Some(line) = self.pending.take() {
                line
            } else if self.eof {
                break;
            } else {
                self.scratch.clear();
                let size = self
                    .reader
                    .read_until(b'\n', &mut self.scratch)
                    .map_err(|error| error.to_string())?;
                if size == 0 {
                    self.eof = true;
                    break;
                }
                let start = self.file_offset;
                let preceding = self.preceding_lines;
                self.file_offset = self
                    .file_offset
                    .checked_add(size as u64)
                    .ok_or("source offset overflow")?;
                if self.scratch.last() == Some(&b'\n') {
                    self.preceding_lines = self
                        .preceding_lines
                        .checked_add(1)
                        .ok_or("source line count overflow")?;
                }
                PendingBuildLine {
                    bytes: std::mem::take(&mut self.scratch),
                    file_offset: start,
                    preceding_lines: preceding,
                }
            };
            let content_len = line.bytes.len() - usize::from(line.bytes.last() == Some(&b'\n'));
            let record = !line.bytes[..content_len]
                .iter()
                .all(u8::is_ascii_whitespace);
            let full = chunk.as_ref().is_some_and(|chunk| {
                chunk.bytes.len().saturating_add(line.bytes.len()) > max_bytes
                    || (record && chunk.ranges.len() == max_ranges)
            });
            if full {
                self.pending = Some(line);
                break;
            }
            let current = chunk.get_or_insert_with(|| BoundedBuildChunk {
                bytes: Vec::new(),
                ranges: Vec::new(),
                file_offset: line.file_offset,
                preceding_lines: line.preceding_lines,
            });
            let local_start = current.bytes.len();
            let local_end = local_start
                .checked_add(content_len)
                .ok_or("chunk offset overflow")?;
            if current.bytes.is_empty() {
                current.bytes = line.bytes;
            } else {
                current.bytes.extend_from_slice(&line.bytes);
                self.scratch = line.bytes;
            }
            if record {
                current.ranges.push(ByteRange::checked(
                    local_start as u64,
                    local_end as u64,
                    current.bytes.len() as u64,
                )?);
            }
            if current.bytes.len() >= max_bytes || current.ranges.len() == max_ranges {
                break;
            }
        }
        Ok(chunk)
    }
}

#[cfg(test)]
mod tests;
