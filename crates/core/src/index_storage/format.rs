//! Immutable object format: fixed header, section directory, and checksums.
//!
//! Every persisted object is a self-describing little-endian file. Readers
//! never cast mapped bytes to Rust structures; every field is decoded from an
//! explicitly bounds-checked byte slice.

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

use super::backing::Backing;
use super::error::{StorageError, StorageResult};

/// Eight-byte object magic.
pub const OBJECT_MAGIC: [u8; 8] = *b"OBJXOBJ1";
/// Eight-byte store marker magic.
pub const STORE_MAGIC: [u8; 8] = *b"OBJEST01";
/// Current object format version.
pub const FORMAT_VERSION: u16 = 4;
/// Fixed object header length.
pub const HEADER_LEN: usize = 128;
/// Fixed section directory entry length.
pub const SECTION_DESCRIPTOR_LEN: usize = 40;
/// Objects are checksummed in independently validated 8 KiB regions, aligned
/// with the dictionary page size used by paged readers.
pub const CHECKSUM_BLOCK_SIZE: u32 = 8 * 1024;
/// Maximum section count accepted from an object header.
pub const MAX_SECTIONS: usize = 4_096;
/// Hard upper bound for one object accepted by this build.
pub const MAX_OBJECT_BYTES: u64 = 64 * 1024 * 1024 * 1024;
/// Alignment used between object sections.
pub const SECTION_ALIGNMENT: usize = 8;

const CHECKSUM_OFFSET: usize = 64;
const CHECKSUM_LEN: usize = 32;
const HEADER_RESERVED_START: usize = 96;


/// The semantic kind of one immutable object.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ObjectKind {
    /// One frozen search contribution belonging to a collection.
    Segment,
    /// One rewritten file catalogue and path dictionary.
    Catalogue,
    /// Native query-engine compatibility state for one pinned publication.
    State,
    /// Publication-specific active record set for one immutable run.
    Visibility,
}


impl ObjectKind {
    pub fn code(self) -> u32 {
        match self {
            Self::Segment => 1,
            Self::Catalogue => 2,
            Self::State => 3,
            Self::Visibility => 4,
        }
    }

    pub fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::Segment),
            2 => Some(Self::Catalogue),
            3 => Some(Self::State),
            4 => Some(Self::Visibility),
            _ => None,
        }
    }

    pub fn required_sections(self) -> &'static [SectionKind] {
        match self {
            Self::Segment => &[
                SectionKind::Records,
                SectionKind::FileParts,
                SectionKind::LogicalIds,
                SectionKind::IdentityOrder,
                SectionKind::FieldDirectory,
                SectionKind::DictionaryPages,
                SectionKind::PostingPayloads,
                SectionKind::SortKeys,
                SectionKind::SortOrders,
                SectionKind::BigramDictionaries,
                SectionKind::BigramPostings,
                SectionKind::Relations,
                SectionKind::StringArena,
                SectionKind::Coverage,
                SectionKind::Statistics,
            ],
            Self::Catalogue => &[
                SectionKind::FileRows,
                SectionKind::PathDictionary,
                SectionKind::Diagnostics,
                SectionKind::Statistics,
            ],
            Self::State => &[SectionKind::NativeState],
            Self::Visibility => &[SectionKind::Visibility],
        }
    }
}


/// Section payload families. Unknown kinds are accepted only when the
/// descriptor marks them optional.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u32)]
pub enum SectionKind {
    Records = 1,
    FileParts = 2,
    LogicalIds = 3,
    FieldDirectory = 4,
    DictionaryPages = 5,
    PostingPayloads = 6,
    SortKeys = 7,
    SortOrders = 8,
    RelationOffsets = 9,
    RelationEdges = 10,
    StringArena = 11,
    Coverage = 12,
    Statistics = 13,
    BigramDictionaries = 14,
    BigramPostings = 15,
    FileRows = 16,
    PathDictionary = 17,
    Diagnostics = 18,
    /// Opaque frontend metadata, bounded by the caller and ignored by core.
    FrontendMetadata = 19,
    /// Reserved for a packed representation of native compatibility state.
    Compatibility = 20,
    /// Identity-order physical slots for default ordering.
    IdentityOrder = 21,
    /// Packed relation adjacency and target identities.
    Relations = 22,
    /// Native query-engine compatibility state.
    NativeState = 23,
    /// Publication-specific active record set for one run.
    Visibility = 24,
    /// Active file IDs in the packed catalogue domain.
    ActiveFiles = 25,
    /// Persisted directory prefixes and collection memberships.
    Directories = 26,
    /// Per-collection active file-ID sets.
    CollectionFiles = 27,
}


