//! Versioned, bounded streaming encoding of the finished compressed filter.

use super::{
    BigramFilter, ColumnSlab,
    filter::{BigramEncoding, NO_COLUMN},
};
use crate::{Error, Result};
#[cfg(feature = "index-timings")]
use std::{cell::Cell, time::Instant};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
};

#[cfg(feature = "index-timings")]
thread_local! {
    static SPARSE_VALIDATION_NANOS: Cell<u64> = const { Cell::new(0) };
}

const LEGACY_MAGIC: &[u8; 8] = b"OBJBG001";
const MAGIC: &[u8; 8] = b"OBJBG002";
const BLOCK_WORDS: usize = 4096;

fn bad(message: &str) -> Error {
    Error::Cache(format!("invalid bigram filter: {message}"))
}

fn written(result: std::io::Result<()>) -> Result<()> {
    result.map_err(|error| Error::Cache(format!("cannot write bigram filter: {error}")))
}

fn put_u32(writer: &mut impl Write, value: u32) -> Result<()> {
    written(writer.write_all(&value.to_le_bytes()))
}

fn put_u64(writer: &mut impl Write, value: usize) -> Result<()> {
    let value = u64::try_from(value).map_err(|_| bad("size exceeds the file format"))?;
    written(writer.write_all(&value.to_le_bytes()))
}

fn write_u16s(writer: &mut impl Write, values: &[u16]) -> Result<()> {
    let mut block = [0_u8; BLOCK_WORDS * 2];
    for chunk in values.chunks(BLOCK_WORDS) {
        for (word, bytes) in chunk.iter().zip(block.as_chunks_mut::<2>().0) {
            bytes.copy_from_slice(&word.to_le_bytes());
        }
        written(writer.write_all(&block[..chunk.len() * 2]))?;
    }
    Ok(())
}

fn write_u32s(writer: &mut impl Write, values: &[u32]) -> Result<()> {
    let mut block = [0_u8; BLOCK_WORDS * 4];
    for chunk in values.chunks(BLOCK_WORDS) {
        for (word, bytes) in chunk.iter().zip(block.as_chunks_mut::<4>().0) {
            bytes.copy_from_slice(&word.to_le_bytes());
        }
        written(writer.write_all(&block[..chunk.len() * 4]))?;
    }
    Ok(())
}

fn write_u64s(writer: &mut impl Write, values: &[u64]) -> Result<()> {
    let mut block = [0_u8; BLOCK_WORDS * 8];
    for chunk in values.chunks(BLOCK_WORDS) {
        for (word, bytes) in chunk.iter().zip(block.as_chunks_mut::<8>().0) {
            bytes.copy_from_slice(&word.to_le_bytes());
        }
        written(writer.write_all(&block[..chunk.len() * 8]))?;
    }
    Ok(())
}

fn write_filter(filter: &BigramFilter, writer: &mut impl Write, depth: usize) -> Result<()> {
    if depth > 1 || depth == 1 && filter.skip_index.is_some() {
        return Err(bad("nested skip index"));
    }
    if filter.lookup.len() != filter.encoding.key_slots()
        || filter.rare_keys.len() != filter.encoding.key_slots().div_ceil(64)
        || filter.columns_used() > filter.encoding.max_columns()
        || filter
            .skip_index
            .as_ref()
            .is_some_and(|skip| skip.encoding != filter.encoding)
    {
        return Err(bad("lookup dimensions"));
    }
    let identifier = filter.encoding.as_str().as_bytes();
    written(writer.write_all(&[identifier.len() as u8]))?;
    written(writer.write_all(identifier))?;
    put_u32(writer, filter.lookup.len() as u32)?;
    put_u64(writer, filter.file_count)?;
    put_u64(writer, filter.populated)?;
    put_u64(writer, filter.words)?;
    put_u64(writer, filter.dense_count)?;
    write_u16s(writer, &filter.lookup)?;
    write_u64s(writer, &filter.rare_keys)?;
    write_u64s(writer, &filter.dense_data)?;
    put_u32(
        writer,
        u32::try_from(filter.sparse_offsets.len()).map_err(|_| bad("too many offsets"))?,
    )?;
    write_u32s(writer, &filter.sparse_offsets)?;
    put_u64(writer, filter.sparse_data.len())?;
    written(writer.write_all(&filter.sparse_data))?;
    written(writer.write_all(&[u8::from(filter.skip_index.is_some())]))?;
    if let Some(skip) = &filter.skip_index {
        write_filter(skip, writer, depth + 1)?;
    }
    Ok(())
}

