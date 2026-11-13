//! Sorted leaf blocks with direct entry offsets and checked typed-key routing.

use std::io::Write;
use std::marker::PhantomData;
use std::sync::Arc;

use bincode::Options;
use serde::{Serialize, de::DeserializeOwned};

use crate::model::Number;
use crate::posting::ScalarKey;
use crate::segment_format::{PostingDescriptor, SectionKind, SegmentReader};

const TARGET_LEAF_BYTES: usize = 16 * 1024;
const MAX_KEY_BYTES: usize = 64 * 1024 * 1024;
const MAX_BLOCKS: usize = 1_000_000;

fn codec() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .reject_trailing_bytes()
}

fn decode_key<K: DeserializeOwned>(bytes: &[u8]) -> Result<K, String> {
    codec()
        .with_limit(bytes.len() as u64)
        .deserialize(bytes)
        .map_err(|error| format!("invalid dictionary key: {error}"))
}

/// Compare a validated portable key against a query key without materializing
/// owned strings at every binary-search probe.
pub(crate) trait EncodedKey: Ord + Clone + DeserializeOwned {
    fn compare_encoded(bytes: &[u8], key: &Self) -> Result<std::cmp::Ordering, String>;
}

fn encoded_tag(bytes: &[u8], variants: u32) -> Result<u32, String> {
    let tag = read_u32(bytes, 0)?;
    if tag >= variants {
        return Err("invalid dictionary key variant".into());
    }
    Ok(tag)
}

impl EncodedKey for u64 {
    fn compare_encoded(bytes: &[u8], key: &Self) -> Result<std::cmp::Ordering, String> {
        let encoded: [u8; 8] = bytes.try_into()
            .map_err(|_| "invalid u64 dictionary key length")?;
        Ok(u64::from_le_bytes(encoded).cmp(key))
    }
}

impl EncodedKey for Number {
    fn compare_encoded(bytes: &[u8], key: &Self) -> Result<std::cmp::Ordering, String> {
        let tag = encoded_tag(bytes, 3)?;
        let value = match tag {
            0 => {
                let encoded: [u8; 16] = bytes.get(4..).ok_or("integer key truncated")?
                    .try_into().map_err(|_| "invalid integer key length")?;
                Number::Integer(i128::from_le_bytes(encoded))
            }
            1 => {
                let encoded: [u8; 8] = bytes.get(4..).ok_or("float key truncated")?
                    .try_into().map_err(|_| "invalid float key length")?;
                let value = f64::from_le_bytes(encoded);
                if !value.is_finite() {
                    return Err("invalid float key".into());
                }
                Number::Float(value)
            }
            _ => decode_key(bytes)?,
        };
        Ok(value.cmp(key))
    }
}

impl EncodedKey for ScalarKey {
    fn compare_encoded(bytes: &[u8], key: &Self) -> Result<std::cmp::Ordering, String> {
        let tag = encoded_tag(bytes, 7)?;
        let target_tag = match key {
            ScalarKey::String(_) => 0,
            ScalarKey::Id(_) => 1,
            ScalarKey::Number(_) => 2,
            ScalarKey::Bool(_) => 3,
            ScalarKey::Date(_) => 4,
            ScalarKey::Datetime(_) => 5,
            ScalarKey::Unixtime(_) => 6,
        };
        if tag != target_tag {
            return Ok(tag.cmp(&target_tag));
        }
        match key {
            ScalarKey::String(target) | ScalarKey::Id(target) => {
                let length = usize::try_from(read_u64(bytes, 4)?)
                    .map_err(|_| "string key too large")?;
                let value = bytes.get(12..).ok_or("string key truncated")?;
                if length != value.len() {
                    return Err("invalid string key length".into());
                }
                let value = std::str::from_utf8(value).map_err(|_| "invalid string key utf8")?;
                Ok(value.cmp(target))
            }
            ScalarKey::Number(target) => Number::compare_encoded(
                bytes.get(4..).ok_or("number key truncated")?, target,
            ),
            ScalarKey::Bool(target) => match bytes.get(4..) {
                Some([value]) if *value <= 1 => Ok((*value != 0).cmp(target)),
                _ => Err("invalid bool key".into()),
            },
            ScalarKey::Unixtime(target) => {
                let encoded: [u8; 8] = bytes.get(4..).ok_or("unixtime key truncated")?
                    .try_into().map_err(|_| "invalid unixtime key length")?;
                Ok(i64::from_le_bytes(encoded).cmp(target))
            }
            _ => Ok(decode_key::<ScalarKey>(bytes)?.cmp(key)),
        }
    }
}

