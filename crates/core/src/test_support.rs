//! Managed generation round trips shared by lifecycle tests.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use crate::index_storage::{IndexResidency, IndexStorageLocation, IndexStoragePolicy};
use crate::lifecycle::Index;
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::SourceRegistry;

static NEXT_STORE: AtomicU64 = AtomicU64::new(0);

pub(crate) struct ManagedRestored {
    index: Index,
    base: PathBuf,
}

impl Deref for ManagedRestored {
    type Target = Index;

    fn deref(&self) -> &Self::Target {
        &self.index
    }
}

impl Drop for ManagedRestored {
    fn drop(&mut self) {
        self.index.shutdown().expect("managed test index shutdown");
        std::fs::remove_dir_all(&self.base).expect("remove managed test store");
    }
}

pub(crate) fn restore_through_managed_store(
    index: &Index,
    root: &Path,
    definitions: Vec<CollectionDefinition>,
    options: RepositoryOptions,
) -> ManagedRestored {
    let base = std::env::temp_dir().join(format!(
        "objekte-managed-roundtrip-{}-{}",
        std::process::id(),
        NEXT_STORE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&base).expect("create managed test store");
    let policy = IndexStoragePolicy {
        location: IndexStorageLocation::CustomDirectory(base.clone()),
        residency: IndexResidency::OnDemand,
    };
    let sources = SourceRegistry::builtin();
    let prepared = policy
        .prepare(root, &definitions, options, &sources)
        .expect("prepare managed test store");
    index
        .attach_storage(prepared.resolved)
        .expect("attach managed test store");
    index.flush_storage().expect("flush managed test store");
    let reopened = Index::open_with_storage(root, definitions, options, policy, sources)
        .expect("reopen managed test store");
    assert!(reopened.restored_from_storage());
    ManagedRestored {
        index: reopened,
        base,
    }
}
