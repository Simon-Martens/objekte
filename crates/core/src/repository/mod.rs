//! Resident repository ownership and the independent scan execution path.
//!
//! The [public caller test](https://github.com/Simon-Martens/objekte/blob/3bf4c1f7c9167bfed012fe7011c553bc72a16dd9/crates/core/tests/inline_sort.rs)
//! exercises the public configuration, construction, query, and
//! result-inspection API end to end.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::index::segment::PublishedSegment;
use crate::index::storage::StorageFactory;
use crate::index::storage::file_catalog::{FileCatalogBase, FileCatalogHandle};
use crate::inventory::{ActiveRecords, FileInventory};
use crate::lifecycle::IndexRevision;
use crate::source::registry::SourceRegistry;

use self::configuration::RepositoryOptions;
use self::state::{CollectionState, GlobalInferenceSummary};
use self::statistics::StartupStats;

static NEXT_REPOSITORY_INSTANCE: AtomicU64 = AtomicU64::new(1);

pub(crate) fn next_repository_instance() -> u64 {
    NEXT_REPOSITORY_INSTANCE.fetch_add(1, Ordering::Relaxed)
}

pub mod configuration;
mod inspection;
pub mod posting_values;
mod records;
pub mod relations;
mod search;
pub mod state;
pub mod statistics;

/// Resident owner of file inventory, collections, workers, and caches.
///
/// Published structural state is only available through shared references.
/// A future whole-file replacement must build new generations and publish them
/// together with its file-to-record mapping; it cannot edit a published view.
///
/// ```compile_fail
/// use objekte_search_core::repository::Repository;
/// fn mutate_published(repo: &mut Repository) {
///     repo.collections.clear();
/// }
/// ```
pub struct Repository {
    pub(crate) storage_factory: Arc<dyn StorageFactory>,
    pub(crate) files: Arc<FileInventory>,
    /// Whole-file SHA-256 captured during the fresh builder's indexed reads.
    /// A missing digest means at least one eligible binding could not read
    /// complete bytes, so verified maintenance must fall back.
    pub(crate) base_fingerprints: Vec<Option<[u8; 32]>>,
    pub(crate) collections: Vec<CollectionState>,
    pub(crate) active: Vec<ActiveRecords>,
    pub(crate) segments: Vec<Arc<Vec<Arc<PublishedSegment>>>>,
    pub(crate) catalog: FileCatalogHandle,
    pub(crate) configuration_identity: [u8; 32],
    pub(crate) programmatic: bool,
    pub(crate) catalog_registered: bool,
    pub(crate) record_registrations: Arc<Vec<crate::lifecycle::builder::RegisteredRecordSource>>,
    pub(crate) revision: IndexRevision,
    pub(crate) sources: SourceRegistry,
    pub(crate) build_parses: u64,
    pub(crate) build_failed_parses: u64,
    pub(crate) instance_id: u64,
    pub(crate) options: RepositoryOptions,
    pub(crate) startup: StartupStats,
    pub(crate) workers: rayon::ThreadPool,
    pub(crate) stream_budget: crate::construction::file_preparation::StreamBudget,
    pub(crate) verified: Mutex<crate::query_results::verified::VerifiedCache>,
    pub(crate) inferred_orders: Mutex<crate::index::sorting::InferredOrderCache>,
}
impl Repository {
    pub(crate) fn file_catalog_base(&self) -> impl FileCatalogBase + use<> {
        self.files.catalog_base(
            self.base_fingerprints.clone(),
            self.configuration_identity,
            self.collections.len(),
        )
    }

    pub(crate) fn create_workers(count: usize) -> Result<rayon::ThreadPool, String> {
        if count == 0 || count > 256 {
            return Err("query_workers must be between 1 and 256".into());
        }
        rayon::ThreadPoolBuilder::new()
            .num_threads(count)
            .thread_name(|index| format!("objekte-query-{index}"))
            .build()
            .map_err(|error| error.to_string())
    }

    pub fn configure_verified_cache(&self, bytes: usize) -> Result<(), String> {
        let displaced = self
            .verified
            .lock()
            .map_err(|e| e.to_string())?
            .set_budget(bytes);
        drop(displaced);
        Ok(())
    }

    /// Construct zero-record indexes with the same configuration, without
    /// enumerating or opening any source file. Maintenance can then publish
    /// active-only compacted contributions as new segments.
    pub(crate) fn empty_like(&self) -> Result<Self, String> {
        let mut empty = if !self.programmatic {
            crate::construction::repository_state::initialize_empty(
                self.files.required_root().to_path_buf(),
                self.collections
                    .iter()
                    .map(|state| state.required_file_definition().clone())
                    .collect(),
                self.options,
                &self.sources,
                None,
                Arc::clone(&self.storage_factory),
            )?
        } else {
            // The same frozen registration identity is carried across maintenance.
            let mut empty = crate::construction::repository_state::initialize_empty_programmatic(
                self.collections
                    .iter()
                    .map(|state| state.definition.clone())
                    .collect(),
                self.options,
                &[],
                self.files.root().map(|root| root.to_path_buf()),
                self.collections
                    .iter()
                    .map(|state| state.file_source.clone())
                    .collect(),
                &self.sources,
                Arc::clone(&self.storage_factory),
            )?;
            empty.configuration_identity = self.configuration_identity;
            empty.catalog = empty
                .storage_factory
                .file_catalog(&empty.file_catalog_base())?;
            empty
        };
        for (source, target) in self.collections.iter().zip(&mut empty.collections) {
            target.global_inference = source.global_inference.clone().or_else(|| {
                source
                    .inference
                    .as_ref()
                    .map(|state| GlobalInferenceSummary {
                        report: state.report.clone(),
                        diagnostics: state.path_diagnostics(),
                    })
            });
            if let Some(summary) = target.global_inference.as_mut() {
                summary.report.complete = false;
            }
        }
        empty.build_parses = self.build_parses;
        empty.record_registrations = Arc::clone(&self.record_registrations);
        empty.build_failed_parses = self.build_failed_parses;
        empty.startup = self.startup.clone();
        Ok(empty)
    }
}
#[cfg(test)]
#[path = "tests.rs"]
mod tests;