fn body_len(filter: &BigramFilter, depth: usize) -> Result<u64> {
    if depth > 1 || depth == 1 && filter.skip_index.is_some() {
        return Err(bad("nested skip index"));
    }
    let length = [
        1,
        filter.encoding.as_str().len(),
        4,
        4 * 8,
        filter
            .lookup
            .len()
            .checked_mul(2)
            .ok_or_else(|| bad("length overflow"))?,
        filter
            .rare_keys
            .len()
            .checked_mul(8)
            .ok_or_else(|| bad("length overflow"))?,
        filter
            .dense_data
            .len()
            .checked_mul(8)
            .ok_or_else(|| bad("length overflow"))?,
        4,
        filter
            .sparse_offsets
            .len()
            .checked_mul(4)
            .ok_or_else(|| bad("length overflow"))?,
        8,
        filter.sparse_data.len(),
        1,
    ]
    .into_iter()
    .try_fold(0usize, |sum, part| {
        sum.checked_add(part).ok_or_else(|| bad("length overflow"))
    })?;
    let mut length = u64::try_from(length).map_err(|_| bad("length overflow"))?;
    if let Some(skip) = &filter.skip_index {
        length = length
            .checked_add(body_len(skip, depth + 1)?)
            .ok_or_else(|| bad("length overflow"))?;
    }
    Ok(length)
}


/// Bounds every allocation by bytes still available in this filter section.
struct Reader<R> {
    inner: R,
    remaining: u64,
}


impl<R: Read> Reader<R> {
    fn read_into(&mut self, output: &mut [u8]) -> Result<()> {
        let length = u64::try_from(output.len()).map_err(|_| bad("length overflow"))?;
        if length > self.remaining {
            return Err(bad("truncated data"));
        }
        self.inner
            .read_exact(output)
            .map_err(|error| Error::Cache(format!("cannot read bigram filter: {error}")))?;
        self.remaining -= length;
        Ok(())
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut bytes = [0; N];
        self.read_into(&mut bytes)?;
        Ok(bytes)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.array::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.array()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.array()?))
    }

    fn usize(&mut self) -> Result<usize> {
        usize::try_from(self.u64()?).map_err(|_| bad("size exceeds this platform"))
    }

    fn checked_bytes(&self, len: usize, width: usize) -> Result<()> {
        let bytes = len
            .checked_mul(width)
            .ok_or_else(|| bad("length overflow"))?;
        if u64::try_from(bytes).map_err(|_| bad("length overflow"))? > self.remaining {
            return Err(bad("truncated data"));
        }
        Ok(())
    }

    fn bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        self.checked_bytes(len, 1)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(len)
            .map_err(|_| bad("cannot allocate bytes"))?;
        values.resize(len, 0);
        self.read_into(&mut values)?;
        Ok(values)
    }

    fn u16s(&mut self, len: usize) -> Result<Vec<u16>> {
        self.checked_bytes(len, 2)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(len)
            .map_err(|_| bad("cannot allocate lookup"))?;
        let mut block = [0_u8; BLOCK_WORDS * 2];
        for count in (0..len)
            .step_by(BLOCK_WORDS)
            .map(|at| (len - at).min(BLOCK_WORDS))
        {
            self.read_into(&mut block[..count * 2])?;
            values.extend(
                block[..count * 2]
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]])),
            );
        }
        Ok(values)
    }

    fn u32s(&mut self, len: usize) -> Result<Vec<u32>> {
        self.checked_bytes(len, 4)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(len)
            .map_err(|_| bad("cannot allocate offsets"))?;
        let mut block = [0_u8; BLOCK_WORDS * 4];
        for count in (0..len)
            .step_by(BLOCK_WORDS)
            .map(|at| (len - at).min(BLOCK_WORDS))
        {
            self.read_into(&mut block[..count * 4])?;
            values.extend(
                block[..count * 4]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|bytes| u32::from_le_bytes(*bytes)),
            );
        }
        Ok(values)
    }

    fn u64s(&mut self, len: usize) -> Result<Vec<u64>> {
        self.checked_bytes(len, 8)?;
        let mut values = Vec::new();
        values
            .try_reserve_exact(len)
            .map_err(|_| bad("cannot allocate flags"))?;
        let mut block = [0_u8; BLOCK_WORDS * 8];
        for count in (0..len)
            .step_by(BLOCK_WORDS)
            .map(|at| (len - at).min(BLOCK_WORDS))
        {
            self.read_into(&mut block[..count * 8])?;
            values.extend(
                block[..count * 8]
                    .as_chunks::<8>()
                    .0
                    .iter()
                    .map(|bytes| u64::from_le_bytes(*bytes)),
            );
        }
        Ok(values)
    }

    fn dense(&mut self, len: usize) -> Result<ColumnSlab> {
        self.checked_bytes(len, 8)?;
        let mut slab = ColumnSlab::new(len).ok_or_else(|| bad("cannot allocate columns"))?;
        let mut block = [0_u8; BLOCK_WORDS * 8];
        for chunk in slab.as_mut_slice().chunks_mut(BLOCK_WORDS) {
            self.read_into(&mut block[..chunk.len() * 8])?;
            for (word, bytes) in chunk.iter_mut().zip(block.as_chunks::<8>().0) {
                *word = u64::from_le_bytes(*bytes);
            }
        }
        Ok(slab)
    }
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