struct PendingEntry {
    key_start: usize,
    key_length: usize,
    posting: PostingDescriptor,
}

struct EncodedBlockMeta {
    offset: u64,
    length: u32,
    count: u32,
    first: Vec<u8>,
    last: Vec<u8>,
}

struct BlockMeta<K> {
    offset: u64,
    length: u32,
    count: u32,
    first: K,
    last: K,
}

fn write_counted(writer: &mut dyn Write, position: &mut u64, bytes: &[u8]) -> Result<(), String> {
    *position = position
        .checked_add(bytes.len() as u64)
        .ok_or("dictionary position overflow")?;
    writer.write_all(bytes).map_err(|error| error.to_string())
}

fn flush_leaf(
    writer: &mut dyn Write,
    position: &mut u64,
    pending: &mut Vec<PendingEntry>,
    key_arena: &[u8],
    blocks: &mut Vec<EncodedBlockMeta>,
) -> Result<(), String> {
    if pending.is_empty() {
        return Ok(());
    }
    if blocks.len() >= MAX_BLOCKS {
        return Err("dictionary block limit exceeded".into());
    }
    let start = *position;
    let count = u32::try_from(pending.len()).map_err(|_| "leaf entry count overflow")?;
    write_counted(writer, position, &count.to_le_bytes())?;
    let mut entry_offset = 4usize
        .checked_add(pending.len().checked_mul(4).ok_or("leaf offset table overflow")?)
        .ok_or("leaf offset table overflow")?;
    for entry in pending.iter() {
        let offset = u32::try_from(entry_offset).map_err(|_| "leaf offset overflow")?;
        write_counted(writer, position, &offset.to_le_bytes())?;
        entry_offset = entry_offset
            .checked_add(4 + entry.key_length + 24)
            .ok_or("dictionary entry overflow")?;
    }
    let key_bytes = |entry: &PendingEntry| {
        key_arena
            .get(entry.key_start..entry.key_start + entry.key_length)
            .ok_or("dictionary key arena out of bounds")
    };
    let first = key_bytes(pending.first().unwrap())?.to_vec();
    let last = key_bytes(pending.last().unwrap())?.to_vec();
    for entry in pending.drain(..) {
        let len = u32::try_from(entry.key_length).map_err(|_| "dictionary key too large")?;
        write_counted(writer, position, &len.to_le_bytes())?;
        write_counted(writer, position, key_bytes(&entry)?)?;
        write_counted(writer, position, &entry.posting.encode())?;
    }
    blocks.push(EncodedBlockMeta {
        offset: start,
        length: u32::try_from(*position - start).map_err(|_| "leaf block too large")?,
        count,
        first,
        last,
    });
    Ok(())
}

/// Incremental leaf writer. The visitor retains only one leaf's encoded keys
/// and the sparse directory, avoiding a second vector of every dictionary key.
pub(crate) struct OrderedBlockWriter<K, W: Write> {
    writer: W,
    position: u64,
    pending: Vec<PendingEntry>,
    key_arena: Vec<u8>,
    pending_size: usize,
    blocks: Vec<EncodedBlockMeta>,
    previous: Option<K>,
    validate_order: bool,
}

impl<K: Ord + Clone + Serialize, W: Write> OrderedBlockWriter<K, W> {
    pub(crate) fn new(writer: W, validate_order: bool) -> Self {
        Self {
            writer,
            position: 0,
            pending: Vec::new(),
            key_arena: Vec::new(),
            pending_size: 4,
            blocks: Vec::new(),
            previous: None,
            validate_order,
        }
    }

