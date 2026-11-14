//! Borrowed typed-posting readers over packed segments.

use std::collections::{BTreeMap, HashMap};

use roaring::RoaringBitmap;

use crate::query::Operator;
use std::sync::Arc;

use crate::model::{LogicalType, Number};
use crate::posting::ScalarKey;

use super::backing::Backing;
use super::dictionary::{DictionaryReader, KeyOrder};
use super::error::{StorageError, StorageResult};
use super::format::SectionDescriptor;
use super::posting_builder::{
    FIELD_DIRECTORY_VERSION, FIELD_EMPTY_STRING_COMPLETE, FIELD_HAS_LENGTHS, FIELD_HAS_STRUCTURAL, FIELD_LENGTH_COMPLETE,
    FIELD_VALUE_COMPLETE, FIELD_VALUES_INDEXED, POSTING_FIELD_DESCRIPTOR_LEN,
    POSTING_PAYLOAD_MAGIC, POSTING_PAYLOAD_VERSION,
};
use super::record_set::RecordSetRef;
use super::record_set::RecordSetView;
use super::scalar::encode_scalar_key;

/// One decoded field directory entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldDefinitionView {
    pub ordinal: u32,
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub multi: bool,
}

/// Map one logical type to `(kind code, array element code)`.
pub fn logical_type_codes(kind: &LogicalType) -> (u8, u8) {
    match kind {
        LogicalType::Unknown => (0, 0),
        LogicalType::String => (1, 0),
        LogicalType::Id => (2, 0),
        LogicalType::Number => (3, 0),
        LogicalType::Bool => (4, 0),
        LogicalType::Date => (5, 0),
        LogicalType::Datetime => (6, 0),
        LogicalType::Unixtime => (7, 0),
        LogicalType::Object => (8, 0),
        LogicalType::Array(element) => (9, logical_type_codes(element).0),
    }
}

/// Reconstruct one logical type from its persisted codes.
pub fn logical_type_from_codes(kind: u8, element: u8) -> StorageResult<LogicalType> {
    Ok(match kind {
        0 => LogicalType::Unknown,
        1 => LogicalType::String,
        2 => LogicalType::Id,
        3 => LogicalType::Number,
        4 => LogicalType::Bool,
        5 => LogicalType::Date,
        6 => LogicalType::Datetime,
        7 => LogicalType::Unixtime,
        8 => LogicalType::Object,
        9 => LogicalType::Array(Box::new(logical_type_from_codes(element, 0)?)),
        other => {
            return Err(StorageError::UnsupportedFormat(format!(
                "unknown logical type code {other}"
            )))
        }
    })
}

/// All typed posting fields in one frozen segment.
pub struct PostingSectionView {
    backing: Backing,
    slot_count: u32,
    fields: Vec<FieldPostingsView>,
    by_path: HashMap<Vec<String>, usize>,
    by_ordinal: BTreeMap<u32, usize>,
}

impl std::fmt::Debug for PostingSectionView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PostingSectionView")
            .field("slot_count", &self.slot_count)
            .field("fields", &self.fields.len())
            .finish_non_exhaustive()
    }
}