fn read_filter<R: Read>(
    reader: &mut Reader<R>,
    depth: usize,
    legacy: bool,
) -> Result<BigramFilter> {
    if depth > 1 {
        return Err(bad("nested skip index"));
    }
    let encoding = if legacy {
        BigramEncoding::Ascii
    } else {
        let len = reader.u8()? as usize;
        let name = reader.bytes(len)?;
        let name = std::str::from_utf8(&name).map_err(|_| bad("unknown encoding"))?;
        BigramEncoding::parse(name).ok_or_else(|| bad("unknown encoding"))?
    };
    if !legacy && reader.u32()? as usize != encoding.key_slots() {
        return Err(bad("lookup dimensions"));
    }
    let file_count = reader.usize()?;
    let populated = reader.usize()?;
    let words = reader.usize()?;
    let dense_count = reader.usize()?;
    if words != file_count.div_ceil(64)
        || populated > file_count
        || dense_count > encoding.max_columns()
    {
        return Err(bad("invalid dimensions"));
    }
    let lookup = reader.u16s(encoding.key_slots())?;
    let rare_keys = reader.u64s(encoding.key_slots().div_ceil(64))?;
    let dense_len = dense_count
        .checked_mul(words)
        .ok_or_else(|| bad("length overflow"))?;
    let dense_data = reader.dense(dense_len)?;
    let offset_count = reader.u32()? as usize;
    if offset_count == 0 || offset_count > encoding.max_columns() + 1 {
        return Err(bad("invalid sparse offsets"));
    }
    let sparse_offsets = reader.u32s(offset_count)?;
    let sparse_len = reader.usize()?;
    let sparse_data = reader.bytes(sparse_len)?;
    let sparse_len_u32 = u32::try_from(sparse_len).map_err(|_| bad("sparse data too large"))?;
    let used_columns = dense_count + offset_count - 1;
    let mut seen = BTreeSet::new();
    if sparse_offsets[0] != 0
        || sparse_offsets.last().copied() != Some(sparse_len_u32)
        || sparse_offsets.windows(2).any(|pair| pair[0] > pair[1])
        || used_columns > encoding.max_columns()
        || lookup.iter().any(|&column| {
            column != NO_COLUMN && (usize::from(column) >= used_columns || !seen.insert(column))
        })
    {
        return Err(bad("invalid column table"));
    }
    if seen.len() != used_columns {
        return Err(bad("unreferenced column"));
    }

    #[cfg(feature = "index-timings")]
    let sparse_started = Instant::now();
    for pair in sparse_offsets.windows(2) {
        if !sparse_valid(&sparse_data[pair[0] as usize..pair[1] as usize], file_count) {
            return Err(bad("invalid sparse column"));
        }
    }

    #[cfg(feature = "index-timings")]
    SPARSE_VALIDATION_NANOS.with(|total| {
        total.set(
            total
                .get()
                .saturating_add(sparse_started.elapsed().as_nanos() as u64),
        );
    });
    let skip_index = match reader.u8()? {
        0 => None,
        1 if depth == 0 => Some(Box::new(read_filter(reader, depth + 1, legacy)?)),
        _ => return Err(bad("invalid skip index flag")),
    };
    if skip_index
        .as_ref()
        .is_some_and(|skip| skip.file_count != file_count || skip.encoding != encoding)
    {
        return Err(bad("skip index file count differs"));
    }
    Ok(BigramFilter {
        encoding,
        scalar_postings: false,
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
    #[cfg(feature = "index-timings")]
    pub(crate) fn take_sparse_validation_nanos() -> u64 {
        SPARSE_VALIDATION_NANOS.with(|total| total.replace(0))
    }

    pub(crate) fn encoded_len(&self) -> Result<u64> {
        body_len(self, 0)?
            .checked_add(MAGIC.len() as u64)
            .ok_or_else(|| bad("length overflow"))
    }

    /// Write the portable bytes without constructing a second filter-sized vector.
    pub(crate) fn write_to(&self, writer: &mut impl Write) -> Result<()> {
        written(writer.write_all(MAGIC))?;
        write_filter(self, writer, 0)
    }

    /// Decode into final dense storage from a bounded filter section.
    pub(crate) fn read_from(reader: &mut impl Read, length: u64) -> Result<Self> {
        let mut reader = Reader {
            inner: reader,
            remaining: length,
        };
        let magic = reader.array::<8>()?;
        let legacy = match &magic {
            value if value == MAGIC => false,
            value if value == LEGACY_MAGIC => true,
            _ => return Err(bad("unknown format version")),
        };
        let filter = read_filter(&mut reader, 0, legacy)?;
        if reader.remaining != 0 {
            return Err(bad("trailing data"));
        }
        Ok(filter)
    }

    /// Serialize every compressed column and the optional skip index.
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut output = Vec::new();
        self.write_to(&mut output)?;
        Ok(output)
    }

    /// Restore a filter from its versioned binary representation.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::read_from(&mut &bytes[..], bytes.len() as u64)
    }
}


#[cfg(test)]
#[path = "codec.test.rs"]
mod tests;
