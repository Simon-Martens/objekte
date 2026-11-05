//! Explicit snapshots for selected-provider representation postings.
//! The portable bigram payload keeps its concrete codec; a companion manifest
//! binds synthetic slot labels to the actual provider selection and semantics.

use super::*;
use crate::{
    bigram::{BigramFile, BigramSnapshot},
    extensions::{BindingContext, Descriptor},
    repository::extension_index::{
        RepresentationIndex, RepresentationIndexKey, base_source_fingerprint, binding_path,
        field_format,
    },
};
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use std::{io::Write as _, sync::Arc};

const MANIFEST_VERSION: u32 = 6;
const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;


#[derive(Serialize, Deserialize)]
struct SelectedFilterManifest {
    version: u32,
    key: RepresentationIndexKey,
    descriptors: Vec<Descriptor>,
    slots: usize,
    filter_sha256: String,
    observed_bigrams: Vec<u64>,
    base_to_current: Option<Vec<Option<u32>>>,
    overlay_slots: Vec<u32>,
    malformed_slots: Vec<u32>,
    base_slots: usize,
    base_source_fingerprints: Vec<[u8; 16]>,
}


fn companion(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(".selected-meta");
    PathBuf::from(name)
}

fn file_digest(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path).map_err(|error| io(path, error))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| io(path, error))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("{:x}", digest.finalize()))
}