impl PostingSectionView {
    pub fn open(
        backing: Backing,
        payloads: SectionDescriptor,
        field_directory: SectionDescriptor,
        slot_count: u32,
    ) -> StorageResult<Self> {
        let header = backing.read(payloads.offset, 16)?;
        if header[..4] != POSTING_PAYLOAD_MAGIC {
            return Err(StorageError::corrupt("posting payload magic mismatch"));
        }
        let version = u16::from_le_bytes([header[4], header[5]]);
        if version != POSTING_PAYLOAD_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "posting payload version {version}"
            )));
        }
        if header[8..16].iter().any(|byte| *byte != 0) {
            return Err(StorageError::corrupt(
                "posting payload reserved bytes are not zero",
            ));
        }
        let field_count = u16::from_le_bytes([header[6], header[7]]) as usize;
        let definitions = parse_field_directory(&backing, field_directory)?;
        if definitions.len() != field_count {
            return Err(StorageError::corrupt(
                "posting field directory does not match the payload header",
            ));
        }
        let expected_len = 16 + field_count * POSTING_FIELD_DESCRIPTOR_LEN;
        if expected_len as u64 > payloads.length {
            return Err(StorageError::corrupt(
                "posting field directory exceeds its section",
            ));
        }
        let directory = backing.read(payloads.offset, expected_len as u64)?;
        let mut fields = Vec::with_capacity(field_count);
        let mut by_ordinal = BTreeMap::new();
        let mut by_path = HashMap::new();
        for (index, definition) in definitions.into_iter().enumerate() {
            let at = 16 + index * POSTING_FIELD_DESCRIPTOR_LEN;
            let descriptor = &directory[at..at + POSTING_FIELD_DESCRIPTOR_LEN];
            if descriptor[204..208].iter().any(|byte| *byte != 0) {
                return Err(StorageError::corrupt(
                    "posting field descriptor reserved bytes are not zero",
                ));
            }
            let ordinal = u32::from_le_bytes(descriptor[..4].try_into().expect("field ordinal"));
            if ordinal != definition.ordinal {
                return Err(StorageError::corrupt(
                    "posting field ordinal differs from the field directory",
                ));
            }
            let flags = u32::from_le_bytes(descriptor[4..8].try_into().expect("field flags"));
            let presence = RecordSetRef::decode(&descriptor[8..40])?;
            let nulls = RecordSetRef::decode(&descriptor[40..72])?;
            let empty_selections = RecordSetRef::decode(&descriptor[72..104])?;
            let empty_string = RecordSetRef::decode(&descriptor[104..136])?;
            let values_offset =
                u64::from_le_bytes(descriptor[136..144].try_into().expect("values offset"));
            let values_len =
                u64::from_le_bytes(descriptor[144..152].try_into().expect("values length"));
            let lengths_offset =
                u64::from_le_bytes(descriptor[152..160].try_into().expect("lengths offset"));
            let lengths_len =
                u64::from_le_bytes(descriptor[160..168].try_into().expect("lengths length"));
            let structural_offset = u64::from_le_bytes(
                descriptor[168..176]
                    .try_into()
                    .expect("structural offset"),
            );
            let structural_len = u64::from_le_bytes(
                descriptor[176..184]
                    .try_into()
                    .expect("structural length"),
            );
            let arena_offset =
                u64::from_le_bytes(descriptor[184..192].try_into().expect("arena offset"));
            let arena_len = u64::from_le_bytes(descriptor[192..200].try_into().expect("arena length"));
            let covered_records =
                u32::from_le_bytes(descriptor[200..204].try_into().expect("covered records"));
            for (offset, length) in [
                (values_offset, values_len),
                (lengths_offset, lengths_len),
                (structural_offset, structural_len),
                (arena_offset, arena_len),
            ] {
                if offset
                    .checked_add(length)
                    .is_none_or(|end| end > payloads.length)
                {
                    return Err(StorageError::corrupt(
                        "posting field payload lies outside its section",
                    ));
                }
            }
            let values = DictionaryReader::open(
                backing.clone(),
                payloads.offset + values_offset,
                values_len,
            )?;
            let lengths = if flags & FIELD_HAS_LENGTHS != 0 {
                Some(DictionaryReader::open(
                    backing.clone(),
                    payloads.offset + lengths_offset,
                    lengths_len,
                )?)
            } else {
                None
            };
            let structural = if flags & FIELD_HAS_STRUCTURAL != 0 {
                parse_structural(
                    &backing.read(payloads.offset + structural_offset, structural_len)?,
                )?
            } else {
                BTreeMap::new()
            };
            let field = FieldPostingsView {
                ordinal,
                path: definition.path,
                kind: definition.kind,
                multi: definition.multi,
                flags,
                presence,
                nulls,
                empty_selections,
                empty_string,
                values,
                lengths,
                structural,
                arena_base: payloads.offset + arena_offset,
                arena_len,
                slot_count,
                covered_records,
                backing: backing.clone(),
            };
            // Validate fixed record-set descriptors without touching payloads.
            for reference in [
                field.presence,
                field.nulls,
                field.empty_selections,
                field.empty_string,
            ] {
                let _ = field.view(reference)?;
            }
            by_ordinal.insert(ordinal, fields.len());
            by_path.insert(field.path.clone(), fields.len());
            fields.push(field);
        }
        Ok(Self {
            backing,
            slot_count,
            fields,
            by_path,
            by_ordinal,
        })
    }

    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }

    pub fn fields(&self) -> &[FieldPostingsView] {
        &self.fields
    }

    pub fn field(&self, path: &[String]) -> Option<&FieldPostingsView> {
        self.by_path.get(path).map(|index| &self.fields[*index])
    }

    pub fn field_by_ordinal(&self, ordinal: u32) -> Option<&FieldPostingsView> {
        self.by_ordinal
            .get(&ordinal)
            .map(|index| &self.fields[*index])
    }

    /// Full structural verification, including every posting block.
    pub fn verify_full(&self) -> StorageResult<()> {
        for field in &self.fields {
            field.verify_full()?;
        }
        Ok(())
    }

    pub fn backing(&self) -> &Backing {
        &self.backing
    }
}