    pub(crate) fn push(&mut self, key: &K, posting: PostingDescriptor) -> Result<(), String> {
        if self.validate_order {
            if self.previous.as_ref().is_some_and(|last| last >= key) {
                return Err("dictionary keys must be strictly ordered".into());
            }
            self.previous = Some(key.clone());
        }
        let key_start = self.key_arena.len();
        if let Err(error) = codec().serialize_into(&mut self.key_arena, key) {
            self.key_arena.truncate(key_start);
            return Err(error.to_string());
        }
        let key_length = self.key_arena.len() - key_start;
        if key_length > MAX_KEY_BYTES {
            self.key_arena.truncate(key_start);
            return Err("dictionary key exceeds format limit".into());
        }
        let entry_size = 4usize
            .checked_add(key_length)
            .and_then(|length| length.checked_add(24 + 4))
            .ok_or("dictionary entry size overflow")?;
        if !self.pending.is_empty()
            && self.pending_size.saturating_add(entry_size) > TARGET_LEAF_BYTES
        {
            flush_leaf(
                &mut self.writer,
                &mut self.position,
                &mut self.pending,
                &self.key_arena,
                &mut self.blocks,
            )?;
            self.key_arena.copy_within(key_start.., 0);
            self.key_arena.truncate(key_length);
            // An exceptional long key should not pin its allocation for every
            // later ordinary leaf in the same dictionary.
            if self.key_arena.capacity() > 1024 * 1024 && key_length < TARGET_LEAF_BYTES {
                self.key_arena.shrink_to(TARGET_LEAF_BYTES);
            }
            self.pending_size = 4;
        }
        self.pending_size = self.pending_size
            .checked_add(entry_size)
            .ok_or("dictionary leaf size overflow")?;
        self.pending.push(PendingEntry {
            key_start: self.key_arena.len() - key_length,
            key_length,
            posting,
        });
        Ok(())
    }

    pub(crate) fn finish(mut self) -> Result<(), String> {
        flush_leaf(
            &mut self.writer,
            &mut self.position,
            &mut self.pending,
            &self.key_arena,
            &mut self.blocks,
        )?;
        let directory_offset = self.position;
        write_counted(
            &mut self.writer,
            &mut self.position,
            &u32::try_from(self.blocks.len())
                .map_err(|_| "dictionary block count overflow")?
                .to_le_bytes(),
        )?;
        for block in self.blocks {
            write_counted(&mut self.writer, &mut self.position, &block.offset.to_le_bytes())?;
            write_counted(&mut self.writer, &mut self.position, &block.length.to_le_bytes())?;
            write_counted(&mut self.writer, &mut self.position, &block.count.to_le_bytes())?;
            for bytes in [&block.first, &block.last] {
                write_counted(
                    &mut self.writer,
                    &mut self.position,
                    &u32::try_from(bytes.len())
                        .map_err(|_| "dictionary boundary key too large")?
                        .to_le_bytes(),
                )?;
                write_counted(&mut self.writer, &mut self.position, bytes)?;
            }
        }
        write_counted(
            &mut self.writer,
            &mut self.position,
            &directory_offset.to_le_bytes(),
        )?;
        Ok(())
    }
}

