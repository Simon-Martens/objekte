//! Packed relation adjacency and target identities.
//!
//! Cross-run targets are identified by logical ID; a resolved slot is only a
//! convenience for the run that wrote the edge. Readers resolve targets against
//! the current published view.

use super::backing::Backing;
use super::error::{StorageError, StorageResult};
use super::format::SectionDescriptor;
use super::record_set::{RecordSetRef, RecordSetTag, RecordSetView, encode_record_set};

pub const RELATION_MAGIC: [u8; 4] = *b"OBRL";
pub const RELATION_VERSION: u16 = 1;
pub const RELATION_DESCRIPTOR_LEN: usize = 256;
pub const RELATION_EDGE_LEN: usize = 24;


/// One relation edge supplied to the builder.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationEdgeInput {
    pub source_slot: u32,
    pub target_slot: Option<u32>,
    pub target_id: String,
    pub positions: Vec<u32>,
}


/// One relation declaration and its adjacency.
#[derive(Clone, Debug)]
pub struct RelationInput {
    pub id_ordinal: u32,
    pub target_collection: u32,
    pub path: Vec<String>,
    pub cardinality: u8,
    pub reverse_alias: Option<String>,
    pub offsets: Vec<u32>,
    pub edges: Vec<RelationEdgeInput>,
    pub present: Vec<u32>,
    pub nulls: Vec<u32>,
    pub empty_arrays: Vec<u32>,
    pub unavailable: Vec<u32>,
    pub unresolved_count: u64,
}