/// One field's borrowed postings.
pub struct FieldPostingsView {
    ordinal: u32,
    path: Vec<String>,
    kind: LogicalType,
    multi: bool,
    flags: u32,
    presence: RecordSetRef,
    nulls: RecordSetRef,
    empty_selections: RecordSetRef,
    empty_string: RecordSetRef,
    values: DictionaryReader,
    lengths: Option<DictionaryReader>,
    structural: BTreeMap<u8, RecordSetRef>,
    arena_base: u64,
    arena_len: u64,
    slot_count: u32,
    covered_records: u32,
    backing: Backing,
}

impl std::fmt::Debug for FieldPostingsView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("FieldPostingsView")
            .field("ordinal", &self.ordinal)
            .field("path", &self.path)
            .field("kind", &self.kind)
            .field("values", &self.values.entry_count())
            .finish_non_exhaustive()
    }
}

impl FieldPostingsView {
    pub fn ordinal(&self) -> u32 {
        self.ordinal
    }

    pub fn path(&self) -> &[String] {
        &self.path
    }

    pub fn kind(&self) -> &LogicalType {
        &self.kind
    }

    pub fn multi(&self) -> bool {
        self.multi
    }

    pub fn covered_records(&self) -> u32 {
        self.covered_records
    }

    pub fn values_indexed(&self) -> bool {
        self.flags & FIELD_VALUES_INDEXED != 0
    }

    pub fn value_complete(&self) -> bool {
        self.flags & FIELD_VALUE_COMPLETE != 0
    }

    pub fn length_complete(&self) -> bool {
        self.flags & FIELD_LENGTH_COMPLETE != 0
    }

    pub fn empty_string_complete(&self) -> bool {
        self.flags & FIELD_EMPTY_STRING_COMPLETE != 0
    }

    pub fn values(&self) -> &DictionaryReader {
        &self.values
    }

    pub fn lengths(&self) -> Option<&DictionaryReader> {
        self.lengths.as_ref()
    }

