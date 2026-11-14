//! Packed sort-key columns and persisted order arrays.
//!
//! Keys are stored as an offset array plus a byte arena. Order arrays are
//! physical slots and cursor readers merge them without rebuilding complete
//! order vectors.

use crate::model::LogicalType;
use crate::posting::ScalarKey;
use crate::sorting::{CollationStrength, Placement, SortKey, SortOptions};

use super::backing::Backing;
use super::error::{StorageError, StorageResult};
use super::format::SectionDescriptor;
use super::posting::{logical_type_codes, logical_type_from_codes};
use super::scalar::{decode_scalar_key, encode_scalar_key};

pub const SORT_KEYS_MAGIC: [u8; 4] = *b"OBSK";
pub const SORT_KEYS_VERSION: u16 = 1;
pub const SORT_ORDERS_MAGIC: [u8; 4] = *b"OBSO";
pub const SORT_ORDERS_VERSION: u16 = 1;
pub const SORT_COLUMN_DESCRIPTOR_LEN: usize = 120;
pub const SORT_KEYS_HEADER_LEN: usize = 16;
pub const SORT_ORDERS_HEADER_LEN: usize = 16;


/// One sort column supplied to the segment builder.
#[derive(Clone, Debug)]
pub struct SortColumnInput {
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub locale: String,
    pub options: SortOptions,
    pub collation_version: String,
    pub keys: Vec<SortKey>,
    pub ascending: Vec<u32>,
    pub descending: Vec<u32>,
}


/// Packed sort sections.
pub struct SortSections {
    pub keys: Vec<u8>,
    pub orders: Vec<u8>,
}


