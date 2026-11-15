//! Borrowed local record-set descriptors and block codecs.
//!
//! A persistent posting is one fixed 32-byte descriptor plus, for the `Blocks`
//! form, a sorted directory and independently compressed 65,536-ID blocks.
//! Query-owned `RoaringBitmap` values remain temporary results; they are never
//! the persisted representation.

use std::cmp::Ordering;

use roaring::RoaringBitmap;

use super::error::{StorageError, StorageResult};

/// Encoded width of one record-set descriptor.
pub const RECORD_SET_DESCRIPTOR_LEN: usize = 32;
/// Encoded width of one block-directory descriptor.
pub const BLOCK_DESCRIPTOR_LEN: usize = 32;
/// Number of IDs covered by one block.
pub const BLOCK_SIZE: u32 = 1 << 16;
const MAX_BITMAP_WORDS: usize = 1024;


/// Descriptor form.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum RecordSetTag {
    Empty = 0,
    All = 1,
    Range = 2,
    Inline = 3,
    Blocks = 4,
}


impl RecordSetTag {
    fn from_code(code: u8) -> StorageResult<Self> {
        Ok(match code {
            0 => Self::Empty,
            1 => Self::All,
            2 => Self::Range,
            3 => Self::Inline,
            4 => Self::Blocks,
            other => {
                return Err(StorageError::corrupt(format!(
                    "unknown record-set tag {other}"
                )));
            }
        })
    }
}


/// Fixed 32-byte borrowed record-set descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RecordSetRef {
    pub tag: RecordSetTag,
    pub flags: u8,
    pub inline_count: u16,
    pub a: u64,
    pub b: u64,
    pub cardinality: u64,
}


impl RecordSetRef {
    pub const EMPTY: Self = Self {
        tag: RecordSetTag::Empty,
        flags: 0,
        inline_count: 0,
        a: 0,
        b: 0,
        cardinality: 0,
    };

    pub fn all(domain_len: u32) -> Self {
        Self {
            tag: RecordSetTag::All,
            flags: 0,
            inline_count: 0,
            a: 0,
            b: 0,
            cardinality: domain_len as u64,
        }
    }

    pub fn range(start: u32, end: u32) -> StorageResult<Self> {
        if end < start {
            return Err(StorageError::InvalidArgument(
                "record-set range end precedes start".into(),
            ));
        }
        Ok(Self {
            tag: RecordSetTag::Range,
            flags: 0,
            inline_count: 0,
            a: start as u64,
            b: end as u64,
            cardinality: (end - start) as u64,
        })
    }

