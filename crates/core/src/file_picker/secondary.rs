//! Process-local indexes stored beside the portable bigram snapshot.

use super::*;
use bincode::Options;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{Read, Write},
};
use view::SavedViewIndex;

const MAGIC: &[u8; 8] = b"OBJSEC01";
const HEADER: usize = 8 + 32 + 32 + 8 + 32;
const MAX_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Serialize)]
struct Saved<'a> {
    slots: usize,
    malformed: &'a BTreeSet<usize>,
    array_fields: &'a BTreeSet<String>,
    key_summary: &'a KeySummary,
    presence: &'a PresenceIndex,
    typed: &'a TypedIndex,
    view: &'a SavedViewIndex,
    text: &'a Option<(Vec<u8>, Vec<u8>, usize)>,
}

#[derive(Deserialize)]
struct Restored {
    slots: usize,
    malformed: BTreeSet<usize>,
    array_fields: BTreeSet<String>,
    key_summary: KeySummary,
    presence: PresenceIndex,
    typed: TypedIndex,
    view: SavedViewIndex,
    text: Option<(Vec<u8>, Vec<u8>, usize)>,
}

fn sidecar(path: &Path) -> PathBuf {
    path.with_extension("secondary")
}

fn base_digest(path: &Path) -> Result<[u8; 32]> {
    let mut file = fs::File::open(path).map_err(|error| io(path, error))?;
    let mut header = [0_u8; 56];
    file.read_exact(&mut header)
        .map_err(|error| io(path, error))?;
    if &header[..8] != b"OBJSNP01" {
        return Err(Error::Cache("unknown bigram snapshot version".into()));
    }
    Ok(header[24..56].try_into().expect("digest length"))
}

pub(super) fn config_digest(
    indexes: &[IndexField],
    advanced_paths: &[crate::JsonPath],
    definitions: &BTreeMap<String, Field>,
) -> Result<[u8; 32]> {
    let bytes = serde_json::to_vec(&(indexes, advanced_paths, definitions))
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
        let bytes = fs::read(&sidecar).map_err(|error| io(&sidecar, error))?;
        if &bytes[..8] != MAGIC
            || bytes[8..40] != base_digest(path)?
            || bytes[40..72] != self.config_digest
        {
            return Ok(false);
        }
        let length = u64::from_le_bytes(bytes[72..80].try_into().expect("length"));
        if length != (bytes.len() - HEADER) as u64
            || bytes[80..112] != Sha256::digest(&bytes[HEADER..]).as_slice()[..]
        {
            return Ok(false);
        }
        let restored: Restored = match bincode::DefaultOptions::new()
            .with_limit(MAX_BYTES)
            .deserialize(&bytes[HEADER..])
        {
            Ok(restored) => restored,
            Err(_) => return Ok(false),
        };
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
        if !self.view.restore(restored.view, self.files.len()) {
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
        let text = self
            .text_index
            .as_ref()
            .map(TextIndex::save_parts)
            .transpose()?;
        let payload = bincode::DefaultOptions::new()
            .serialize(&Saved {
                slots: self.files.len(),
                malformed: &self.malformed,
                array_fields: &self.array_fields,
                key_summary: &self.key_summary,
                presence: &self.presence,
                typed: &self.typed,
                view: &view,
                text: &text,
            })
            .map_err(|error| Error::Cache(format!("cannot encode secondary index: {error}")))?;
        if payload.len() as u64 > MAX_BYTES - HEADER as u64 {
            return Err(Error::Cache("secondary index exceeds size limit".into()));
        }
        let sidecar = sidecar(path);
        let parent = sidecar.parent().expect("sidecar has parent");
        fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| io(parent, error))?;
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
            .write_all(&(payload.len() as u64).to_le_bytes())
            .map_err(|error| io(&sidecar, error))?;
        temporary
            .write_all(&Sha256::digest(&payload))
            .map_err(|error| io(&sidecar, error))?;
        temporary
            .write_all(&payload)
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