impl SectionKind {
    pub fn code(self) -> u32 {
        self as u32
    }

    pub fn from_code(code: u32) -> Option<Self> {
        Some(match code {
            1 => Self::Records,
            2 => Self::FileParts,
            3 => Self::LogicalIds,
            4 => Self::FieldDirectory,
            5 => Self::DictionaryPages,
            6 => Self::PostingPayloads,
            7 => Self::SortKeys,
            8 => Self::SortOrders,
            9 => Self::RelationOffsets,
            10 => Self::RelationEdges,
            11 => Self::StringArena,
            12 => Self::Coverage,
            13 => Self::Statistics,
            14 => Self::BigramDictionaries,
            15 => Self::BigramPostings,
            16 => Self::FileRows,
            17 => Self::PathDictionary,
            18 => Self::Diagnostics,
            19 => Self::FrontendMetadata,
            20 => Self::Compatibility,
            21 => Self::IdentityOrder,
            22 => Self::Relations,
            23 => Self::NativeState,
            24 => Self::Visibility,
            25 => Self::ActiveFiles,
            26 => Self::Directories,
            27 => Self::CollectionFiles,
            _ => return None,
        })
    }
}


/// Descriptor flag: readers may ignore a section whose kind they do not know.
pub const SECTION_OPTIONAL: u16 = 1 << 0;


/// One decoded object header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ObjectHeader {
    pub kind: ObjectKind,
    pub total_len: u64,
    pub section_dir_offset: u64,
    pub section_count: u32,
    pub flags: u32,
    pub checksum_table_offset: u64,
    pub checksum_block_size: u32,
    pub checksum_count: u32,
}


impl ObjectHeader {
    pub fn new(
        kind: ObjectKind,
        total_len: u64,
        section_dir_offset: u64,
        section_count: u32,
        checksum_table_offset: u64,
        checksum_count: u32,
    ) -> StorageResult<Self> {
        if checksum_table_offset < HEADER_LEN as u64 {
            return Err(StorageError::InvalidHeader(
                "checksum table overlaps the object header".into(),
            ));
        }
        if section_dir_offset < HEADER_LEN as u64 {
            return Err(StorageError::InvalidHeader(
                "section directory overlaps the object header".into(),
            ));
        }
        let section_bytes = (section_count as u64)
            .checked_mul(SECTION_DESCRIPTOR_LEN as u64)
            .ok_or_else(|| {
                StorageError::InvalidHeader("section directory length overflow".into())
            })?;
        let section_end = section_dir_offset
            .checked_add(section_bytes)
            .ok_or_else(|| StorageError::InvalidHeader("section directory end overflow".into()))?;
        if section_end > checksum_table_offset {
            return Err(StorageError::InvalidHeader(
                "section directory overlaps object payload".into(),
            ));
        }
        let table_bytes = (checksum_count as u64)
            .checked_mul(4)
            .ok_or_else(|| StorageError::InvalidHeader("checksum table length overflow".into()))?;
        if checksum_table_offset
            .checked_add(table_bytes)
            .is_none_or(|end| end != total_len)
        {
            return Err(StorageError::InvalidHeader(
                "checksum table does not end the object".into(),
            ));
        }
        Ok(Self {
            kind,
            total_len,
            section_dir_offset,
            section_count,
            flags: 0,
            checksum_table_offset,
            checksum_block_size: CHECKSUM_BLOCK_SIZE,
            checksum_count,
        })
    }

