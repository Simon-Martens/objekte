//! Immutable ordered B+tree dictionaries and cursors.
//!
//! Pages target 8 KiB. Internal pages carry separator keys and child offsets;
//! leaves carry encoded keys and small value blobs. Keys and values too large
//! for a page live in a dictionary-local overflow arena. Readers descend only
//! the pages required by a lookup and keep one cursor stack per traversal.

use std::cmp::Ordering;
use std::ops::Bound;

use super::backing::Backing;
use super::error::{StorageError, StorageResult};
pub use super::scalar::KeyOrder;

/// Dictionary magic.
pub const DICTIONARY_MAGIC: [u8; 4] = *b"OBD1";
/// Dictionary format version.
pub const DICTIONARY_VERSION: u16 = 1;
/// Fixed dictionary header length.
pub const DICTIONARY_HEADER_LEN: usize = 96;
/// Default leaf/internal page target.
pub const DEFAULT_PAGE_SIZE: u32 = 8 * 1024;
/// Minimum accepted page size.
pub const MIN_PAGE_SIZE: u32 = 1024;
/// Maximum page size accepted by this build.
pub const MAX_PAGE_SIZE: u32 = 1024 * 1024;
/// Fixed page header length.
pub const PAGE_HEADER_LEN: usize = 32;
/// Maximum accepted page count depth.
pub const MAX_DEPTH: u32 = 32;

pub(crate) const PAGE_LEAF: u8 = 0;
pub(crate) const PAGE_INTERNAL: u8 = 1;
pub(crate) const BLOB_INLINE: u8 = 0;
pub(crate) const BLOB_OVERFLOW: u8 = 1;
pub(crate) const INLINE_BLOB_LIMIT: usize = 256;


/// Decoded dictionary header.
#[derive(Clone, Copy, Debug)]
pub struct DictionaryHeader {
    pub order: KeyOrder,
    pub page_size: u32,
    pub root_offset: u64,
    pub root_len: u32,
    pub depth: u32,
    pub entry_count: u64,
    pub key_bytes: u64,
    pub page_count: u64,
    pub overflow_offset: u64,
    pub overflow_len: u64,
}


impl DictionaryHeader {
    pub(crate) fn new(
        order: KeyOrder,
        page_size: u32,
        root_offset: u64,
        root_len: u32,
        depth: u32,
        entry_count: u64,
        key_bytes: u64,
        page_count: u64,
        overflow_offset: u64,
        overflow_len: u64,
    ) -> Self {
        Self {
            order,
            page_size,
            root_offset,
            root_len,
            depth,
            entry_count,
            key_bytes,
            page_count,
            overflow_offset,
            overflow_len,
        }
    }

    pub(crate) fn write(self, destination: &mut [u8]) {
        destination[..4].copy_from_slice(&DICTIONARY_MAGIC);
        destination[4..6].copy_from_slice(&DICTIONARY_VERSION.to_le_bytes());
        destination[6] = self.order.code();
        destination[7] = 0;
        destination[8..12].copy_from_slice(&self.page_size.to_le_bytes());
        destination[12..16].fill(0);
        destination[16..24].copy_from_slice(&self.root_offset.to_le_bytes());
        destination[24..28].copy_from_slice(&self.root_len.to_le_bytes());
        destination[28..32].copy_from_slice(&self.depth.to_le_bytes());
        destination[32..40].copy_from_slice(&self.entry_count.to_le_bytes());
        destination[40..48].copy_from_slice(&self.key_bytes.to_le_bytes());
        destination[48..56].copy_from_slice(&self.page_count.to_le_bytes());
        destination[56..64].copy_from_slice(&self.overflow_offset.to_le_bytes());
        destination[64..72].copy_from_slice(&self.overflow_len.to_le_bytes());
        destination[72..].fill(0);
    }