    pub fn structural_keys(&self) -> impl Iterator<Item = u8> + '_ {
        self.structural.keys().copied()
    }

    pub fn presence(&self) -> StorageResult<RecordSetView<'_>> {
        self.view(self.presence)
    }

    pub fn nulls(&self) -> StorageResult<RecordSetView<'_>> {
        self.view(self.nulls)
    }

    pub fn empty_selections(&self) -> StorageResult<RecordSetView<'_>> {
        self.view(self.empty_selections)
    }

    pub fn empty_string(&self) -> StorageResult<RecordSetView<'_>> {
        self.view(self.empty_string)
    }

    pub fn structural(&self, key: u8) -> StorageResult<Option<RecordSetView<'_>>> {
        let Some(reference) = self.structural.get(&key).copied() else {
            return Ok(None);
        };
        Ok(Some(self.view(reference)?))
    }

    /// Return the posting for one exact scalar value.
    pub fn value(&self, value: &ScalarKey) -> StorageResult<Option<RecordSetView<'_>>> {
        let encoded = encode_scalar_key(value)?;
        self.value_encoded(&encoded)
    }

    pub fn value_encoded(&self, encoded: &[u8]) -> StorageResult<Option<RecordSetView<'_>>> {
        let Some(bytes) = self.values.get(encoded)? else {
            return Ok(None);
        };
        let reference = RecordSetRef::decode(&bytes)?;
        Ok(Some(self.view(reference)?))
    }

    pub fn length(&self, length: &Number) -> StorageResult<Option<RecordSetView<'_>>> {
        let Some(dictionary) = &self.lengths else {
            return Ok(None);
        };
        let encoded = encode_scalar_key(&ScalarKey::Number(length.clone()))?;
        let Some(bytes) = dictionary.get(&encoded)? else {
            return Ok(None);
        };
        Ok(Some(self.view(RecordSetRef::decode(&bytes)?)?))
    }

    pub fn contains_value(&self, slot: u32, value: &ScalarKey) -> StorageResult<bool> {
        let Some(view) = self.value(value)? else {
            return Ok(false);
        };
        view.contains(slot)
    }

    pub fn value_cardinality(&self, value: &ScalarKey) -> StorageResult<u64> {
        let Some(view) = self.value(value)? else {
            return Ok(0);
        };
        Ok(view.cardinality())
    }

    /// Exact equality posting as a query-owned bitmap.
    pub fn exact(&self, value: &ScalarKey) -> StorageResult<RoaringBitmap> {
        let mut result = RoaringBitmap::new();
        if let Some(view) = self.value(value)? {
            view.union_into(&mut result)?;
        }
        Ok(result)
    }

    /// Inequality over explicit presence, excluding nulls and the equal set.
    pub fn not_equal(&self, value: &ScalarKey) -> StorageResult<RoaringBitmap> {
        let mut result = RoaringBitmap::new();
        self.presence()?.union_into(&mut result)?;
        let mut nulls = RoaringBitmap::new();
        self.nulls()?.union_into(&mut nulls)?;
        result -= nulls;
        result -= self.exact(value)?;
        Ok(result)
    }

    /// Range posting with the engine's same-logical-type boundary rule.
    pub fn range(&self, value: &ScalarKey, operator: Operator) -> StorageResult<RoaringBitmap> {
        self.range_restricted(value, operator, None).map(|(result, _)| result)
    }

    /// Range posting with an optional input restriction and visited-key count.
    pub fn range_restricted(
        &self,
        value: &ScalarKey,
        operator: Operator,
        restriction: Option<&RoaringBitmap>,
    ) -> StorageResult<(RoaringBitmap, usize)> {
        let mut result = RoaringBitmap::new();
        let mut visited = 0usize;
        match operator {
            Operator::Eq => {
                let exact = self.exact(value)?;
                result = restriction.map_or(exact.clone(), |input| &exact & input);
                visited = 1;
            }
            Operator::Ne => {
                let not_equal = self.not_equal(value)?;
                result = restriction.map_or(not_equal.clone(), |input| &not_equal & input);
                visited = 2;
            }
            Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le => {
                let encoded = encode_scalar_key(value)?;
                let (lower, upper) = match operator {
                    Operator::Gt => (std::ops::Bound::Excluded(encoded), std::ops::Bound::Unbounded),
                    Operator::Ge => (std::ops::Bound::Included(encoded), std::ops::Bound::Unbounded),
                    Operator::Lt => (std::ops::Bound::Unbounded, std::ops::Bound::Excluded(encoded)),
                    Operator::Le => (std::ops::Bound::Unbounded, std::ops::Bound::Included(encoded)),
                    _ => unreachable!(),
                };
                let discriminant = std::mem::discriminant(value);
                let descending = matches!(operator, Operator::Lt | Operator::Le);
                let entries = if descending {
                    self.values
                        .range_reverse(owned_bound_ref(&lower), owned_bound_ref(&upper))
                        .collect::<StorageResult<Vec<_>>>()?
                } else {
                    self.values
                        .range(owned_bound_ref(&lower), owned_bound_ref(&upper))
                        .collect::<StorageResult<Vec<_>>>()?
                };
                for (key, ref_bytes) in entries {
                    let key = super::scalar::decode_scalar_key(&key)?;
                    if std::mem::discriminant(&key) != discriminant {
                        break;
                    }
                    visited += 1;
                    let reference = RecordSetRef::decode(&ref_bytes)?;
                    let posting = self.view(reference)?;
                    if let Some(input) = restriction {
                        let mut all = RoaringBitmap::new();
                        posting.union_into(&mut all)?;
                        result |= &all & input;
                    } else {
                        posting.union_into(&mut result)?;
                    }
                }
            }
            _ => {}
        }
        Ok((result, visited))
    }

    /// Ordered value keys with cardinalities, without materializing postings.
    pub fn value_keys(
        &self,
        lower: std::ops::Bound<&ScalarKey>,
        upper: std::ops::Bound<&ScalarKey>,
        descending: bool,
    ) -> StorageResult<Vec<(ScalarKey, u64)>> {
        let encode = |bound: std::ops::Bound<&ScalarKey>| -> StorageResult<std::ops::Bound<Vec<u8>>> {
            Ok(match bound {
                std::ops::Bound::Included(key) => {
                    std::ops::Bound::Included(encode_scalar_key(key)?)
                }
                std::ops::Bound::Excluded(key) => {
                    std::ops::Bound::Excluded(encode_scalar_key(key)?)
                }
                std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
            })
        };
        let lower = encode(lower)?;
        let upper = encode(upper)?;
        let entries = if descending {
            self.values
                .range_reverse(owned_bound_ref(&lower), owned_bound_ref(&upper))
                .collect::<StorageResult<Vec<_>>>()?
        } else {
            self.values
                .range(owned_bound_ref(&lower), owned_bound_ref(&upper))
                .collect::<StorageResult<Vec<_>>>()?
        };
        let mut result = Vec::with_capacity(entries.len());
        for (key, ref_bytes) in entries {
            let key = super::scalar::decode_scalar_key(&key)?;
            let reference = RecordSetRef::decode(&ref_bytes)?;
            result.push((key, reference.cardinality()));
        }
        Ok(result)
    }

    pub fn length_keys(
        &self,
        lower: std::ops::Bound<&Number>,
        upper: std::ops::Bound<&Number>,
    ) -> StorageResult<Vec<(Number, u64)>> {
        let Some(dictionary) = &self.lengths else {
            return Ok(Vec::new());
        };
        let encode = |bound: std::ops::Bound<&Number>| -> StorageResult<std::ops::Bound<Vec<u8>>> {
            Ok(match bound {
                std::ops::Bound::Included(value) => std::ops::Bound::Included(encode_scalar_key(
                    &ScalarKey::Number(value.clone()),
                )?),
                std::ops::Bound::Excluded(value) => std::ops::Bound::Excluded(encode_scalar_key(
                    &ScalarKey::Number(value.clone()),
                )?),
                std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
            })
        };
        let lower = encode(lower)?;
        let upper = encode(upper)?;
        let entries = dictionary
            .range(owned_bound_ref(&lower), owned_bound_ref(&upper))
            .collect::<StorageResult<Vec<_>>>()?;
        let mut result = Vec::with_capacity(entries.len());
        for (key, ref_bytes) in entries {
            let ScalarKey::Number(value) = super::scalar::decode_scalar_key(&key)? else {
                return Err(StorageError::corrupt(
                    "length dictionary contains a non-number key",
                ));
            };
            result.push((value, RecordSetRef::decode(&ref_bytes)?.cardinality()));
        }
        Ok(result)
    }

    pub fn verify_full(&self) -> StorageResult<()> {
        for reference in [
            self.presence,
            self.nulls,
            self.empty_selections,
            self.empty_string,
        ] {
            self.view(reference)?.verify_full()?;
        }
        for reference in self.structural.values() {
            self.view(*reference)?.verify_full()?;
        }
        for entry in self
            .values
            .range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        {
            let (_, bytes) = entry?;
            self.view(RecordSetRef::decode(&bytes)?)?.verify_full()?;
        }
        if let Some(lengths) = &self.lengths {
            for entry in lengths.range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded) {
                let (_, bytes) = entry?;
                self.view(RecordSetRef::decode(&bytes)?)?.verify_full()?;
            }
        }
        Ok(())
    }

    fn view(&self, reference: RecordSetRef) -> StorageResult<RecordSetView<'_>> {
        let arena = self.arena()?;
        reference.view(arena, self.slot_count)
    }

    fn arena(&self) -> StorageResult<&[u8]> {
        self.backing.read(self.arena_base, self.arena_len)
    }
}

