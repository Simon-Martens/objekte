//! Reference line framer used by the scheduler's test fixtures.

use std::io::{BufRead, BufReader, Read};

use crate::inventory::ByteRange;
use crate::source::{BuildChunk, BuildFramer};

pub(crate) struct FixtureLines<R: BufRead = BufReader<std::fs::File>> {
    input: R,
    pending: Vec<u8>,
    offset: u64,
    lines: u64,
    record_limit: usize,
    input_capacity: usize,
}

impl<R: Read> FixtureLines<BufReader<R>> {
    pub(crate) fn with_buffered_reader(input: BufReader<R>) -> Self {
        Self {
            input_capacity: input.capacity(),
            ..Self::from_reader(input)
        }
    }
}

impl<R: BufRead> FixtureLines<R> {
    pub(crate) fn from_reader(input: R) -> Self {
        Self {
            input,
            pending: Vec::new(),
            offset: 0,
            lines: 0,
            record_limit: usize::MAX,
            input_capacity: 0,
        }
    }

    pub(crate) fn limit_record_bytes(&mut self, max: usize) {
        self.record_limit = max;
    }

    pub(crate) fn retained_line_capacity(&self) -> usize {
        self.input_capacity + self.pending.capacity()
    }

    fn fill_line(&mut self) -> Result<(), String> {
        loop {
            let input = self.input.fill_buf().map_err(|error| error.to_string())?;
            if input.is_empty() {
                return Ok(());
            }
            let delimiter = memchr::memchr(b'\n', input);
            let count = delimiter.map_or(input.len(), |end| end + 1);
            if self.pending.len().saturating_add(count) > self.record_limit {
                return Err(format!(
                    "fixture source record exceeds {} bytes",
                    self.record_limit
                ));
            }
            self.pending.extend_from_slice(&input[..count]);
            self.input.consume(count);
            if delimiter.is_some() {
                return Ok(());
            }
        }
    }

    pub(crate) fn next_chunk(
        &mut self,
        max_bytes: usize,
        max_ranges: usize,
    ) -> Result<Option<BuildChunk>, String> {
        let mut chunk = BuildChunk {
            bytes: Vec::new(),
            ranges: Vec::new(),
            file_offset: self.offset,
            preceding_lines: self.lines,
        };
        while chunk.bytes.len() < max_bytes && chunk.ranges.len() < max_ranges {
            if self.pending.is_empty() {
                self.fill_line()?;
            }
            if self.pending.is_empty()
                || !chunk.bytes.is_empty() && chunk.bytes.len() + self.pending.len() > max_bytes
            {
                break;
            }
            let len = self.pending.len();
            let newline = self.pending.last() == Some(&b'\n');
            let content = len - usize::from(newline);
            if !self.pending[..content].iter().all(u8::is_ascii_whitespace) {
                let start = chunk.bytes.len() as u64;
                chunk.ranges.push(ByteRange {
                    start,
                    end: start + content as u64,
                });
            }
            chunk.bytes.extend_from_slice(&self.pending);
            self.pending.clear();
            self.offset += len as u64;
            self.lines += u64::from(newline);
        }
        Ok((!chunk.bytes.is_empty()).then_some(chunk))
    }
}

impl<R: BufRead> BuildFramer for FixtureLines<R> {
    fn next_chunk(
        &mut self,
        max_bytes: usize,
        max_ranges: usize,
    ) -> Result<Option<BuildChunk>, String> {
        Self::next_chunk(self, max_bytes, max_ranges)
    }

    fn retained_input_capacity(&self) -> usize {
        self.retained_line_capacity()
    }
}