    pub fn inline(slots: &[u32]) -> StorageResult<Self> {
        if slots.len() > 4 {
            return Err(StorageError::InvalidArgument(
                "inline record sets contain at most four members".into(),
            ));
        }
        if slots.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(StorageError::InvalidArgument(
                "inline record-set members must be sorted and unique".into(),
            ));
        }
        let mut first = [0u32; 4];
        first[..slots.len()].copy_from_slice(slots);
        let a = (first[0] as u64) | ((first[1] as u64) << 32);
        let b = (first[2] as u64) | ((first[3] as u64) << 32);
        Ok(Self {
            tag: RecordSetTag::Inline,
            flags: 0,
            inline_count: slots.len() as u16,
            a,
            b,
            cardinality: slots.len() as u64,
        })
    }

    pub fn blocks(offset: u64, directory_len: u64, cardinality: u64) -> Self {
        Self {
            tag: RecordSetTag::Blocks,
            flags: 0,
            inline_count: 0,
            a: offset,
            b: directory_len,
            cardinality,
        }
    }

    pub fn encode(self) -> [u8; RECORD_SET_DESCRIPTOR_LEN] {
        let mut bytes = [0u8; RECORD_SET_DESCRIPTOR_LEN];
        bytes[0] = self.tag as u8;
        bytes[1] = self.flags;
        bytes[2..4].copy_from_slice(&self.inline_count.to_le_bytes());
        bytes[4..8].fill(0);
        bytes[8..16].copy_from_slice(&self.a.to_le_bytes());
        bytes[16..24].copy_from_slice(&self.b.to_le_bytes());
        bytes[24..32].copy_from_slice(&self.cardinality.to_le_bytes());
        bytes
    }

    pub fn decode(bytes: &[u8]) -> StorageResult<Self> {
        if bytes.len() < RECORD_SET_DESCRIPTOR_LEN {
            return Err(StorageError::corrupt("truncated record-set descriptor"));
        }
        let tag = RecordSetTag::from_code(bytes[0])?;
        let flags = bytes[1];
        if flags != 0 || bytes[4..8].iter().any(|byte| *byte != 0) {
            return Err(StorageError::corrupt(
                "record-set descriptor reserved bytes are not zero",
            ));
        }
        let inline_count = u16::from_le_bytes([bytes[2], bytes[3]]);
        Ok(Self {
            tag,
            flags,
            inline_count,
            a: u64::from_le_bytes(
                bytes[8..16]
                    .try_into()
                    .expect("eight-byte descriptor field"),
            ),
            b: u64::from_le_bytes(
                bytes[16..24]
                    .try_into()
                    .expect("eight-byte descriptor field"),
            ),
            cardinality: u64::from_le_bytes(
                bytes[24..32]
                    .try_into()
                    .expect("eight-byte descriptor field"),
            ),
        })
    }

    pub fn cardinality(self) -> u64 {
        self.cardinality
    }

    pub fn is_empty(self) -> bool {
        self.cardinality == 0
    }

    /// Bind the descriptor to its owning posting arena.
    pub fn view<'a>(self, arena: &'a [u8], domain_len: u32) -> StorageResult<RecordSetView<'a>> {
        RecordSetView::new(self, arena, domain_len)
    }
}


/// Block payload codec.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum BlockCodec {
    Array = 0,
    Bitmap = 1,
    Run = 2,
}


impl BlockCodec {
    fn from_code(code: u8) -> StorageResult<Self> {
        Ok(match code {
            0 => Self::Array,
            1 => Self::Bitmap,
            2 => Self::Run,
            other => {
                return Err(StorageError::corrupt(format!(
                    "unknown block codec {other}"
                )));
            }
        })
    }
}


#[derive(Clone, Copy, Debug)]
struct BlockDescriptor {
    key: u32,
    codec: BlockCodec,
    cardinality: u32,
    payload_offset: u64,
    payload_length: u32,
}


/// One checked record-set view. Block descriptors are decoded eagerly; block
/// payloads are decoded only when their IDs are needed.
pub struct RecordSetView<'a> {
    reference: RecordSetRef,
    arena: &'a [u8],
    paged: Option<PagedArena>,
    arena_len: u64,
    domain_len: u32,
    blocks: Vec<BlockDescriptor>,
}


struct PagedArena {
    backing: super::backing::Backing,
    offset: u64,
}


impl<'a> RecordSetView<'a> {
    pub fn new(reference: RecordSetRef, arena: &'a [u8], domain_len: u32) -> StorageResult<Self> {
        let mut view = Self {
            reference,
            arena,
            paged: None,
            arena_len: arena.len() as u64,
            domain_len,
            blocks: Vec::new(),
        };
        view.validate()?;
        Ok(view)
    }

    /// Open a record set whose arena remains in its backing. Only the block
    /// directory and the requested block payloads are read.
    pub fn new_paged(
        reference: RecordSetRef,
        backing: super::backing::Backing,
        arena_offset: u64,
        arena_len: u64,
        domain_len: u32,
    ) -> StorageResult<Self> {
        arena_offset
            .checked_add(arena_len)
            .ok_or_else(|| StorageError::corrupt("record-set arena bounds overflow"))?;
        let mut view = Self {
            reference,
            arena: &[],
            paged: Some(PagedArena {
                backing,
                offset: arena_offset,
            }),
            arena_len,
            domain_len,
            blocks: Vec::new(),
        };
        view.validate()?;
        Ok(view)
    }