/// Build the packed relation section. Target IDs, paths, and aliases append to
/// the segment's shared string arena.
pub fn build_relations(
    relations: &[RelationInput],
    slot_count: u32,
    string_arena: &mut Vec<u8>,
) -> StorageResult<Vec<u8>> {
    let directory_len = 16 + relations.len() * RELATION_DESCRIPTOR_LEN;
    let mut payload = Vec::new();
    let mut descriptors = Vec::with_capacity(relations.len());
    for relation in relations {
        if relation.offsets.len() != slot_count as usize + 1 {
            return Err(StorageError::InvalidArgument(
                "relation offsets must contain one entry per slot plus one".into(),
            ));
        }
        if relation.offsets.first() != Some(&0)
            || relation.offsets.last().copied() != Some(relation.edges.len() as u32)
            || relation.offsets.windows(2).any(|pair| pair[0] > pair[1])
        {
            return Err(StorageError::InvalidArgument(
                "relation offsets are not a monotonic prefix table".into(),
            ));
        }
        let mut arena = Vec::new();
        let present = embed_relation_set(&relation.present, slot_count, &mut arena)?;
        let nulls = embed_relation_set(&relation.nulls, slot_count, &mut arena)?;
        let empty_arrays = embed_relation_set(&relation.empty_arrays, slot_count, &mut arena)?;
        let unavailable = embed_relation_set(&relation.unavailable, slot_count, &mut arena)?;
        let path = encode_path(&relation.path)?;
        let path_offset = string_arena.len() as u64;
        string_arena.extend_from_slice(&path);
        let path_len = path.len() as u32;
        let (reverse_offset, reverse_len) = match &relation.reverse_alias {
            Some(alias) => {
                let offset = string_arena.len() as u64;
                string_arena.extend_from_slice(alias.as_bytes());
                (offset, alias.len() as u32)
            }
            None => (0, 0),
        };
        let offsets_start = payload.len();
        for offset in &relation.offsets {
            payload.extend_from_slice(&offset.to_le_bytes());
        }
        let offsets_len = payload.len() - offsets_start;
        let offsets_offset = directory_len + offsets_start;
        let mut edge_positions = Vec::new();
        let mut edge_rows = Vec::with_capacity(relation.edges.len() * RELATION_EDGE_LEN);
        for edge in &relation.edges {
            if edge.source_slot >= slot_count
                || edge.target_slot.is_some_and(|slot| slot >= slot_count)
            {
                return Err(StorageError::InvalidArgument(
                    "relation edge slot is out of bounds".into(),
                ));
            }
            let target_id_offset = string_arena.len() as u64;
            string_arena.extend_from_slice(edge.target_id.as_bytes());
            let target_id_len = edge.target_id.len() as u32;
            let positions_offset = edge_positions.len() as u32;
            for position in &edge.positions {
                edge_positions.extend_from_slice(&position.to_le_bytes());
            }
            let target_slot = edge.target_slot.unwrap_or(u32::MAX);
            edge_rows.extend_from_slice(&target_slot.to_le_bytes());
            edge_rows.extend_from_slice(&(target_id_offset as u32).to_le_bytes());
            edge_rows.extend_from_slice(&target_id_len.to_le_bytes());
            edge_rows.extend_from_slice(&positions_offset.to_le_bytes());
            edge_rows.extend_from_slice(&(edge.positions.len() as u32).to_le_bytes());
            edge_rows.push(u8::from(target_slot != u32::MAX));
            edge_rows.extend_from_slice(&[0u8; 3]);
        }
        let edges_start = payload.len();
        payload.extend_from_slice(&edge_rows);
        let edges_len = edge_rows.len();
        let edges_offset = directory_len + edges_start;
        let positions_start = payload.len();
        payload.extend_from_slice(&edge_positions);
        let positions_len = edge_positions.len();
        let positions_offset = directory_len + positions_start;
        let arena_start = payload.len();
        payload.extend_from_slice(&arena);
        let arena_offset = directory_len + arena_start;
        descriptors.push((
            relation,
            present,
            nulls,
            empty_arrays,
            unavailable,
            path_offset,
            path_len,
            reverse_offset,
            reverse_len,
            offsets_offset,
            offsets_len,
            edges_offset,
            edges_len,
            positions_offset,
            positions_len,
            arena_offset,
            arena.len(),
        ));
    }
    // Positions are currently appended before edges; move them after the edge
    // rows so offsets remain stable for readers.
    let mut section = vec![0u8; directory_len];
    section[..4].copy_from_slice(&RELATION_MAGIC);
    section[4..6].copy_from_slice(&RELATION_VERSION.to_le_bytes());
    section[6..8].fill(0);
    section[8..12].copy_from_slice(&(relations.len() as u32).to_le_bytes());
    section[12..16].fill(0);
    for (index, descriptor) in descriptors.into_iter().enumerate() {
        let at = 16 + index * RELATION_DESCRIPTOR_LEN;
        let slot = &mut section[at..at + RELATION_DESCRIPTOR_LEN];
        slot[..4].copy_from_slice(&descriptor.0.id_ordinal.to_le_bytes());
        slot[4..8].copy_from_slice(&descriptor.0.target_collection.to_le_bytes());
        slot[8..12].copy_from_slice(&slot_count.to_le_bytes());
        slot[12] = descriptor.0.cardinality;
        slot[13..16].fill(0);
        slot[16..24].copy_from_slice(&descriptor.5.to_le_bytes());
        slot[24..28].copy_from_slice(&descriptor.6.to_le_bytes());
        slot[28..36].copy_from_slice(&descriptor.7.to_le_bytes());
        slot[36..40].copy_from_slice(&descriptor.8.to_le_bytes());
        slot[40..72].copy_from_slice(&descriptor.1.encode());
        slot[72..104].copy_from_slice(&descriptor.2.encode());
        slot[104..136].copy_from_slice(&descriptor.3.encode());
        slot[136..168].copy_from_slice(&descriptor.4.encode());
        slot[168..176].copy_from_slice(&(descriptor.9 as u64).to_le_bytes());
        slot[176..184].copy_from_slice(&(descriptor.10 as u64).to_le_bytes());
        slot[184..192].copy_from_slice(&(descriptor.11 as u64).to_le_bytes());
        slot[192..200].copy_from_slice(&(descriptor.12 as u64).to_le_bytes());
        slot[200..208].copy_from_slice(&(descriptor.13 as u64).to_le_bytes());
        slot[208..216].copy_from_slice(&(descriptor.14 as u64).to_le_bytes());
        slot[216..224].copy_from_slice(&(descriptor.15 as u64).to_le_bytes());
        slot[224..232].copy_from_slice(&(descriptor.16 as u64).to_le_bytes());
        slot[232..240].copy_from_slice(&descriptor.0.unresolved_count.to_le_bytes());
        slot[240..256].fill(0);
    }
    section.extend_from_slice(&payload);
    Ok(section)
}

fn embed_relation_set(
    slots: &[u32],
    slot_count: u32,
    arena: &mut Vec<u8>,
) -> StorageResult<RecordSetRef> {
    let mut encoded = encode_record_set(slots, slot_count)?;
    if encoded.reference.tag == RecordSetTag::Blocks {
        encoded.reference.a = encoded
            .reference
            .a
            .checked_add(arena.len() as u64)
            .ok_or_else(|| StorageError::ResourceLimit("relation arena offset overflow".into()))?;
    }
    arena.extend_from_slice(&encoded.arena);
    Ok(encoded.reference)
}