    pub(crate) fn parse(bytes: &[u8], dictionary_len: u64) -> StorageResult<Self> {
        if bytes.len() < DICTIONARY_HEADER_LEN {
            return Err(StorageError::corrupt("truncated dictionary header"));
        }
        if bytes[..4] != DICTIONARY_MAGIC {
            return Err(StorageError::corrupt("dictionary magic mismatch"));
        }
        let version = u16::from_le_bytes([bytes[4], bytes[5]]);
        if version != DICTIONARY_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "dictionary version {version}"
            )));
        }
        let order = KeyOrder::from_code(bytes[6])?;
        if bytes[7] != 0 || bytes[12..16].iter().any(|byte| *byte != 0) {
            return Err(StorageError::corrupt(
                "dictionary header reserved bytes are not zero",
            ));
        }
        let page_size = u32::from_le_bytes(bytes[8..12].try_into().expect("four-byte page size"));
        if !(MIN_PAGE_SIZE..=MAX_PAGE_SIZE).contains(&page_size) {
            return Err(StorageError::corrupt(
                "dictionary page size is out of range",
            ));
        }
        let root_offset =
            u64::from_le_bytes(bytes[16..24].try_into().expect("eight-byte root offset"));
        let root_len = u32::from_le_bytes(bytes[24..28].try_into().expect("four-byte root length"));
        let depth = u32::from_le_bytes(bytes[28..32].try_into().expect("four-byte depth"));
        if depth > MAX_DEPTH {
            return Err(StorageError::corrupt(
                "dictionary depth exceeds the supported maximum",
            ));
        }
        let entry_count =
            u64::from_le_bytes(bytes[32..40].try_into().expect("eight-byte entry count"));
        let key_bytes = u64::from_le_bytes(bytes[40..48].try_into().expect("eight-byte key bytes"));
        let page_count =
            u64::from_le_bytes(bytes[48..56].try_into().expect("eight-byte page count"));
        let overflow_offset = u64::from_le_bytes(
            bytes[56..64]
                .try_into()
                .expect("eight-byte overflow offset"),
        );
        let overflow_len = u64::from_le_bytes(
            bytes[64..72]
                .try_into()
                .expect("eight-byte overflow length"),
        );
        if bytes[72..].iter().any(|byte| *byte != 0) {
            return Err(StorageError::corrupt(
                "dictionary header reserved bytes are not zero",
            ));
        }
        if root_offset < DICTIONARY_HEADER_LEN as u64
            || root_len < PAGE_HEADER_LEN as u32
            || root_offset
                .checked_add(root_len as u64)
                .is_none_or(|end| end > dictionary_len)
        {
            return Err(StorageError::corrupt("dictionary root is out of bounds"));
        }
        if overflow_offset < DICTIONARY_HEADER_LEN as u64
            || overflow_offset
                .checked_add(overflow_len)
                .is_none_or(|end| end > dictionary_len)
        {
            return Err(StorageError::corrupt(
                "dictionary overflow arena is out of bounds",
            ));
        }
        if overflow_offset < root_offset.saturating_add(root_len as u64) {
            return Err(StorageError::corrupt(
                "dictionary overflow arena overlaps page storage",
            ));
        }
        Ok(Self {
            order,
            page_size,
            root_offset,
            root_len,
            depth,
            entry_count,
            key_bytes,
            page_count,
            overflow_offset,
            overflow_len,
        })
    }
}


/// One page decoded from the backing.
#[derive(Clone, Debug)]
struct Page {
    kind: u8,
    count: u16,
    next: u64,
    #[allow(dead_code)]
    previous: u64,
    data: Vec<u8>,
}


impl Page {
    fn offsets(&self) -> StorageResult<&[u8]> {
        let count = self.count as usize;
        let end = PAGE_HEADER_LEN
            .checked_add(
                count
                    .checked_mul(4)
                    .ok_or_else(|| StorageError::corrupt("page offset array overflow"))?,
            )
            .ok_or_else(|| StorageError::corrupt("page offset array overflow"))?;
        self.data
            .get(PAGE_HEADER_LEN..end)
            .ok_or_else(|| StorageError::corrupt("truncated page offset array"))
    }

    fn entry_offset(&self, index: usize) -> StorageResult<usize> {
        if index >= self.count as usize {
            return Err(StorageError::corrupt("page entry index out of bounds"));
        }
        let offsets = self.offsets()?;
        let at = index * 4;
        let offset = u32::from_le_bytes(
            offsets[at..at + 4]
                .try_into()
                .expect("four-byte page entry offset"),
        ) as usize;
        let header_end = PAGE_HEADER_LEN + self.count as usize * 4;
        if offset < header_end || offset >= self.data.len() {
            return Err(StorageError::corrupt("page entry offset is out of bounds"));
        }
        Ok(offset)
    }
}


/// One immutable ordered dictionary over a checked backing.
pub struct DictionaryReader {
    backing: Backing,
    base: u64,
    len: u64,
    header: DictionaryHeader,
}


impl std::fmt::Debug for DictionaryReader {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("DictionaryReader")
            .field("base", &self.base)
            .field("len", &self.len)
            .field("header", &self.header)
            .finish_non_exhaustive()
    }
}


impl DictionaryReader {
    /// Open the dictionary at `base..base + len` in one backing.
    pub fn open(backing: Backing, base: u64, len: u64) -> StorageResult<Self> {
        let end = base
            .checked_add(len)
            .ok_or_else(|| StorageError::corrupt("dictionary end overflow"))?;
        if end > backing.len() {
            return Err(StorageError::corrupt("dictionary lies outside its backing"));
        }
        let header_bytes = backing.read(base, DICTIONARY_HEADER_LEN as u64)?;
        let header = DictionaryHeader::parse(header_bytes, len)?;
        let reader = Self {
            backing,
            base,
            len,
            header,
        };
        let root = reader.read_page(header.root_offset)?;
        let expected = if header.depth == 0 {
            PAGE_LEAF
        } else {
            PAGE_INTERNAL
        };
        if root.kind != expected {
            return Err(StorageError::corrupt(
                "dictionary root page kind does not match its depth",
            ));
        }
        if root.count as u64 > header.entry_count && header.depth == 0 {
            return Err(StorageError::corrupt(
                "dictionary root has more entries than its header",
            ));
        }
        Ok(reader)
    }