    fn arena_slice(&self, start: u64, length: u64) -> StorageResult<&[u8]> {
        let end = start
            .checked_add(length)
            .ok_or_else(|| StorageError::corrupt("record-set arena range overflow"))?;
        if end > self.arena_len {
            return Err(StorageError::corrupt(
                "record-set arena range out of bounds",
            ));
        }
        match &self.paged {
            Some(paged) => paged.backing.read(paged.offset + start, length),
            None => self
                .arena
                .get(start as usize..end as usize)
                .ok_or_else(|| StorageError::corrupt("record-set arena range out of bounds")),
        }
    }

    fn validate(&mut self) -> StorageResult<()> {
        self.blocks.clear();
        let reference = self.reference;
        match reference.tag {
            RecordSetTag::Empty => {
                if reference.cardinality != 0 {
                    return Err(StorageError::corrupt(
                        "empty record set has nonzero cardinality",
                    ));
                }
            }
            RecordSetTag::All => {
                if reference.cardinality != self.domain_len as u64 {
                    return Err(StorageError::corrupt(
                        "all record set does not match its owning domain",
                    ));
                }
            }
            RecordSetTag::Range => {
                if reference.b > self.domain_len as u64
                    || reference.a > reference.b
                    || reference.cardinality != reference.b - reference.a
                {
                    return Err(StorageError::corrupt("invalid range record set"));
                }
            }
            RecordSetTag::Inline => {
                if reference.inline_count > 4
                    || reference.cardinality != reference.inline_count as u64
                {
                    return Err(StorageError::corrupt("invalid inline record set"));
                }
                let slots = inline_slots(reference);
                let used = &slots[..reference.inline_count as usize];
                if used.windows(2).any(|pair| pair[0] >= pair[1])
                    || used.iter().any(|slot| *slot >= self.domain_len)
                {
                    return Err(StorageError::corrupt(
                        "inline record members are not sorted or are out of domain",
                    ));
                }
            }
            RecordSetTag::Blocks => {
                let dir = self.arena_slice(reference.a, reference.b)?;
                if reference.b == 0 || reference.b % BLOCK_DESCRIPTOR_LEN as u64 != 0 {
                    return Err(StorageError::corrupt("invalid block directory length"));
                }
                let mut previous_key = None;
                let mut total = 0u64;
                let mut blocks = Vec::new();
                for chunk in dir.chunks_exact(BLOCK_DESCRIPTOR_LEN) {
                    let key =
                        u32::from_le_bytes(chunk[..4].try_into().expect("four-byte block key"));
                    let codec = BlockCodec::from_code(chunk[4])?;
                    if chunk[5..8].iter().any(|byte| *byte != 0) {
                        return Err(StorageError::corrupt(
                            "block descriptor reserved bytes are not zero",
                        ));
                    }
                    let cardinality = u32::from_le_bytes(
                        chunk[8..12]
                            .try_into()
                            .expect("four-byte block cardinality"),
                    );
                    if chunk[12..16].iter().any(|byte| *byte != 0) {
                        return Err(StorageError::corrupt(
                            "block descriptor reserved bytes are not zero",
                        ));
                    }
                    let payload_offset = u64::from_le_bytes(
                        chunk[16..24].try_into().expect("eight-byte payload offset"),
                    );
                    let payload_length = u32::from_le_bytes(
                        chunk[24..28].try_into().expect("four-byte payload length"),
                    );
                    if chunk[28..32].iter().any(|byte| *byte != 0) {
                        return Err(StorageError::corrupt(
                            "block descriptor reserved bytes are not zero",
                        ));
                    }
                    if previous_key.is_some_and(|previous| previous >= key) {
                        return Err(StorageError::corrupt(
                            "block directory keys are not strictly increasing",
                        ));
                    }
                    previous_key = Some(key);
                    if payload_offset < reference.b {
                        return Err(StorageError::corrupt(
                            "block payload overlaps its directory",
                        ));
                    }
                    let payload_end = payload_offset
                        .checked_add(payload_length as u64)
                        .ok_or_else(|| StorageError::corrupt("block payload end overflow"))?;
                    if reference
                        .a
                        .checked_add(payload_end)
                        .is_none_or(|end| end > self.arena_len)
                    {
                        return Err(StorageError::corrupt("block payload out of bounds"));
                    }
                    total = total
                        .checked_add(cardinality as u64)
                        .ok_or_else(|| StorageError::corrupt("block cardinality overflow"))?;
                    blocks.push(BlockDescriptor {
                        key,
                        codec,
                        cardinality,
                        payload_offset,
                        payload_length,
                    });
                }
                if total != reference.cardinality {
                    return Err(StorageError::corrupt(
                        "block cardinalities do not match the record-set descriptor",
                    ));
                }
                self.blocks = blocks;
            }
        }
        Ok(())
    }

