//! Resident and mapped immutable bytes with checked, lazy validation.
//!
//! The only unsafe operation in the storage subsystem is the read-only
//! `memmap2::Mmap` creation below. Its invariant is that the backing file is
//! opened read-only, never truncated or modified while a mapping exists, and
//! every access is bounds-checked before the mapped slice is exposed. Saving
//! and compaction never mutate a published object; they stage new objects.

use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use memmap2::{Mmap, MmapOptions};

use super::error::{StorageError, StorageResult};
use super::format::{CHECKSUM_BLOCK_SIZE, HEADER_LEN, OBJECT_MAGIC, ObjectHeader};


/// One checked shared reader lease. Dropping the lease releases the lock.
pub struct ReaderLease {
    _file: File,
    path: PathBuf,
}


impl std::fmt::Debug for ReaderLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ReaderLease")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}


impl ReaderLease {
    pub fn path(&self) -> &Path {
        &self.path
    }
}


/// One exclusive writer lease. A second writer receives a typed conflict.
pub struct WriterLease {
    _file: File,
    path: PathBuf,
}


impl std::fmt::Debug for WriterLease {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("WriterLease")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}


impl WriterLease {
    pub fn path(&self) -> &Path {
        &self.path
    }
}


fn open_lease_file(path: &Path) -> StorageResult<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)
        .map_err(|error| StorageError::io(path, error))
}

#[cfg(unix)]
fn flock(file: &File, operation: libc::c_int, nonblocking: bool) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    let mut operation = operation;
    if nonblocking {
        operation |= libc::LOCK_NB;
    }
    // SAFETY: `file` owns a valid descriptor for the duration of the call.
    let result = unsafe { libc::flock(file.as_raw_fd(), operation) };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(unix))]
fn flock(_file: &File, _operation: i32, _nonblocking: bool) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
const LOCK_SHARED: libc::c_int = libc::LOCK_SH;
#[cfg(unix)]
const LOCK_EXCLUSIVE: libc::c_int = libc::LOCK_EX;
#[cfg(not(unix))]
const LOCK_SHARED: i32 = 1;
#[cfg(not(unix))]
const LOCK_EXCLUSIVE: i32 = 2;

/// Acquire a shared reader lease. Waiting is intentional: a cleanup holding
/// the exclusive lease is short-lived.
pub fn acquire_reader_lease(path: &Path) -> StorageResult<Arc<ReaderLease>> {
    let file = open_lease_file(path)?;
    flock(&file, LOCK_SHARED, false)
        .map_err(|error| StorageError::Lock(format!("{}: {error}", path.display())))?;
    Ok(Arc::new(ReaderLease {
        _file: file,
        path: path.to_owned(),
    }))
}

/// Acquire an exclusive reader lease without waiting. `Ok(None)` means a
/// reader or another cleanup still holds a shared lease.
pub fn try_acquire_exclusive_reader_lease(path: &Path) -> StorageResult<Option<Arc<ReaderLease>>> {
    let file = open_lease_file(path)?;
    match flock(&file, LOCK_EXCLUSIVE, true) {
        Ok(()) => Ok(Some(Arc::new(ReaderLease {
            _file: file,
            path: path.to_owned(),
        }))),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(error) => Err(StorageError::Lock(format!("{}: {error}", path.display()))),
    }
}

/// Acquire the store writer lease without waiting.
pub fn acquire_writer_lease(path: &Path) -> StorageResult<WriterLease> {
    let file = open_lease_file(path)?;
    match flock(&file, LOCK_EXCLUSIVE, true) {
        Ok(()) => Ok(WriterLease {
            _file: file,
            path: path.to_owned(),
        }),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Err(StorageError::Lock(
            format!("another writer holds {}", path.display()),
        )),
        Err(error) => Err(StorageError::Lock(format!("{}: {error}", path.display()))),
    }
}


/// A read-only mapped immutable object.
pub struct MappedObject {
    path: PathBuf,
    mapping: Mmap,
    header: ObjectHeader,
    validation: Mutex<ValidationState>,
    bytes_validated: AtomicU64,
    _lease: Option<Arc<ReaderLease>>,
}


