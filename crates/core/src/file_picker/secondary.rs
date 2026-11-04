//! Process-local indexes stored beside the portable bigram snapshot.

use super::*;
use bincode::Options;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sort_index::{RestoredSortIndex, SavedSortIndex};
use std::{
    fs,
    io::{BufRead, BufReader, Read, Seek, SeekFrom, Write},
};
use view::{RestoredViewIndex, SavedViewIndex};

// Flat sort-key storage changes the secondary payload. Older sidecars are
// cache misses; their portable bigram snapshot remains reusable.
// The global rendered filter now holds only text outside registered fields.
// 07: typed postings are patched for changed files, so older companions
// with unpatched overlays are cache misses.
// 08: normalized text drops Unicode punctuation, so rendered filters built
// with punctuation are cache misses.
const MAGIC: &[u8; 8] = b"OBJSEC08";
const HEADER: usize = 8 + 32 + 32 + 8 + 32;
// A single declared sort order can exceed 1 GiB for multi-million-record collections.
const MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;


/// Borrowed secondary structures written before optional text-filter sections.
#[derive(Serialize)]
struct Saved<'a> {
    slots: usize,
    modified_order: &'a [u32],
    malformed: &'a BTreeSet<usize>,
    array_fields: &'a BTreeSet<String>,
    key_summary: &'a KeySummary,
    presence: &'a PresenceIndex,
    typed: &'a TypedIndex,
    view: &'a SavedViewIndex<'a>,
    sort: &'a SavedSortIndex<'a>,
    text: bool,
    text_paths: Vec<String>,
    global_clean_present: bool,
    field_presence: Vec<(bool, bool)>,
    global_clean_observed: &'a [u64],
    field_normal_observed: Vec<&'a [u64]>,
    field_clean_observed: Vec<&'a [u64]>,
}


/// Owned secondary structures held until the checksum and slot checks pass.
#[derive(Deserialize)]
struct Restored {
    slots: usize,
    modified_order: Vec<u32>,
    malformed: BTreeSet<usize>,
    array_fields: BTreeSet<String>,
    key_summary: KeySummary,
    presence: PresenceIndex,
    typed: TypedIndex,
    view: RestoredViewIndex,
    sort: RestoredSortIndex,
    text: bool,
    text_paths: Vec<String>,
    global_clean_present: bool,
    field_presence: Vec<(bool, bool)>,
    global_clean_observed: Vec<u64>,
    field_normal_observed: Vec<Vec<u64>>,
    field_clean_observed: Vec<Vec<u64>>,
}


/// Caps and hashes the framed sidecar payload as it reaches the file.
struct DigestWriter<'a> {
    file: &'a mut fs::File,
    digest: &'a mut Sha256,
    bytes: &'a mut u64,
}


/// Hashes buffered payload reads while tracking their physical byte count.
struct DigestReader<R> {
    inner: R,
    digest: Sha256,
    bytes: u64,
    #[cfg(feature = "index-timings")]
    hash_nanos: u64,
}


impl<R: Read> Read for DigestReader<R> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buffer)?;
        #[cfg(feature = "index-timings")]
        let hash_started = Instant::now();
        self.digest.update(&buffer[..read]);
        #[cfg(feature = "index-timings")]
        {
            self.hash_nanos += elapsed_nanos(hash_started);
        }
        self.bytes += read as u64;
        Ok(read)
    }
}


/// Keep the common buffered bincode read small regardless of whether LLVM
/// chooses to inline `BufReader`'s slow `read_exact` fallback.
struct ExactReader<'a, R: Read>(&'a mut BufReader<DigestReader<R>>);


impl<R: Read> Read for ExactReader<'_, R> {
    fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(output)
    }

    fn read_exact(&mut self, output: &mut [u8]) -> std::io::Result<()> {
        if output.len() <= self.0.buffer().len() {
            output.copy_from_slice(&self.0.buffer()[..output.len()]);
            self.0.consume(output.len());
            Ok(())
        } else {
            slow_read_exact(self.0, output)
        }
    }
}