/// Encode all sort columns.
pub fn build_sort_sections(
    columns: &[SortColumnInput],
    slot_count: u32,
) -> StorageResult<SortSections> {
    if let Some(column) = columns
        .iter()
        .find(|column| column.keys.len() != slot_count as usize)
    {
        return Err(StorageError::InvalidArgument(format!(
            "sort column {:?} does not carry one key per slot",
            column.path
        )));
    }
    let mut orders = vec![0u8; SORT_ORDERS_HEADER_LEN];
    orders[..4].copy_from_slice(&SORT_ORDERS_MAGIC);
    orders[4..6].copy_from_slice(&SORT_ORDERS_VERSION.to_le_bytes());
    orders[6..8].fill(0);
    orders[8..12].copy_from_slice(&(columns.len() as u32).to_le_bytes());
    orders[12..16].fill(0);
    let mut order_offsets = Vec::with_capacity(columns.len());
    for column in columns {
        validate_order(&column.ascending, slot_count, "ascending")?;
        validate_order(&column.descending, slot_count, "descending")?;
        let ascending_offset = orders.len();
        for slot in &column.ascending {
            orders.extend_from_slice(&slot.to_le_bytes());
        }
        let descending_offset = orders.len();
        for slot in &column.descending {
            orders.extend_from_slice(&slot.to_le_bytes());
        }
        order_offsets.push((
            ascending_offset,
            column.ascending.len() * 4,
            descending_offset,
            column.descending.len() * 4,
        ));
    }
    let directory_len = SORT_KEYS_HEADER_LEN + columns.len() * SORT_COLUMN_DESCRIPTOR_LEN;
    let mut payload = Vec::new();
    let mut descriptors = Vec::with_capacity(columns.len());
    for (index, column) in columns.iter().enumerate() {
        let (kind, element) = logical_type_codes(&column.kind);
        let path = encode_path(&column.path)?;
        let locale = column.locale.as_bytes();
        let collation = column.collation_version.as_bytes();
        let path_offset = directory_len + payload.len();
        payload.extend_from_slice(&path);
        let locale_offset = directory_len + payload.len();
        payload.extend_from_slice(locale);
        let collation_offset = directory_len + payload.len();
        payload.extend_from_slice(collation);
        let mut key_offsets = Vec::with_capacity(column.keys.len() + 1);
        let mut key_arena = Vec::new();
        key_offsets.push(0u32);
        for key in &column.keys {
            let encoded = encode_sort_key(key)?;
            key_arena.extend_from_slice(&encoded);
            let offset = u32::try_from(key_arena.len())
                .map_err(|_| StorageError::ResourceLimit("sort-key arena exceeds 4 GiB".into()))?;
            key_offsets.push(offset);
        }
        let mut offsets_bytes = Vec::with_capacity(key_offsets.len() * 4);
        for offset in key_offsets {
            offsets_bytes.extend_from_slice(&offset.to_le_bytes());
        }
        let offsets_offset = directory_len + payload.len();
        payload.extend_from_slice(&offsets_bytes);
        let arena_offset = directory_len + payload.len();
        payload.extend_from_slice(&key_arena);
        let (ascending_offset, ascending_len, descending_offset, descending_len) =
            order_offsets[index];
        descriptors.push((
            kind,
            element,
            path_offset,
            path.len(),
            locale_offset,
            locale.len(),
            collation_offset,
            collation.len(),
            column.options,
            column.keys.len(),
            offsets_offset,
            offsets_bytes.len(),
            arena_offset,
            key_arena.len(),
            ascending_offset,
            ascending_len,
            descending_offset,
            descending_len,
        ));
    }
    let mut keys = vec![0u8; directory_len];
    keys[..4].copy_from_slice(&SORT_KEYS_MAGIC);
    keys[4..6].copy_from_slice(&SORT_KEYS_VERSION.to_le_bytes());
    keys[6..8].fill(0);
    keys[8..12].copy_from_slice(&(columns.len() as u32).to_le_bytes());
    keys[12..16].fill(0);
    for (index, descriptor) in descriptors.into_iter().enumerate() {
        let at = SORT_KEYS_HEADER_LEN + index * SORT_COLUMN_DESCRIPTOR_LEN;
        let slot = &mut keys[at..at + SORT_COLUMN_DESCRIPTOR_LEN];
        slot[0] = descriptor.0;
        slot[1] = descriptor.1;
        slot[2] = 0b111;
        slot[3] = 0;
        slot[4..12].copy_from_slice(&(descriptor.2 as u64).to_le_bytes());
        slot[12..16].copy_from_slice(&(descriptor.3 as u32).to_le_bytes());
        slot[16..24].copy_from_slice(&(descriptor.4 as u64).to_le_bytes());
        slot[24..28].copy_from_slice(&(descriptor.5 as u32).to_le_bytes());
        slot[28..36].copy_from_slice(&(descriptor.6 as u64).to_le_bytes());
        slot[36..40].copy_from_slice(&(descriptor.7 as u32).to_le_bytes());
        slot[40..44].copy_from_slice(&encode_options(descriptor.8).to_le_bytes());
        slot[44..48].fill(0);
        slot[48..52].copy_from_slice(&(descriptor.9 as u32).to_le_bytes());
        slot[52..56].fill(0);
        slot[56..64].copy_from_slice(&(descriptor.10 as u64).to_le_bytes());
        slot[64..72].copy_from_slice(&(descriptor.11 as u64).to_le_bytes());
        slot[72..80].copy_from_slice(&(descriptor.12 as u64).to_le_bytes());
        slot[80..88].copy_from_slice(&(descriptor.13 as u64).to_le_bytes());
        slot[88..96].copy_from_slice(&(descriptor.14 as u64).to_le_bytes());
        slot[96..104].copy_from_slice(&(descriptor.15 as u64).to_le_bytes());
        slot[104..112].copy_from_slice(&(descriptor.16 as u64).to_le_bytes());
        slot[112..120].copy_from_slice(&(descriptor.17 as u64).to_le_bytes());
    }
    keys.extend_from_slice(&payload);
    Ok(SortSections { keys, orders })
}