    fn to_bytes(self) -> StorageResult<[u8; HEADER_LEN]> {
        let mut header = [0u8; HEADER_LEN];
        header[..8].copy_from_slice(&OBJECT_MAGIC);
        header[8..10].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
        header[10..12].copy_from_slice(&(HEADER_LEN as u16).to_le_bytes());
        header[12..16].copy_from_slice(&self.kind.code().to_le_bytes());
        header[16..24].copy_from_slice(&self.total_len.to_le_bytes());
        header[24..32].copy_from_slice(&self.section_dir_offset.to_le_bytes());
        header[32..36].copy_from_slice(&self.section_count.to_le_bytes());
        header[36..40].copy_from_slice(&self.flags.to_le_bytes());
        header[40..48].copy_from_slice(&self.checksum_table_offset.to_le_bytes());
        header[48..52].copy_from_slice(&self.checksum_block_size.to_le_bytes());
        header[52..56].copy_from_slice(&self.checksum_count.to_le_bytes());
        let checksum = header_checksum(&header)?;
        header[CHECKSUM_OFFSET..CHECKSUM_OFFSET + CHECKSUM_LEN].copy_from_slice(&checksum);
        Ok(header)
    }

    /// Decode and verify one fixed header. The caller guarantees at least 128
    /// bytes were read.
    pub fn parse(bytes: &[u8]) -> StorageResult<Self> {
        if bytes.len() < HEADER_LEN {
            return Err(StorageError::InvalidHeader(
                "object header is truncated".into(),
            ));
        }
        if bytes[..8] != OBJECT_MAGIC {
            return Err(StorageError::InvalidHeader("object magic mismatch".into()));
        }
        let version = read_u16(bytes, 8)?;
        if version != FORMAT_VERSION {
            return Err(StorageError::UnsupportedFormat(format!(
                "object format version {version}"
            )));
        }
        let header_len = read_u16(bytes, 10)? as usize;
        if header_len != HEADER_LEN {
            return Err(StorageError::InvalidHeader(format!(
                "unsupported object header length {header_len}"
            )));
        }
        let kind_code = read_u32(bytes, 12)?;
        let kind = ObjectKind::from_code(kind_code).ok_or_else(|| {
            StorageError::UnsupportedFormat(format!("unknown object kind {kind_code}"))
        })?;
        let total_len = read_u64(bytes, 16)?;
        if total_len > MAX_OBJECT_BYTES {
            return Err(StorageError::InvalidHeader(format!(
                "object length {total_len} exceeds the supported maximum"
            )));
        }
        let section_dir_offset = read_u64(bytes, 24)?;
        let section_count = read_u32(bytes, 32)?;
        if section_count as usize > MAX_SECTIONS {
            return Err(StorageError::InvalidHeader(format!(
                "section count {section_count} exceeds the supported maximum"
            )));
        }
        let flags = read_u32(bytes, 36)?;
        if flags != 0 {
            return Err(StorageError::InvalidHeader(format!(
                "unknown object header flags {flags:#x}"
            )));
        }
        let checksum_table_offset = read_u64(bytes, 40)?;
        let checksum_block_size = read_u32(bytes, 48)?;
        if checksum_block_size != CHECKSUM_BLOCK_SIZE {
            return Err(StorageError::InvalidHeader(format!(
                "unsupported checksum block size {checksum_block_size}"
            )));
        }
        let checksum_count = read_u32(bytes, 52)?;
        let header = &bytes[..HEADER_LEN];
        let expected = header_checksum(header)?;
        if header[CHECKSUM_OFFSET..CHECKSUM_OFFSET + CHECKSUM_LEN] != expected {
            return Err(StorageError::InvalidHeader(
                "object header checksum mismatch".into(),
            ));
        }
        if header[56..64].iter().any(|byte| *byte != 0)
            || header[HEADER_RESERVED_START..]
                .iter()
                .any(|byte| *byte != 0)
        {
            return Err(StorageError::InvalidHeader(
                "object header reserved bytes are not zero".into(),
            ));
        }
        Self::new(
            kind,
            total_len,
            section_dir_offset,
            section_count,
            checksum_table_offset,
            checksum_count,
        )
    }

