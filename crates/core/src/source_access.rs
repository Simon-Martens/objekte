//! Query-scoped, bounded source handles and range windows.

use roaring::RoaringBitmap;

use crate::inventory::{ByteRange, FileInventory, SourceRevision};
use crate::model::FileId;
use crate::source::RecordRangeReader;

const MAX_OPEN_HANDLES: usize = 8;
const WINDOW_BYTES: u64 = 64 * 1024;


struct OpenSource {
    file_id: FileId,
    revision: SourceRevision,
    reader: RecordRangeReader,
    window_start: u64,
    window: Vec<u8>,
    last_used: u64,
}


pub(crate) struct SourceAccessSession<'a> {
    files: &'a FileInventory,
    open: Vec<OpenSource>,
    distinct: RoaringBitmap,
    clock: u64,
}


pub(crate) struct RangeBatch {
    pub records: Vec<(u32, Vec<u8>)>,
    pub opened: u64,
    pub bytes_read: u64,
}


impl<'a> SourceAccessSession<'a> {
    pub fn new(files: &'a FileInventory) -> Self {
        Self {
            files,
            open: Vec::new(),
            distinct: RoaringBitmap::new(),
            clock: 0,
        }
    }

    pub fn distinct_files(&self) -> u64 {
        self.distinct.len()
    }

    pub fn accessed_files(&self) -> &RoaringBitmap {
        &self.distinct
    }

    fn handle(&mut self, file_id: FileId) -> Result<(usize, u64), String> {
        self.clock = self.clock.wrapping_add(1);
        if let Some(index) = self.open.iter().position(|entry| entry.file_id == file_id) {
            self.open[index].last_used = self.clock;
            return Ok((index, 0));
        }
        let file = self
            .files
            .files
            .get(file_id.0 as usize)
            .ok_or("unknown source file")?;
        let path = self
            .files
            .native_path(file_id)
            .ok_or("missing source path")?;
        let reader = RecordRangeReader::open(&path, file.revision)?;
        self.distinct.insert(file_id.0);
        if self.open.len() == MAX_OPEN_HANDLES {
            let oldest = self
                .open
                .iter()
                .enumerate()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(index, _)| index)
                .ok_or("missing source handle")?;
            self.open.swap_remove(oldest);
        }
        self.open.push(OpenSource {
            file_id,
            revision: file.revision,
            reader,
            window_start: 0,
            window: Vec::new(),
            last_used: self.clock,
        });
        Ok((self.open.len() - 1, 1))
    }

    pub fn read(
        &mut self,
        file_id: FileId,
        ranges: &[(u32, ByteRange)],
    ) -> Result<RangeBatch, String> {
        if ranges.is_empty() {
            return Ok(RangeBatch {
                records: Vec::new(),
                opened: 0,
                bytes_read: 0,
            });
        }
        let (index, opened) = self.handle(file_id)?;
        let source = &mut self.open[index];
        if let [(slot, range)] = ranges {
            ByteRange::checked(range.start, range.end, source.revision.size)?;
            let window_end = source
                .window_start
                .saturating_add(source.window.len() as u64);
            if !source.window.is_empty()
                && range.start >= source.window_start
                && range.end <= window_end
            {
                let start = usize::try_from(range.start - source.window_start)
                    .map_err(|_| "range offset overflow")?;
                let end = usize::try_from(range.end - source.window_start)
                    .map_err(|_| "range offset overflow")?;
                return Ok(RangeBatch {
                    records: vec![(*slot, source.window[start..end].to_vec())],
                    opened,
                    bytes_read: 0,
                });
            }
            if range.len() <= WINDOW_BYTES {
                let end = range
                    .start
                    .saturating_add(WINDOW_BYTES)
                    .max(range.end)
                    .min(source.revision.size);
                let window = ByteRange::checked(range.start, end, source.revision.size)?;
                let (mut bytes, bytes_read) = source.reader.read(&[(0, window)])?;
                source.window_start = range.start;
                source.window = bytes.pop().ok_or("source window missing")?.1;
                let end = usize::try_from(range.len()).map_err(|_| "record range too large")?;
                return Ok(RangeBatch {
                    records: vec![(*slot, source.window[..end].to_vec())],
                    opened,
                    bytes_read,
                });
            }
        }
        let (records, bytes_read) = source.reader.read(ranges)?;
        Ok(RangeBatch {
            records,
            opened,
            bytes_read,
        })
    }

    /// Stream read without retained lookahead windows, under a caller-supplied
    /// bound for each source window. The returned record bytes are still
    /// charged separately by the caller.
    pub(crate) fn read_bounded(
        &mut self,
        file_id: FileId,
        ranges: &[(u32, ByteRange)],
        max_total_bytes: usize,
    ) -> Result<RangeBatch, String> {
        if ranges.is_empty() {
            return Ok(RangeBatch {
                records: Vec::new(),
                opened: 0,
                bytes_read: 0,
            });
        }
        let (index, opened) = self.handle(file_id)?;
        let source = &mut self.open[index];
        source.window = Vec::new();
        let (records, bytes_read) = source.reader.read_bounded(ranges, max_total_bytes)?;
        Ok(RangeBatch {
            records,
            opened,
            bytes_read,
        })
    }
}


#[cfg(test)]
#[path = "source_access.test.rs"]
mod tests;