/// Encode one sort key.
pub fn encode_sort_key(key: &SortKey) -> StorageResult<Vec<u8>> {
    let mut bytes = Vec::with_capacity(24);
    match key {
        SortKey::Missing => bytes.push(0),
        SortKey::Unavailable => bytes.push(1),
        SortKey::Null => bytes.push(2),
        SortKey::String(value) => {
            bytes.push(3);
            push_bytes(&mut bytes, value)?;
        }
        SortKey::Id(value) => {
            bytes.push(4);
            push_bytes(&mut bytes, value.as_bytes())?;
        }
        SortKey::Number(value) => {
            bytes.push(5);
            bytes.extend_from_slice(&encode_scalar_key(&ScalarKey::Number(value.clone()))?);
        }
        SortKey::Bool(value) => {
            bytes.push(6);
            bytes.push(u8::from(*value));
        }
        SortKey::Date(value) => {
            bytes.push(7);
            bytes.extend_from_slice(&value.to_julian_day().to_le_bytes());
        }
        SortKey::Datetime(value) => {
            bytes.push(8);
            bytes.extend_from_slice(&value.unix_timestamp_nanos().to_le_bytes());
        }
        SortKey::Unixtime(value) => {
            bytes.push(9);
            bytes.extend_from_slice(&value.to_le_bytes());
        }
    }
    Ok(bytes)
}

/// Decode one sort key.
pub fn decode_sort_key(bytes: &[u8]) -> StorageResult<SortKey> {
    let (&tag, payload) = bytes
        .split_first()
        .ok_or_else(|| StorageError::corrupt("empty sort key"))?;
    Ok(match tag {
        0 => SortKey::Missing,
        1 => SortKey::Unavailable,
        2 => SortKey::Null,
        3 => SortKey::String(read_bytes(payload)?.to_vec()),
        4 => SortKey::Id(
            std::str::from_utf8(read_bytes(payload)?)
                .map_err(|_| StorageError::corrupt("sort ID is not UTF-8"))?
                .to_owned(),
        ),
        5 => match decode_scalar_key(payload)? {
            ScalarKey::Number(value) => SortKey::Number(value),
            _ => return Err(StorageError::corrupt("sort number decodes as another type")),
        },
        6 => match payload {
            [0] => SortKey::Bool(false),
            [1] => SortKey::Bool(true),
            _ => return Err(StorageError::corrupt("invalid boolean sort key")),
        },
        7 => {
            if payload.len() != 4 {
                return Err(StorageError::corrupt("invalid date sort key length"));
            }
            let day = i32::from_le_bytes(payload.try_into().expect("four-byte date"));
            SortKey::Date(
                time::Date::from_julian_day(day)
                    .map_err(|_| StorageError::corrupt("sort date is out of range"))?,
            )
        }
        8 => {
            if payload.len() != 16 {
                return Err(StorageError::corrupt("invalid datetime sort key length"));
            }
            let nanos = i128::from_le_bytes(payload.try_into().expect("sixteen-byte datetime"));
            SortKey::Datetime(
                time::OffsetDateTime::from_unix_timestamp_nanos(nanos)
                    .map_err(|_| StorageError::corrupt("sort datetime is out of range"))?,
            )
        }
        9 => {
            if payload.len() != 8 {
                return Err(StorageError::corrupt("invalid unixtime sort key length"));
            }
            SortKey::Unixtime(i64::from_le_bytes(
                payload.try_into().expect("eight-byte unixtime"),
            ))
        }
        other => {
            return Err(StorageError::UnsupportedFormat(format!(
                "unknown sort-key tag {other}"
            )));
        }
    })
}

fn encode_path(path: &[String]) -> StorageResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let count = u32::try_from(path.len())
        .map_err(|_| StorageError::ResourceLimit("sort path segment count overflow".into()))?;
    bytes.extend_from_slice(&count.to_le_bytes());
    for segment in path {
        push_bytes(&mut bytes, segment.as_bytes())?;
    }
    Ok(bytes)
}

