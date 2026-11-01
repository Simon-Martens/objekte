//! Process-local indexes stored beside the portable bigram snapshot.

use super::*;
use bincode::Options;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Seek, SeekFrom, Write},
};
use view::SavedViewIndex;
use sort_index::{RestoredSortIndex, SavedSortIndex};

const MAGIC: &[u8; 8] = b"OBJSEC03";
const HEADER: usize = 8 + 32 + 32 + 8 + 32;
// A single declared sort order can exceed 1 GiB for multi-million-record collections.
const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;

#[derive(Serialize)]
struct Saved<'a> {
    slots: usize,
    modified_order: &'a [u32],
    malformed: &'a BTreeSet<usize>,
    array_fields: &'a BTreeSet<String>,
    key_summary: &'a KeySummary,
    presence: &'a PresenceIndex,
    typed: &'a TypedIndex,
    view: &'a SavedViewIndex,
    sort: &'a SavedSortIndex<'a>,
    text: &'a Option<(Vec<u8>, Vec<u8>, usize)>,
}

#[derive(Deserialize)]
struct Restored {
    slots: usize,
    modified_order: Vec<u32>,
    malformed: BTreeSet<usize>,
    array_fields: BTreeSet<String>,
    key_summary: KeySummary,
    presence: PresenceIndex,
    typed: TypedIndex,
    view: SavedViewIndex,
    sort: RestoredSortIndex,
    text: Option<(Vec<u8>, Vec<u8>, usize)>,
}

struct DigestWriter<'a> {
    file: &'a mut fs::File,
    digest: &'a mut Sha256,
    bytes: &'a mut u64,
}

struct DigestReader<R> {
    inner: R,
    digest: Sha256,
    bytes: u64,
}

impl<R: Read> Read for DigestReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        self.digest.update(&buffer[..read]);
        self.bytes += read as u64;
        Ok(read)
    }
}

impl Write for DigestWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        let written = self.file.write(buffer)?;
        self.digest.update(&buffer[..written]);
        *self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

fn sidecar(path: &Path) -> PathBuf {
    path.with_extension("secondary")
}

fn base_digest(path: &Path) -> Result<[u8; 32]> {
    let mut file = fs::File::open(path).map_err(|error| io(path, error))?;
    let mut header = [0_u8; 56];
    file.read_exact(&mut header)
        .map_err(|error| io(path, error))?;
    if &header[..8] != b"OBJSNP01" && &header[..8] != b"OBJSNP02" {
        return Err(Error::Cache("unknown bigram snapshot version".into()));
    }
    Ok(header[24..56].try_into().expect("digest length"))
}

pub(super) fn config_digest(
    indexes: &[IndexField],
    advanced_paths: &[crate::JsonPath],
    sort_indexes: &[Vec<crate::Sort>],
    definitions: &BTreeMap<String, Field>,
) -> Result<[u8; 32]> {
    let bytes = serde_json::to_vec(&(indexes, advanced_paths, sort_indexes, definitions, "icu_collator/2.4.0"))
        .map_err(|error| Error::Cache(format!("cannot encode index config: {error}")))?;
    Ok(Sha256::digest(bytes).into())
}