    pub fn header(&self) -> DictionaryHeader {
        self.header
    }

    pub fn order(&self) -> KeyOrder {
        self.header.order
    }

    pub fn entry_count(&self) -> u64 {
        self.header.entry_count
    }

    pub fn key_bytes(&self) -> u64 {
        self.header.key_bytes
    }

    pub fn page_count(&self) -> u64 {
        self.header.page_count
    }

    pub fn is_empty(&self) -> bool {
        self.header.entry_count == 0
    }

    pub fn get(&self, key: &[u8]) -> StorageResult<Option<Vec<u8>>> {
        let Some((found_key, value)) = self.lower_bound(key)? else {
            return Ok(None);
        };
        if self.header.order.compare(&found_key, key)? == Ordering::Equal {
            Ok(Some(value))
        } else {
            Ok(None)
        }
    }

    pub fn get_entry(&self, key: &[u8]) -> StorageResult<Option<(Vec<u8>, Vec<u8>)>> {
        let Some((found_key, value)) = self.lower_bound(key)? else {
            return Ok(None);
        };
        if self.header.order.compare(&found_key, key)? == Ordering::Equal {
            Ok(Some((found_key, value)))
        } else {
            Ok(None)
        }
    }

    pub fn contains_key(&self, key: &[u8]) -> StorageResult<bool> {
        Ok(self.get(key)?.is_some())
    }

    /// First entry whose key is greater than or equal to `key`.
    pub fn lower_bound(&self, key: &[u8]) -> StorageResult<Option<(Vec<u8>, Vec<u8>)>> {
        let mut cursor = self.cursor();
        cursor.seek_forward(key, false)?;
        cursor
            .current()
            .map(|(key, value)| Ok((key.to_vec(), value.to_vec())))
            .transpose()
    }

    /// First entry whose key is strictly greater than `key`.
    pub fn upper_bound(&self, key: &[u8]) -> StorageResult<Option<(Vec<u8>, Vec<u8>)>> {
        let mut cursor = self.cursor();
        cursor.seek_forward(key, true)?;
        cursor
            .current()
            .map(|(key, value)| Ok((key.to_vec(), value.to_vec())))
            .transpose()
    }

    pub fn first(&self) -> StorageResult<Option<(Vec<u8>, Vec<u8>)>> {
        let mut cursor = self.cursor();
        cursor.first()?;
        cursor
            .current()
            .map(|(key, value)| Ok((key.to_vec(), value.to_vec())))
            .transpose()
    }

    pub fn last(&self) -> StorageResult<Option<(Vec<u8>, Vec<u8>)>> {
        let mut cursor = self.cursor();
        cursor.last()?;
        cursor
            .current()
            .map(|(key, value)| Ok((key.to_vec(), value.to_vec())))
            .transpose()
    }