fn decode_path(bytes: &[u8]) -> StorageResult<Vec<String>> {
    if bytes.len() < 4 {
        return Err(StorageError::corrupt("truncated sort path"));
    }
    let count = u32::from_le_bytes(bytes[..4].try_into().expect("sort path count")) as usize;
    let mut at = 4usize;
    let mut path = Vec::with_capacity(count);
    for _ in 0..count {
        let length = read_u32_at(bytes, &mut at)? as usize;
        let end = at
            .checked_add(length)
            .ok_or_else(|| StorageError::corrupt("sort path segment overflow"))?;
        let segment = bytes
            .get(at..end)
            .ok_or_else(|| StorageError::corrupt("truncated sort path segment"))?;
        path.push(
            std::str::from_utf8(segment)
                .map_err(|_| StorageError::corrupt("sort path segment is not UTF-8"))?
                .to_owned(),
        );
        at = end;
    }
    if at != bytes.len() {
        return Err(StorageError::corrupt("sort path has trailing bytes"));
    }
    Ok(path)
}

fn encode_options(options: SortOptions) -> u32 {
    (options.strength as u32)
        | ((options.numeric as u32) << 8)
        | ((options.nulls as u32) << 16)
        | ((options.missing as u32) << 24)
}

fn decode_options(bits: u32) -> StorageResult<SortOptions> {
    let strength = match bits & 0xff {
        0 => CollationStrength::LocaleDefault,
        1 => CollationStrength::Primary,
        2 => CollationStrength::Secondary,
        3 => CollationStrength::Tertiary,
        4 => CollationStrength::Quaternary,
        5 => CollationStrength::Identical,
        _ => return Err(StorageError::corrupt("invalid collation strength")),
    };
    let nulls = match (bits >> 16) & 0xff {
        0 => Placement::First,
        1 => Placement::Last,
        _ => return Err(StorageError::corrupt("invalid null placement")),
    };
    let missing = match (bits >> 24) & 0xff {
        0 => Placement::First,
        1 => Placement::Last,
        _ => return Err(StorageError::corrupt("invalid missing placement")),
    };
    Ok(SortOptions {
        strength,
        numeric: bits & (1 << 8) != 0,
        nulls,
        missing,
    })
}

fn validate_order(order: &[u32], slot_count: u32, label: &str) -> StorageResult<()> {
    if order.len() != slot_count as usize {
        return Err(StorageError::InvalidArgument(format!(
            "{label} sort order does not contain every slot"
        )));
    }
    let mut seen = vec![false; slot_count as usize];
    for slot in order {
        let Some(seen) = seen.get_mut(*slot as usize) else {
            return Err(StorageError::InvalidArgument(format!(
                "{label} sort order slot is out of bounds"
            )));
        };
        if *seen {
            return Err(StorageError::InvalidArgument(format!(
                "{label} sort order contains a duplicate slot"
            )));
        }
        *seen = true;
    }
    Ok(())
}


/// One decoded sort column.
pub struct SortColumnView {
    backing: Backing,
    keys_base: u64,
    orders_base: u64,
    slot_count: u32,
    path: Vec<String>,
    kind: LogicalType,
    locale: String,
    options: SortOptions,
    collation_version: String,
    key_count: u32,
    offsets_offset: u64,
    offsets_len: u64,
    arena_offset: u64,
    arena_len: u64,
    ascending_offset: u64,
    ascending_len: u64,
    descending_offset: u64,
    descending_len: u64,
}


impl std::fmt::Debug for SortColumnView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SortColumnView")
            .field("path", &self.path)
            .field("kind", &self.kind)
            .field("locale", &self.locale)
            .field("key_count", &self.key_count)
            .finish_non_exhaustive()
    }
}


impl SortColumnView {
    pub fn path(&self) -> &[String] {
        &self.path
    }

