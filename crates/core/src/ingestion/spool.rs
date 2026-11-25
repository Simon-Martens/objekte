//! Process-local overflow frames. No restart replay or durability promise.

use std::collections::{HashMap, VecDeque};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bincode::Options;
use sha2::{Digest, Sha256};

use super::{Batch, IngestionOptions, Operation};
use crate::lifecycle::RecordRegistration;

const MAGIC: &[u8; 4] = b"OBRQ";
const FORMAT_VERSION: u32 = 4;
const CHECKSUM_BYTES: usize = 4 + 4 + 8 + 32;
const HEADER_BYTES: usize = CHECKSUM_BYTES + 8 + 16 + 8 + 1;
const CHUNK_TARGET_BYTES: u64 = 64 * 1024 * 1024;
static NEXT_CHUNK: AtomicU64 = AtomicU64::new(1);

fn encoding() -> impl Options {
    bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .reject_trailing_bytes()
}

pub(super) struct EncodedFrame(Vec<u8>);

impl EncodedFrame {
    pub(super) fn len(&self) -> u64 {
        self.0.len() as u64
    }
}

pub(super) fn encode(batch: &Batch) -> Result<EncodedFrame, String> {
    let expected = encoded_len(batch)?;
    let mut frame = Vec::with_capacity(expected as usize);
    frame.resize(HEADER_BYTES, 0);
    encoding()
        .serialize_into(&mut frame, &(batch.registration, &batch.operations))
        .map_err(|error| error.to_string())?;
    if frame.len() as u64 != expected {
        return Err("spool serialized size changed during encoding".into());
    }
    let length = expected - HEADER_BYTES as u64;
    frame[..4].copy_from_slice(MAGIC);
    frame[4..8].copy_from_slice(&FORMAT_VERSION.to_le_bytes());
    frame[8..16].copy_from_slice(&length.to_le_bytes());
    frame[48..56].copy_from_slice(&(batch.estimated_bytes as u64).to_le_bytes());
    frame[56..64].copy_from_slice(&batch.registration.builder_id.to_le_bytes());
    frame[64..68].copy_from_slice(&batch.registration.ordinal.to_le_bytes());
    frame[68..72].copy_from_slice(&batch.registration.collection.0.to_le_bytes());
    frame[72..80].copy_from_slice(&(batch.operations.len() as u64).to_le_bytes());
    if let Some((provider, implicit_text)) = super::queue::group_shape(batch) {
        frame[80] = 1 | (u8::from(provider) << 1) | (u8::from(implicit_text) << 2);
    }
    let digest = Sha256::digest(&frame[CHECKSUM_BYTES..]);
    frame[16..48].copy_from_slice(&digest);
    Ok(EncodedFrame(frame))
}

pub(super) fn encoded_len(batch: &Batch) -> Result<u64, String> {
    let payload = encoding()
        .serialized_size(&(batch.registration, &batch.operations))
        .map_err(|error| error.to_string())?;
    let total = payload
        .checked_add(HEADER_BYTES as u64)
        .ok_or("spool frame too large")?;
    if total > usize::MAX as u64 {
        return Err("spool frame too large".into());
    }
    Ok(total)
}

pub(super) struct SpoolManager {
    root: Option<PathBuf>,
    configured_parent: Option<PathBuf>,
    session_id: u64,
    active: Option<(PathBuf, u64)>,
    // Frame count and physical bytes; an already consumed prefix still uses
    // disk until the final frame in its chunk is released.
    pending_frames: HashMap<PathBuf, (usize, u64)>,
    unread: VecDeque<UnreadChunk>,
    #[cfg(test)]
    disk_full_after: Option<usize>,
}

struct UnreadChunk {
    path: PathBuf,
    offset: u64,
    end: u64,
}

pub(super) struct WaitingFrame {
    pub(super) frame: SpoolFrame,
    pub(super) weight: usize,
    pub(super) registration: RecordRegistration,
    pub(super) records: usize,
    pub(super) shape: Option<(bool, bool)>,
}

#[derive(Clone)]
pub(super) struct SpoolFrame {
    pub(super) path: PathBuf,
    offset: u64,
    pub(super) length: u64,
}

impl SpoolManager {
    pub(super) fn new(session_id: u64, options: &IngestionOptions) -> Self {
        Self {
            root: None,
            configured_parent: options.spool_directory.clone(),
            session_id,
            active: None,
            pending_frames: HashMap::new(),
            unread: VecDeque::new(),
            #[cfg(test)]
            disk_full_after: None,
        }
    }

