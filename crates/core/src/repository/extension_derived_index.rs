//! Optional derived-value postings over selected provider records. Storage is
//! the same typed B-tree used by the JSON picker, with a separate source table.

use super::*;
use crate::{
    Direction, JsonPath,
    extensions::{Extraction, TypedValue},
    file_picker::{TypedIndex, TypedKey},
    json_filter::decimal_cmp,
};
use bincode::Options as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{
    cmp::Ordering,
    fs,
    io::{Seek as _, SeekFrom, Write as _},
    path::Path,
    sync::Arc,
};

const SNAPSHOT_MAGIC: &[u8; 8] = b"OBJDRV01";
const SNAPSHOT_HEADER: usize = 8 + 8 + 32;
const MAX_SNAPSHOT_BYTES: u64 = 8 * 1024 * 1024 * 1024;


#[derive(Serialize)]
struct SavedSelectedDerived<'a> {
    collection: String,
    path: String,
    selection: String,
    semantics: String,
    slots: usize,
    scalar_safe: bool,
    index: &'a TypedIndex,
}


#[derive(Deserialize)]
struct RestoredSelectedDerived {
    collection: String,
    path: String,
    selection: String,
    semantics: String,
    slots: usize,
    scalar_safe: bool,
    index: TypedIndex,
}


fn snapshot_digest(mut reader: impl Read) -> std::io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(digest.finalize().into())
}


#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct SelectedDerivedKey {
    pub collection: String,
    pub path: JsonPath,
    pub selection: String,
    pub semantics: String,
}


impl SelectedDerivedKey {
    pub(super) fn new(
        collection: &Collection,
        path: &JsonPath,
        selection: &str,
        extensions: &crate::extensions::Extensions,
    ) -> Self {
        Self {
            collection: collection.name.clone(),
            path: path.clone(),
            selection: selection.into(),
            semantics: crate::config::fingerprint(
                "selected-derived-posting/v1",
                &(
                    collection.mapping_hash.as_str(),
                    collection.schema_hash.as_deref(),
                    extensions.fingerprint(),
                ),
            ),
        }
    }
}


pub(super) struct SelectedDerivedPosting {
    pub index: TypedIndex,
    slots: usize,
    scalar_safe: bool,
    order_asc: Option<Arc<Vec<u32>>>,
    order_desc: Option<Arc<Vec<u32>>>,
}


fn sort_rank(key: Option<&TypedKey>) -> u8 {
    match key {
        Some(TypedKey::Bool(_)) => 0,
        Some(TypedKey::Number(_)) => 1,
        Some(TypedKey::String(_)) => 2,
        Some(TypedKey::Null) => 3,
        Some(TypedKey::Missing) | None => 4,
    }
}

fn compare_sort_keys(
    left: Option<&TypedKey>,
    right: Option<&TypedKey>,
    direction: Direction,
) -> Ordering {
    let left_rank = sort_rank(left);
    let right_rank = sort_rank(right);
    if left_rank != right_rank {
        return left_rank.cmp(&right_rank);
    }
    let order = match (left, right) {
        (Some(TypedKey::Bool(left)), Some(TypedKey::Bool(right))) => left.cmp(right),
        (Some(TypedKey::Number(left)), Some(TypedKey::Number(right))) => decimal_cmp(left, right),
        (Some(TypedKey::String(left)), Some(TypedKey::String(right))) => left.cmp(right),
        _ => Ordering::Equal,
    };
    if direction == Direction::Desc && left_rank < 3 {
        order.reverse()
    } else {
        order
    }
}


impl SelectedDerivedPosting {
    pub(super) fn from_rows(
        path: JsonPath,
        rows: Vec<(TypedKey, usize)>,
        present: Vec<u64>,
        scalar_safe: bool,
        slots: usize,
    ) -> Self {
        let mut index = TypedIndex::new(Vec::new());
        index.replace_path(path, rows, present, scalar_safe, true, true);
        Self {
            index,
            slots,
            scalar_safe,
            order_asc: None,
            order_desc: None,
        }
    }