    pub fn kind(&self) -> &LogicalType {
        &self.kind
    }

    pub fn locale(&self) -> &str {
        &self.locale
    }

    pub fn options(&self) -> SortOptions {
        self.options
    }

    pub fn collation_version(&self) -> &str {
        &self.collation_version
    }

    pub fn key_count(&self) -> u32 {
        self.key_count
    }

    /// Fetch one stored sort key by physical slot.
    pub fn key_at(&self, slot: u32) -> StorageResult<Option<SortKey>> {
        if slot >= self.key_count {
            return Ok(None);
        }
        let at = slot as u64 * 4;
        if at + 8 > self.offsets_len {
            return Err(StorageError::corrupt("sort-key offset array is truncated"));
        }
        let bounds = self
            .backing
            .read(self.keys_base + self.offsets_offset + at, 8)?;
        let start = u32::from_le_bytes(bounds[..4].try_into().expect("sort key start")) as u64;
        let end = u32::from_le_bytes(bounds[4..8].try_into().expect("sort key end")) as u64;
        if end < start || end > self.arena_len {
            return Err(StorageError::corrupt("sort-key arena range is invalid"));
        }
        let bytes = self
            .backing
            .read(self.keys_base + self.arena_offset + start, end - start)?;
        Ok(Some(decode_sort_key(bytes)?))
    }

    pub fn ascending(&self) -> SortOrderCursor<'_> {
        SortOrderCursor {
            column: self,
            offset: self.ascending_offset,
            length: self.ascending_len,
            index: 0,
        }
    }

    pub fn descending(&self) -> SortOrderCursor<'_> {
        SortOrderCursor {
            column: self,
            offset: self.descending_offset,
            length: self.descending_len,
            index: 0,
        }
    }

    pub fn ascending_slots(&self) -> StorageResult<Vec<u32>> {
        self.ascending().collect()
    }

    pub fn descending_slots(&self) -> StorageResult<Vec<u32>> {
        self.descending().collect()
    }

    pub fn verify_full(&self) -> StorageResult<()> {
        if self.key_count != self.slot_count {
            return Err(StorageError::corrupt(
                "sort column does not carry one key per slot",
            ));
        }
        for slot in 0..self.slot_count {
            self.key_at(slot)?;
        }
        let ascending = self.ascending_slots()?;
        validate_order(&ascending, self.slot_count, "ascending")
            .map_err(|error| StorageError::corrupt(error.to_string()))?;
        let descending = self.descending_slots()?;
        validate_order(&descending, self.slot_count, "descending")
            .map_err(|error| StorageError::corrupt(error.to_string()))?;
        Ok(())
    }

    fn order_slot(&self, offset: u64, index: u32) -> StorageResult<u32> {
        let bytes = self
            .backing
            .read(self.orders_base + offset + index as u64 * 4, 4)?;
        Ok(u32::from_le_bytes(
            bytes.try_into().expect("four-byte order slot"),
        ))
    }
}


/// Cursor over a persisted order array.
pub struct SortOrderCursor<'a> {
    column: &'a SortColumnView,
    offset: u64,
    length: u64,
    index: u32,
}


impl Iterator for SortOrderCursor<'_> {
    type Item = StorageResult<u32>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.index as u64 * 4 >= self.length {
            return None;
        }
        let slot = self.column.order_slot(self.offset, self.index);
        self.index = self.index.saturating_add(1);
        Some(slot)
    }
}


/// All sort columns in one frozen segment.
pub struct SortSectionView {
    slot_count: u32,
    columns: Vec<SortColumnView>,
}


impl std::fmt::Debug for SortSectionView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("SortSectionView")
            .field("columns", &self.columns.len())
            .finish_non_exhaustive()
    }
}


