//! Storage status exposed by the core index.

use std::path::PathBuf;

use uuid::Uuid;


/// How the current live index was obtained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StorageOrigin {
    Built,
    Loaded,
}


/// Explicit source-freshness state. Loading never checks source files.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FreshnessState {
    NotApplicable,
    Unchecked,
    Checking,
    Checked,
}


/// One live storage status snapshot.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StorageStatus {
    pub origin: StorageOrigin,
    pub directory: Option<PathBuf>,
    pub resident_packed_bytes: u64,
    pub mapped_virtual_bytes: u64,
    pub searchable_run_count: u64,
    pub publication_id: Uuid,
    pub saved_publication_id: Option<Uuid>,
    pub saved_directory: Option<PathBuf>,
    pub unsaved_changes: bool,
    pub freshness: FreshnessState,
    pub active_save_task: bool,
}


impl StorageStatus {
    pub fn built(publication_id: Uuid) -> Self {
        Self {
            origin: StorageOrigin::Built,
            directory: None,
            resident_packed_bytes: 0,
            mapped_virtual_bytes: 0,
            searchable_run_count: 0,
            publication_id,
            saved_publication_id: None,
            saved_directory: None,
            unsaved_changes: true,
            freshness: FreshnessState::NotApplicable,
            active_save_task: false,
        }
    }

    pub fn loaded(
        directory: PathBuf,
        mapped_virtual_bytes: u64,
        searchable_run_count: u64,
        publication_id: Uuid,
    ) -> Self {
        Self {
            origin: StorageOrigin::Loaded,
            directory: Some(directory.clone()),
            resident_packed_bytes: 0,
            mapped_virtual_bytes,
            searchable_run_count,
            publication_id,
            saved_publication_id: Some(publication_id),
            saved_directory: Some(directory),
            unsaved_changes: false,
            freshness: FreshnessState::Unchecked,
            active_save_task: false,
        }
    }

    pub fn note_publication(&mut self, publication_id: Uuid) {
        self.publication_id = publication_id;
        self.unsaved_changes = self.saved_publication_id != Some(publication_id);
    }

    pub fn note_saved(&mut self, publication_id: Uuid, directory: PathBuf) {
        self.saved_publication_id = Some(publication_id);
        self.saved_directory = Some(directory);
        self.unsaved_changes = self.publication_id != publication_id;
    }

    pub fn note_freshness(&mut self, freshness: FreshnessState) {
        self.freshness = freshness;
    }
}
