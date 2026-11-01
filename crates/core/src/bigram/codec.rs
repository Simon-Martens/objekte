//! Versioned binary encoding of the finished compressed filter.

use super::{
    BigramFilter, ColumnSlab,
    filter::{BIGRAM_KEY_SLOTS, MAX_BIGRAM_COLUMNS, NO_COLUMN},
};
use crate::{Error, Result};
use std::collections::BTreeSet;

const MAGIC: &[u8; 8] = b"OBJBG001";
const RARE_WORDS: usize = BIGRAM_KEY_SLOTS.div_ceil(64);

fn bad(message: &str) -> Error {
    Error::Cache(format!("invalid bigram filter: {message}"))
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: usize) -> Result<()> {
    let value = u64::try_from(value).map_err(|_| bad("size exceeds the file format"))?;
    out.extend_from_slice(&value.to_le_bytes());
    Ok(())
}


struct Reader<'a> {
    bytes: &'a [u8],
    position: usize,
}


impl<'a> Reader<'a> {
    fn take(&mut self, len: usize) -> Result<&'a [u8]> {
        let end = self
            .position
            .checked_add(len)
            .ok_or_else(|| bad("length overflow"))?;
        let value = self
            .bytes
            .get(self.position..end)
            .ok_or_else(|| bad("truncated data"))?;
        self.position = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn usize(&mut self) -> Result<usize> {
        usize::try_from(self.u64()?).map_err(|_| bad("size exceeds this platform"))
    }

    fn u16s(&mut self, len: usize) -> Result<Vec<u16>> {
        Ok(self
            .take(len.checked_mul(2).ok_or_else(|| bad("length overflow"))?)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|bytes| u16::from_le_bytes(*bytes))
            .collect())
    }

    fn u32s(&mut self, len: usize) -> Result<Vec<u32>> {
        Ok(self
            .take(len.checked_mul(4).ok_or_else(|| bad("length overflow"))?)?
            .as_chunks::<4>()
            .0
            .iter()
            .map(|bytes| u32::from_le_bytes(*bytes))
            .collect())
    }

    fn u64s(&mut self, len: usize) -> Result<Vec<u64>> {
        Ok(self
            .take(len.checked_mul(8).ok_or_else(|| bad("length overflow"))?)?
            .as_chunks::<8>()
            .0
            .iter()
            .map(|bytes| u64::from_le_bytes(*bytes))
            .collect())
    }
}


fn write_filter(filter: &BigramFilter, out: &mut Vec<u8>, depth: usize) -> Result<()> {
    if depth > 1 || depth == 1 && filter.skip_index.is_some() {
        return Err(bad("nested skip index"));
    }
    if filter.lookup.len() != BIGRAM_KEY_SLOTS || filter.rare_keys.len() != RARE_WORDS {
        return Err(bad("lookup dimensions"));
    }
    put_u64(out, filter.file_count)?;
    put_u64(out, filter.populated)?;
    put_u64(out, filter.words)?;
    put_u64(out, filter.dense_count)?;
    for &value in &filter.lookup {
        put_u16(out, value);
    }
    for &value in &filter.rare_keys {
        out.extend_from_slice(&value.to_le_bytes());
    }
    for &value in filter.dense_data.iter() {
        out.extend_from_slice(&value.to_le_bytes());
    }
    put_u32(
        out,
        u32::try_from(filter.sparse_offsets.len()).map_err(|_| bad("too many offsets"))?,
    );
    for &value in &filter.sparse_offsets {
        put_u32(out, value);
    }
    put_u64(out, filter.sparse_data.len())?;
    out.extend_from_slice(&filter.sparse_data);
    out.push(u8::from(filter.skip_index.is_some()));
    if let Some(skip) = &filter.skip_index {
        write_filter(skip, out, depth + 1)?;
    }
    Ok(())
}