/// Bulk-load sorted keys while retaining at most one leaf and its directory.
pub(crate) fn write_ordered_dictionary<K: Ord + Clone + Serialize>(
    writer: &mut dyn Write,
    entries: impl IntoIterator<Item = (K, PostingDescriptor)>,
) -> Result<(), String> {
    let mut output = OrderedBlockWriter::<K, _>::new(writer, true);
    for (key, posting) in entries {
        output.push(&key, posting)?;
    }
    output.finish()
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct DictionaryCursor {
    block: usize,
    entry: usize,
}

/// The reader owns its byte source and keeps only sparse boundary keys. Open
/// validates all typed ordering, offsets and descriptors before an absence or
/// range boundary may affect a query result.
pub(crate) struct OrderedBlockDictionary<K> {
    reader: Arc<SegmentReader>,
    section: SectionKind,
    start: usize,
    length: usize,
    blocks: Vec<BlockMeta<K>>,
    entries: usize,
    _key: PhantomData<K>,
}

impl<K: Ord + Clone + DeserializeOwned> OrderedBlockDictionary<K> {
    pub(crate) fn open(
        reader: Arc<SegmentReader>,
        section: SectionKind,
        start: usize,
        length: usize,
    ) -> Result<Self, String> {
        Self::open_impl(reader, section, start, length, true)
    }

    /// Only for a dictionary just emitted by `write_ordered_dictionary` into
    /// this process's owned bytes. The writer already proved leaf ordering and
    /// descriptor construction; external bytes must use `open` instead.
    pub(crate) fn from_constructed(
        reader: Arc<SegmentReader>,
        section: SectionKind,
        start: usize,
        length: usize,
    ) -> Result<Self, String> {
        Self::open_impl(reader, section, start, length, false)
    }

    fn open_impl(
        reader: Arc<SegmentReader>,
        section: SectionKind,
        start: usize,
        length: usize,
        validate_leaves: bool,
    ) -> Result<Self, String> {
        let whole = reader
            .section(section)?
            .ok_or("dictionary section missing")?;
        let end = start
            .checked_add(length)
            .ok_or("dictionary span overflow")?;
        let bytes = whole
            .get(start..end)
            .ok_or("dictionary span out of bounds")?;
        if bytes.len() < 12 {
            return Err("dictionary footer truncated".into());
        }
        let footer = bytes.len() - 8;
        let directory_offset = usize::try_from(read_u64(bytes, footer)?)
            .map_err(|_| "dictionary directory offset too large")?;
        if directory_offset > footer - 4 {
            return Err("dictionary directory out of bounds".into());
        }
        let count = read_u32(bytes, directory_offset)? as usize;
        if count > MAX_BLOCKS || count > (footer - directory_offset - 4) / 24 {
            return Err("dictionary block count too large".into());
        }
        let mut position = directory_offset + 4;
        let mut blocks = Vec::with_capacity(count);
        for _ in 0..count {
            let offset = read_u64(bytes, position)?;
            let length = read_u32(bytes, position + 8)?;
            let entries = read_u32(bytes, position + 12)?;
            position += 16;
            let first = read_directory_key::<K>(bytes, &mut position, footer)?;
            let last = read_directory_key::<K>(bytes, &mut position, footer)?;
            if first > last || entries == 0 {
                return Err("invalid dictionary block boundaries".into());
            }
            blocks.push(BlockMeta {
                offset,
                length,
                count: entries,
                first,
                last,
            });
        }
        if position != footer || (count == 0 && directory_offset != 0) {
            return Err("dictionary directory trailing bytes".into());
        }
        let mut total = 0usize;
        let mut previous_end = 0usize;
        let mut previous_key: Option<K> = None;
        for block in &blocks {
            let offset = usize::try_from(block.offset).map_err(|_| "block offset too large")?;
            let end = offset
                .checked_add(block.length as usize)
                .ok_or("block span overflow")?;
            if offset != previous_end || end > directory_offset {
                return Err("dictionary blocks overlap or leave gaps".into());
            }
            if previous_key
                .as_ref()
                .is_some_and(|prior| prior >= &block.first)
            {
                return Err("dictionary block boundaries are unordered".into());
            }
            if !validate_leaves {
                previous_key = Some(block.last.clone());
                previous_end = end;
                total = total
                    .checked_add(block.count as usize)
                    .ok_or("dictionary entry count overflow")?;
                continue;
            }
            let leaf = &bytes[offset..end];
            if read_u32(leaf, 0)? != block.count {
                return Err("dictionary leaf count mismatch".into());
            }
            let table_end = 4usize
                .checked_add(
                    (block.count as usize)
                        .checked_mul(4)
                        .ok_or("leaf table overflow")?,
                )
                .ok_or("leaf table overflow")?;
            if table_end > leaf.len() {
                return Err("dictionary leaf table truncated".into());
            }
            let mut expected = table_end;
            let mut first: Option<K> = None;
            let mut last: Option<K> = None;
            for index in 0..block.count as usize {
                let offset = read_u32(leaf, 4 + index * 4)? as usize;
                if offset != expected {
                    return Err("dictionary leaf entry offset mismatch".into());
                }
                let (key, descriptor, next) = read_leaf_entry::<K>(leaf, offset)?;
                reader.validate_posting_descriptor(descriptor)?;
                if last.as_ref().is_some_and(|prior| prior >= &key) {
                    return Err("dictionary leaf keys are unordered".into());
                }
                if first.is_none() {
                    first = Some(key.clone());
                }
                last = Some(key);
                expected = next;
            }
            if expected != leaf.len()
                || first.as_ref() != Some(&block.first)
                || last.as_ref() != Some(&block.last)
                || previous_key
                    .as_ref()
                    .is_some_and(|prior| prior >= &block.first)
            {
                return Err("dictionary leaf boundary mismatch".into());
            }
            previous_key = last;
            previous_end = end;
            total = total
                .checked_add(block.count as usize)
                .ok_or("dictionary entry count overflow")?;
        }
        if previous_end != directory_offset {
            return Err("dictionary block extent mismatch".into());
        }
        Ok(Self {
            reader,
            section,
            start,
            length,
            blocks,
            entries: total,
            _key: PhantomData,
        })
    }

    pub(crate) fn len(&self) -> usize {
        self.entries
    }

    pub(crate) fn first(&self) -> Option<DictionaryCursor> {
        (!self.blocks.is_empty()).then_some(DictionaryCursor { block: 0, entry: 0 })
    }

    pub(crate) fn last(&self) -> Option<DictionaryCursor> {
        self.blocks.last().map(|block| DictionaryCursor {
            block: self.blocks.len() - 1,
            entry: block.count as usize - 1,
        })
    }

    pub(crate) fn get(&self, key: &K) -> Result<Option<PostingDescriptor>, String>
    where K: EncodedKey {
        self.get_with(key, K::compare_encoded)
    }

    /// Checked fallback for key types without a borrowed comparator.
    #[allow(dead_code)]
    pub(crate) fn get_decoded(&self, key: &K) -> Result<Option<PostingDescriptor>, String> {
        self.get_with(key, |bytes, target| Ok(decode_key::<K>(bytes)?.cmp(target)))
    }

    fn get_with(
        &self,
        key: &K,
        compare: impl Fn(&[u8], &K) -> Result<std::cmp::Ordering, String>,
    ) -> Result<Option<PostingDescriptor>, String> {
        let mut lo = 0usize;
        let mut hi = self.blocks.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.blocks[mid].last < *key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let Some(block) = self.blocks.get(lo) else {
            return Ok(None);
        };
        let leaf = self.leaf(block)?;
        let mut left = 0usize;
        let mut right = block.count as usize;
        while left < right {
            let mid = left + (right - left) / 2;
            let offset = read_u32(leaf, 4 + mid * 4)? as usize;
            let sample = entry_key_bytes(leaf, offset)?;
            match compare(sample, key)? {
                std::cmp::Ordering::Less => left = mid + 1,
                std::cmp::Ordering::Greater => right = mid,
                std::cmp::Ordering::Equal => {
                    let end = offset.checked_add(4 + sample.len() + 24)
                        .ok_or("dictionary entry overflow")?;
                    return Ok(Some(PostingDescriptor::decode(
                        leaf.get(end - 24..end)
                            .ok_or("dictionary posting descriptor truncated")?,
                    )?));
                }
            }
        }
        Ok(None)
    }

    pub(crate) fn lower_bound(&self, key: &K) -> Result<Option<DictionaryCursor>, String>
    where K: EncodedKey {
        self.lower_bound_with(key, K::compare_encoded)
    }

    #[allow(dead_code)]
    pub(crate) fn lower_bound_decoded(&self, key: &K) -> Result<Option<DictionaryCursor>, String> {
        self.lower_bound_with(key, |bytes, target| Ok(decode_key::<K>(bytes)?.cmp(target)))
    }

    fn lower_bound_with(
        &self,
        key: &K,
        compare: impl Fn(&[u8], &K) -> Result<std::cmp::Ordering, String>,
    ) -> Result<Option<DictionaryCursor>, String> {
        let mut lo = 0usize;
        let mut hi = self.blocks.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.blocks[mid].last < *key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let Some(block) = self.blocks.get(lo) else {
            return Ok(None);
        };
        let leaf = self.leaf(block)?;
        let mut left = 0usize;
        let mut right = block.count as usize;
        while left < right {
            let mid = left + (right - left) / 2;
            let offset = read_u32(leaf, 4 + mid * 4)? as usize;
            let sample = entry_key_bytes(leaf, offset)?;
            if compare(sample, key)?.is_lt() {
                left = mid + 1;
            } else {
                right = mid;
            }
        }
        if left == block.count as usize {
            return Ok((lo + 1 < self.blocks.len()).then_some(DictionaryCursor {
                block: lo + 1,
                entry: 0,
            }));
        }
        Ok(Some(DictionaryCursor {
            block: lo,
            entry: left,
        }))
    }

    pub(crate) fn entry(&self, cursor: DictionaryCursor) -> Result<(K, PostingDescriptor), String> {
        let block = self
            .blocks
            .get(cursor.block)
            .ok_or("dictionary cursor block missing")?;
        if cursor.entry >= block.count as usize {
            return Err("dictionary cursor entry missing".into());
        }
        let leaf = self.leaf(block)?;
        let offset = read_u32(leaf, 4 + cursor.entry * 4)? as usize;
        let (key, descriptor, _) = read_leaf_entry::<K>(leaf, offset)?;
        Ok((key, descriptor))
    }

    pub(crate) fn compare_at(
        &self,
        cursor: DictionaryCursor,
        key: &K,
    ) -> Result<std::cmp::Ordering, String>
    where K: EncodedKey {
        let block = self.blocks.get(cursor.block).ok_or("dictionary cursor block missing")?;
        if cursor.entry >= block.count as usize {
            return Err("dictionary cursor entry missing".into());
        }
        let leaf = self.leaf(block)?;
        let offset = read_u32(leaf, 4 + cursor.entry * 4)? as usize;
        K::compare_encoded(entry_key_bytes(leaf, offset)?, key)
    }

    /// Visit the encoded keys after the external opener has validated every
    /// leaf (or the internal writer has constructed them). Callers can inspect
    /// a borrowed key without allocating an owned value for each entry.
    pub(crate) fn visit_encoded(
        &self,
        mut visit: impl FnMut(&[u8], PostingDescriptor) -> Result<(), String>,
    ) -> Result<(), String> {
        for block in &self.blocks {
            let leaf = self.leaf(block)?;
            for index in 0..block.count as usize {
                let offset = read_u32(leaf, 4 + index * 4)? as usize;
                let bytes = entry_key_bytes(leaf, offset)?;
                let end = offset
                    .checked_add(4 + bytes.len() + 24)
                    .ok_or("dictionary entry overflow")?;
                let descriptor = PostingDescriptor::decode(
                    leaf.get(end - 24..end)
                        .ok_or("dictionary posting descriptor truncated")?,
                )?;
                visit(bytes, descriptor)?;
            }
        }
        Ok(())
    }

    pub(crate) fn next(&self, cursor: DictionaryCursor) -> Option<DictionaryCursor> {
        let block = self.blocks.get(cursor.block)?;
        if cursor.entry + 1 < block.count as usize {
            Some(DictionaryCursor {
                block: cursor.block,
                entry: cursor.entry + 1,
            })
        } else {
            (cursor.block + 1 < self.blocks.len()).then_some(DictionaryCursor {
                block: cursor.block + 1,
                entry: 0,
            })
        }
    }

    pub(crate) fn previous(&self, cursor: DictionaryCursor) -> Option<DictionaryCursor> {
        if cursor.entry > 0 {
            Some(DictionaryCursor {
                block: cursor.block,
                entry: cursor.entry - 1,
            })
        } else if cursor.block > 0 {
            Some(DictionaryCursor {
                block: cursor.block - 1,
                entry: self.blocks.get(cursor.block - 1)?.count as usize - 1,
            })
        } else {
            None
        }
    }

    fn leaf<'a>(&'a self, block: &BlockMeta<K>) -> Result<&'a [u8], String> {
        let whole = self
            .reader
            .section(self.section)?
            .ok_or("dictionary section missing")?;
        let dictionary = whole
            .get(self.start..self.start + self.length)
            .ok_or("dictionary span changed")?;
        let start = block.offset as usize;
        dictionary
            .get(start..start + block.length as usize)
            .ok_or("dictionary block changed".into())
    }
}

