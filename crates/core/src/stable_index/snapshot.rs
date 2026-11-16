//! Self-contained component files, generation manifests and publication.

use std::collections::BTreeSet;
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::codec::{ComponentDescriptor, ComponentId, ComponentKind, FORMAT_MAGIC, hex_digest};

pub const MANIFEST_NAME: &str = "manifest.json";
pub const MANIFEST_FORMAT_VERSION: u32 = 2;
pub const MAX_MANIFEST_BYTES: u64 = 64 * 1024 * 1024;

#[cfg(test)]
static ARMED_FAULT: std::sync::Mutex<Option<(&'static str, String)>> = std::sync::Mutex::new(None);

#[cfg(test)]
static FAULT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
pub(crate) fn fault_test_lock() -> std::sync::MutexGuard<'static, ()> {
    FAULT_TEST_LOCK
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
}

#[cfg(test)]
pub(crate) fn arm_fault(point: &'static str, marker: impl Into<String>) {
    *ARMED_FAULT.lock().expect("fault lock") = Some((point, marker.into()));
}

#[cfg(test)]
pub(crate) fn clear_fault() {
    *ARMED_FAULT.lock().expect("fault lock") = None;
}

#[allow(unused_variables)]
pub(crate) fn inject_fault(point: &'static str, path: Option<&Path>) -> Result<(), String> {
    #[cfg(test)]
    {
        let mut fault = ARMED_FAULT.lock().expect("fault lock");
        let matches_path = match (fault.as_ref(), path) {
            (Some((_, marker)), Some(path)) => path.to_string_lossy().contains(marker),
            (Some(_), None) => false,
            (None, _) => false,
        };
        if matches_path && fault.as_ref().is_some_and(|(armed, _)| *armed == point) {
            *fault = None;
            return Err(format!("injected fault at {point}"));
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManifestCollection {
    pub id: u32,
    pub name: String,
    pub generation: u64,
    pub slot_high_water: u32,
    pub live_records: u64,
    #[serde(default = "default_identity_policy")]
    pub identity_policy: crate::stable_index::PersistedIdentityPolicy,
}

fn default_identity_policy() -> crate::stable_index::PersistedIdentityPolicy {
    crate::stable_index::PersistedIdentityPolicy::Generated
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ManifestComponent {
    pub id: ComponentId,
    pub path: String,
    pub kind: ComponentKind,
    pub descriptor: ComponentDescriptor,
    pub descriptor_sha256: String,
    pub payload_len: u64,
    pub entry_count: u64,
    pub payload_sha256: String,
}

impl ManifestComponent {
    pub fn validate_descriptor(&self) -> Result<(), String> {
        let bytes = super::codec::encode_descriptor(&self.descriptor)?;
        if hex_digest(&super::codec::digest(&bytes)) != self.descriptor_sha256 {
            return Err(format!(
                "manifest descriptor hash for component {} does not match its descriptor",
                self.id.0
            ));
        }
        if self.descriptor.id != self.id || self.descriptor.kind != self.kind {
            return Err(format!(
                "manifest component {} identity does not match its descriptor",
                self.id.0
            ));
        }
        if self.payload_len % super::memory::MEMORY_PAGE_SIZE as u64 != 0 {
            return Err(format!(
                "manifest payload for component {} is not page aligned",
                self.id.0
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotManifest {
    pub magic: String,
    pub format_version: u32,
    pub repository_identity: String,
    pub configuration_identity: String,
    pub schema_fingerprint: Option<String>,
    pub slot_namespace: String,
    pub collections: Vec<ManifestCollection>,
    pub components: Vec<ManifestComponent>,
}

impl SnapshotManifest {
    pub fn new(
        repository_identity: [u8; 32],
        configuration_identity: [u8; 32],
        schema_fingerprint: Option<[u8; 32]>,
        slot_namespace: String,
        collections: Vec<ManifestCollection>,
        components: Vec<ManifestComponent>,
    ) -> Self {
        Self {
            magic: String::from_utf8_lossy(FORMAT_MAGIC).into_owned(),
            format_version: MANIFEST_FORMAT_VERSION,
            repository_identity: hex_digest(&repository_identity),
            configuration_identity: hex_digest(&configuration_identity),
            schema_fingerprint: schema_fingerprint.map(|value| hex_digest(&value)),
            slot_namespace,
            collections,
            components,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.magic != String::from_utf8_lossy(FORMAT_MAGIC)
            || self.format_version != MANIFEST_FORMAT_VERSION
        {
            return Err("unsupported snapshot manifest format".into());
        }
        if self.slot_namespace.is_empty() {
            return Err("snapshot manifest has no slot namespace".into());
        }
        let mut component_ids = BTreeSet::new();
        let mut paths = BTreeSet::new();
        let mut identities = BTreeSet::new();
        for component in &self.components {
            component.validate_descriptor()?;
            if !component_ids.insert(component.id.0) {
                return Err(format!("duplicate component ID {}", component.id.0));
            }
            if !identities.insert(component.descriptor.identity_key()) {
                return Err(format!(
                    "duplicate logical component identity {}",
                    component.descriptor.identity_key()
                ));
            }
            if !paths.insert(component.path.clone()) {
                return Err(format!("duplicate component path {}", component.path));
            }
            validate_relative_component_path(&component.path)?;
            if component.descriptor.slot_namespace != self.slot_namespace {
                return Err(format!(
                    "component {} uses another slot namespace",
                    component.id.0
                ));
            }
            if component.descriptor.component_revision == 0 {
                return Err(format!("component {} has a zero revision", component.id.0));
            }
        }
        for component in &self.components {
            for dependency in &component.descriptor.dependency_ids {
                if dependency == &component.id {
                    return Err(format!("component {} depends on itself", component.id.0));
                }
                let target = self.component(*dependency).ok_or_else(|| {
                    format!(
                        "component {} depends on missing component {}",
                        component.id.0, dependency.0
                    )
                })?;
                if target.descriptor.slot_namespace != component.descriptor.slot_namespace {
                    return Err(format!(
                        "component {} dependency {} has another slot namespace",
                        component.id.0, dependency.0
                    ));
                }
                if component
                    .descriptor
                    .collection
                    .as_ref()
                    .map(|value| value.id)
                    != target.descriptor.collection.as_ref().map(|value| value.id)
                {
                    return Err(format!(
                        "component {} dependency {} targets another collection",
                        component.id.0, dependency.0
                    ));
                }
            }
        }
        let mut collection_ids = BTreeSet::new();
        for collection in &self.collections {
            if !collection_ids.insert(collection.id) {
                return Err(format!("duplicate collection ID {}", collection.id));
            }
            if collection.live_records > u64::from(collection.slot_high_water) {
                return Err(format!(
                    "collection {} live count exceeds slot high water",
                    collection.id
                ));
            }
        }
        Ok(())
    }

    pub fn component(&self, id: ComponentId) -> Option<&ManifestComponent> {
        self.components.iter().find(|entry| entry.id == id)
    }
}

pub fn validate_relative_component_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.contains('\0') {
        return Err("empty or invalid component path".into());
    }
    let native = Path::new(path);
    if native.is_absolute() {
        return Err(format!("absolute component path {path}"));
    }
    for component in native.components() {
        match component {
            Component::Normal(_) => {}
            Component::CurDir
            | Component::ParentDir
            | Component::RootDir
            | Component::Prefix(_) => {
                return Err(format!("unsafe component path {path}"));
            }
        }
    }
    Ok(())
}

pub fn component_relative_path(descriptor: &ComponentDescriptor) -> String {
    match descriptor.kind {
        ComponentKind::Files => "files.icss".into(),
        ComponentKind::Paths => "paths.icss".into(),
        _ => {
            let collection = descriptor.collection.as_ref().map_or(0, |value| value.id.0);
            let directory = format!("c{collection:06}");
            match descriptor.kind {
                ComponentKind::Records => format!("{directory}/records.icss"),
                ComponentKind::Identities => format!("{directory}/identities.icss"),
                ComponentKind::Domains => format!("{directory}/domains.icss"),
                kind => format!(
                    "{directory}/i{:06}-{}.icss",
                    descriptor.id.0,
                    kind.file_suffix()
                ),
            }
        }
    }
}

pub fn write_manifest(directory: &Path, manifest: &SnapshotManifest) -> Result<PathBuf, String> {
    inject_fault("snapshot-before-manifest", Some(directory))?;
    manifest.validate()?;
    let path = directory.join(MANIFEST_NAME);
    let bytes =
        serde_json::to_vec_pretty(manifest).map_err(|error| format!("manifest encode: {error}"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("snapshot manifest exceeds the configured size limit".into());
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| format!("create {}: {error}", path.display()))?;
    file.write_all(&bytes)
        .map_err(|error| format!("write manifest: {error}"))?;
    file.flush()
        .map_err(|error| format!("flush manifest: {error}"))?;
    file.sync_all()
        .map_err(|error| format!("sync manifest: {error}"))?;
    Ok(path)
}

pub fn read_manifest(directory: &Path) -> Result<SnapshotManifest, String> {
    let path = directory.join(MANIFEST_NAME);
    let file = File::open(&path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let len = file
        .metadata()
        .map_err(|error| format!("stat {}: {error}", path.display()))?
        .len();
    if len > MAX_MANIFEST_BYTES {
        return Err("snapshot manifest exceeds the configured size limit".into());
    }
    let mut bytes = Vec::with_capacity(len as usize);
    file.take(MAX_MANIFEST_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("read manifest: {error}"))?;
    if bytes.len() as u64 > MAX_MANIFEST_BYTES {
        return Err("snapshot manifest grew while it was read".into());
    }
    let manifest: SnapshotManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("manifest decode {}: {error}", path.display()))?;
    manifest.validate()?;
    Ok(manifest)
}

pub fn sync_directory(directory: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        let file = File::open(directory)
            .map_err(|error| format!("open directory {}: {error}", directory.display()))?;
        file.sync_all()
            .map_err(|error| format!("sync directory {}: {error}", directory.display()))?;
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        // Windows does not provide a portable directory fsync equivalent
        // through std::fs.  The files themselves are still synced.
    }
    Ok(())
}

static STAGING_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

pub fn next_staging_ordinal() -> u64 {
    STAGING_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub fn staging_directory(destination: &Path, ordinal: u64) -> Result<PathBuf, String> {
    let parent = destination
        .parent()
        .ok_or_else(|| format!("{} has no parent directory", destination.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| format!("{} has no file name", destination.display()))?;
    let staging = parent.join(format!(
        ".objekte-icss-staging-{}-{ordinal}-{}",
        std::process::id(),
        name.to_string_lossy()
    ));
    if staging.exists() {
        return Err(format!(
            "staging path {} already exists; refusing to reuse it",
            staging.display()
        ));
    }
    Ok(staging)
}

pub fn safe_join(root: &Path, relative: &str) -> Result<PathBuf, String> {
    validate_relative_component_path(relative)?;
    let path = root.join(relative);
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn component_paths_reject_traversal() {
        for path in ["../x", "/tmp/x", "a/../../x", "", "a\\..\\x"] {
            let result = validate_relative_component_path(path);
            if cfg!(windows) || !path.contains('\\') {
                assert!(result.is_err(), "{path:?} should be rejected");
            }
        }
        assert!(validate_relative_component_path("c000001/records.icss").is_ok());
    }
}