fn encode_path(path: &[String]) -> StorageResult<Vec<u8>> {
    let mut bytes = Vec::new();
    let count = u32::try_from(path.len())
        .map_err(|_| StorageError::ResourceLimit("relation path segment count overflow".into()))?;
    bytes.extend_from_slice(&count.to_le_bytes());
    for segment in path {
        let length = u32::try_from(segment.len())
            .map_err(|_| StorageError::ResourceLimit("relation path segment too large".into()))?;
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(segment.as_bytes());
    }
    Ok(bytes)
}


/// Reader over the packed relation section.
pub struct RelationSectionView {
    relations: Vec<RelationView>,
}


impl std::fmt::Debug for RelationSectionView {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("RelationSectionView")
            .field("relations", &self.relations.len())
            .finish_non_exhaustive()
    }
}


impl RelationSectionView {
    pub fn open(
        backing: Backing,
        relations: SectionDescriptor,
        string_arena: SectionDescriptor,
        slot_count: u32,
        expected_count: u32,
    ) -> StorageResult<Self> {
        let header = backing.read(relations.offset, 16)?;
        if header[..4] != RELATION_MAGIC {
            return Err(StorageError::corrupt("relation magic mismatch"));
        }
        let version = u16::from_le_bytes([header[4], header[5]]);
        if version != RELATION_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "relation version {version}"
            )));
        }
        if header[6..8] != [0, 0] || header[12..16] != [0, 0, 0, 0] {
            return Err(StorageError::corrupt(
                "relation header reserved bytes are not zero",
            ));
        }
        let count = u32::from_le_bytes(header[8..12].try_into().expect("relation count")) as usize;
        if count != expected_count as usize {
            return Err(StorageError::corrupt(
                "relation section count differs from segment statistics",
            ));
        }
        let directory_len =
            16usize
                .checked_add(count.checked_mul(RELATION_DESCRIPTOR_LEN).ok_or_else(|| {
                    StorageError::corrupt("relation descriptor directory overflow")
                })?)
                .ok_or_else(|| StorageError::corrupt("relation descriptor directory overflow"))?;
        if directory_len as u64 > relations.length {
            return Err(StorageError::corrupt(
                "relation descriptor directory is truncated",
            ));
        }
        let directory = backing.read(relations.offset, directory_len as u64)?;
        let mut views = Vec::with_capacity(count);
        for index in 0..count {
            let at = 16 + index * RELATION_DESCRIPTOR_LEN;
            let slot = &directory[at..at + RELATION_DESCRIPTOR_LEN];
            let relation_slot_count = read_u32(slot, 8)?;
            if relation_slot_count != slot_count {
                return Err(StorageError::corrupt(
                    "relation slot count differs from its segment",
                ));
            }
            if slot[13..16].iter().any(|byte| *byte != 0)
                || slot[240..256].iter().any(|byte| *byte != 0)
            {
                return Err(StorageError::corrupt(
                    "relation descriptor reserved bytes are not zero",
                ));
            }
            let path_offset = read_u64(slot, 16)?;
            let path_len = read_u32(slot, 24)? as u64;
            let reverse_offset = read_u64(slot, 28)?;
            let reverse_len = read_u32(slot, 36)? as u64;
            let present = RecordSetRef::decode(&slot[40..72])?;
            let nulls = RecordSetRef::decode(&slot[72..104])?;
            let empty_arrays = RecordSetRef::decode(&slot[104..136])?;
            let unavailable = RecordSetRef::decode(&slot[136..168])?;
            let offsets_offset = read_u64(slot, 168)?;
            let offsets_len = read_u64(slot, 176)?;
            let edges_offset = read_u64(slot, 184)?;
            let edges_len = read_u64(slot, 192)?;
            let positions_offset = read_u64(slot, 200)?;
            let positions_len = read_u64(slot, 208)?;
            let arena_offset = read_u64(slot, 216)?;
            let arena_len = read_u64(slot, 224)?;
            for (offset, length) in [
                (offsets_offset, offsets_len),
                (edges_offset, edges_len),
                (positions_offset, positions_len),
                (arena_offset, arena_len),
            ] {
                if offset
                    .checked_add(length)
                    .is_none_or(|end| end > relations.length)
                {
                    return Err(StorageError::corrupt(
                        "relation descriptor reference lies outside its section",
                    ));
                }
            }
            if offsets_len != (slot_count as u64 + 1) * 4
                || edges_len % RELATION_EDGE_LEN as u64 != 0
            {
                return Err(StorageError::corrupt(
                    "relation offsets or edges have an invalid length",
                ));
            }
            if path_len > 0
                && path_offset
                    .checked_add(path_len)
                    .is_none_or(|end| end > string_arena.length)
            {
                return Err(StorageError::corrupt("relation path is out of bounds"));
            }
            if reverse_len > 0
                && reverse_offset
                    .checked_add(reverse_len)
                    .is_none_or(|end| end > string_arena.length)
            {
                return Err(StorageError::corrupt(
                    "relation reverse alias is out of bounds",
                ));
            }
            let path = decode_path(backing.read(string_arena.offset + path_offset, path_len)?)?;
            let reverse_alias = if reverse_len == 0 {
                None
            } else {
                Some(
                    std::str::from_utf8(
                        backing.read(string_arena.offset + reverse_offset, reverse_len)?,
                    )
                    .map_err(|_| StorageError::corrupt("relation reverse alias is not UTF-8"))?
                    .to_owned(),
                )
            };
            views.push(RelationView {
                backing: backing.clone(),
                base: relations.offset,
                string_arena,
                slot_count,
                id_ordinal: read_u32(slot, 0)?,
                target_collection: read_u32(slot, 4)?,
                path,
                reverse_alias,
                cardinality: slot[12],
                present,
                nulls,
                empty_arrays,
                unavailable,
                offsets_offset,
                offsets_len,
                edges_offset,
                edges_len,
                positions_offset,
                positions_len,
                arena_offset,
                arena_len,
                unresolved_count: read_u64(slot, 232)?,
            });
        }
        Ok(Self { relations: views })
    }

    pub fn relations(&self) -> &[RelationView] {
        &self.relations
    }

    pub fn relation(&self, ordinal: u32) -> Option<&RelationView> {
        self.relations
            .iter()
            .find(|relation| relation.id_ordinal == ordinal)
    }

    pub fn verify_full(&self) -> StorageResult<()> {
        for relation in &self.relations {
            relation.verify_full()?;
        }
        Ok(())
    }
}