impl OrderedBlockDictionary<u64> {
    /// Bincode fixint stores a u64 key as eight little-endian bytes. External
    /// opening checked every typed leaf; constructed leaves came from the
    /// writer, so comparisons can borrow that validated representation.
    pub(crate) fn get_u64(&self, key: u64) -> Result<Option<PostingDescriptor>, String> {
        let mut lo = 0usize;
        let mut hi = self.blocks.len();
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if self.blocks[mid].last < key {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        let Some(block) = self.blocks.get(lo) else {
            return Ok(None);
        };
        let leaf = self.leaf(block)?;
        let mut left = 0usize;
        let mut right = block.count as usize;
        while left < right {
            let mid = left + (right - left) / 2;
            let offset = read_u32(leaf, 4 + mid * 4)? as usize;
            let encoded = entry_key_bytes(leaf, offset)?;
            let value = u64::from_le_bytes(
                encoded.try_into()
                    .map_err(|_| "validated u64 dictionary key length changed")?,
            );
            match value.cmp(&key) {
                std::cmp::Ordering::Less => left = mid + 1,
                std::cmp::Ordering::Greater => right = mid,
                std::cmp::Ordering::Equal => {
                    let end = offset.checked_add(4 + encoded.len() + 24)
                        .ok_or("dictionary entry overflow")?;
                    let descriptor = PostingDescriptor::decode(
                        leaf.get(end - 24..end)
                            .ok_or("dictionary posting descriptor truncated")?,
                    )?;
                    return Ok(Some(descriptor));
                }
            }
        }
        Ok(None)
    }
}

fn read_u32(bytes: &[u8], position: usize) -> Result<u32, String> {
    let end = position
        .checked_add(4)
        .ok_or("dictionary offset overflow")?;
    Ok(u32::from_le_bytes(
        bytes
            .get(position..end)
            .ok_or("dictionary integer truncated")?
            .try_into()
            .unwrap(),
    ))
}

fn read_u64(bytes: &[u8], position: usize) -> Result<u64, String> {
    let end = position
        .checked_add(8)
        .ok_or("dictionary offset overflow")?;
    Ok(u64::from_le_bytes(
        bytes
            .get(position..end)
            .ok_or("dictionary integer truncated")?
            .try_into()
            .unwrap(),
    ))
}

fn read_directory_key<K: DeserializeOwned>(
    bytes: &[u8],
    position: &mut usize,
    footer: usize,
) -> Result<K, String> {
    let length = read_u32(bytes, *position)? as usize;
    if length > MAX_KEY_BYTES {
        return Err("dictionary boundary key too large".into());
    }
    let begin = position.checked_add(4).ok_or("directory key overflow")?;
    let end = begin.checked_add(length).ok_or("directory key overflow")?;
    if end > footer {
        return Err("directory boundary key out of bounds".into());
    }
    *position = end;
    decode_key(&bytes[begin..end])
}

fn entry_key_bytes(leaf: &[u8], offset: usize) -> Result<&[u8], String> {
    let length = read_u32(leaf, offset)? as usize;
    if length > MAX_KEY_BYTES {
        return Err("dictionary key too large".into());
    }
    let begin = offset
        .checked_add(4)
        .ok_or("dictionary key offset overflow")?;
    let end = begin
        .checked_add(length)
        .ok_or("dictionary key offset overflow")?;
    leaf.get(begin..end)
        .ok_or("dictionary key out of bounds".into())
}

fn read_leaf_entry<K: DeserializeOwned>(
    leaf: &[u8],
    offset: usize,
) -> Result<(K, PostingDescriptor, usize), String> {
    let bytes = entry_key_bytes(leaf, offset)?;
    let end = offset
        .checked_add(4 + bytes.len() + 24)
        .ok_or("dictionary entry overflow")?;
    let descriptor = PostingDescriptor::decode(
        leaf.get(end - 24..end)
            .ok_or("dictionary posting descriptor truncated")?,
    )?;
    Ok((decode_key(bytes)?, descriptor, end))
}

#[cfg(test)]
#[path = "ordered_block.test.rs"]
mod tests;