impl FilePicker {
    pub(super) fn load_secondary_snapshot(&mut self, path: &Path) -> Result<bool> {
        let sidecar = sidecar(path);
        let size = match fs::metadata(&sidecar) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io(&sidecar, error)),
        };
        if size < HEADER as u64 || size > MAX_BYTES {
            return Ok(false);
        }
        let mut file = std::io::BufReader::new(fs::File::open(&sidecar).map_err(|error| io(&sidecar, error))?);
        let mut header = [0_u8; HEADER];
        file.read_exact(&mut header).map_err(|error| io(&sidecar, error))?;
        if &header[..8] != MAGIC
            || header[8..40] != base_digest(path)?
            || header[40..72] != self.config_digest
        {
            return Ok(false);
        }
        let length = u64::from_le_bytes(header[72..80].try_into().expect("length"));
        if length != size - HEADER as u64 {
            return Ok(false);
        }
        let mut reader = DigestReader { inner: file.take(length), digest: Sha256::new(), bytes: 0 };
        let restored: Restored = match bincode::DefaultOptions::new()
            .with_limit(MAX_BYTES)
            .deserialize_from(&mut reader)
        {
            Ok(restored) => restored,
            Err(_) => return Ok(false),
        };
        if reader.bytes != length || header[80..112] != reader.digest.finalize().as_slice()[..] {
            return Ok(false);
        }
        if restored.slots != self.files.len()
            || restored.presence.fields() != self.presence.fields()
            || !restored.presence.valid_for_slots(self.files.len())
            || restored.typed.fields() != self.typed.fields()
            || !restored.typed.valid_for_slots(self.files.len())
            || restored
                .malformed
                .iter()
                .any(|&slot| slot >= self.files.len())
        {
            return Ok(false);
        }
        if !self.modified_order.restore(restored.modified_order, &self.files) {
            return Ok(false);
        }
        if !self.view.restore(restored.view, self.files.len()) {
            return Ok(false);
        }
        if !self.sort.restore(restored.sort, self.files.len()) {
            return Ok(false);
        }
        let text_index = match restored.text {
            Some((clean, raw, len)) if len == self.files.len() => {
                Some(TextIndex::restore_parts(&clean, &raw, len)?)
            }
            Some(_) => return Ok(false),
            None => None,
        };
        if self.eager_text_index && text_index.is_none() {
            return Ok(false);
        }
        self.malformed = restored.malformed;
        self.array_fields = restored.array_fields;
        self.key_summary = restored.key_summary;
        self.presence = restored.presence;
        self.typed = restored.typed;
        self.text_index = text_index;
        self.secondary_loaded = true;
        Ok(true)
    }

    pub(crate) fn save_secondary_snapshot(&self, path: &Path) -> Result<()> {
        let view = self.view.saved();
        let sort = self.sort.saved();
        let modified_order = self.modified_order.snapshot(&self.files, &self.id_bytes);
        let text = self
            .text_index
            .as_ref()
            .map(TextIndex::save_parts)
            .transpose()?;
        let sidecar = sidecar(path);
        let parent = sidecar.parent().expect("sidecar has parent");
        fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| io(parent, error))?;
        temporary.write_all(&[0; HEADER]).map_err(|error| io(&sidecar, error))?;
        let mut payload_bytes = 0_u64;
        let mut payload_digest = Sha256::new();
        {
            let writer = DigestWriter {
                file: temporary.as_file_mut(),
                digest: &mut payload_digest,
                bytes: &mut payload_bytes,
            };
            let mut buffered = std::io::BufWriter::with_capacity(64 * 1024, writer);
            bincode::DefaultOptions::new()
                .serialize_into(&mut buffered, &Saved {
                    slots: self.files.len(),
                    modified_order: &modified_order,
                    malformed: &self.malformed,
                    array_fields: &self.array_fields,
                    key_summary: &self.key_summary,
                    presence: &self.presence,
                    typed: &self.typed,
                    view: &view,
                    sort: &sort,
                    text: &text,
                })
                .map_err(|error| Error::Cache(format!("cannot encode secondary index: {error}")))?;
            buffered.flush().map_err(|error| io(&sidecar, error))?;
        }
        if payload_bytes > MAX_BYTES - HEADER as u64 {
            return Err(Error::Cache("secondary index exceeds size limit".into()));
        }
        temporary.as_file_mut().seek(SeekFrom::Start(0)).map_err(|error| io(&sidecar, error))?;
        temporary
            .write_all(MAGIC)
            .map_err(|error| io(&sidecar, error))?;
        temporary
            .write_all(&base_digest(path)?)
            .map_err(|error| io(&sidecar, error))?;
        temporary
            .write_all(&self.config_digest)
            .map_err(|error| io(&sidecar, error))?;
        temporary
            .write_all(&payload_bytes.to_le_bytes())
            .map_err(|error| io(&sidecar, error))?;
        temporary
            .write_all(&payload_digest.finalize())
            .map_err(|error| io(&sidecar, error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| io(&sidecar, error))?;
        temporary
            .persist(&sidecar)
            .map_err(|error| io(&sidecar, error.error))?;
        Ok(())
    }
}