    fn root(&mut self) -> Result<&Path, String> {
        if self.root.is_none() {
            let parent = self
                .configured_parent
                .clone()
                .unwrap_or_else(std::env::temp_dir);
            let path = parent.join(format!(
                "objekte-ingestion-{}-{}",
                std::process::id(),
                self.session_id
            ));
            #[cfg(unix)]
            {
                use std::os::unix::fs::DirBuilderExt;
                fs::DirBuilder::new()
                    .mode(0o700)
                    .create(&path)
                    .map_err(|error| error.to_string())?;
            }
            #[cfg(not(unix))]
            fs::create_dir(&path).map_err(|error| error.to_string())?;
            self.root = Some(path);
        }
        Ok(self.root.as_deref().expect("spool root created"))
    }

    pub(super) fn write(&mut self, frame: &EncodedFrame) -> Result<SpoolFrame, String> {
        let length = frame.len();
        let append = self.active.as_ref().filter(|(_, size)| {
            size.checked_add(length)
                .is_some_and(|total| total <= CHUNK_TARGET_BYTES)
        });
        let (path, offset, created) = if let Some((path, size)) = append {
            (path.clone(), *size, false)
        } else {
            let ordinal = NEXT_CHUNK.fetch_add(1, Ordering::Relaxed);
            (
                self.root()?.join(format!("chunk-{ordinal:020}.bin")),
                0,
                true,
            )
        };
        #[cfg(test)]
        let disk_full_after = self.disk_full_after.take();
        let result = (|| {
            let mut file = if created {
                OpenOptions::new().write(true).create_new(true).open(&path)
            } else {
                OpenOptions::new().append(true).open(&path)
            }
            .map_err(|error| error.to_string())?;
            #[cfg(test)]
            if let Some(bytes) = disk_full_after {
                file.write_all(&frame.0[..bytes.min(frame.0.len())])
                    .map_err(|error| error.to_string())?;
                return Err("No space left on device (injected)".into());
            }
            file.write_all(&frame.0)
                .map_err(|error| error.to_string())?;
            file.flush().map_err(|error| error.to_string())?;
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            if created {
                let _ = fs::remove_file(&path);
            } else if let Ok(file) = OpenOptions::new().write(true).open(&path) {
                let _ = file.set_len(offset);
            }
            self.active = None;
            return Err(error);
        }
        self.active =
            (offset + length < CHUNK_TARGET_BYTES).then(|| (path.clone(), offset + length));
        let pending = self.pending_frames.entry(path.clone()).or_default();
        pending.0 += 1;
        pending.1 = offset + length;
        Ok(SpoolFrame {
            path,
            offset,
            length,
        })
    }

    /// Called only after the complete frame can be admitted. One cursor
    /// describes the contiguous waiting suffix of each physical chunk.
    pub(super) fn queue_waiting(&mut self, frame: &SpoolFrame) {
        if let Some(chunk) = self
            .unread
            .back_mut()
            .filter(|chunk| chunk.path == frame.path)
        {
            debug_assert_eq!(chunk.end, frame.offset);
            chunk.end += frame.length;
        } else {
            self.unread.push_back(UnreadChunk {
                path: frame.path.clone(),
                offset: frame.offset,
                end: frame.offset + frame.length,
            });
        }
    }

    pub(super) fn peek_waiting(&self, max_weight: usize) -> Result<WaitingFrame, String> {
        let chunk = self.unread.front().ok_or("spool waiting cursor missing")?;
        let mut file = File::open(&chunk.path).map_err(|error| error.to_string())?;
        file.seek(SeekFrom::Start(chunk.offset))
            .map_err(|error| error.to_string())?;
        let mut header = [0u8; HEADER_BYTES];
        file.read_exact(&mut header)
            .map_err(|error| error.to_string())?;
        let payload = frame_payload_length(&header)?;
        let length = payload
            .checked_add(HEADER_BYTES as u64)
            .ok_or("spool frame too large")?;
        let weight = usize::try_from(u64::from_le_bytes(header[48..56].try_into().unwrap()))
            .map_err(|_| "spool frame weight overflow")?;
        let records = usize::try_from(u64::from_le_bytes(header[72..80].try_into().unwrap()))
            .map_err(|_| "spool frame record count overflow")?;
        let flags = header[80];
        if length > chunk.end - chunk.offset
            || weight > max_weight
            || records == 0
            || (flags != 0 && (flags & 1 == 0 || flags > 7))
        {
            return Err("invalid spool waiting metadata".into());
        }
        Ok(WaitingFrame {
            frame: SpoolFrame {
                path: chunk.path.clone(),
                offset: chunk.offset,
                length,
            },
            weight,
            registration: RecordRegistration {
                builder_id: u64::from_le_bytes(header[56..64].try_into().unwrap()),
                ordinal: u32::from_le_bytes(header[64..68].try_into().unwrap()),
                collection: crate::model::CollectionId(u32::from_le_bytes(
                    header[68..72].try_into().unwrap(),
                )),
            },
            records,
            shape: (flags != 0).then_some((flags & 2 != 0, flags & 4 != 0)),
        })
    }

    pub(super) fn consume_waiting(&mut self, frame: &SpoolFrame) {
        let chunk = self.unread.front_mut().expect("waiting frame has a cursor");
        debug_assert_eq!(
            (chunk.path.as_path(), chunk.offset),
            (frame.path.as_path(), frame.offset)
        );
        chunk.offset += frame.length;
        if chunk.offset == chunk.end {
            self.unread.pop_front();
        }
    }

    #[cfg(test)]
    pub(super) fn inject_disk_full_after(&mut self, bytes: usize) {
        self.disk_full_after = Some(bytes);
    }

    /// Return the physical bytes reclaimed when the last frame of a chunk
    /// leaves the queue. The caller can then release the spool reservation.
    pub(super) fn release(&mut self, frame: &SpoolFrame) -> u64 {
        let Some(pending) = self.pending_frames.get_mut(&frame.path) else {
            return 0;
        };
        pending.0 -= 1;
        if pending.0 == 0 {
            let bytes = self
                .pending_frames
                .remove(&frame.path)
                .expect("chunk exists")
                .1;
            if self
                .active
                .as_ref()
                .is_some_and(|(path, _)| path == &frame.path)
            {
                self.active = None;
            }
            let _ = fs::remove_file(&frame.path);
            return bytes;
        }
        0
    }

    pub(super) fn discard_uncommitted_tail(&mut self, frame: &SpoolFrame) -> u64 {
        let Some((count, physical)) = self.pending_frames.get_mut(&frame.path) else {
            return 0;
        };
        if *count == 1 {
            return self.release(frame);
        }
        if *physical == frame.offset + frame.length
            && OpenOptions::new()
                .write(true)
                .open(&frame.path)
                .and_then(|file| file.set_len(frame.offset))
                .is_ok()
        {
            *count -= 1;
            *physical = frame.offset;
            self.active = Some((frame.path.clone(), frame.offset));
            frame.length
        } else {
            self.release(frame)
        }
    }

    pub(super) fn cleanup(&mut self) {
        self.active = None;
        self.pending_frames.clear();
        self.unread.clear();
        if let Some(root) = self.root.take() {
            let _ = fs::remove_dir_all(root);
        }
    }
}

fn frame_payload_length(header: &[u8; HEADER_BYTES]) -> Result<u64, String> {
    if &header[..4] != MAGIC
        || u32::from_le_bytes(header[4..8].try_into().unwrap()) != FORMAT_VERSION
    {
        return Err("unsupported spool frame format".into());
    }
    Ok(u64::from_le_bytes(header[8..16].try_into().unwrap()))
}

pub(super) fn read(
    frame: &SpoolFrame,
    max_frame_bytes: u64,
    session_id: u64,
) -> Result<Batch, String> {
    let mut file = File::open(&frame.path).map_err(|error| error.to_string())?;
    file.seek(SeekFrom::Start(frame.offset))
        .map_err(|error| error.to_string())?;
    let mut header = [0u8; HEADER_BYTES];
    file.read_exact(&mut header)
        .map_err(|error| error.to_string())?;
    let length = frame_payload_length(&header)?;
    if length > max_frame_bytes
        || length > usize::MAX as u64
        || length.checked_add(HEADER_BYTES as u64) != Some(frame.length)
    {
        return Err("spool frame length exceeds configured limit".into());
    }
    let mut payload = vec![0u8; length as usize];
    file.read_exact(&mut payload)
        .map_err(|error| error.to_string())?;
    let mut digest = Sha256::new();
    digest.update(&header[CHECKSUM_BYTES..]);
    digest.update(&payload);
    if digest.finalize().as_slice() != &header[16..48] {
        return Err("spool frame checksum mismatch".into());
    }
    let (registration, operations): (RecordRegistration, Vec<Operation>) = encoding()
        .with_limit(max_frame_bytes)
        .deserialize(&payload)
        .map_err(|error| error.to_string())?;
    let mut batch = Batch::new(session_id, registration);
    batch.operations = operations;
    Ok(batch)
}