impl SortSectionView {
    pub fn open(
        backing: Backing,
        keys: SectionDescriptor,
        orders: SectionDescriptor,
        slot_count: u32,
    ) -> StorageResult<Self> {
        let keys_header = backing.read(keys.offset, SORT_KEYS_HEADER_LEN as u64)?;
        if keys_header[..4] != SORT_KEYS_MAGIC {
            return Err(StorageError::corrupt("sort-key magic mismatch"));
        }
        let version = u16::from_le_bytes([keys_header[4], keys_header[5]]);
        if version != SORT_KEYS_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "sort-key version {version}"
            )));
        }
        if keys_header[6..8] != [0, 0] || keys_header[12..16] != [0, 0, 0, 0] {
            return Err(StorageError::corrupt(
                "sort-key header reserved bytes are not zero",
            ));
        }
        let count = u32::from_le_bytes(keys_header[8..12].try_into().expect("sort count")) as usize;
        let directory_len = SORT_KEYS_HEADER_LEN
            .checked_add(
                count
                    .checked_mul(SORT_COLUMN_DESCRIPTOR_LEN)
                    .ok_or_else(|| StorageError::corrupt("sort descriptor directory overflow"))?,
            )
            .ok_or_else(|| StorageError::corrupt("sort descriptor directory overflow"))?;
        if directory_len as u64 > keys.length {
            return Err(StorageError::corrupt(
                "sort descriptor directory is truncated",
            ));
        }
        let orders_header = backing.read(orders.offset, SORT_ORDERS_HEADER_LEN as u64)?;
        if orders_header[..4] != SORT_ORDERS_MAGIC {
            return Err(StorageError::corrupt("sort-order magic mismatch"));
        }
        let order_version = u16::from_le_bytes([orders_header[4], orders_header[5]]);
        if order_version != SORT_ORDERS_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "sort-order version {order_version}"
            )));
        }
        let order_count =
            u32::from_le_bytes(orders_header[8..12].try_into().expect("order count")) as usize;
        if order_count != count {
            return Err(StorageError::corrupt(
                "sort-order column count differs from the key directory",
            ));
        }
        let directory = backing.read(keys.offset, directory_len as u64)?;
        let mut columns = Vec::with_capacity(count);
        for index in 0..count {
            let at = SORT_KEYS_HEADER_LEN + index * SORT_COLUMN_DESCRIPTOR_LEN;
            let slot = &directory[at..at + SORT_COLUMN_DESCRIPTOR_LEN];
            let kind = logical_type_from_codes(slot[0], slot[1])?;
            if slot[2] & 0b111 != 0b111 || slot[3] != 0 {
                return Err(StorageError::corrupt(
                    "sort column flags or reserved bytes are invalid",
                ));
            }
            let path_offset = read_u64(slot, 4)?;
            let path_len = read_u32(slot, 12)? as u64;
            let locale_offset = read_u64(slot, 16)?;
            let locale_len = read_u32(slot, 24)? as u64;
            let collation_offset = read_u64(slot, 28)?;
            let collation_len = read_u32(slot, 36)? as u64;
            if slot[44..48].iter().any(|byte| *byte != 0) {
                return Err(StorageError::corrupt(
                    "sort descriptor reserved bytes are not zero",
                ));
            }
            let options = decode_options(read_u32(slot, 40)?)?;
            let key_count = read_u32(slot, 48)?;
            if read_u32(slot, 52)? != 0 {
                return Err(StorageError::corrupt(
                    "sort descriptor reserved bytes are not zero",
                ));
            }
            let offsets_offset = read_u64(slot, 56)?;
            let offsets_len = read_u64(slot, 64)?;
            let arena_offset = read_u64(slot, 72)?;
            let arena_len = read_u64(slot, 80)?;
            let ascending_offset = read_u64(slot, 88)?;
            let ascending_len = read_u64(slot, 96)?;
            let descending_offset = read_u64(slot, 104)?;
            let descending_len = read_u64(slot, 112)?;
            for (offset, length) in [
                (path_offset, path_len),
                (locale_offset, locale_len),
                (collation_offset, collation_len),
                (offsets_offset, offsets_len),
                (arena_offset, arena_len),
            ] {
                if offset
                    .checked_add(length)
                    .is_none_or(|end| end > keys.length)
                {
                    return Err(StorageError::corrupt(
                        "sort descriptor reference lies outside its section",
                    ));
                }
            }
            for (offset, length) in [
                (ascending_offset, ascending_len),
                (descending_offset, descending_len),
            ] {
                if offset
                    .checked_add(length)
                    .is_none_or(|end| end > orders.length)
                    || length % 4 != 0
                    || length / 4 != key_count as u64
                {
                    return Err(StorageError::corrupt("sort order array length is invalid"));
                }
            }
            if offsets_len != (key_count as u64 + 1) * 4 {
                return Err(StorageError::corrupt(
                    "sort-key offset array length is invalid",
                ));
            }
            let path = decode_path(backing.read(keys.offset + path_offset, path_len)?)?;
            let locale =
                std::str::from_utf8(backing.read(keys.offset + locale_offset, locale_len)?)
                    .map_err(|_| StorageError::corrupt("sort locale is not UTF-8"))?
                    .to_owned();
            let collation_version =
                std::str::from_utf8(backing.read(keys.offset + collation_offset, collation_len)?)
                    .map_err(|_| StorageError::corrupt("sort collation version is not UTF-8"))?
                    .to_owned();
            if key_count != slot_count {
                return Err(StorageError::corrupt(
                    "sort column does not carry one key per slot",
                ));
            }
            columns.push(SortColumnView {
                backing: backing.clone(),
                keys_base: keys.offset,
                orders_base: orders.offset,
                slot_count,
                path,
                kind,
                locale,
                options,
                collation_version,
                key_count,
                offsets_offset,
                offsets_len,
                arena_offset,
                arena_len,
                ascending_offset,
                ascending_len,
                descending_offset,
                descending_len,
            });
        }
        Ok(Self {
            slot_count,
            columns,
        })
    }

    pub fn columns(&self) -> &[SortColumnView] {
        &self.columns
    }

    pub fn column(&self, path: &[String]) -> Option<&SortColumnView> {
        self.columns
            .iter()
            .find(|column| column.path.as_slice() == path)
    }

    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }

    pub fn verify_full(&self) -> StorageResult<()> {
        for column in &self.columns {
            column.verify_full()?;
        }
        Ok(())
    }
}


