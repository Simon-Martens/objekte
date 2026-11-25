//! Bounded forward framing of JSON Lines.

use objekte_search_core::inventory::ByteRange;
use objekte_search_core::source::{BuildChunk as BoundedBuildChunk, BuildFramer};
use std::io::{BufRead, BufReader, Read};
#[cfg(test)]
use std::path::Path;

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
    record_limit: Option<usize>,
    reader_capacity: usize,
}

#[cfg(test)]
impl BoundedJsonLines<BufReader<std::fs::File>> {
    pub fn open(path: &Path) -> Result<Self, String> {
        let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
        Ok(Self::from_reader(BufReader::new(file)))
    }
}

impl<R: std::io::Read> BoundedJsonLines<BufReader<R>> {
    pub(crate) fn with_buffered_reader(reader: BufReader<R>) -> Self {
        let reader_capacity = reader.capacity();
        Self {
            reader_capacity,
            ..Self::from_reader(reader)
        }
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
            record_limit: None,
            reader_capacity: 0,
        }
    }

    pub(crate) fn limit_record_bytes(&mut self, max: usize) {
        self.record_limit = Some(max);
    }

    pub(crate) fn retained_line_capacity(&self) -> usize {
        self.reader_capacity
            + self.scratch.capacity()
            + self
                .pending
                .as_ref()
                .map_or(0, |line| line.bytes.capacity())
    }

    fn read_line(&mut self) -> Result<usize, String> {
        self.scratch.clear();
        let Some(limit) = self.record_limit else {
            return self
                .reader
                .read_until(b'\n', &mut self.scratch)
                .map_err(|error| error.to_string());
        };
        loop {
            let buffer = self.reader.fill_buf().map_err(|error| error.to_string())?;
            if buffer.is_empty() {
                return Ok(self.scratch.len());
            }
            let newline = memchr::memchr(b'\n', buffer);
            let len = newline.map_or(buffer.len(), |at| at + 1);
            if self.scratch.len().saturating_add(len) > limit {
                return Err(format!("JSONL source record exceeds {limit} bytes"));
            }
            self.scratch.extend_from_slice(&buffer[..len]);
            self.reader.consume(len);
            if newline.is_some() {
                return Ok(self.scratch.len());
            }
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
                let size = self.read_line()?;
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

impl<R: BufRead> BuildFramer for BoundedJsonLines<R> {
    fn next_chunk(
        &mut self,
        max_bytes: usize,
        max_records: usize,
    ) -> Result<Option<BoundedBuildChunk>, String> {
        BoundedJsonLines::next_chunk(self, max_bytes, max_records)
    }
    fn retained_input_capacity(&self) -> usize {
        self.retained_line_capacity()
    }
}

pub(crate) fn build_framer(
    input: &mut dyn Read,
    max_record_bytes: Option<usize>,
) -> Box<dyn BuildFramer + '_> {
    let mut framer = BoundedJsonLines::with_buffered_reader(BufReader::new(input));
    if let Some(limit) = max_record_bytes {
        framer.limit_record_bytes(limit);
    }
    Box::new(framer)
}

#[cfg(test)]
#[path = "framing/tests.rs"]
mod tests;