struct ValidationState {
    blocks: Vec<u64>,
}


impl std::fmt::Debug for MappedObject {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MappedObject")
            .field("path", &self.path)
            .field("header", &self.header)
            .field(
                "bytes_validated",
                &self.bytes_validated.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}


impl MappedObject {
    /// Open and map one object. Header and checksum-table bounds are validated
    /// eagerly; payload regions are validated on first access.
    pub fn open(path: &Path, lease: Option<Arc<ReaderLease>>) -> StorageResult<Self> {
        let file = File::open(path).map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                StorageError::NotFound(path.to_owned())
            } else {
                StorageError::io(path, error)
            }
        })?;
        let metadata = file
            .metadata()
            .map_err(|error| StorageError::io(path, error))?;
        if !metadata.is_file() {
            return Err(StorageError::corrupt_at(
                path,
                "object path is not a regular file",
            ));
        }
        if metadata.len() < HEADER_LEN as u64 {
            return Err(StorageError::InvalidHeader("object is truncated".into()));
        }
        // SAFETY: the file is opened read-only. Objekte never writes or
        // truncates a published object; replacement always stages a new file
        // and atomically switches the generation pointer. All offsets are
        // checked before any mapped slice escapes this module.
        let mapping = unsafe { MmapOptions::new().map(&file) }
            .map_err(|error| StorageError::io(path, error))?;
        if mapping.len() as u64 != metadata.len() {
            return Err(StorageError::corrupt_at(
                path,
                "mapped length differs from file length",
            ));
        }
        let header = ObjectHeader::parse(&mapping[..HEADER_LEN])?;
        if header.total_len != mapping.len() as u64 {
            return Err(StorageError::InvalidHeader(format!(
                "header length {} differs from file length {}",
                header.total_len,
                mapping.len()
            )));
        }
        let expected_blocks = ObjectHeader::required_checksum_count(header.checksum_table_offset)?;
        if header.checksum_count != expected_blocks {
            return Err(StorageError::InvalidHeader(format!(
                "checksum count {} does not cover {} bytes",
                header.checksum_count, header.checksum_table_offset
            )));
        }
        let table_end = header
            .checksum_table_offset
            .checked_add(header.checksum_count as u64 * 4)
            .ok_or_else(|| StorageError::InvalidHeader("checksum table overflow".into()))?;
        if table_end != header.total_len {
            return Err(StorageError::InvalidHeader(
                "checksum table does not end the object".into(),
            ));
        }
        let block_count = header.checksum_count as usize;
        let words = block_count.div_ceil(64);
        Ok(Self {
            path: path.to_owned(),
            validation: Mutex::new(ValidationState {
                blocks: vec![0u64; words],
            }),
            mapping,
            header,
            bytes_validated: AtomicU64::new(0),
            _lease: lease,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn header(&self) -> ObjectHeader {
        self.header
    }

    pub fn len(&self) -> u64 {
        self.header.total_len
    }

    pub fn is_empty(&self) -> bool {
        self.header.total_len == 0
    }

    /// Bytes whose checksums have already been validated in this object.
    pub fn bytes_validated(&self) -> u64 {
        self.bytes_validated.load(Ordering::Relaxed)
    }

    fn table_crc(&self, block: usize) -> StorageResult<u32> {
        let at =
            (self.header.checksum_table_offset as usize)
                .checked_add(block.checked_mul(4).ok_or_else(|| {
                    StorageError::corrupt_at(&self.path, "checksum index overflow")
                })?)
                .ok_or_else(|| StorageError::corrupt_at(&self.path, "checksum offset overflow"))?;
        let bytes = self
            .mapping
            .get(at..at + 4)
            .ok_or_else(|| StorageError::corrupt_at(&self.path, "truncated checksum table"))?;
        Ok(u32::from_le_bytes(
            bytes.try_into().expect("four-byte checksum slice"),
        ))
    }

    fn block_bounds(&self, block: usize) -> (usize, usize) {
        let start = block * CHECKSUM_BLOCK_SIZE as usize;
        let end =
            (start + CHECKSUM_BLOCK_SIZE as usize).min(self.header.checksum_table_offset as usize);
        (start, end)
    }

    /// Validate the blocks covering `[offset, offset + length)`.
    fn validate_range(&self, offset: u64, length: u64) -> StorageResult<()> {
        if length == 0 {
            return Ok(());
        }
        let end = offset
            .checked_add(length)
            .ok_or_else(|| StorageError::corrupt_at(&self.path, "mapped range end overflow"))?;
        if end > self.header.total_len {
            return Err(StorageError::corrupt_at(
                &self.path,
                "mapped range lies beyond the object",
            ));
        }
        if offset >= self.header.checksum_table_offset {
            return Ok(());
        }
        let capped_end = end.min(self.header.checksum_table_offset);
        let first = (offset / CHECKSUM_BLOCK_SIZE as u64) as usize;
        let last = ((capped_end - 1) / CHECKSUM_BLOCK_SIZE as u64) as usize;
        let mut validated = 0u64;
        {
            let mut state = self
                .validation
                .lock()
                .map_err(|_| StorageError::Lock("mapped validation lock poisoned".into()))?;
            for block in first..=last {
                let (block_start, block_end) = self.block_bounds(block);
                if block_start == block_end {
                    continue;
                }
                let word = block / 64;
                let bit = 1u64 << (block % 64);
                if state.blocks.get(word).is_none_or(|bits| bits & bit == 0) {
                    let actual = crc32fast::hash(&self.mapping[block_start..block_end]);
                    let expected = self.table_crc(block)?;
                    if actual != expected {
                        return Err(StorageError::ChecksumMismatch {
                            path: Some(self.path.clone()),
                            offset: block_start as u64,
                        });
                    }
                    let bits = state.blocks.get_mut(word).ok_or_else(|| {
                        StorageError::corrupt_at(&self.path, "validation bitset overflow")
                    })?;
                    *bits |= bit;
                    validated = validated.saturating_add((block_end - block_start) as u64);
                }
            }
        }
        if validated != 0 {
            self.bytes_validated.fetch_add(validated, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Validate every checksum block. Normal load and query paths never call
    /// this; it exists for explicit diagnostics and corruption tests.
    pub fn verify_all(&self) -> StorageResult<()> {
        self.validate_range(0, self.header.checksum_table_offset)
    }
}


/// One immutable byte owner, either resident or mapped.
#[derive(Clone)]
pub enum Backing {
    Resident(Arc<[u8]>),
    Mapped(Arc<MappedObject>),
}


impl std::fmt::Debug for Backing {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Resident(bytes) => formatter
                .debug_tuple("Backing::Resident")
                .field(&bytes.len())
                .finish(),
            Self::Mapped(object) => formatter
                .debug_tuple("Backing::Mapped")
                .field(object)
                .finish(),
        }
    }
}


impl Backing {
    pub fn resident(bytes: impl Into<Arc<[u8]>>) -> Self {
        Self::Resident(bytes.into())
    }

    pub fn mapped(object: Arc<MappedObject>) -> Self {
        Self::Mapped(object)
    }

    pub fn mapped_object(&self) -> Option<&Arc<MappedObject>> {
        match self {
            Self::Mapped(object) => Some(object),
            Self::Resident(_) => None,
        }
    }

    pub fn is_mapped(&self) -> bool {
        matches!(self, Self::Mapped(_))
    }

    pub fn len(&self) -> u64 {
        match self {
            Self::Resident(bytes) => bytes.len() as u64,
            Self::Mapped(object) => object.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read one checked byte range. Mapped reads validate the covering
    /// checksum regions on first access.
    pub fn read(&self, offset: u64, length: u64) -> StorageResult<&[u8]> {
        let end = offset
            .checked_add(length)
            .ok_or_else(|| StorageError::corrupt("byte range end overflow"))?;
        if end > self.len() {
            return Err(StorageError::corrupt(format!(
                "byte range {offset}..{end} exceeds backing length {}",
                self.len()
            )));
        }
        let start = usize::try_from(offset)
            .map_err(|_| StorageError::ResourceLimit("byte offset does not fit usize".into()))?;
        let length = usize::try_from(length)
            .map_err(|_| StorageError::ResourceLimit("byte length does not fit usize".into()))?;
        match self {
            Self::Resident(bytes) => Ok(&bytes[start..start + length]),
            Self::Mapped(object) => {
                object.validate_range(offset, length as u64)?;
                let start = start as usize;
                Ok(&object.mapping[start..start + length])
            }
        }
    }

    pub fn read_u8(&self, offset: u64) -> StorageResult<u8> {
        Ok(self.read(offset, 1)?[0])
    }

    pub fn read_u16(&self, offset: u64) -> StorageResult<u16> {
        let bytes = self.read(offset, 2)?;
        Ok(u16::from_le_bytes(
            bytes.try_into().expect("two-byte checked slice"),
        ))
    }

    pub fn read_u32(&self, offset: u64) -> StorageResult<u32> {
        let bytes = self.read(offset, 4)?;
        Ok(u32::from_le_bytes(
            bytes.try_into().expect("four-byte checked slice"),
        ))
    }

    pub fn read_u64(&self, offset: u64) -> StorageResult<u64> {
        let bytes = self.read(offset, 8)?;
        Ok(u64::from_le_bytes(
            bytes.try_into().expect("eight-byte checked slice"),
        ))
    }

    pub fn verify_all(&self) -> StorageResult<()> {
        match self {
            Self::Resident(bytes) => verify_resident(bytes),
            Self::Mapped(object) => object.verify_all(),
        }
    }
}


/// Read the fixed header prefix of a file without mapping it.
pub fn read_header_prefix(path: &Path) -> StorageResult<[u8; HEADER_LEN]> {
    use std::io::Read;
    let mut file = File::open(path).map_err(|error| StorageError::io(path, error))?;
    let mut header = [0u8; HEADER_LEN];
    file.read_exact(&mut header)
        .map_err(|error| StorageError::io(path, error))?;
    if header[..8] != OBJECT_MAGIC {
        return Err(StorageError::InvalidHeader(format!(
            "{} is not an Objekte object",
            path.display()
        )));
    }
    Ok(header)
}

fn verify_resident(bytes: &[u8]) -> StorageResult<()> {
    if bytes.len() < HEADER_LEN {
        return Err(StorageError::InvalidHeader("object is truncated".into()));
    }
    let header = ObjectHeader::parse(&bytes[..HEADER_LEN])?;
    if header.total_len != bytes.len() as u64 {
        return Err(StorageError::InvalidHeader(
            "resident object length differs from its header".into(),
        ));
    }
    let expected_blocks = ObjectHeader::required_checksum_count(header.checksum_table_offset)?;
    if header.checksum_count != expected_blocks {
        return Err(StorageError::InvalidHeader(
            "resident checksum table does not cover the object".into(),
        ));
    }
    let table_end = header
        .checksum_table_offset
        .checked_add(header.checksum_count as u64 * 4)
        .ok_or_else(|| StorageError::InvalidHeader("checksum table overflow".into()))?;
    if table_end != header.total_len {
        return Err(StorageError::InvalidHeader(
            "resident checksum table does not end the object".into(),
        ));
    }
    for block in 0..header.checksum_count {
        let start = block as usize * CHECKSUM_BLOCK_SIZE as usize;
        let end = (start + CHECKSUM_BLOCK_SIZE as usize).min(header.checksum_table_offset as usize);
        let at = header.checksum_table_offset as usize + block as usize * 4;
        let expected = u32::from_le_bytes(
            bytes[at..at + 4]
                .try_into()
                .expect("four-byte resident checksum"),
        );
        let actual = crc32fast::hash(&bytes[start..end]);
        if actual != expected {
            return Err(StorageError::ChecksumMismatch {
                path: None,
                offset: start as u64,
            });
        }
    }
    Ok(())
}