fn parse_field_directory(
    backing: &Backing,
    descriptor: SectionDescriptor,
) -> StorageResult<Vec<FieldDefinitionView>> {
    let mut fields = Vec::new();
    let mut offset = 0u64;
    let header = read_section(backing, descriptor, offset, 8)?;
    let version = u16::from_le_bytes([header[0], header[1]]);
    if version != FIELD_DIRECTORY_VERSION {
        return Err(StorageError::UnsupportedFormat(format!(
            "field directory version {version}"
        )));
    }
    if header[2..4] != [0, 0] {
        return Err(StorageError::corrupt(
            "field directory reserved bytes are not zero",
        ));
    }
    let count = u32::from_le_bytes(header[4..8].try_into().expect("field count")) as usize;
    offset += 8;
    for _ in 0..count {
        let head = read_section(backing, descriptor, offset, 12)?;
        let ordinal = u32::from_le_bytes(head[..4].try_into().expect("field ordinal"));
        let kind = logical_type_from_codes(head[4], head[5])?;
        if head[7] != 0 {
            return Err(StorageError::corrupt(
                "field directory reserved byte is not zero",
            ));
        }
        let multi = head[6] != 0;
        let segments = u32::from_le_bytes(head[8..12].try_into().expect("path segments")) as usize;
        offset += 12;
        let mut path = Vec::with_capacity(segments);
        for _ in 0..segments {
            let length_bytes = read_section(backing, descriptor, offset, 4)?;
            offset += 4;
            let length =
                u32::from_le_bytes(length_bytes.try_into().expect("path segment length")) as u64;
            let bytes = read_section(backing, descriptor, offset, length)?;
            offset += length;
            let segment = std::str::from_utf8(bytes)
                .map_err(|_| StorageError::corrupt("field path segment is not UTF-8"))?;
            path.push(segment.to_owned());
        }
        fields.push(FieldDefinitionView {
            ordinal,
            path,
            kind,
            multi,
        });
    }
    if offset != descriptor.length {
        return Err(StorageError::corrupt(
            "field directory has trailing bytes",
        ));
    }
    Ok(fields)
}

