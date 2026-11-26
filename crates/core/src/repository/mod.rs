//! Resident repository ownership and the independent scan execution path.
//!
//! The [public caller test](https://github.com/Simon-Martens/objekte/blob/3bf4c1f7c9167bfed012fe7011c553bc72a16dd9/crates/core/tests/inline_sort.rs)
//! exercises the public configuration, construction, query, and
//! result-inspection API end to end.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::index::traits::StorageFactory;
#[cfg(test)]
use crate::index::traits::file_catalog::FileCatalogBase;
use crate::index::traits::file_catalog::{EmptyFileCatalogBase, FileCatalogHandle};
use crate::index::traits::publication::SegmentGroup;
use crate::index::traits::segment_catalog::{SegmentCatalogHandle, pin_segment_group};
use crate::index::traits::transfer::StorageResult;
use crate::inventory::ActiveRecords;
#[cfg(test)]
use crate::inventory::FileInventory;
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
    /// Registered path for source access and locator interpretation.
    pub(crate) source_root: Option<Arc<std::path::Path>>,
    #[cfg(test)]
    pub(crate) files: Arc<FileInventory>,
    /// Whole-file SHA-256 captured during the fresh builder's indexed reads.
    /// A missing digest means at least one eligible binding could not read
    /// complete bytes, so verified maintenance must fall back.
    #[cfg(test)]
    pub(crate) base_fingerprints: Vec<Option<[u8; 32]>>,
    pub(crate) collections: Vec<CollectionState>,
    pub(crate) active: Vec<ActiveRecords>,
    pub(crate) segment_catalogs: Vec<SegmentCatalogHandle>,
    #[cfg(test)]
    pub(crate) segments: Vec<SegmentGroup>,
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
    pub(crate) inferred_orders:
        Mutex<crate::index::traits::order::inferred_cache::InferredOrderCache>,
}
impl Repository {
    pub(crate) fn segment_groups(&self) -> StorageResult<Vec<SegmentGroup>> {
        #[cfg(test)]
        if self.segments.iter().any(|group| !group.is_empty()) {
            return Ok(self.segments.clone());
        }
        self.segment_catalogs
            .iter()
            .map(|directory| pin_segment_group(directory.as_ref()))
            .collect()
    }

    pub(crate) fn query_segment_groups(
        &self,
        query: &crate::query::Query,
        directories: Option<&[SegmentCatalogHandle]>,
    ) -> StorageResult<Vec<SegmentGroup>> {
        self.scoped_segment_groups(
            crate::query::prepared::segment_scope(self, query),
            directories,
        )
    }

    pub(crate) fn collection_segment_groups(
        &self,
        collections: &[crate::model::CollectionId],
        directories: Option<&[SegmentCatalogHandle]>,
    ) -> StorageResult<Vec<SegmentGroup>> {
        let scope = self
            .collections
            .iter()
            .map(|collection| collections.contains(&collection.collection_id()));
        self.scoped_segment_groups(scope, directories)
    }

    pub(crate) fn scoped_segment_groups(
        &self,
        scope: impl IntoIterator<Item = bool>,
        directories: Option<&[SegmentCatalogHandle]>,
    ) -> StorageResult<Vec<SegmentGroup>> {
        let mut empty = None;
        scope
            .into_iter()
            .enumerate()
            .map(|(index, selected)| {
                if !selected {
                    return Ok(Arc::clone(empty.get_or_insert_with(Arc::default)));
                }
                let Some(directories) = directories else {
                    #[cfg(test)]
                    return Ok(Arc::clone(&self.segments[index]));
                    #[cfg(not(test))]
                    unreachable!("production repository has segment directories");
                };
                let directory = directories.get(index).ok_or_else(|| {
                    crate::index::traits::transfer::StorageError::invalid(
                        "open query segment directories",
                        "selected collection has no segment directory",
                    )
                })?;
                pin_segment_group(directory.as_ref())
            })
            .collect()
    }

    pub(crate) fn segment_groups_for_query(
        &self,
        query: &crate::query::Query,
    ) -> StorageResult<Vec<SegmentGroup>> {
        self.query_segment_groups(query, self.lookup_segment_catalogs())
    }

    pub(crate) fn lookup_segment_catalogs(&self) -> Option<&[SegmentCatalogHandle]> {
        #[cfg(test)]
        if self.segments.iter().any(|group| !group.is_empty()) {
            return None;
        }
        Some(&self.segment_catalogs)
    }

    pub(crate) fn empty_file_catalog_base(&self) -> EmptyFileCatalogBase {
        EmptyFileCatalogBase {
            root: self.source_root.clone(),
            configuration: self.configuration_identity,
            collections: self.collections.len(),
        }
    }

    #[cfg(test)]
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

    #[cfg(test)]
    pub(crate) fn has_flat_base(&self) -> bool {
        self.collections
            .iter()
            .any(|collection| collection.base_records.is_some())
    }

    /// Construct zero-record indexes with the same configuration, without
    /// enumerating or opening any source file. Maintenance can then publish
    /// active-only compacted contributions as new segments.
    pub(crate) fn empty_like(&self) -> Result<Self, String> {
        let mut empty = if !self.programmatic {
            crate::construction::repository_state::initialize_empty(
                self.required_file_root().to_path_buf(),
                self.collections
                    .iter()
                    .map(|state| state.required_file_definition().clone())
                    .collect(),
                self.options,
                &self.sources,
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
                self.repository_root().map(|root| root.to_path_buf()),
                self.collections
                    .iter()
                    .map(|state| state.file_source.clone())
                    .collect(),
                &self.sources,
                Arc::clone(&self.storage_factory),
            )?;
            empty.configuration_identity = self.configuration_identity;
            empty.catalog = empty.storage_factory.file_catalog(
                &empty.empty_file_catalog_base(),
                crate::inventory::FileVersion::INITIAL,
            )?;
            empty
        };
        for (source, target) in self.collections.iter().zip(&mut empty.collections) {
            target.global_inference =
                source
                    .inference_report()
                    .map(|report| GlobalInferenceSummary {
                        report: report.clone(),
                        diagnostics: source.inference_diagnostics().unwrap_or_default(),
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