    pub fn reference(&self) -> RecordSetRef {
        self.reference
    }

    pub fn cardinality(&self) -> u64 {
        self.reference.cardinality
    }

    pub fn is_empty(&self) -> bool {
        self.reference.cardinality == 0
    }

    pub fn domain_len(&self) -> u32 {
        self.domain_len
    }

    pub fn contains(&self, slot: u32) -> StorageResult<bool> {
        if slot >= self.domain_len {
            return Ok(false);
        }
        Ok(match self.reference.tag {
            RecordSetTag::Empty => false,
            RecordSetTag::All => true,
            RecordSetTag::Range => {
                slot as u64 >= self.reference.a && (slot as u64) < self.reference.b
            }
            RecordSetTag::Inline => inline_slots(self.reference)
                [..self.reference.inline_count as usize]
                .binary_search(&slot)
                .is_ok(),
            RecordSetTag::Blocks => {
                let key = slot >> 16;
                let Ok(index) = self.blocks.binary_search_by_key(&key, |block| block.key) else {
                    return Ok(false);
                };
                self.decode_block(self.blocks[index])?
                    .binary_search(&slot)
                    .is_ok()
            }
        })
    }

    pub fn ids(&self) -> RecordSetIter<'_, 'a> {
        RecordSetIter {
            view: self,
            block_index: 0,
            current: Vec::new(),
            current_index: 0,
            range_next: match self.reference.tag {
                RecordSetTag::Range => self.reference.a as u32,
                _ => 0,
            },
            inline: match self.reference.tag {
                RecordSetTag::Inline => inline_slots(self.reference).to_vec(),
                _ => Vec::new(),
            },
        }
    }

    /// Count members shared with another borrowed set without allocating a
    /// result bitmap.
    pub fn intersection_count(&self, other: &RecordSetView<'_>) -> StorageResult<u64> {
        if self.is_empty() || other.is_empty() {
            return Ok(0);
        }
        if self.reference.tag == RecordSetTag::All {
            return other.count_in_range(0, self.domain_len);
        }
        if other.reference.tag == RecordSetTag::All {
            return self.count_in_range(0, other.domain_len);
        }
        if self.reference.tag == RecordSetTag::Range {
            return other.count_in_range(self.reference.a as u32, self.reference.b as u32);
        }
        if other.reference.tag == RecordSetTag::Range {
            return self.count_in_range(other.reference.a as u32, other.reference.b as u32);
        }
        let mut count = 0u64;
        let mut left = self.ids();
        let mut right = other.ids();
        let mut a = left.next().transpose()?;
        let mut b = right.next().transpose()?;
        while let (Some(left_slot), Some(right_slot)) = (a, b) {
            match left_slot.cmp(&right_slot) {
                Ordering::Less => a = left.next().transpose()?,
                Ordering::Greater => b = right.next().transpose()?,
                Ordering::Equal => {
                    count += 1;
                    a = left.next().transpose()?;
                    b = right.next().transpose()?;
                }
            }
        }
        Ok(count)
    }