fn parse_structural(bytes: &[u8]) -> StorageResult<BTreeMap<u8, RecordSetRef>> {
    if bytes.len() < 4 {
        return Err(StorageError::corrupt("truncated structural posting table"));
    }
    let count = u32::from_le_bytes(bytes[..4].try_into().expect("structural count")) as usize;
    let expected = 4usize
        .checked_add(count.checked_mul(40).ok_or_else(|| {
            StorageError::corrupt("structural posting count overflow")
        })?)
        .ok_or_else(|| StorageError::corrupt("structural posting length overflow"))?;
    if bytes.len() != expected {
        return Err(StorageError::corrupt(
            "structural posting table length mismatch",
        ));
    }
    let mut result = BTreeMap::new();
    for index in 0..count {
        let at = 4 + index * 40;
        let key = bytes[at];
        if bytes[at + 1..at + 8].iter().any(|byte| *byte != 0) {
            return Err(StorageError::corrupt(
                "structural posting reserved bytes are not zero",
            ));
        }
        let reference = RecordSetRef::decode(&bytes[at + 8..at + 40])?;
        if result.insert(key, reference).is_some() {
            return Err(StorageError::corrupt(
                "duplicate structural posting key",
            ));
        }
    }
    Ok(result)
}

fn read_section(
    backing: &Backing,
    descriptor: SectionDescriptor,
    offset: u64,
    length: u64,
) -> StorageResult<&[u8]> {
    let end = offset
        .checked_add(length)
        .ok_or_else(|| StorageError::corrupt("field directory reference overflow"))?;
    if end > descriptor.length {
        return Err(StorageError::corrupt(
            "field directory reference is out of bounds",
        ));
    }
    backing.read(descriptor.offset + offset, length)
}

/// Shared helper for callers that need an owned posting view over one field.
pub fn field_definition(view: &FieldPostingsView) -> FieldDefinitionView {
    FieldDefinitionView {
        ordinal: view.ordinal,
        path: view.path.clone(),
        kind: view.kind.clone(),
        multi: view.multi,
    }
}

/// Keep the `Arc<[u8]>` conversion available for resident callers.
pub fn resident_backing(bytes: Arc<[u8]>) -> Backing {
    Backing::resident(bytes)
}

/// Exercise the planned key order of a posting dictionary.
pub fn value_key_order(view: &FieldPostingsView) -> KeyOrder {
    view.values.order()
}

fn owned_bound_ref(bound: &std::ops::Bound<Vec<u8>>) -> std::ops::Bound<&[u8]> {
    match bound {
        std::ops::Bound::Included(value) => std::ops::Bound::Included(value.as_slice()),
        std::ops::Bound::Excluded(value) => std::ops::Bound::Excluded(value.as_slice()),
        std::ops::Bound::Unbounded => std::ops::Bound::Unbounded,
    }
}
