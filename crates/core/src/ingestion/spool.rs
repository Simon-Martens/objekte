//! Process-local overflow frames. No restart replay or durability promise.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use bincode::Options;
use sha2::{Digest, Sha256};

use super::{Batch, IngestionOptions, Operation, StagedFile};
use crate::lifecycle::RecordRegistration;

const MAGIC: &[u8; 4] = b"OBRQ";
const FORMAT_VERSION: u32 = 2;
const HEADER_BYTES: usize = 4 + 4 + 8 + 32;
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
    let payload = encoding()
        .serialize(&(batch.registration, &batch.files, &batch.operations))
        .map_err(|error| error.to_string())?;
    let length = u64::try_from(payload.len()).map_err(|_| "spool frame too large")?;
    let mut frame = Vec::with_capacity(HEADER_BYTES.saturating_add(payload.len()));
    frame.extend_from_slice(MAGIC);
    frame.extend_from_slice(&FORMAT_VERSION.to_le_bytes());
    frame.extend_from_slice(&length.to_le_bytes());
    frame.extend_from_slice(&Sha256::digest(&payload));
    frame.extend_from_slice(&payload);
    Ok(EncodedFrame(frame))
}

pub(super) struct SpoolManager {
    root: Option<PathBuf>,
    configured_parent: Option<PathBuf>,
    session_id: u64,
    active: Option<(PathBuf, u64)>,
    pending_frames: HashMap<PathBuf, usize>,
}

#[derive(Clone)]
pub(super) struct SpoolFrame {
    pub(super) path: PathBuf,
    offset: u64,
    length: u64,
}

impl SpoolManager {
    pub(super) fn new(session_id: u64, options: &IngestionOptions) -> Self {
        Self {
            root: None,
            configured_parent: options.spool_directory.clone(),
            session_id,
            active: None,
            pending_frames: HashMap::new(),
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
        let result = (|| {
            let mut file = if created {
                OpenOptions::new().write(true).create_new(true).open(&path)
            } else {
                OpenOptions::new().append(true).open(&path)
            }
            .map_err(|error| error.to_string())?;
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
        *self.pending_frames.entry(path.clone()).or_default() += 1;
        Ok(SpoolFrame {
            path,
            offset,
            length,
        })
    }

    pub(super) fn release(&mut self, frame: &SpoolFrame) {
        let Some(pending) = self.pending_frames.get_mut(&frame.path) else {
            return;
        };
        *pending -= 1;
        if *pending == 0 {
            self.pending_frames.remove(&frame.path);
            if self
                .active
                .as_ref()
                .is_some_and(|(path, _)| path == &frame.path)
            {
                self.active = None;
            }
            let _ = fs::remove_file(&frame.path);
        }
    }

    pub(super) fn cleanup(&mut self) {
        self.active = None;
        self.pending_frames.clear();
        if let Some(root) = self.root.take() {
            let _ = fs::remove_dir_all(root);
        }
    }
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
    if &header[..4] != MAGIC
        || u32::from_le_bytes(header[4..8].try_into().unwrap()) != FORMAT_VERSION
    {
        return Err("unsupported spool frame format".into());
    }
    let length = u64::from_le_bytes(header[8..16].try_into().unwrap());
    if length > max_frame_bytes
        || length > usize::MAX as u64
        || length.checked_add(HEADER_BYTES as u64) != Some(frame.length)
    {
        return Err("spool frame length exceeds configured limit".into());
    }
    let mut payload = vec![0u8; length as usize];
    file.read_exact(&mut payload)
        .map_err(|error| error.to_string())?;
    if Sha256::digest(&payload).as_slice() != &header[16..48] {
        return Err("spool frame checksum mismatch".into());
    }
    let (registration, files, operations): (RecordRegistration, Vec<StagedFile>, Vec<Operation>) =
        encoding()
            .with_limit(max_frame_bytes)
            .deserialize(&payload)
            .map_err(|error| error.to_string())?;
    let mut batch = Batch::new(session_id, registration);
    batch.files = files;
    batch.operations = operations;
    Ok(batch)
}