    pub fn union_into(&self, destination: &mut RoaringBitmap) -> StorageResult<()> {
        if self.reference.tag == RecordSetTag::Range {
            destination.insert_range(self.reference.a as u32..self.reference.b as u32);
            return Ok(());
        }
        for slot in self.ids() {
            let slot = slot?;
            destination.insert(slot);
        }
        Ok(())
    }

    pub fn intersect_into(&self, other: &RecordSetView<'_>) -> StorageResult<RoaringBitmap> {
        let mut result = RoaringBitmap::new();
        if self.is_empty() || other.is_empty() {
            return Ok(result);
        }
        if self.reference.tag == RecordSetTag::All {
            other.union_into(&mut result)?;
            return Ok(result);
        }
        if other.reference.tag == RecordSetTag::All {
            self.union_into(&mut result)?;
            return Ok(result);
        }
        let (small, large) = if self.cardinality() <= other.cardinality() {
            (self, other)
        } else {
            (other, self)
        };
        if small.reference.tag == RecordSetTag::Range {
            large.for_each_in_range(
                small.reference.a as u32,
                small.reference.b as u32,
                |slot| {
                    result.insert(slot);
                },
            )?;
            return Ok(result);
        }
        for slot in small.ids() {
            let slot = slot?;
            if large.contains(slot)? {
                result.insert(slot);
            }
        }
        Ok(result)
    }

    pub fn difference_into(&self, other: &RecordSetView<'_>) -> StorageResult<RoaringBitmap> {
        let mut result = RoaringBitmap::new();
        if self.is_empty() {
            return Ok(result);
        }
        if other.is_empty() {
            self.union_into(&mut result)?;
            return Ok(result);
        }
        if self.reference.tag == RecordSetTag::Range {
            other.for_each_in_range(self.reference.a as u32, self.reference.b as u32, |slot| {
                result.insert(slot);
            })?;
            let mut included = RoaringBitmap::new();
            included.insert_range(self.reference.a as u32..self.reference.b as u32);
            result = &included - &result;
            return Ok(result);
        }
        for slot in self.ids() {
            let slot = slot?;
            if !other.contains(slot)? {
                result.insert(slot);
            }
        }
        Ok(result)
    }

    /// Decode every block and validate its payload and cardinality.
    pub fn verify_full(&self) -> StorageResult<()> {
        for block in &self.blocks {
            let decoded = self.decode_block(*block)?;
            if decoded.len() as u64 != block.cardinality as u64 {
                return Err(StorageError::corrupt(
                    "decoded block cardinality differs from its descriptor",
                ));
            }
        }
        Ok(())
    }

    fn count_in_range(&self, start: u32, end: u32) -> StorageResult<u64> {
        if end <= start {
            return Ok(0);
        }
        let mut count = 0u64;
        self.for_each_in_range(start, end, |_| count += 1)?;
        Ok(count)
    }