    pub fn cursor(&self) -> DictionaryCursor<'_> {
        DictionaryCursor {
            reader: self,
            stack: Vec::new(),
            current: None,
        }
    }

    /// Iterate one direction over a bounded key range.
    pub fn range<'a>(&'a self, lower: Bound<&[u8]>, upper: Bound<&[u8]>) -> DictionaryRange<'a> {
        DictionaryRange {
            cursor: self.cursor(),
            upper: owned_bound(upper),
            reverse: false,
            started: false,
            done: false,
            start: Some((owned_bound(lower), owned_bound(upper))),
        }
    }

    /// Reverse iteration over a bounded key range.
    pub fn range_reverse<'a>(
        &'a self,
        lower: Bound<&[u8]>,
        upper: Bound<&[u8]>,
    ) -> DictionaryRange<'a> {
        DictionaryRange {
            cursor: self.cursor(),
            upper: owned_bound(lower),
            reverse: true,
            started: false,
            done: false,
            start: Some((owned_bound(lower), owned_bound(upper))),
        }
    }

    /// Walk every entry and verify page links, key ordering, and the stored
    /// entry count. This is intentionally explicit, not part of load.
    pub fn verify_full(&self) -> StorageResult<()> {
        let mut cursor = self.cursor();
        cursor.first()?;
        let mut previous: Option<Vec<u8>> = None;
        let mut count = 0u64;
        while let Some((key, _)) = cursor.current() {
            if let Some(previous) = &previous {
                if self.header.order.compare(previous, key)? != Ordering::Less {
                    return Err(StorageError::corrupt(
                        "dictionary keys are not strictly increasing",
                    ));
                }
            }
            previous = Some(key.to_vec());
            count += 1;
            cursor.advance_forward()?;
        }
        if count != self.header.entry_count {
            return Err(StorageError::corrupt(format!(
                "dictionary entry count {count} differs from header {}",
                self.header.entry_count
            )));
        }
        Ok(())
    }

    fn read_page(&self, page_offset: u64) -> StorageResult<Page> {
        if page_offset
            .checked_add(PAGE_HEADER_LEN as u64)
            .is_none_or(|end| end > self.len)
        {
            return Err(StorageError::corrupt(
                "dictionary page offset is out of bounds",
            ));
        }
        let header = self
            .backing
            .read(self.checked_offset(page_offset)?, PAGE_HEADER_LEN as u64)?;
        let kind = header[0];
        if kind != PAGE_LEAF && kind != PAGE_INTERNAL {
            return Err(StorageError::corrupt(format!(
                "unknown dictionary page kind {kind}"
            )));
        }
        let count = u16::from_le_bytes([header[1], header[2]]);
        if header[3..8].iter().any(|byte| *byte != 0) {
            return Err(StorageError::corrupt(
                "dictionary page header reserved bytes are not zero",
            ));
        }
        let next = u64::from_le_bytes(header[8..16].try_into().expect("eight-byte page link"));
        let previous = u64::from_le_bytes(header[16..24].try_into().expect("eight-byte page link"));
        let data_len =
            u32::from_le_bytes(header[24..28].try_into().expect("four-byte page length")) as usize;
        if header[28..32].iter().any(|byte| *byte != 0) {
            return Err(StorageError::corrupt(
                "dictionary page header reserved bytes are not zero",
            ));
        }
        if data_len < PAGE_HEADER_LEN || data_len > MAX_PAGE_SIZE as usize {
            return Err(StorageError::corrupt(
                "dictionary page length is out of range",
            ));
        }
        if page_offset
            .checked_add(data_len as u64)
            .is_none_or(|end| end > self.len)
        {
            return Err(StorageError::corrupt("dictionary page exceeds its section"));
        }
        let data = self
            .backing
            .read(self.checked_offset(page_offset)?, data_len as u64)?
            .to_vec();
        let page = Page {
            kind,
            count,
            next,
            previous,
            data,
        };
        let offsets = page.offsets()?;
        let mut previous_offset = PAGE_HEADER_LEN + count as usize * 4;
        for chunk in offsets.chunks_exact(4) {
            let offset =
                u32::from_le_bytes(chunk.try_into().expect("four-byte page offset")) as usize;
            if offset < previous_offset || offset >= data_len {
                return Err(StorageError::corrupt(
                    "dictionary page entry offsets are not increasing",
                ));
            }
            previous_offset = offset;
        }
        if kind == PAGE_INTERNAL && count == u16::MAX {
            return Err(StorageError::corrupt(
                "dictionary internal page has an impossible key count",
            ));
        }
        Ok(page)
    }

    fn checked_offset(&self, relative: u64) -> StorageResult<u64> {
        let offset = self
            .base
            .checked_add(relative)
            .ok_or_else(|| StorageError::corrupt("dictionary offset overflow"))?;
        if relative >= self.len {
            return Err(StorageError::corrupt(
                "dictionary relative offset is out of bounds",
            ));
        }
        Ok(offset)
    }

    fn leaf_entry(&self, page: &Page, index: usize) -> StorageResult<(Vec<u8>, Vec<u8>)> {
        if page.kind != PAGE_LEAF {
            return Err(StorageError::corrupt("expected a leaf page"));
        }
        let offset = page.entry_offset(index)?;
        let (key, value, _) = self.decode_entry(page, offset, true)?;
        Ok((key, value))
    }

    /// Decode one leaf or internal entry. Returns `(key, value, child)`; value
    /// is empty for internal entries.
    fn decode_entry(
        &self,
        page: &Page,
        offset: usize,
        has_value: bool,
    ) -> StorageResult<(Vec<u8>, Vec<u8>, Option<u64>)> {
        let mut at = offset;
        let child = if page.kind == PAGE_INTERNAL {
            let child = read_u64(&page.data, at, "internal child offset")?;
            at += 8;
            Some(child)
        } else {
            None
        };
        let key_len = read_u32(&page.data, at, "entry key length")? as usize;
        at += 4;
        let key_kind = *page
            .data
            .get(at)
            .ok_or_else(|| StorageError::corrupt("truncated entry key kind"))?;
        at += 1;
        if page.data.get(at).is_none() {
            return Err(StorageError::corrupt("truncated entry flags"));
        }
        at += 3;
        let value_len = if has_value {
            let value_len = read_u32(&page.data, at, "entry value length")? as usize;
            at += 4;
            value_len
        } else {
            0
        };
        let key = match key_kind {
            BLOB_INLINE => {
                let end = at
                    .checked_add(key_len)
                    .ok_or_else(|| StorageError::corrupt("entry key end overflow"))?;
                let key = page
                    .data
                    .get(at..end)
                    .ok_or_else(|| StorageError::corrupt("truncated inline entry key"))?;
                at = end;
                key.to_vec()
            }
            BLOB_OVERFLOW => {
                let relative = read_u64(&page.data, at, "overflow key offset")?;
                at += 8;
                self.read_overflow(relative, key_len)?
            }
            other => {
                return Err(StorageError::corrupt(format!(
                    "unknown entry key encoding {other}"
                )));
            }
        };
        let value = if has_value {
            let value_kind = *page
                .data
                .get(at)
                .ok_or_else(|| StorageError::corrupt("truncated entry value kind"))?;
            at += 1;
            if page.data.get(at).is_none()
                || page.data.get(at + 1).is_none()
                || page.data.get(at + 2).is_none()
            {
                return Err(StorageError::corrupt("truncated entry value flags"));
            }
            at += 3;
            match value_kind {
                BLOB_INLINE => {
                    let end = at
                        .checked_add(value_len)
                        .ok_or_else(|| StorageError::corrupt("entry value end overflow"))?;
                    let value = page
                        .data
                        .get(at..end)
                        .ok_or_else(|| StorageError::corrupt("truncated inline entry value"))?;
                    at = end;
                    value.to_vec()
                }
                BLOB_OVERFLOW => {
                    let relative = read_u64(&page.data, at, "overflow value offset")?;
                    at += 8;
                    self.read_overflow(relative, value_len)?
                }
                other => {
                    return Err(StorageError::corrupt(format!(
                        "unknown entry value encoding {other}"
                    )));
                }
            }
        } else {
            Vec::new()
        };
        if at > page.data.len() {
            return Err(StorageError::corrupt("entry extends past its page"));
        }
        Ok((key, value, child))
    }

    fn read_overflow(&self, relative: u64, length: usize) -> StorageResult<Vec<u8>> {
        let end = relative
            .checked_add(length as u64)
            .ok_or_else(|| StorageError::corrupt("overflow key end overflow"))?;
        if end > self.header.overflow_len {
            return Err(StorageError::corrupt(
                "dictionary overflow reference is out of bounds",
            ));
        }
        let absolute = self
            .header
            .overflow_offset
            .checked_add(relative)
            .ok_or_else(|| StorageError::corrupt("overflow absolute offset overflow"))?;
        Ok(self
            .backing
            .read(self.checked_offset(absolute)?, length as u64)?
            .to_vec())
    }

    fn internal_child(&self, page: &Page, child_index: usize) -> StorageResult<u64> {
        if page.kind != PAGE_INTERNAL {
            return Err(StorageError::corrupt("expected an internal page"));
        }
        let count = page.count as usize;
        if child_index > count {
            return Err(StorageError::corrupt("internal child index out of bounds"));
        }
        if child_index == count {
            return Ok(page.next);
        }
        let offset = page.entry_offset(child_index)?;
        let (_, _, child) = self.decode_entry(page, offset, false)?;
        child.ok_or_else(|| StorageError::corrupt("internal entry has no child"))
    }

    fn internal_key(&self, page: &Page, key_index: usize) -> StorageResult<Vec<u8>> {
        if page.kind != PAGE_INTERNAL || key_index >= page.count as usize {
            return Err(StorageError::corrupt("internal key index out of bounds"));
        }
        let offset = page.entry_offset(key_index)?;
        let (key, _, _) = self.decode_entry(page, offset, false)?;
        Ok(key)
    }

    fn child_for_key(&self, page: &Page, key: &[u8]) -> StorageResult<usize> {
        let count = page.count as usize;
        let mut low = 0usize;
        let mut high = count;
        while low < high {
            let middle = low + (high - low) / 2;
            let separator = self.internal_key(page, middle)?;
            if self.header.order.compare(&separator, key)? == Ordering::Less {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Ok(low)
    }

    fn leaf_position(&self, page: &Page, key: &[u8], strict: bool) -> StorageResult<usize> {
        let count = page.count as usize;
        let mut low = 0usize;
        let mut high = count;
        while low < high {
            let middle = low + (high - low) / 2;
            let (found, _) = self.leaf_entry(page, middle)?;
            let ordering = self.header.order.compare(&found, key)?;
            let before = ordering == Ordering::Less || (strict && ordering == Ordering::Equal);
            if before {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Ok(low)
    }
}


/// One cursor stack over a dictionary.
pub struct DictionaryCursor<'a> {
    reader: &'a DictionaryReader,
    stack: Vec<Frame>,
    current: Option<(Vec<u8>, Vec<u8>)>,
}


#[derive(Clone, Copy)]
enum Frame {
    Internal {
        offset: u64,
        child: usize,
        key_count: usize,
    },
    Leaf {
        offset: u64,
        index: usize,
    },
}


impl<'a> DictionaryCursor<'a> {
    pub fn current(&self) -> Option<(&[u8], &[u8])> {
        self.current
            .as_ref()
            .map(|(key, value)| (key.as_slice(), value.as_slice()))
    }

    pub fn current_key(&self) -> Option<&[u8]> {
        self.current.as_ref().map(|(key, _)| key.as_slice())
    }

    pub fn current_value(&self) -> Option<&[u8]> {
        self.current.as_ref().map(|(_, value)| value.as_slice())
    }

    /// Position at the first entry.
    pub fn first(&mut self) -> StorageResult<()> {
        self.stack.clear();
        let mut offset = self.reader.header.root_offset;
        for _ in 0..self.reader.header.depth {
            let page = self.reader.read_page(offset)?;
            if page.kind != PAGE_INTERNAL {
                return Err(StorageError::corrupt(
                    "dictionary descent reached a leaf before its declared depth",
                ));
            }
            let key_count = page.count as usize;
            self.stack.push(Frame::Internal {
                offset,
                child: 0,
                key_count,
            });
            offset = self.reader.internal_child(&page, 0)?;
        }
        let leaf = self.reader.read_page(offset)?;
        if leaf.kind != PAGE_LEAF {
            return Err(StorageError::corrupt(
                "dictionary descent did not reach a leaf",
            ));
        }
        self.stack.push(Frame::Leaf { offset, index: 0 });
        self.attach_leaf(offset, 0)
    }

    /// Position at the last entry.
    pub fn last(&mut self) -> StorageResult<()> {
        self.stack.clear();
        let mut offset = self.reader.header.root_offset;
        for _ in 0..self.reader.header.depth {
            let page = self.reader.read_page(offset)?;
            if page.kind != PAGE_INTERNAL {
                return Err(StorageError::corrupt(
                    "dictionary descent reached a leaf before its declared depth",
                ));
            }
            let key_count = page.count as usize;
            let child = key_count;
            self.stack.push(Frame::Internal {
                offset,
                child,
                key_count,
            });
            offset = self.reader.internal_child(&page, child)?;
        }
        let leaf = self.reader.read_page(offset)?;
        if leaf.kind != PAGE_LEAF {
            return Err(StorageError::corrupt(
                "dictionary descent did not reach a leaf",
            ));
        }
        let index = leaf.count.saturating_sub(1) as usize;
        self.stack.push(Frame::Leaf { offset, index });
        if leaf.count == 0 {
            self.current = None;
            Ok(())
        } else {
            self.attach_leaf(offset, index)
        }
    }

    /// Position at the first entry at or after `key`, or strictly after it.
    pub fn seek_forward(&mut self, key: &[u8], strict: bool) -> StorageResult<()> {
        self.stack.clear();
        let mut offset = self.reader.header.root_offset;
        for _ in 0..self.reader.header.depth {
            let page = self.reader.read_page(offset)?;
            if page.kind != PAGE_INTERNAL {
                return Err(StorageError::corrupt(
                    "dictionary descent reached a leaf before its declared depth",
                ));
            }
            let child = self.reader.child_for_key(&page, key)?;
            self.stack.push(Frame::Internal {
                offset,
                child,
                key_count: page.count as usize,
            });
            offset = self.reader.internal_child(&page, child)?;
        }
        let leaf = self.reader.read_page(offset)?;
        let index = self.reader.leaf_position(&leaf, key, strict)?;
        self.stack.push(Frame::Leaf { offset, index });
        if index < leaf.count as usize {
            self.attach_leaf(offset, index)
        } else {
            self.current = None;
            self.advance_past_leaf()
        }
    }

    /// Position at the last entry at or before `key`, or strictly before it.
    pub fn seek_backward(&mut self, key: &[u8], strict: bool) -> StorageResult<()> {
        self.stack.clear();
        let mut offset = self.reader.header.root_offset;
        for _ in 0..self.reader.header.depth {
            let page = self.reader.read_page(offset)?;
            if page.kind != PAGE_INTERNAL {
                return Err(StorageError::corrupt(
                    "dictionary descent reached a leaf before its declared depth",
                ));
            }
            let child = self.reader.child_for_key(&page, key)?;
            self.stack.push(Frame::Internal {
                offset,
                child,
                key_count: page.count as usize,
            });
            offset = self.reader.internal_child(&page, child)?;
        }
        let leaf = self.reader.read_page(offset)?;
        let insertion = self.reader.leaf_position(&leaf, key, false)?;
        let index = if strict {
            insertion.checked_sub(1)
        } else if insertion < leaf.count as usize {
            let (found, _) = self.reader.leaf_entry(&leaf, insertion)?;
            if self.reader.header.order.compare(&found, key)? == Ordering::Equal {
                Some(insertion)
            } else {
                insertion.checked_sub(1)
            }
        } else {
            insertion.checked_sub(1)
        };
        let Some(index) = index else {
            self.stack.push(Frame::Leaf { offset, index: 0 });
            self.current = None;
            return self.retreat_before_leaf();
        };
        self.stack.push(Frame::Leaf { offset, index });
        self.attach_leaf(offset, index)
    }

    /// Advance to the next entry.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> StorageResult<bool> {
        if self.current.is_some() {
            self.advance_forward()?;
        }
        Ok(self.current.is_some())
    }

    /// Move to the previous entry.
    pub fn previous(&mut self) -> StorageResult<bool> {
        if self.current.is_some() {
            self.advance_backward()?;
        }
        Ok(self.current.is_some())
    }

    fn attach_leaf(&mut self, offset: u64, index: usize) -> StorageResult<()> {
        let page = self.reader.read_page(offset)?;
        if page.kind != PAGE_LEAF {
            return Err(StorageError::corrupt("cursor expected a leaf page"));
        }
        if index >= page.count as usize {
            self.current = None;
            return Ok(());
        }
        self.current = Some(self.reader.leaf_entry(&page, index)?);
        Ok(())
    }

    fn advance_forward(&mut self) -> StorageResult<()> {
        let Some(Frame::Leaf { offset, index }) = self.stack.pop() else {
            self.current = None;
            return Ok(());
        };
        let page = self.reader.read_page(offset)?;
        let next = index + 1;
        if next < page.count as usize {
            self.stack.push(Frame::Leaf {
                offset,
                index: next,
            });
            return self.attach_leaf(offset, next);
        }
        self.current = None;
        loop {
            match self.stack.last_mut() {
                Some(Frame::Internal {
                    offset,
                    child,
                    key_count,
                }) => {
                    if *child < *key_count {
                        *child += 1;
                        let parent = self.reader.read_page(*offset)?;
                        let next_child = self.reader.internal_child(&parent, *child)?;
                        return self.descend_leftmost(next_child);
                    }
                    self.stack.pop();
                }
                Some(Frame::Leaf { .. }) => {
                    self.stack.pop();
                }
                None => return Ok(()),
            }
        }
    }

    fn advance_backward(&mut self) -> StorageResult<()> {
        let Some(Frame::Leaf { offset, index }) = self.stack.pop() else {
            self.current = None;
            return Ok(());
        };
        if index > 0 {
            let next = index - 1;
            self.stack.push(Frame::Leaf {
                offset,
                index: next,
            });
            return self.attach_leaf(offset, next);
        }
        self.current = None;
        loop {
            match self.stack.last_mut() {
                Some(Frame::Internal {
                    offset,
                    child,
                    key_count: _,
                }) => {
                    if *child > 0 {
                        *child -= 1;
                        let parent = self.reader.read_page(*offset)?;
                        let next_child = self.reader.internal_child(&parent, *child)?;
                        return self.descend_rightmost(next_child);
                    }
                    self.stack.pop();
                }
                Some(Frame::Leaf { .. }) => {
                    self.stack.pop();
                }
                None => return Ok(()),
            }
        }
    }

    fn advance_past_leaf(&mut self) -> StorageResult<()> {
        loop {
            match self.stack.last_mut() {
                Some(Frame::Internal {
                    offset,
                    child,
                    key_count,
                }) => {
                    if *child < *key_count {
                        *child += 1;
                        let parent = self.reader.read_page(*offset)?;
                        let next_child = self.reader.internal_child(&parent, *child)?;
                        return self.descend_leftmost(next_child);
                    }
                    self.stack.pop();
                }
                Some(Frame::Leaf { .. }) => {
                    self.stack.pop();
                }
                None => return Ok(()),
            }
        }
    }

    fn retreat_before_leaf(&mut self) -> StorageResult<()> {
        loop {
            match self.stack.last_mut() {
                Some(Frame::Internal { offset, child, .. }) => {
                    if *child > 0 {
                        *child -= 1;
                        let parent = self.reader.read_page(*offset)?;
                        let next_child = self.reader.internal_child(&parent, *child)?;
                        return self.descend_rightmost(next_child);
                    }
                    self.stack.pop();
                }
                Some(Frame::Leaf { .. }) => {
                    self.stack.pop();
                }
                None => return Ok(()),
            }
        }
    }

    fn descend_leftmost(&mut self, mut offset: u64) -> StorageResult<()> {
        let leaf_page = self.reader.read_page(offset)?;
        if leaf_page.kind == PAGE_LEAF {
            self.stack.push(Frame::Leaf { offset, index: 0 });
            return self.attach_leaf(offset, 0);
        }
        loop {
            let page = self.reader.read_page(offset)?;
            if page.kind == PAGE_LEAF {
                self.stack.push(Frame::Leaf { offset, index: 0 });
                return self.attach_leaf(offset, 0);
            }
            self.stack.push(Frame::Internal {
                offset,
                child: 0,
                key_count: page.count as usize,
            });
            offset = self.reader.internal_child(&page, 0)?;
        }
    }

    fn descend_rightmost(&mut self, mut offset: u64) -> StorageResult<()> {
        loop {
            let page = self.reader.read_page(offset)?;
            if page.kind == PAGE_LEAF {
                let index = page.count.saturating_sub(1) as usize;
                self.stack.push(Frame::Leaf { offset, index });
                if page.count == 0 {
                    self.current = None;
                    return Ok(());
                }
                return self.attach_leaf(offset, index);
            }
            let child = page.count as usize;
            self.stack.push(Frame::Internal {
                offset,
                child,
                key_count: page.count as usize,
            });
            offset = self.reader.internal_child(&page, child)?;
        }
    }
}


fn read_u32(bytes: &[u8], at: usize, context: &str) -> StorageResult<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| StorageError::corrupt(format!("{context} offset overflow")))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt(format!("truncated {context}")))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte checked slice"),
    ))
}