/// One relation adjacency reader.
pub struct RelationView {
    backing: Backing,
    base: u64,
    string_arena: SectionDescriptor,
    slot_count: u32,
    pub id_ordinal: u32,
    pub target_collection: u32,
    pub path: Vec<String>,
    pub reverse_alias: Option<String>,
    cardinality: u8,
    present: RecordSetRef,
    nulls: RecordSetRef,
    empty_arrays: RecordSetRef,
    unavailable: RecordSetRef,
    offsets_offset: u64,
    offsets_len: u64,
    edges_offset: u64,
    edges_len: u64,
    positions_offset: u64,
    positions_len: u64,
    arena_offset: u64,
    arena_len: u64,
    unresolved_count: u64,
}


/// One decoded relation edge.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RelationEdgeView {
    pub source_slot: u32,
    pub target_slot: Option<u32>,
    pub target_id: String,
    pub positions: Vec<u32>,
}


impl RelationView {
    pub fn slot_count(&self) -> u32 {
        self.slot_count
    }

    pub fn unresolved_count(&self) -> u64 {
        self.unresolved_count
    }

    pub fn cardinality(&self) -> u8 {
        self.cardinality
    }

    pub fn present(&self) -> StorageResult<RecordSetView<'_>> {
        self.set(self.present)
    }

    pub fn nulls(&self) -> StorageResult<RecordSetView<'_>> {
        self.set(self.nulls)
    }

    pub fn empty_arrays(&self) -> StorageResult<RecordSetView<'_>> {
        self.set(self.empty_arrays)
    }

    pub fn unavailable(&self) -> StorageResult<RecordSetView<'_>> {
        self.set(self.unavailable)
    }

    pub fn edges(&self, source_slot: u32) -> StorageResult<Vec<RelationEdgeView>> {
        if source_slot >= self.slot_count {
            return Ok(Vec::new());
        }
        let at = source_slot as u64 * 4;
        if at + 8 > self.offsets_len {
            return Err(StorageError::corrupt("relation offset table is truncated"));
        }
        let bounds = self.backing.read(self.base + self.offsets_offset + at, 8)?;
        let start = u32::from_le_bytes(bounds[..4].try_into().expect("edge start")) as u64;
        let end = u32::from_le_bytes(bounds[4..8].try_into().expect("edge end")) as u64;
        if end < start || end * RELATION_EDGE_LEN as u64 > self.edges_len {
            return Err(StorageError::corrupt("relation edge range is invalid"));
        }
        let mut edges = Vec::with_capacity((end - start) as usize);
        for index in start..end {
            let bytes = self.backing.read(
                self.base + self.edges_offset + index * RELATION_EDGE_LEN as u64,
                RELATION_EDGE_LEN as u64,
            )?;
            let target_slot = read_u32(bytes, 0)?;
            let target_id_offset = read_u32(bytes, 4)? as u64;
            let target_id_len = read_u32(bytes, 8)? as u64;
            let positions_offset = read_u32(bytes, 12)? as u64;
            let positions_count = read_u32(bytes, 16)? as u64;
            if bytes[21..24].iter().any(|byte| *byte != 0) {
                return Err(StorageError::corrupt(
                    "relation edge reserved bytes are not zero",
                ));
            }
            if target_id_offset
                .checked_add(target_id_len)
                .is_none_or(|end| end > self.string_arena.length)
            {
                return Err(StorageError::corrupt("relation target ID is out of bounds"));
            }
            let target_id = std::str::from_utf8(
                self.backing
                    .read(self.string_arena.offset + target_id_offset, target_id_len)?,
            )
            .map_err(|_| StorageError::corrupt("relation target ID is not UTF-8"))?
            .to_owned();
            if positions_offset
                .checked_add(positions_count * 4)
                .is_none_or(|end| end > self.positions_len)
            {
                return Err(StorageError::corrupt(
                    "relation positions are out of bounds",
                ));
            }
            let mut positions = Vec::with_capacity(positions_count as usize);
            for position in 0..positions_count {
                let bytes = self.backing.read(
                    self.base + self.positions_offset + positions_offset + position * 4,
                    4,
                )?;
                positions.push(u32::from_le_bytes(
                    bytes.try_into().expect("four-byte relation position"),
                ));
            }
            edges.push(RelationEdgeView {
                source_slot,
                target_slot: (target_slot != u32::MAX).then_some(target_slot),
                target_id,
                positions,
            });
        }
        Ok(edges)
    }

    pub fn verify_full(&self) -> StorageResult<()> {
        for reference in [
            self.present,
            self.nulls,
            self.empty_arrays,
            self.unavailable,
        ] {
            self.set(reference)?.verify_full()?;
        }
        for slot in 0..self.slot_count {
            self.edges(slot)?;
        }
        Ok(())
    }

    fn set(&self, reference: RecordSetRef) -> StorageResult<RecordSetView<'_>> {
        let arena = self
            .backing
            .read(self.base + self.arena_offset, self.arena_len)?;
        reference.view(arena, self.slot_count)
    }
}


