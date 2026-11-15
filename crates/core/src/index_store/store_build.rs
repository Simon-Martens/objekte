//! Metadata-first source discovery into the authoritative redb catalog.
//!
//! This is the first Phase-6 construction stage. It enumerates selected source
//! files, assigns global file IDs, writes file/path/build-state/pending rows
//! and marks the catalog complete. No content is parsed and no native
//! `FileInventory` is retained.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::discovery::discover;
use crate::inventory::{FileEntry, FileStatus, SharedPath, SourceRevision};
use crate::model::{CollectionId, FileId};
use crate::repository::CollectionDefinition;
use crate::source::SourceRegistry;

use super::value_codec::{
    BuildFileState, CollectionMetadata, ConstructionState, FileBuildState,
};
use super::{IndexStore, read, write};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveryReport {
    pub collections: usize,
    pub files: u64,
    pub memberships: u64,
    pub catalog_complete: bool,
}

/// Enumerate every selected source file and write the catalog to the store.
///
/// The store must be empty (or freshly reset). All collection membership and
/// pending-file rows commit before `catalog_complete` is set.
pub fn discover_into_store(
    store: &IndexStore,
    root: &Path,
    definitions: &[CollectionDefinition],
    sources: &SourceRegistry,
) -> Result<DiscoveryReport, String> {
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let mut report = DiscoveryReport {
        collections: definitions.len(),
        ..DiscoveryReport::default()
    };
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    let mut store_metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
    if store_metadata.construction != ConstructionState::Empty {
        return Err("discovery requires an empty store".into());
    }
    store_metadata.construction = ConstructionState::Discovering;
    store_metadata.catalog_complete = false;
    store_metadata.revision = store_metadata.revision.saturating_add(1);
    write::put_metadata(&tx, &store_metadata).map_err(|error| error.to_string())?;
    let mut collections = Vec::with_capacity(definitions.len());
    for definition in definitions {
        if collections
            .iter()
            .any(|collection: &CollectionMetadata| collection.name == definition.name)
        {
            return Err(format!("duplicate collection: {}", definition.name));
        }
        let id = CollectionId(
            u32::try_from(collections.len()).map_err(|_| "too many collections")?,
        );
        collections.push(CollectionMetadata {
            id,
            name: definition.name.clone(),
            generation: 1,
            source_fingerprint: [0; 32],
            slot_high_water: 0,
            live_records: 0,
            discovered_files: 0,
            pending_files: 0,
            indexed_files: 0,
            unavailable_files: 0,
            processed_files: 0,
            next_binding_id: 0,
        });
        write::put_collection(
            &tx,
            collections.last().expect("collection was just pushed"),
        )
        .map_err(|error| error.to_string())?;
    }
    tx.commit().map_err(|error| error.to_string())?;

    let mut next_file_id = store_metadata.next_file_id;
    let mut physical = HashMap::<PathBuf, FileIdCollections>::new();
    for (collection_index, definition) in definitions.iter().enumerate() {
        let collection = CollectionId(collection_index as u32);
        let source = sources.resolve(&definition.format).map_err(|error| error.to_string())?;
        let paths = discover(&root, &definition.source_path, &definition.discovery, &source)
            .map_err(|error| error.to_string())?;
        let tx = store.begin_write().map_err(|error| error.to_string())?;
        let mut collection_metadata =
            read::collection_from_write(&tx, collection).map_err(|error| error.to_string())?;
        for path in paths {
            let relative = path
                .strip_prefix(&root)
                .map_err(|error| error.to_string())?
                .to_path_buf();
            let existing = physical.get(&relative).cloned();
            let file_id = if let Some(existing) = existing {
                existing.id
            } else {
                let file_metadata =
                    std::fs::metadata(&path).map_err(|error| error.to_string())?;
                let revision = SourceRevision::from_metadata(&file_metadata)?;
                let created_ns = file_metadata
                    .created()
                    .ok()
                    .and_then(|time| {
                        time.duration_since(std::time::SystemTime::UNIX_EPOCH)
                            .ok()
                    })
                    .map(|duration| duration.as_nanos() as i128);
                let id = crate::model::FileId(next_file_id);
                next_file_id = next_file_id
                    .checked_add(1)
                    .ok_or_else(|| "file ID overflow".to_string())?;
                let entry = FileEntry {
                    id,
                    relative_path: SharedPath::from(relative.clone()),
                    revision,
                    created_ns,
                    collections: Vec::new(),
                    status: FileStatus::Valid,
                    diagnostics: Vec::new(),
                    omitted_diagnostics: 0,
                };
                write::put_file(&tx, &entry, 1, None).map_err(|error| error.to_string())?;
                write::put_build_state(
                    &tx,
                    id,
                    &FileBuildState {
                        state: BuildFileState::Pending,
                        expected_file_version: 1,
                        build_epoch: store_metadata.build_epoch,
                        last_admission_revision: store_metadata.revision,
                        error: None,
                    },
                )
                .map_err(|error| error.to_string())?;
                physical.insert(
                    relative.clone(),
                    FileIdCollections {
                        id,
                        collections: Vec::new(),
                    },
                );
                report.files += 1;
                id
            };
            let record = physical.get_mut(&relative).expect("physical file exists");
            if !record.collections.contains(&collection) {
                record.collections.push(collection);
                write::add_pending_file(&tx, collection, file_id)
                    .map_err(|error| error.to_string())?;
                collection_metadata.discovered_files += 1;
                collection_metadata.pending_files += 1;
                report.memberships += 1;
            }
        }
        write::put_collection(&tx, &collection_metadata).map_err(|error| error.to_string())?;
        tx.commit().map_err(|error| error.to_string())?;
    }

    // Rewrite shared file ownership after all collection passes and publish
    // catalog readiness in one final transaction.
    let tx = store.begin_write().map_err(|error| error.to_string())?;
    let mut metadata = read::metadata_from_write(&tx).map_err(|error| error.to_string())?;
    for record in physical.values() {
        let Some(mut file) = write::get_file(&tx, record.id).map_err(|error| error.to_string())?
        else {
            return Err("discovered file disappeared".into());
        };
        file.entry.collections = record.collections.clone();
        write::put_file(&tx, &file.entry, file.file_version, file.fingerprint)
            .map_err(|error| error.to_string())?;
    }
    metadata.next_file_id = next_file_id;
    metadata.construction = ConstructionState::Building;
    metadata.catalog_complete = true;
    metadata.revision = metadata.revision.saturating_add(1);
    write::put_metadata(&tx, &metadata).map_err(|error| error.to_string())?;
    tx.commit().map_err(|error| error.to_string())?;
    report.catalog_complete = true;
    Ok(report)
}