#[cold]
#[inline(never)]
fn slow_read_exact<R: Read>(
    reader: &mut BufReader<DigestReader<R>>,
    output: &mut [u8],
) -> std::io::Result<()> {
    reader.read_exact(output)
}

fn consumed<R: Read>(reader: &BufReader<DigestReader<R>>) -> u64 {
    reader.get_ref().bytes - reader.buffer().len() as u64
}

fn read_filter_section<R: Read>(
    reader: &mut BufReader<DigestReader<R>>,
    total: u64,
) -> Result<BigramFilter> {
    let mut encoded_len = [0_u8; 8];
    reader
        .read_exact(&mut encoded_len)
        .map_err(|error| Error::Cache(format!("invalid secondary text length: {error}")))?;
    let length = u64::from_le_bytes(encoded_len);
    if length > total.saturating_sub(consumed(reader)) {
        return Err(Error::Cache(
            "secondary text section exceeds payload".into(),
        ));
    }
    let mut section = (&mut *reader).take(length);
    let filter = BigramFilter::read_from(&mut section, length)?;
    if section.limit() != 0 {
        return Err(Error::Cache(
            "secondary text section has trailing bytes".into(),
        ));
    }
    Ok(filter)
}


impl Write for DigestWriter<'_> {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        if bytes_saturating_add(*self.bytes, buffer.len()) > MAX_BYTES - HEADER as u64 {
            return Err(std::io::Error::other("secondary index exceeds size limit"));
        }
        let written = self.file.write(buffer)?;
        self.digest.update(&buffer[..written]);
        *self.bytes += written as u64;
        Ok(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}


fn bytes_saturating_add(bytes: u64, extra: usize) -> u64 {
    bytes.saturating_add(u64::try_from(extra).unwrap_or(u64::MAX))
}

fn sidecar(path: &Path) -> PathBuf {
    path.with_extension("secondary")
}

fn base_digest(path: &Path) -> Result<[u8; 32]> {
    let mut file = fs::File::open(path).map_err(|error| io(path, error))?;
    let mut header = [0_u8; 56];
    file.read_exact(&mut header)
        .map_err(|error| io(path, error))?;
    if &header[..8] != b"OBJSNP01"
        && &header[..8] != b"OBJSNP02"
        && &header[..8] != b"OBJSNP03"
        && &header[..8] != b"OBJSNP04"
        && &header[..8] != b"OBJSNP05"
    {
        return Err(Error::Cache("unknown bigram snapshot version".into()));
    }
    Ok(header[24..56].try_into().expect("digest length"))
}

pub(super) fn config_digest(
    indexes: &[IndexField],
    advanced_paths: &[crate::JsonPath],
    sort_indexes: &[Vec<crate::Sort>],
    definitions: &BTreeMap<String, Field>,
    language: Option<&str>,
    encoding: BigramEncoding,
) -> Result<[u8; 32]> {
    let bytes = serde_json::to_vec(&(
        indexes,
        advanced_paths,
        sort_indexes,
        definitions,
        language,
        encoding,
        if indexes.is_empty() {
            "icu_collator/2.4.0"
        } else {
            "icu_collator/2.4.0/scalar_postings_v1"
        },
    ))
    .map_err(|error| Error::Cache(format!("cannot encode index config: {error}")))?;
    Ok(Sha256::digest(bytes).into())
}