fn read_u32(bytes: &[u8], at: usize) -> StorageResult<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| StorageError::corrupt("relation u32 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated relation u32"))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte relation field"),
    ))
}

fn read_u64(bytes: &[u8], at: usize) -> StorageResult<u64> {
    let end = at
        .checked_add(8)
        .ok_or_else(|| StorageError::corrupt("relation u64 offset overflow"))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::corrupt("truncated relation u64"))?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("eight-byte relation field"),
    ))
}

fn decode_path(bytes: &[u8]) -> StorageResult<Vec<String>> {
    if bytes.len() < 4 {
        return Err(StorageError::corrupt("truncated relation path"));
    }
    let count = u32::from_le_bytes(bytes[..4].try_into().expect("relation path count")) as usize;
    let mut at = 4usize;
    let mut path = Vec::with_capacity(count);
    for _ in 0..count {
        let length = read_u32(bytes, at)? as usize;
        at += 4;
        let end = at
            .checked_add(length)
            .ok_or_else(|| StorageError::corrupt("relation path segment overflow"))?;
        let segment = bytes
            .get(at..end)
            .ok_or_else(|| StorageError::corrupt("truncated relation path segment"))?;
        path.push(
            std::str::from_utf8(segment)
                .map_err(|_| StorageError::corrupt("relation path segment is not UTF-8"))?
                .to_owned(),
        );
        at = end;
    }
    if at != bytes.len() {
        return Err(StorageError::corrupt("relation path has trailing bytes"));
    }
    Ok(path)
}
