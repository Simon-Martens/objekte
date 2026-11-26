//! Bounded filesystem discovery metadata.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::index::storage::file_catalog::DiscoveredFile;
use crate::inventory::SourceRevision;

pub(crate) fn prepare_discovered_file(
    root: &Path,
    path: PathBuf,
) -> Result<DiscoveredFile, String> {
    let relative = path
        .strip_prefix(root)
        .map_err(|error| error.to_string())?
        .to_owned();
    let metadata = std::fs::metadata(&path).map_err(|error| error.to_string())?;
    let revision = SourceRevision::from_metadata(&metadata)?;
    let created_ns = metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128);
    Ok(DiscoveredFile {
        relative,
        revision,
        created_ns,
    })
}

/// Indexed Rayon collection keeps every result at its source ordinal. Errors
/// are examined by the caller in that order, independent of completion order.
pub(crate) fn prepare_discovered_window(
    pool: &rayon::ThreadPool,
    root: &Path,
    window: Vec<PathBuf>,
    profile: bool,
    peak_active: &AtomicUsize,
) -> Vec<Result<DiscoveredFile, String>> {
    let active = AtomicUsize::new(0);
    pool.install(|| {
        window
            .into_par_iter()
            .map(|path| {
                if profile {
                    let count = active.fetch_add(1, Ordering::Relaxed) + 1;
                    peak_active.fetch_max(count, Ordering::Relaxed);
                }
                let result = prepare_discovered_file(root, path);
                if profile {
                    active.fetch_sub(1, Ordering::Relaxed);
                }
                result
            })
            .collect()
    })
}

#[cfg(test)]
#[path = "discovery/tests.rs"]
mod tests;