    pub fn required_checksum_count(content_len: u64) -> StorageResult<u32> {
        let blocks = content_len.div_ceil(CHECKSUM_BLOCK_SIZE as u64);
        u32::try_from(blocks)
            .map_err(|_| StorageError::ResourceLimit("too many checksum blocks".into()))
    }
}


fn header_checksum(bytes: &[u8]) -> StorageResult<[u8; 32]> {
    if bytes.len() < HEADER_LEN {
        return Err(StorageError::InvalidHeader(
            "object header is truncated".into(),
        ));
    }
    let mut copy = [0u8; HEADER_LEN];
    copy.copy_from_slice(&bytes[..HEADER_LEN]);
    copy[CHECKSUM_OFFSET..CHECKSUM_OFFSET + CHECKSUM_LEN].fill(0);
    Ok(Sha256::digest(copy).into())
}


/// One decoded section descriptor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SectionDescriptor {
    pub kind: Option<SectionKind>,
    pub raw_kind: u32,
    pub flags: u16,
    pub entry_width: u16,
    pub offset: u64,
    pub length: u64,
    pub entry_count: u64,
}


impl SectionDescriptor {
    pub fn new(
        kind: SectionKind,
        flags: u16,
        entry_width: u16,
        offset: u64,
        length: u64,
        entry_count: u64,
    ) -> StorageResult<Self> {
        if entry_width != 0 {
            let expected = entry_count
                .checked_mul(entry_width as u64)
                .ok_or_else(|| StorageError::InvalidHeader("section width overflow".into()))?;
            if expected != length {
                return Err(StorageError::InvalidHeader(
                    "fixed-width section length does not match entry count".into(),
                ));
            }
        }
        Ok(Self {
            kind: Some(kind),
            raw_kind: kind.code(),
            flags,
            entry_width,
            offset,
            length,
            entry_count,
        })
    }

    pub fn end(&self) -> StorageResult<u64> {
        self.offset
            .checked_add(self.length)
            .ok_or_else(|| StorageError::Corrupt {
                path: None,
                message: "section end overflow".into(),
            })
    }

    fn parse(bytes: &[u8], at: usize) -> StorageResult<Self> {
        let end = at
            .checked_add(SECTION_DESCRIPTOR_LEN)
            .ok_or_else(|| StorageError::InvalidHeader("section directory overflow".into()))?;
        let slice = bytes
            .get(at..end)
            .ok_or_else(|| StorageError::InvalidHeader("section directory is truncated".into()))?;
        let raw_kind = read_u32(slice, 0)?;
        let flags = read_u16(slice, 4)?;
        let entry_width = read_u16(slice, 6)?;
        let offset = read_u64(slice, 8)?;
        let length = read_u64(slice, 16)?;
        let entry_count = read_u64(slice, 24)?;
        if read_u64(slice, 32)? != 0 {
            return Err(StorageError::InvalidHeader(
                "section descriptor reserved bytes are not zero".into(),
            ));
        }
        Ok(Self {
            kind: SectionKind::from_code(raw_kind),
            raw_kind,
            flags,
            entry_width,
            offset,
            length,
            entry_count,
        })
    }

    fn write(&self, destination: &mut [u8]) -> StorageResult<()> {
        if destination.len() < SECTION_DESCRIPTOR_LEN {
            return Err(StorageError::Corrupt {
                path: None,
                message: "section descriptor destination is truncated".into(),
            });
        }
        destination[..4].copy_from_slice(&self.raw_kind.to_le_bytes());
        destination[4..6].copy_from_slice(&self.flags.to_le_bytes());
        destination[6..8].copy_from_slice(&self.entry_width.to_le_bytes());
        destination[8..16].copy_from_slice(&self.offset.to_le_bytes());
        destination[16..24].copy_from_slice(&self.length.to_le_bytes());
        destination[24..32].copy_from_slice(&self.entry_count.to_le_bytes());
        destination[32..40].fill(0);
        Ok(())
    }

