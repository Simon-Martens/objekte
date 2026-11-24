use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, RwLock};

use crate::lifecycle::reconciliation::MaintenanceState;
use crate::lifecycle::{
    Index, IndexError, IndexInner, IndexRevision, IndexTimingsMs, PublishedIndex,
    ReconciliationState,
};
use crate::repository::Repository;
impl Index {
    /// Adopt a completed static repository as the first published view.
    /// This does not trigger discovery, reconciliation or cache persistence.
    pub fn from_repository(repository: Repository) -> Result<Self, IndexError> {
        let build_ns = repository.startup().build_ns;
        let metadata_ns = repository.startup().build_discovery_ns;
        let inference_enabled =
            repository.options().inference.mode != crate::inference::InferenceMode::Disabled;
        let instance_id = crate::repository::next_repository_instance();
        let published =
            PublishedIndex::from_repository_owned(instance_id, IndexRevision::INITIAL, repository)?;
        let index = Self::from_published(published);
        if build_ns > 0 {
            let full_ms = build_ns / 1_000_000;
            index.update_timings_ms(|timings| {
                timings.metadata = Some(metadata_ns / 1_000_000);
                timings.search_available = Some(full_ms);
                timings.full_available = Some(full_ms);
                if inference_enabled {
                    timings.inference_full_available = Some(full_ms);
                }
            })?;
        }
        Ok(index)
    }

    /// Consume a completed blocking build and retain its adopted native
    /// segments as the Repository's primary payload.
    #[cfg(test)]
    pub(crate) fn into_repository(self) -> Result<Repository, IndexError> {
        let inner = Arc::try_unwrap(self.0)
            .map_err(|_| IndexError::InvalidPublication("index has other owners".into()))?;
        let published = inner.published.into_inner().map_err(|_| {
            IndexError::InvalidPublication("index publication lock poisoned".into())
        })?;
        let published = Arc::try_unwrap(published).map_err(|_| {
            IndexError::InvalidPublication("published index has other owners".into())
        })?;
        let mut repository = Arc::try_unwrap(published.repository).map_err(|_| {
            IndexError::InvalidPublication("repository snapshot has other owners".into())
        })?;
        repository.active = published.active;
        repository.segments = published.segments;
        repository.catalog = published.catalog;
        repository.revision = published.revision;
        Ok(repository)
    }

    pub(crate) fn from_published(published: PublishedIndex) -> Self {
        let instance_id = published.instance_id;
        let verified_cache_bytes = published.repository.options().verified_cache_bytes;
        Self(Arc::new(IndexInner {
            instance_id,
            timings: Mutex::new(IndexTimingsMs::default()),
            quiescing: AtomicBool::new(false),
            verified_cache: Mutex::new(crate::query_results::verified::VerifiedCache::new(
                verified_cache_bytes,
            )),
            published: RwLock::new(Arc::new(published)),
            writer: Mutex::new(()),
            ingestion: Mutex::new(std::sync::Weak::new()),
            maintenance: Mutex::new(MaintenanceState {
                reconciliation: ReconciliationState::NotStarted,
                current: None,
            }),
            maintenance_operation: Mutex::new(None),
            periodic_transition: Mutex::new(()),
            periodic: Mutex::new(None),
            #[cfg(test)]
            maintenance_worker_hook: Mutex::new(None),
            #[cfg(test)]
            quiesce_slot_observation: Mutex::new(None),
        }))
    }
}