fn read_u64(bytes: &[u8], at: usize, context: &str) -> StorageResult<u64> {
    let end = at
        .checked_add(8)
        .ok_or_else(|| StorageError::corrupt(format!("{context} offset overflow")))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt(format!("truncated {context}")))?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("eight-byte checked slice"),
    ))
}

/// Owned keys for a range bound so the iterator cannot borrow caller data.
fn owned_bound(bound: Bound<&[u8]>) -> Bound<Vec<u8>> {
    match bound {
        Bound::Included(value) => Bound::Included(value.to_vec()),
        Bound::Excluded(value) => Bound::Excluded(value.to_vec()),
        Bound::Unbounded => Bound::Unbounded,
    }
}


/// Forward or reverse iterator over a bounded dictionary range.
pub struct DictionaryRange<'a> {
    cursor: DictionaryCursor<'a>,
    upper: Bound<Vec<u8>>,
    reverse: bool,
    started: bool,
    done: bool,
    start: Option<(Bound<Vec<u8>>, Bound<Vec<u8>>)>,
}


impl Iterator for DictionaryRange<'_> {
    type Item = StorageResult<(Vec<u8>, Vec<u8>)>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done {
            return None;
        }
        if !self.started {
            self.started = true;
            let Some((lower, upper)) = self.start.take() else {
                self.done = true;
                return None;
            };
            let result = if self.reverse {
                match upper {
                    Bound::Unbounded => self.cursor.last(),
                    Bound::Included(key) => self.cursor.seek_backward(&key, false),
                    Bound::Excluded(key) => self.cursor.seek_backward(&key, true),
                }
            } else {
                match lower {
                    Bound::Unbounded => self.cursor.first(),
                    Bound::Included(key) => self.cursor.seek_forward(&key, false),
                    Bound::Excluded(key) => self.cursor.seek_forward(&key, true),
                }
            };
            if let Err(error) = result {
                self.done = true;
                return Some(Err(error));
            }
        }
        let Some((key, value)) = self.cursor.current() else {
            self.done = true;
            return None;
        };
        let in_range = if self.reverse {
            match &self.upper {
                Bound::Unbounded => true,
                Bound::Included(lower) => {
                    matches!(
                        self.cursor.reader.header.order.compare(key, lower),
                        Ok(Ordering::Greater | Ordering::Equal)
                    )
                }
                Bound::Excluded(lower) => {
                    matches!(
                        self.cursor.reader.header.order.compare(key, lower),
                        Ok(Ordering::Greater)
                    )
                }
            }
        } else {
            match &self.upper {
                Bound::Unbounded => true,
                Bound::Included(upper) => {
                    matches!(
                        self.cursor.reader.header.order.compare(key, upper),
                        Ok(Ordering::Less | Ordering::Equal)
                    )
                }
                Bound::Excluded(upper) => {
                    matches!(
                        self.cursor.reader.header.order.compare(key, upper),
                        Ok(Ordering::Less)
                    )
                }
            }
        };
        if !in_range {
            self.done = true;
            return None;
        }
        let entry = (key.to_vec(), value.to_vec());
        let advanced = if self.reverse {
            self.cursor.advance_backward()
        } else {
            self.cursor.advance_forward()
        };
        if let Err(error) = advanced {
            self.done = true;
            return Some(Err(error));
        }
        Some(Ok(entry))
    }
}