fn push_bytes(bytes: &mut Vec<u8>, value: &[u8]) -> StorageResult<()> {
    let length = u32::try_from(value.len())
        .map_err(|_| StorageError::ResourceLimit("sort key blob exceeds 4 GiB".into()))?;
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(value);
    Ok(())
}

fn read_bytes(bytes: &[u8]) -> StorageResult<&[u8]> {
    if bytes.len() < 4 {
        return Err(StorageError::corrupt("truncated sort-key blob length"));
    }
    let length = u32::from_le_bytes(bytes[..4].try_into().expect("blob length")) as usize;
    bytes
        .get(4..)
        .filter(|value| value.len() == length)
        .ok_or_else(|| StorageError::corrupt("truncated sort-key blob"))
}

fn read_u32_at(bytes: &[u8], at: &mut usize) -> StorageResult<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| StorageError::corrupt("sort path offset overflow"))?;
    let slice = bytes
        .get(*at..end)
        .ok_or_else(|| StorageError::corrupt("truncated sort path length"))?;
    *at = end;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte sort path length"),
    ))
}

fn read_u32(bytes: &[u8], at: usize) -> StorageResult<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| StorageError::corrupt("sort u32 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated sort u32"))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte sort field"),
    ))
}

fn read_u64(bytes: &[u8], at: usize) -> StorageResult<u64> {
    let end = at
        .checked_add(8)
        .ok_or_else(|| StorageError::corrupt("sort u64 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated sort u64"))?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("eight-byte sort field"),
    ))
}