impl Repository {
    fn selected_filter_snapshot_key(
        &self,
        collection: &str,
        representation: &str,
        path: &str,
    ) -> Result<(
        RepresentationIndexKey,
        Vec<crate::extensions::SourceDescriptor>,
        Vec<Descriptor>,
    )> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return Err(Error::Request(
                "selected representation snapshots require memory cache and non-hash freshness"
                    .into(),
            ));
        }
        let selected_collection = self.model.collection(collection)?;
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no text representations are registered".into()))?;
        let definition = extensions.representation_definition(representation)?;
        if !definition.descriptor().cacheable() {
            return Err(Error::Request(format!(
                "volatile representation {representation} cannot be snapshotted"
            )));
        }
        let parsed = crate::text::parse_text_path(path)?;
        if !definition.binds_explicit(&BindingContext {
            collection,
            path: &binding_path(&parsed),
            format: field_format(selected_collection, &parsed),
            schema: extensions.schema(collection).map(Arc::as_ref),
        }) {
            return Err(Error::Request(format!(
                "representation {representation} does not bind to {collection}.{path}"
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
        if selected.len() > u32::MAX as usize {
            return Err(Error::Request(
                "selected index source table exceeds 2^32 slots".into(),
            ));
        }
        for source in &selected {
            if !extensions.decoder(source)?.descriptor().cacheable() {
                return Err(Error::Request(format!(
                    "volatile decoder {} cannot be snapshotted",
                    source.format
                )));
            }
        }
        let (selection, _, mut descriptors) =
            extensions.selection_dependencies(selected_collection, &selected)?;
        let encoding = selected_collection
            .bigram_encoding
            .unwrap_or(self.options.bigram_encoding);
        let key = RepresentationIndexKey::new(
            selected_collection,
            path,
            representation,
            &selection,
            extensions,
            encoding,
        );
        descriptors.push(definition.descriptor().clone());
        descriptors.sort_by(|a, b| a.identity.cmp(&b.identity));
        Ok((key, selected, descriptors))
    }

    /// Save one explicitly built selected-source representation posting.
    /// The companion manifest records descriptors and the exact payload hash.
    pub fn save_selected_representation_snapshot(
        &self,
        collection: &str,
        representation: &str,
        field: &str,
        path: &Path,
    ) -> Result<()> {
        let (key, selected, descriptors) =
            self.selected_filter_snapshot_key(collection, representation, field)?;
        let posting = self.extension_text_indexes.get(&key).ok_or_else(|| {
            Error::Cache("selected representation posting is unavailable or stale".into())
        })?;
        let collection = self.model.collection(collection)?;
        let files = (0..posting.filter.file_count())
            .map(|slot| BigramFile {
                id: slot.to_string(),
                // The portable compact codec stores only slot IDs. The
                // companion manifest holds the real descriptor/locator; no
                // source read ever uses this synthetic codec filename.
                filename: format!("{slot}.json"),
                size_bytes: 0,
                modified_unix_nanos: "0".into(),
                identity: String::new(),
            })
            .collect::<Vec<_>>();
        BigramSnapshot::save_selected_filter(
            path,
            &collection.path,
            collection.recursive,
            &files,
            &posting.filter,
        )?;
        let manifest = SelectedFilterManifest {
            version: MANIFEST_VERSION,
            key,
            descriptors,
            slots: selected.len(),
            filter_sha256: file_digest(path)?,
            observed_bigrams: posting.observed_bigrams.clone(),
            base_to_current: posting.base_to_current.clone(),
            overlay_slots: posting.overlay_slots.clone(),
            malformed_slots: posting.malformed_slots.clone(),
            base_slots: posting.filter.file_count(),
            base_source_fingerprints: posting.base_source_fingerprints.clone(),
        };
        let encoded = serde_json::to_vec(&manifest)
            .map_err(|error| Error::Cache(format!("encode selected filter manifest: {error}")))?;
        if encoded.len() as u64 > MAX_MANIFEST_BYTES {
            return Err(Error::Cache(
                "selected filter manifest exceeds size limit".into(),
            ));
        }
        let companion = companion(path);
        let parent = companion
            .parent()
            .ok_or_else(|| Error::Request("snapshot path has no parent".into()))?;
        fs::create_dir_all(parent).map_err(|error| io(parent, error))?;
        let mut temporary =
            tempfile::NamedTempFile::new_in(parent).map_err(|error| io(parent, error))?;
        temporary
            .write_all(&encoded)
            .map_err(|error| io(&companion, error))?;
        temporary
            .as_file()
            .sync_all()
            .map_err(|error| io(&companion, error))?;
        temporary
            .persist(&companion)
            .map_err(|error| io(&companion, error.error))?;
        Ok(())
    }

    /// Restore only when the complete selected-source identity, bound
    /// registration and filter payload match the saved companion manifest.
    pub fn restore_selected_representation_snapshot(
        &mut self,
        collection: &str,
        representation: &str,
        field: &str,
        path: &Path,
    ) -> Result<bool> {
        let (key, selected, descriptors) =
            self.selected_filter_snapshot_key(collection, representation, field)?;
        let companion = companion(path);
        let file = match fs::File::open(&companion) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(io(&companion, error)),
        };
        let mut bytes = Vec::new();
        file.take(MAX_MANIFEST_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| io(&companion, error))?;
        if bytes.len() as u64 > MAX_MANIFEST_BYTES {
            return Ok(false);
        }
        let manifest: SelectedFilterManifest = match serde_json::from_slice(&bytes) {
            Ok(manifest) => manifest,
            Err(_) => return Ok(false),
        };
        let mut covered = vec![false; selected.len()];
        let mapped_sources_match = manifest.base_source_fingerprints.len() == manifest.base_slots
            && (0..manifest.base_slots).all(|base_slot| {
                let current = manifest
                    .base_to_current
                    .as_ref()
                    .map_or(Some(base_slot as u32), |map| {
                        map.get(base_slot).copied().flatten()
                    });
                current.is_none_or(|slot| {
                    selected.get(slot as usize).is_some_and(|source| {
                        manifest
                            .base_source_fingerprints
                            .get(base_slot)
                            .is_some_and(|saved| *saved == base_source_fingerprint(source))
                    })
                })
            });
        if let Some(map) = &manifest.base_to_current {
            for &slot in map.iter().flatten() {
                if let Some(covered) = covered.get_mut(slot as usize) {
                    *covered = true;
                }
            }
        } else if manifest.base_slots == selected.len() {
            covered.fill(true);
        }
        for &slot in &manifest.overlay_slots {
            if let Some(covered) = covered.get_mut(slot as usize) {
                *covered = true;
            }
        }
        if manifest.version != MANIFEST_VERSION
            || manifest.key != key
            || manifest.descriptors != descriptors
            || manifest.slots != selected.len()
            || manifest.observed_bigrams.len() != 1024
            || manifest.base_source_fingerprints.len() != manifest.base_slots
            || !mapped_sources_match
            || manifest
                .base_to_current
                .as_ref()
                .is_some_and(|map| map.len() != manifest.base_slots)
            || manifest.base_to_current.is_none() && manifest.base_slots != selected.len()
            || manifest.base_to_current.as_ref().is_some_and(|map| {
                map.iter()
                    .flatten()
                    .any(|slot| *slot as usize >= selected.len())
            })
            || manifest
                .overlay_slots
                .iter()
                .any(|slot| *slot as usize >= selected.len())
            || manifest
                .malformed_slots
                .iter()
                .any(|slot| *slot as usize >= selected.len())
            || covered.iter().any(|covered| !covered)
        {
            return Ok(false);
        }
        let digest = match file_digest(path) {
            Ok(digest) => digest,
            Err(Error::Io { .. }) if !path.exists() => return Ok(false),
            Err(error) => return Err(error),
        };
        if manifest.filter_sha256 != digest {
            return Ok(false);
        }
        let collection = self.model.collection(collection)?;
        let snapshot =
            match BigramSnapshot::load_scoped(path, &collection.path, collection.recursive) {
                Ok(snapshot) => snapshot,
                Err(_) => return Ok(false),
            };
        if snapshot.filter.encoding()
            != collection
                .bigram_encoding
                .unwrap_or(self.options.bigram_encoding)
            || !snapshot.changed_slots.is_empty()
            || snapshot.files.len() != manifest.base_slots
            || snapshot
                .files
                .iter()
                .enumerate()
                .any(|(slot, file)| file.id != slot.to_string())
        {
            return Ok(false);
        }
        self.extension_text_indexes.insert(
            key,
            RepresentationIndex {
                filter: snapshot.filter,
                observed_bigrams: manifest.observed_bigrams,
                base_to_current: manifest.base_to_current,
                overlay_slots: manifest.overlay_slots,
                malformed_slots: manifest.malformed_slots,
                slots: selected.len(),
                base_source_fingerprints: manifest.base_source_fingerprints,
            },
        );
        Ok(true)
    }
}