    pub fn is_required(&self) -> bool {
        self.flags & SECTION_OPTIONAL == 0
    }
}


/// A resident object produced by [`ObjectBuilder`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuiltObject {
    bytes: Vec<u8>,
}


impl BuiltObject {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }

    pub fn sha256(&self) -> [u8; 32] {
        Sha256::digest(&self.bytes).into()
    }
}


/// Assemble one immutable object from named sections.
pub struct ObjectBuilder {
    kind: ObjectKind,
    sections: Vec<(SectionKind, u16, u16, Vec<u8>, u64)>,
}


impl ObjectBuilder {
    pub fn new(kind: ObjectKind) -> Self {
        Self {
            kind,
            sections: Vec::new(),
        }
    }

    pub fn add_section(&mut self, kind: SectionKind, bytes: Vec<u8>) -> StorageResult<()> {
        self.add_section_with(kind, 0, 0, bytes, 0)
    }

    pub fn add_optional_section(&mut self, kind: SectionKind, bytes: Vec<u8>) -> StorageResult<()> {
        self.add_section_with(kind, SECTION_OPTIONAL, 0, bytes, 0)
    }

    pub fn add_fixed_section(
        &mut self,
        kind: SectionKind,
        entry_width: u16,
        bytes: Vec<u8>,
        entry_count: u64,
    ) -> StorageResult<()> {
        self.add_section_with(kind, 0, entry_width, bytes, entry_count)
    }

    fn add_section_with(
        &mut self,
        kind: SectionKind,
        flags: u16,
        entry_width: u16,
        bytes: Vec<u8>,
        entry_count: u64,
    ) -> StorageResult<()> {
        if self.sections.iter().any(|(existing, ..)| *existing == kind) {
            return Err(StorageError::InvalidArgument(format!(
                "duplicate object section {kind:?}"
            )));
        }
        if entry_width != 0
            && entry_count
                .checked_mul(entry_width as u64)
                .is_none_or(|length| length != bytes.len() as u64)
        {
            return Err(StorageError::InvalidArgument(
                "fixed-width section byte length does not match its entry count".into(),
            ));
        }
        self.sections
            .push((kind, flags, entry_width, bytes, entry_count));
        Ok(())
    }