impl FilePicker {
    /// Re-serialize the sidecar alone after a base snapshot has been written.
    #[cfg(feature = "index-timings")]
    pub fn profile_secondary_snapshot(&self, path: &Path) -> Result<super::StageMeasurement> {
        let timer = super::StageTimer::start();
        self.save_secondary_snapshot(path)?;
        Ok(timer.stop())
    }

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
        let mut file =
            std::io::BufReader::new(fs::File::open(&sidecar).map_err(|error| io(&sidecar, error))?);
        let mut header = [0_u8; HEADER];
        file.read_exact(&mut header)
            .map_err(|error| io(&sidecar, error))?;
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
        file.seek(SeekFrom::Start(HEADER as u64))
            .map_err(|error| io(&sidecar, error))?;
        // Hash full buffered reads instead of every small bincode field.
        let mut reader = BufReader::with_capacity(
            64 * 1024,
            DigestReader {
                inner: file.into_inner().take(length),
                digest: Sha256::new(),
                bytes: 0,
                #[cfg(feature = "index-timings")]
                hash_nanos: 0,
            },
        );
        #[cfg(feature = "index-timings")]
        let structure_started = Instant::now();
        let restored: Restored = match bincode::DefaultOptions::new()
            .with_limit(MAX_BYTES)
            .deserialize_from(ExactReader(&mut reader))
        {
            Ok(restored) => restored,
            Err(_) => return Ok(false),
        };
        #[cfg(feature = "index-timings")]
        {
            self.restore_timings.secondary_structure_decode_nanos =
                elapsed_nanos(structure_started);
        }

        #[cfg(feature = "index-timings")]
        let filters_started = Instant::now();
        let text_index = if restored.text {
            if restored.text_paths
                != self
                    .text_paths
                    .iter()
                    .map(|path| serde_json::to_string(path).expect("path serializes"))
                    .collect::<Vec<_>>()
                || restored.field_presence.len() != self.text_paths.len()
                || restored.field_normal_observed.len() != self.text_paths.len()
                || restored.field_clean_observed.len() != self.text_paths.len()
            {
                return Ok(false);
            }
            let global_clean = match read_filter_section(&mut reader, length) {
                Ok(filter) => filter,
                Err(_) => return Ok(false),
            };
            #[cfg(feature = "index-timings")]
            {
                self.restore_timings.posting_columns += global_clean.profile_total_columns();
            }
            let mut fields = Vec::with_capacity(self.text_paths.len());
            for ((path, &(normal_present, clean_present)), (normal_observed, clean_observed)) in
                self.text_paths.iter().zip(&restored.field_presence).zip(
                    restored
                        .field_normal_observed
                        .into_iter()
                        .zip(restored.field_clean_observed),
                )
            {
                let normal = match read_filter_section(&mut reader, length) {
                    Ok(filter) => filter,
                    Err(_) => return Ok(false),
                };
                let clean = match read_filter_section(&mut reader, length) {
                    Ok(filter) => filter,
                    Err(_) => return Ok(false),
                };
                #[cfg(feature = "index-timings")]
                {
                    self.restore_timings.posting_columns +=
                        normal.profile_total_columns() + clean.profile_total_columns();
                }
                fields.push(text::FieldTextIndex {
                    path: path.clone(),
                    normal,
                    clean,
                    normal_present,
                    clean_present,
                    normal_observed,
                    clean_observed,
                });
            }
            let mut encoded_len = [0_u8; 8];
            if reader.read_exact(&mut encoded_len).is_err() {
                return Ok(false);
            }
            let indexed_len = u64::from_le_bytes(encoded_len);
            if indexed_len != self.files.len() as u64 {
                return Ok(false);
            }
            match TextIndex::from_filters(
                global_clean,
                restored.global_clean_present,
                restored.global_clean_observed,
                fields,
                self.files.len(),
                self.encoding,
                &self.text_paths,
            ) {
                Ok(index) => Some(index),
                Err(_) => return Ok(false),
            }
        } else {
            if !restored.text_paths.is_empty()
                || restored.global_clean_present
                || !restored.field_presence.is_empty()
                || !restored.global_clean_observed.is_empty()
                || !restored.field_normal_observed.is_empty()
                || !restored.field_clean_observed.is_empty()
            {
                return Ok(false);
            }
            None
        };
        #[cfg(feature = "index-timings")]
        {
            self.restore_timings.secondary_filter_decode_nanos = elapsed_nanos(filters_started);
        }
        let decoded_bytes = consumed(&reader);
        let digest_reader = reader.into_inner();
        #[cfg(feature = "index-timings")]
        {
            self.restore_timings.secondary_checksum_nanos = digest_reader.hash_nanos;
            self.restore_timings.snapshot_bytes_read += HEADER as u64 + 56 + digest_reader.bytes;
        }
        let digest = digest_reader.digest.finalize();
        if decoded_bytes != length || header[80..112] != digest.as_slice()[..] {
            return Ok(false);
        }