    fn for_each_in_range(
        &self,
        start: u32,
        end: u32,
        mut visit: impl FnMut(u32),
    ) -> StorageResult<()> {
        if end <= start {
            return Ok(());
        }
        match self.reference.tag {
            RecordSetTag::Empty => {}
            RecordSetTag::All => {
                for slot in start..end.min(self.domain_len) {
                    visit(slot);
                }
            }
            RecordSetTag::Range => {
                let lo = start.max(self.reference.a as u32);
                let hi = end.min(self.reference.b as u32);
                for slot in lo..hi {
                    visit(slot);
                }
            }
            RecordSetTag::Inline => {
                for slot in &inline_slots(self.reference)[..self.reference.inline_count as usize] {
                    if *slot >= start && *slot < end {
                        visit(*slot);
                    }
                }
            }
            RecordSetTag::Blocks => {
                for block in &self.blocks {
                    let block_start = block.key << 16;
                    let block_end = block_start.saturating_add(BLOCK_SIZE);
                    if block_end <= start || block_start >= end {
                        continue;
                    }
                    for slot in self.decode_block(*block)? {
                        if slot >= start && slot < end {
                            visit(slot);
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn decode_block(&self, block: BlockDescriptor) -> StorageResult<Vec<u32>> {
        let start = self
            .reference
            .a
            .checked_add(block.payload_offset)
            .ok_or_else(|| StorageError::corrupt("block payload offset overflow"))?;
        let payload = self.arena_slice(start, block.payload_length as u64)?;
        let base = block.key << 16;
        let mut ids = Vec::with_capacity(block.cardinality as usize);
        match block.codec {
            BlockCodec::Array => {
                if payload.len() % 2 != 0 {
                    return Err(StorageError::corrupt("array block has an odd length"));
                }
                let mut previous = None;
                for chunk in payload.chunks_exact(2) {
                    let offset = u16::from_le_bytes(chunk.try_into().expect("two-byte offset"));
                    if previous.is_some_and(|previous| previous >= offset) {
                        return Err(StorageError::corrupt(
                            "array block offsets are not strictly increasing",
                        ));
                    }
                    previous = Some(offset);
                    ids.push(base | offset as u32);
                }
            }
            BlockCodec::Bitmap => {
                if payload.len() % 8 != 0 {
                    return Err(StorageError::corrupt("bitmap block is not word-aligned"));
                }
                let words = payload.len() / 8;
                if words > MAX_BITMAP_WORDS {
                    return Err(StorageError::corrupt("bitmap block exceeds 65,536 bits"));
                }
                if words > 0 && payload[payload.len() - 8..].iter().all(|byte| *byte == 0) {
                    return Err(StorageError::corrupt(
                        "bitmap block has a trailing zero word",
                    ));
                }
                for (word_index, chunk) in payload.chunks_exact(8).enumerate() {
                    let mut word =
                        u64::from_le_bytes(chunk.try_into().expect("eight-byte bitmap word"));
                    while word != 0 {
                        let bit = word.trailing_zeros() as u32;
                        ids.push(base | (word_index as u32 * 64 + bit));
                        word &= word - 1;
                    }
                }
            }
            BlockCodec::Run => {
                if payload.len() % 4 != 0 {
                    return Err(StorageError::corrupt("run block has an invalid length"));
                }
                let mut previous_end = None;
                for chunk in payload.chunks_exact(4) {
                    let start = u16::from_le_bytes(chunk[..2].try_into().expect("run start"));
                    let length = u16::from_le_bytes(chunk[2..].try_into().expect("run length"));
                    if start as u32 + length as u32 >= BLOCK_SIZE {
                        return Err(StorageError::corrupt("run block range overflow"));
                    }
                    if previous_end.is_some_and(|previous| previous >= start) {
                        return Err(StorageError::corrupt(
                            "run block ranges overlap or are unsorted",
                        ));
                    }
                    previous_end = Some(start + length);
                    for offset in start..=start.saturating_add(length) {
                        ids.push(base | offset as u32);
                    }
                }
            }
        }
        Ok(ids)
    }
}


/// Ascending borrowed iterator over one record set.
pub struct RecordSetIter<'view, 'arena> {
    view: &'view RecordSetView<'arena>,
    block_index: usize,
    current: Vec<u32>,
    current_index: usize,
    range_next: u32,
    inline: Vec<u32>,
}


impl Iterator for RecordSetIter<'_, '_> {
    type Item = StorageResult<u32>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.view.reference.tag {
            RecordSetTag::Empty => None,
            RecordSetTag::All => {
                if self.range_next >= self.view.domain_len {
                    None
                } else {
                    let slot = self.range_next;
                    self.range_next += 1;
                    Some(Ok(slot))
                }
            }
            RecordSetTag::Range => {
                if self.range_next >= self.view.reference.b as u32 {
                    None
                } else {
                    let slot = self.range_next;
                    self.range_next += 1;
                    Some(Ok(slot))
                }
            }
            RecordSetTag::Inline => {
                if self.current_index >= self.view.reference.inline_count as usize {
                    return None;
                }
                let slot = self.inline.get(self.current_index).copied();
                self.current_index += 1;
                slot.map(Ok)
            }
            RecordSetTag::Blocks => loop {
                if self.current_index < self.current.len() {
                    let slot = self.current[self.current_index];
                    self.current_index += 1;
                    return Some(Ok(slot));
                }
                let block = self.view.blocks.get(self.block_index).copied()?;
                self.block_index += 1;
                match self.view.decode_block(block) {
                    Ok(ids) => {
                        self.current = ids;
                        self.current_index = 0;
                    }
                    Err(error) => return Some(Err(error)),
                }
            },
        }
    }
}


/// A freshly encoded record set. `arena` contains the block directory followed
/// by block payloads; `reference.a` is zero until embedded in a posting arena.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedRecordSet {
    pub reference: RecordSetRef,
    pub arena: Vec<u8>,
}


/// Encode one sorted, unique slot slice into the smallest descriptor form.
pub fn encode_record_set(slots: &[u32], domain_len: u32) -> StorageResult<EncodedRecordSet> {
    if slots.iter().any(|slot| *slot >= domain_len) {
        return Err(StorageError::InvalidArgument(
            "record-set member lies outside its owning domain".into(),
        ));
    }
    if slots.windows(2).any(|pair| pair[0] >= pair[1]) {
        return Err(StorageError::InvalidArgument(
            "record-set members must be sorted and unique".into(),
        ));
    }
    if slots.is_empty() {
        return Ok(EncodedRecordSet {
            reference: RecordSetRef::EMPTY,
            arena: Vec::new(),
        });
    }
    if slots.len() == domain_len as usize
        && slots
            .iter()
            .enumerate()
            .all(|(index, slot)| *slot == index as u32)
    {
        return Ok(EncodedRecordSet {
            reference: RecordSetRef::all(domain_len),
            arena: Vec::new(),
        });
    }
    let first = slots[0];
    let last = *slots.last().expect("nonempty slots");
    if last
        .checked_add(1)
        .is_some_and(|_| (last - first) as usize + 1 == slots.len())
    {
        return Ok(EncodedRecordSet {
            reference: RecordSetRef::range(first, last.saturating_add(1))?,
            arena: Vec::new(),
        });
    }
    if slots.len() <= 4 {
        return Ok(EncodedRecordSet {
            reference: RecordSetRef::inline(slots)?,
            arena: Vec::new(),
        });
    }
    encode_blocks(slots)
}

fn encode_blocks(slots: &[u32]) -> StorageResult<EncodedRecordSet> {
    let mut block_ranges = Vec::new();
    let mut start = 0usize;
    for index in 1..=slots.len() {
        if index == slots.len() || (slots[index] >> 16) != (slots[start] >> 16) {
            block_ranges.push((slots[start] >> 16, &slots[start..index]));
            start = index;
        }
    }
    let directory_len = block_ranges
        .len()
        .checked_mul(BLOCK_DESCRIPTOR_LEN)
        .ok_or_else(|| StorageError::ResourceLimit("block directory overflow".into()))?;
    let mut payloads = Vec::new();
    let mut descriptors = Vec::with_capacity(block_ranges.len());
    for (key, ids) in block_ranges {
        let encoded = encode_block(ids)?;
        let payload_offset = directory_len
            .checked_add(payloads.len())
            .ok_or_else(|| StorageError::ResourceLimit("block payload offset overflow".into()))?;
        payloads.extend_from_slice(&encoded);
        descriptors.push((
            key,
            choose_codec(ids),
            ids.len() as u32,
            payload_offset as u64,
            encoded.len() as u32,
        ));
    }
    let mut arena = vec![0u8; directory_len];
    for (number, (key, codec, cardinality, payload_offset, payload_length)) in
        descriptors.into_iter().enumerate()
    {
        let at = number * BLOCK_DESCRIPTOR_LEN;
        arena[at..at + 4].copy_from_slice(&key.to_le_bytes());
        arena[at + 4] = codec as u8;
        arena[at + 5..at + 8].fill(0);
        arena[at + 8..at + 12].copy_from_slice(&cardinality.to_le_bytes());
        arena[at + 12..at + 16].fill(0);
        arena[at + 16..at + 24].copy_from_slice(&payload_offset.to_le_bytes());
        arena[at + 24..at + 28].copy_from_slice(&payload_length.to_le_bytes());
        arena[at + 28..at + 32].fill(0);
    }
    arena.extend_from_slice(&payloads);
    Ok(EncodedRecordSet {
        reference: RecordSetRef::blocks(0, directory_len as u64, slots.len() as u64),
        arena,
    })
}

fn choose_codec(ids: &[u32]) -> BlockCodec {
    let array_bytes = ids.len() * 2;
    let runs = run_count(ids);
    let run_bytes = runs * 4;
    let bitmap_bytes = bitmap_words(ids) * 8;
    if array_bytes <= run_bytes && array_bytes <= bitmap_bytes {
        BlockCodec::Array
    } else if run_bytes <= bitmap_bytes {
        BlockCodec::Run
    } else {
        BlockCodec::Bitmap
    }
}

fn encode_block(ids: &[u32]) -> StorageResult<Vec<u8>> {
    match choose_codec(ids) {
        BlockCodec::Array => {
            let mut bytes = Vec::with_capacity(ids.len() * 2);
            for id in ids {
                bytes.extend_from_slice(&((id & 0xffff) as u16).to_le_bytes());
            }
            Ok(bytes)
        }
        BlockCodec::Bitmap => {
            let words = bitmap_words(ids);
            let mut bitmap = vec![0u64; words];
            for id in ids {
                let offset = (id & 0xffff) as usize;
                bitmap[offset / 64] |= 1u64 << (offset % 64);
            }
            let mut bytes = Vec::with_capacity(words * 8);
            for word in bitmap {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            Ok(bytes)
        }
        BlockCodec::Run => {
            let mut bytes = Vec::new();
            let mut index = 0usize;
            while index < ids.len() {
                let start = (ids[index] & 0xffff) as u16;
                let mut end = start;
                index += 1;
                while index < ids.len() && ((ids[index] & 0xffff) as u16) == end.wrapping_add(1) {
                    end = (ids[index] & 0xffff) as u16;
                    index += 1;
                }
                bytes.extend_from_slice(&start.to_le_bytes());
                bytes.extend_from_slice(&end.saturating_sub(start).to_le_bytes());
            }
            Ok(bytes)
        }
    }
}

fn run_count(ids: &[u32]) -> usize {
    let mut runs = 0usize;
    let mut previous: Option<u16> = None;
    for id in ids {
        let offset = (*id & 0xffff) as u16;
        if previous.is_none_or(|previous| offset != previous.wrapping_add(1)) {
            runs += 1;
        }
        previous = Some(offset);
    }
    runs
}

fn bitmap_words(ids: &[u32]) -> usize {
    ids.iter()
        .map(|id| ((*id & 0xffff) as usize) / 64 + 1)
        .max()
        .unwrap_or(0)
}

fn inline_slots(reference: RecordSetRef) -> [u32; 4] {
    [
        reference.a as u32,
        (reference.a >> 32) as u32,
        reference.b as u32,
        (reference.b >> 32) as u32,
    ]
}