    pub fn finish(mut self) -> StorageResult<BuiltObject> {
        for required in self.kind.required_sections() {
            if !self.sections.iter().any(|(kind, ..)| kind == required) {
                return Err(StorageError::InvalidArgument(format!(
                    "{:?} object is missing required section {required:?}",
                    self.kind
                )));
            }
        }
        self.sections.sort_by_key(|(kind, ..)| kind.code());
        let dir_len = self
            .sections
            .len()
            .checked_mul(SECTION_DESCRIPTOR_LEN)
            .ok_or_else(|| StorageError::ResourceLimit("section directory overflow".into()))?;
        let section_dir_offset = HEADER_LEN as u64;
        let mut cursor = align_up(
            section_dir_offset
                .checked_add(dir_len as u64)
                .ok_or_else(|| StorageError::ResourceLimit("object length overflow".into()))?,
            SECTION_ALIGNMENT,
        )?;
        let mut descriptors = Vec::with_capacity(self.sections.len());
        for (kind, flags, entry_width, bytes, entry_count) in &self.sections {
            cursor = align_up(cursor, SECTION_ALIGNMENT)?;
            let length = bytes.len() as u64;
            descriptors.push(SectionDescriptor::new(
                *kind,
                *flags,
                *entry_width,
                cursor,
                length,
                *entry_count,
            )?);
            cursor = cursor
                .checked_add(length)
                .ok_or_else(|| StorageError::ResourceLimit("object length overflow".into()))?;
        }
        let checksum_table_offset = align_up(cursor, SECTION_ALIGNMENT)?;
        let checksum_count = ObjectHeader::required_checksum_count(checksum_table_offset)?;
        let table_len = (checksum_count as u64)
            .checked_mul(4)
            .ok_or_else(|| StorageError::ResourceLimit("checksum table overflow".into()))?;
        let total_len = checksum_table_offset
            .checked_add(table_len)
            .ok_or_else(|| StorageError::ResourceLimit("object length overflow".into()))?;
        let total_len_usize = usize::try_from(total_len)
            .map_err(|_| StorageError::ResourceLimit("object does not fit in memory".into()))?;
        let mut output = vec![0u8; total_len_usize];
        let header = ObjectHeader::new(
            self.kind,
            total_len,
            section_dir_offset,
            descriptors.len() as u32,
            checksum_table_offset,
            checksum_count,
        )?;
        output[..HEADER_LEN].copy_from_slice(&header.to_bytes()?);
        for (number, descriptor) in descriptors.iter().enumerate() {
            let at = HEADER_LEN + number * SECTION_DESCRIPTOR_LEN;
            descriptor.write(&mut output[at..at + SECTION_DESCRIPTOR_LEN])?;
        }
        for (number, (_, _, _, bytes, _)) in self.sections.iter().enumerate() {
            let descriptor = descriptors[number];
            let start = usize::try_from(descriptor.offset)
                .map_err(|_| StorageError::ResourceLimit("section offset overflow".into()))?;
            let end = start
                .checked_add(bytes.len())
                .ok_or_else(|| StorageError::ResourceLimit("section end overflow".into()))?;
            output[start..end].copy_from_slice(bytes);
        }
        for block in 0..checksum_count {
            let start = block as u64 * CHECKSUM_BLOCK_SIZE as u64;
            let end = (start + CHECKSUM_BLOCK_SIZE as u64).min(checksum_table_offset);
            let start = start as usize;
            let end = end as usize;
            let crc = crc32fast::hash(&output[start..end]);
            let at = checksum_table_offset as usize + block as usize * 4;
            output[at..at + 4].copy_from_slice(&crc.to_le_bytes());
        }
        Ok(BuiltObject { bytes: output })
    }
}


fn align_up(value: u64, alignment: usize) -> StorageResult<u64> {
    let alignment = alignment as u64;
    value
        .checked_add(alignment - 1)
        .map(|value| value / alignment * alignment)
        .ok_or_else(|| StorageError::ResourceLimit("object alignment overflow".into()))
}


/// One opened object with validated header and section directory.
#[derive(Debug)]
pub struct ObjectView<'a> {
    backing: &'a Backing,
    header: ObjectHeader,
    sections: Vec<SectionDescriptor>,
}


impl<'a> ObjectView<'a> {
    pub fn new(backing: &'a Backing) -> StorageResult<Self> {
        if backing.len() < HEADER_LEN as u64 {
            return Err(StorageError::InvalidHeader("object is truncated".into()));
        }
        let header_bytes = backing.read(0, HEADER_LEN as u64)?;
        let header = ObjectHeader::parse(header_bytes)?;
        if header.total_len != backing.len() {
            return Err(StorageError::InvalidHeader(format!(
                "object length {} does not match mapped length {}",
                header.total_len,
                backing.len()
            )));
        }
        let dir_len = (header.section_count as usize)
            .checked_mul(SECTION_DESCRIPTOR_LEN)
            .ok_or_else(|| StorageError::InvalidHeader("section directory overflow".into()))?;
        let dir_end = header
            .section_dir_offset
            .checked_add(dir_len as u64)
            .ok_or_else(|| StorageError::InvalidHeader("section directory end overflow".into()))?;
        if dir_end > header.checksum_table_offset {
            return Err(StorageError::InvalidHeader(
                "section directory overlaps object payload".into(),
            ));
        }
        let dir_bytes = backing.read(header.section_dir_offset, dir_len as u64)?;
        let mut sections = Vec::with_capacity(header.section_count as usize);
        let mut previous_end = dir_end;
        for number in 0..header.section_count as usize {
            let descriptor = SectionDescriptor::parse(dir_bytes, number * SECTION_DESCRIPTOR_LEN)?;
            let end = descriptor.end()?;
            if descriptor.offset < dir_end
                || end > header.checksum_table_offset
                || descriptor.offset < previous_end
            {
                return Err(StorageError::InvalidHeader(format!(
                    "section {number} lies outside the object payload"
                )));
            }
            if descriptor.kind.is_none() && descriptor.flags & SECTION_OPTIONAL == 0 {
                return Err(StorageError::UnsupportedFormat(format!(
                    "unknown mandatory section kind {}",
                    descriptor.raw_kind
                )));
            }
            previous_end = end;
            sections.push(descriptor);
        }
        for required in header.kind.required_sections() {
            if !sections
                .iter()
                .any(|descriptor| descriptor.kind == Some(*required))
            {
                return Err(StorageError::Corrupt {
                    path: None,
                    message: format!("required section {required:?} is missing"),
                });
            }
        }
        if sections.windows(2).any(|pair| pair[0].kind >= pair[1].kind) {
            return Err(StorageError::InvalidHeader(
                "section descriptors are not strictly ordered".into(),
            ));
        }
        Ok(Self {
            backing,
            header,
            sections,
        })
    }