fn sparse_valid(data: &[u8], file_count: usize) -> bool {
    let mut at = 0;
    let mut previous = 0u64;
    let mut first = true;
    while at < data.len() {
        let mut gap = 0u64;
        let mut shift = 0;
        loop {
            let Some(&byte) = data.get(at) else {
                return false;
            };
            at += 1;
            let part = u64::from(byte & 0x7f);
            if shift >= 64 || part > (u64::MAX >> shift) {
                return false;
            }
            gap |= part << shift;
            if byte & 0x80 == 0 {
                break;
            }
            shift += 7;
        }
        if !first && gap == 0 {
            return false;
        }
        let Some(position) = previous.checked_add(gap) else {
            return false;
        };
        if position >= file_count as u64 {
            return false;
        }
        previous = position;
        first = false;
    }
    true
}

fn read_filter(reader: &mut Reader<'_>, depth: usize) -> Result<BigramFilter> {
    if depth > 1 {
        return Err(bad("nested skip index"));
    }
    let file_count = reader.usize()?;
    let populated = reader.usize()?;
    let words = reader.usize()?;
    let dense_count = reader.usize()?;
    if words != file_count.div_ceil(64)
        || populated > file_count
        || dense_count > MAX_BIGRAM_COLUMNS
    {
        return Err(bad("invalid dimensions"));
    }
    let lookup = reader.u16s(BIGRAM_KEY_SLOTS)?;
    let rare_keys = reader.u64s(RARE_WORDS)?;
    let dense_len = dense_count
        .checked_mul(words)
        .ok_or_else(|| bad("length overflow"))?;
    let dense = reader.u64s(dense_len)?;
    let offset_count = reader.u32()? as usize;
    if offset_count == 0 || offset_count > MAX_BIGRAM_COLUMNS + 1 {
        return Err(bad("invalid sparse offsets"));
    }
    let sparse_offsets = reader.u32s(offset_count)?;
    let sparse_len = reader.usize()?;
    let sparse_data = reader.take(sparse_len)?.to_vec();
    let sparse_len_u32 = u32::try_from(sparse_len).map_err(|_| bad("sparse data too large"))?;
    let used_columns = dense_count + offset_count - 1;
    let mut seen = BTreeSet::new();
    if sparse_offsets[0] != 0
        || sparse_offsets.last().copied() != Some(sparse_len_u32)
        || sparse_offsets.windows(2).any(|pair| pair[0] > pair[1])
        || used_columns > MAX_BIGRAM_COLUMNS
        || lookup.iter().any(|&column| {
            column != NO_COLUMN && (usize::from(column) >= used_columns || !seen.insert(column))
        })
    {
        return Err(bad("invalid column table"));
    }
    for pair in sparse_offsets.windows(2) {
        if !sparse_valid(&sparse_data[pair[0] as usize..pair[1] as usize], file_count) {
            return Err(bad("invalid sparse column"));
        }
    }
    let skip_index = match reader.u8()? {
        0 => None,
        1 if depth == 0 => Some(Box::new(read_filter(reader, depth + 1)?)),
        _ => return Err(bad("invalid skip index flag")),
    };
    if skip_index
        .as_ref()
        .is_some_and(|skip| skip.file_count != file_count)
    {
        return Err(bad("skip index file count differs"));
    }
    let dense_data = ColumnSlab::from_vec(dense).ok_or_else(|| bad("cannot allocate columns"))?;
    Ok(BigramFilter {
        lookup,
        rare_keys,
        dense_data,
        dense_count,
        sparse_offsets,
        sparse_data,
        words,
        file_count,
        populated,
        skip_index,
    })
}


impl BigramFilter {
    /// Serialize every compressed column and the optional skip index.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut out = MAGIC.to_vec();
        write_filter(self, &mut out, 0)?;
        Ok(out)
    }

    /// Restore a filter from its versioned binary representation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader { bytes, position: 0 };
        if reader.take(MAGIC.len())? != MAGIC {
            return Err(bad("unknown format version"));
        }
        let filter = read_filter(&mut reader, 0)?;
        if reader.position != bytes.len() {
            return Err(bad("trailing data"));
        }
        Ok(filter)
    }
}