    pub(super) fn order(&mut self, path: &JsonPath, direction: Direction) -> Option<Arc<Vec<u32>>> {
        if !self.scalar_safe {
            return None;
        }
        let missing = match direction {
            Direction::Asc => self.order_asc.is_none(),
            Direction::Desc => self.order_desc.is_none(),
        };
        if missing {
            let order = {
                let mut slot_values = vec![None; self.slots];
                self.index.visit_derived(path, |slot, value| {
                    slot_values[slot] = Some(value);
                })?;
                let mut order = (0..self.slots as u32).collect::<Vec<_>>();
                order.sort_by(|&left, &right| {
                    compare_sort_keys(
                        slot_values[left as usize],
                        slot_values[right as usize],
                        direction,
                    )
                    .then_with(|| left.cmp(&right))
                });
                Arc::new(order)
            };
            match direction {
                Direction::Asc => self.order_asc = Some(order),
                Direction::Desc => self.order_desc = Some(order),
            }
        }
        match direction {
            Direction::Asc => self.order_asc.as_ref().map(Arc::clone),
            Direction::Desc => self.order_desc.as_ref().map(Arc::clone),
        }
    }
}


impl Repository {
    fn selected_derived_snapshot_key(
        &self,
        collection: &str,
        extractor: &str,
    ) -> Result<(SelectedDerivedKey, usize)> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return Err(Error::Request(
                "selected derived snapshots require memory cache and non-hash freshness".into(),
            ));
        }
        let selected_collection = self.model.collection(collection)?;
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no value extractors are registered".into()))?;
        let binding = extensions.bind_value(
            extractor,
            collection,
            extensions.schema(collection).map(Arc::as_ref),
        )?;
        if !binding.extractor.descriptor().cacheable() {
            return Err(Error::Request(format!(
                "volatile extractor {extractor} cannot be snapshotted"
            )));
        }
        for provider in extensions.source_providers() {
            if provider.may_select(collection) && !provider.descriptor().cacheable() {
                return Err(Error::Request(format!(
                    "volatile source provider {} cannot be snapshotted",
                    provider.descriptor().identity
                )));
            }
        }
        let selected = self.source_descriptors(collection)?;
        for source in &selected {
            if !extensions.decoder(source)?.descriptor().cacheable() {
                return Err(Error::Request(format!(
                    "volatile decoder {} cannot be snapshotted",
                    source.format
                )));
            }
        }
        let selection = crate::config::fingerprint("selected-sources/v1", &selected);
        Ok((
            SelectedDerivedKey::new(selected_collection, &binding.path, &selection, extensions),
            selected.len(),
        ))
    }

    /// Save a selected-provider typed posting with its semantic and source
    /// revision identity. The file is atomically replaced after a full write.
    pub fn save_selected_derived_snapshot(
        &self,
        collection: &str,
        extractor: &str,
        path: &Path,
    ) -> Result<()> {
        let (key, slots) = self.selected_derived_snapshot_key(collection, extractor)?;
        let posting = self.selected_derived_indexes.get(&key).ok_or_else(|| {
            Error::Cache("selected derived posting is unavailable or stale".into())
        })?;
        if posting.slots != slots {
            return Err(Error::Cache("selected derived source count changed".into()));
        }
        let parent = path
            .parent()
            .ok_or_else(|| Error::Request("snapshot path has no parent".into()))?;
        fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| io(parent, error))?;
        temporary
            .write_all(&[0; SNAPSHOT_HEADER])
            .map_err(|error| io(path, error))?;
        let saved = SavedSelectedDerived {
            collection: key.collection,
            path: serde_json::to_string(&key.path).expect("JSON path serializes"),
            selection: key.selection,
            semantics: key.semantics,
            slots,
            scalar_safe: posting.scalar_safe,
            index: &posting.index,
        };
        bincode::DefaultOptions::new()
            .serialize_into(temporary.as_file_mut(), &saved)
            .map_err(|error| Error::Cache(format!("save selected derived snapshot: {error}")))?;
        let size = temporary
            .as_file()
            .metadata()
            .map_err(|error| io(path, error))?
            .len();
        let payload = size - SNAPSHOT_HEADER as u64;
        if payload > MAX_SNAPSHOT_BYTES {
            return Err(Error::Cache(
                "selected derived snapshot exceeds size limit".into(),
            ));
        }
        temporary
            .as_file_mut()
            .seek(SeekFrom::Start(SNAPSHOT_HEADER as u64))
            .map_err(|error| io(path, error))?;
        let digest = snapshot_digest(temporary.as_file_mut().take(payload))
            .map_err(|error| io(path, error))?;
        let mut header = [0_u8; SNAPSHOT_HEADER];
        header[..8].copy_from_slice(SNAPSHOT_MAGIC);
        header[8..16].copy_from_slice(&payload.to_le_bytes());
        header[16..].copy_from_slice(&digest);
        temporary
            .as_file_mut()
            .seek(SeekFrom::Start(0))
            .map_err(|error| io(path, error))?;
        temporary
            .write_all(&header)
            .map_err(|error| io(path, error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| io(path, error))?;
        temporary
            .persist(path)
            .map_err(|error| io(path, error.error))?;
        Ok(())
    }

    /// Restore only if the current registration, schema, selected source
    /// descriptors and revisions exactly match the saved posting.
    pub fn restore_selected_derived_snapshot(
        &mut self,
        collection: &str,
        extractor: &str,
        path: &Path,
    ) -> Result<bool> {
        let (key, slots) = self.selected_derived_snapshot_key(collection, extractor)?;
        let mut file = match fs::File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io(path, error)),
        };
        let size = file.metadata().map_err(|error| io(path, error))?.len();
        if size < SNAPSHOT_HEADER as u64 || size > MAX_SNAPSHOT_BYTES + SNAPSHOT_HEADER as u64 {
            return Ok(false);
        }
        let mut header = [0_u8; SNAPSHOT_HEADER];
        if file.read_exact(&mut header).is_err() || &header[..8] != SNAPSHOT_MAGIC {
            return Ok(false);
        }
        let payload = u64::from_le_bytes(header[8..16].try_into().unwrap());
        if payload != size - SNAPSHOT_HEADER as u64 {
            return Ok(false);
        }
        let Ok(digest) = snapshot_digest((&mut file).take(payload)) else {
            return Ok(false);
        };
        if header[16..] != digest {
            return Ok(false);
        }
        file.seek(SeekFrom::Start(SNAPSHOT_HEADER as u64))
            .map_err(|error| io(path, error))?;
        let restored: RestoredSelectedDerived = match bincode::DefaultOptions::new()
            .with_limit(MAX_SNAPSHOT_BYTES)
            .deserialize_from(&mut file)
        {
            Ok(saved) => saved,
            Err(_) => return Ok(false),
        };
        if restored.collection != key.collection
            || restored.path != serde_json::to_string(&key.path).expect("JSON path serializes")
            || restored.selection != key.selection
            || restored.semantics != key.semantics
            || restored.slots != slots
            || !restored.index.has_derived_posting(&key.path)
        {
            return Ok(false);
        }
        let mut valid_slots = true;
        restored.index.visit_derived(&key.path, |slot, _| {
            valid_slots &= slot < slots;
        });
        if !valid_slots {
            return Ok(false);
        }
        self.selected_derived_indexes.insert(
            key,
            SelectedDerivedPosting {
                index: restored.index,
                slots,
                scalar_safe: restored.scalar_safe,
                order_asc: None,
                order_desc: None,
            },
        );
        Ok(true)
    }

    /// Build a process-local typed posting for one extractor over the selected
    /// source table, including registered providers and format decoders. The
    /// existing B-tree implementation owns keys and slot lists. An error or a
    /// changed request inventory leaves no partial posting visible.
    pub fn build_selected_derived_posting(
        &mut self,
        collection: &str,
        extractor: &str,
    ) -> Result<usize> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return Err(Error::Request(
                "selected derived postings require memory cache and non-hash freshness".into(),
            ));
        }
        let selected_collection = self.model.collection(collection)?.clone();
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no value extractors are registered".into()))?;
        let binding = extensions.bind_value(
            extractor,
            collection,
            extensions.schema(collection).map(Arc::as_ref),
        )?;
        if !binding.cacheable {
            return Err(Error::Request(format!(
                "volatile extractor {extractor} cannot be indexed"
            )));
        }
        let selected = if self.options.freshness == Freshness::MetadataOnOpen {
            if let Some(snapshot) = self.custom_source_snapshot.get(collection) {
                Arc::clone(&snapshot.sources)
            } else {
                self.metrics.enumerations += 1;
                Arc::new(self.source_descriptors(collection)?)
            }
        } else {
            self.metrics.enumerations += 1;
            Arc::new(self.source_descriptors(collection)?)
        };
        if selected.len() > u32::MAX as usize {
            return Err(Error::Request(
                "selected derived posting exceeds 2^32 source slots".into(),
            ));
        }
        let selection = crate::config::fingerprint("selected-sources/v1", selected.as_ref());
        let key =
            SelectedDerivedKey::new(&selected_collection, &binding.path, &selection, extensions);
        if self.selected_derived_indexes.contains_key(&key) {
            return Ok(selected.len());
        }
        let mut rows = Vec::new();
        let mut present = vec![0_u64; selected.len().div_ceil(64)];
        let mut scalar_safe = true;
        let mut bytes = Vec::new();
        let mut output = Vec::<TypedValue>::new();
        for (slot, source) in selected.iter().enumerate() {
            let decoder = extensions.decoder(source)?;
            if !decoder.descriptor().cacheable() {
                return Err(Error::Request(format!(
                    "volatile decoder {} cannot be indexed",
                    decoder.descriptor().identity
                )));
            }
            if let Some(provider) = extensions
                .source_providers()
                .find(|provider| provider.descriptor().identity == source.provider)
                && !provider.descriptor().cacheable()
            {
                return Err(Error::Request(format!(
                    "volatile source provider {} cannot be indexed",
                    provider.descriptor().identity
                )));
            }
            self.read_selected_source_bytes_into(source, &mut bytes)?;
            self.metrics.source_reads += 1;
            self.metrics.source_bytes += bytes.len() as u64;
            let mut records = 0;
            self.metrics.parses += 1;
            decoder.decode(&bytes, &mut |record| {
                records += 1;
                if records > 1 {
                    return Err(Error::Request(
                        "format decoder emitted multiple records".into(),
                    ));
                }
                output.clear();
                let state = binding.extractor.extract(record, &mut output)?;
                if state == Extraction::Missing && !output.is_empty() {
                    return Err(Error::Request(format!(
                        "extractor {} emitted values for a missing field",
                        binding.identity
                    )));
                }
                if state == Extraction::Present {
                    present[slot / 64] |= 1 << (slot % 64);
                } else {
                    rows.push((TypedKey::Missing, slot));
                }
                scalar_safe &= output.len() <= 1;
                for value in output.drain(..) {
                    rows.push((TypedKey::from_extracted(value)?, slot));
                }
                Ok(())
            })?;
            if records == 0 {
                return Err(Error::Request("format decoder emitted no record".into()));
            }
        }
        if self.options.freshness != Freshness::MetadataOnOpen {
            let latest = self.source_descriptors(collection)?;
            self.metrics.enumerations += 1;
            if latest != *selected {
                return Err(Error::Unstable(selected_collection.path));
            }
        }
        let posting = SelectedDerivedPosting::from_rows(
            binding.path,
            rows,
            present,
            scalar_safe,
            selected.len(),
        );
        self.selected_derived_indexes.insert(key, posting);
        Ok(selected.len())
    }

    pub(super) fn selected_derived_candidates(
        &self,
        key: &SelectedDerivedKey,
        predicate: &crate::repository::DerivedPredicate,
        comparison: Option<&TypedKey>,
        source_count: usize,
    ) -> Option<Vec<u64>> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return None;
        }
        let posting = self.selected_derived_indexes.get(key)?;
        let mut bits = vec![0_u64; source_count.div_ceil(64)];
        posting.index.visit_derived(&key.path, |slot, value| {
            if predicate.matches_one(value, comparison) {
                bits[slot / 64] |= 1 << (slot % 64);
            }
        })?;
        Some(bits)
    }
}