    pub fn header(&self) -> ObjectHeader {
        self.header
    }

    pub fn sections(&self) -> &[SectionDescriptor] {
        &self.sections
    }

    pub fn section(&self, kind: SectionKind) -> Option<SectionDescriptor> {
        self.sections
            .iter()
            .find(|descriptor| descriptor.kind == Some(kind))
            .copied()
    }

    pub fn required_section(&self, kind: SectionKind) -> StorageResult<SectionDescriptor> {
        self.section(kind).ok_or_else(|| StorageError::Corrupt {
            path: None,
            message: format!("required section {kind:?} is missing"),
        })
    }

    pub fn section_bytes(&self, kind: SectionKind) -> StorageResult<&'a [u8]> {
        let descriptor = self.required_section(kind)?;
        self.backing.read(descriptor.offset, descriptor.length)
    }

    /// Validate every checksum block in the object. Load never calls this;
    /// tests and explicit diagnostics do.
    pub fn verify_full(&self) -> StorageResult<()> {
        self.backing.verify_all()
    }

    /// Required section kinds, exposed for compatibility tests.
    pub fn required_sections(&self) -> &'static [SectionKind] {
        self.header.kind.required_sections()
    }
}


/// Deterministic section metadata summary used by tests and diagnostics.
pub fn section_summary(view: &ObjectView<'_>) -> BTreeMap<u32, (u64, u64)> {
    view.sections()
        .iter()
        .map(|descriptor| (descriptor.raw_kind, (descriptor.offset, descriptor.length)))
        .collect()
}

fn read_u16(bytes: &[u8], at: usize) -> StorageResult<u16> {
    let end = at
        .checked_add(2)
        .ok_or_else(|| StorageError::InvalidHeader("u16 offset overflow".into()))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::InvalidHeader("truncated u16".into()))?;
    Ok(u16::from_le_bytes(
        slice.try_into().expect("two-byte slice"),
    ))
}

fn read_u32(bytes: &[u8], at: usize) -> StorageResult<u32> {
    let end = at
        .checked_add(4)
        .ok_or_else(|| StorageError::InvalidHeader("u32 offset overflow".into()))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::InvalidHeader("truncated u32".into()))?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four-byte slice"),
    ))
}

fn read_u64(bytes: &[u8], at: usize) -> StorageResult<u64> {
    let end = at
        .checked_add(8)
        .ok_or_else(|| StorageError::InvalidHeader("u64 offset overflow".into()))?;
    let slice = bytes
        .get(at..end)
        .ok_or_else(|| StorageError::InvalidHeader("truncated u64".into()))?;
    Ok(u64::from_le_bytes(
        slice.try_into().expect("eight-byte slice"),
    ))
}