        #[cfg(feature = "index-timings")]
        let validation_started = Instant::now();
        if restored.slots != self.files.len()
            || restored.presence.fields() != self.presence.fields()
            || !restored.presence.valid_for_slots(self.files.len())
            || restored.typed.fields() != self.typed.fields()
            || !restored.typed.valid_for_slots(self.files.len())
            || restored
                .malformed
                .iter()
                .any(|&slot| slot >= self.files.len())
            || !ModifiedOrder::valid_saved_order(&restored.modified_order, &self.files)
            || !self.view.can_restore(&restored.view, self.files.len())
            || !self.sort.can_restore(&restored.sort, self.files.len())
            || self.eager_text_index && text_index.is_none()
        {
            return Ok(false);
        }

        #[cfg(feature = "index-timings")]
        {
            self.restore_timings.secondary_validation_nanos = elapsed_nanos(validation_started);
        }
        self.modified_order.apply_restored(restored.modified_order);
        self.view.apply_restored(restored.view);
        self.sort.apply_restored(restored.sort);
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
        let (global_clean_present, field_presence) =
            self.text_index
                .as_ref()
                .map_or((false, Vec::new()), |text| {
                    let (_, present, fields, _) = text.filters();
                    (
                        present,
                        fields
                            .iter()
                            .map(|field| (field.normal_present, field.clean_present))
                            .collect(),
                    )
                });
        let modified_order = self.modified_order.snapshot(&self.files, &self.id_bytes);
        let sidecar = sidecar(path);
        let parent = sidecar.parent().expect("sidecar has parent");
        fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| io(parent, error))?;
        temporary
            .write_all(&[0; HEADER])
            .map_err(|error| io(&sidecar, error))?;
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
                .serialize_into(
                    &mut buffered,
                    &Saved {
                        slots: self.files.len(),
                        modified_order: &modified_order,
                        malformed: &self.malformed,
                        array_fields: &self.array_fields,
                        key_summary: &self.key_summary,
                        presence: &self.presence,
                        typed: &self.typed,
                        view: &view,
                        sort: &sort,
                        text: self.text_index.is_some(),
                        text_paths: self
                            .text_paths
                            .iter()
                            .map(|path| serde_json::to_string(path).expect("path serializes"))
                            .collect(),
                        global_clean_present,
                        field_presence,
                        global_clean_observed: self
                            .text_index
                            .as_ref()
                            .map_or(&[], |text| text.global_clean_observed.as_slice()),
                        field_normal_observed: self.text_index.as_ref().map_or(
                            Vec::new(),
                            |text| {
                                text.fields
                                    .iter()
                                    .map(|field| field.normal_observed.as_slice())
                                    .collect()
                            },
                        ),
                        field_clean_observed: self.text_index.as_ref().map_or(Vec::new(), |text| {
                            text.fields
                                .iter()
                                .map(|field| field.clean_observed.as_slice())
                                .collect()
                        }),
                    },
                )
                .map_err(|error| Error::Cache(format!("cannot encode secondary index: {error}")))?;
            if let Some(text) = &self.text_index {
                let (global_clean, _, fields, indexed_len) = text.filters();
                for filter in std::iter::once(global_clean).chain(
                    fields
                        .iter()
                        .flat_map(|field| [&field.normal, &field.clean]),
                ) {
                    let length = filter.encoded_len()?;
                    buffered
                        .write_all(&length.to_le_bytes())
                        .map_err(|error| io(&sidecar, error))?;
                    filter.write_to(&mut buffered)?;
                }
                buffered
                    .write_all(&(indexed_len as u64).to_le_bytes())
                    .map_err(|error| io(&sidecar, error))?;
            }
            buffered.flush().map_err(|error| io(&sidecar, error))?;
        }
        if payload_bytes > MAX_BYTES - HEADER as u64 {
            return Err(Error::Cache("secondary index exceeds size limit".into()));
        }
        temporary
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|error| io(&sidecar, error))?;
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