#[derive(Clone)]
struct FileIdCollections {
    id: FileId,
    collections: Vec<CollectionId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::SourceFormat;

    #[test]
    fn discovery_writes_catalog_and_pending_files() {
        let directory = std::env::temp_dir().join(format!(
            "objekte-store-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(directory.join("sub")).expect("directory");
        std::fs::write(directory.join("a.json"), b"{\"x\":1}").expect("a");
        std::fs::write(directory.join("sub/b.json"), b"{\"x\":2}").expect("b");
        let mut definition = CollectionDefinition::folder("test", "", SourceFormat::Json);
        definition.discovery.recursive = true;
        let store = std::sync::Arc::new(
            IndexStore::new(super::super::StoreConfig::default()).expect("store"),
        );
        let registry = SourceRegistry::builtin();
        let report = discover_into_store(
            &store,
            &directory,
            std::slice::from_ref(&definition),
            &registry,
        )
        .expect("discovery");
        assert_eq!(report.files, 2);
        assert_eq!(report.memberships, 2);
        assert!(report.catalog_complete);
        let view = store.view().expect("view");
        assert!(view.metadata().catalog_complete);
        assert_eq!(
            read::pending_files(&view, CollectionId(0)).expect("pending").len(),
            2
        );
        assert_eq!(
            read::build_state_kind(&view, FileId(0)).expect("state"),
            Some(BuildFileState::Pending)
        );
        std::fs::remove_dir_all(directory).expect("cleanup");
    }
}
