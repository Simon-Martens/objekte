//! Bounded filesystem discovery metadata.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use rayon::prelude::*;

use crate::index::traits::file_catalog::{
    DiscoveredFile, FILE_TRANSFER_LIMIT, FileDiscoveryBuilder,
};
use crate::index::traits::transfer::StorageError;
use crate::inventory::SourceRevision;
use crate::lifecycle::IndexError;
use crate::model::{CollectionId, FileId};

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

/// Feed the discovery editor bounded owned rows in metadata order.
pub(crate) fn register_discovery_window(
    writer: &mut dyn FileDiscoveryBuilder,
    collection: CollectionId,
    prepared: impl IntoIterator<Item = Result<DiscoveredFile, String>>,
    members: &mut Vec<FileId>,
) -> Result<(), IndexError> {
    const OP: &str = "register discovered files";
    let mut batch = Vec::new();
    let mut bytes = 0;
    let mut write = |batch: &mut Vec<DiscoveredFile>| {
        let count = batch.len();
        let previous = members.len();
        writer.register(collection, batch, members)?;
        if !batch.is_empty() || members.len().checked_sub(previous) != Some(count) {
            return Err(StorageError::invalid(
                OP,
                "discovery register count mismatch",
            ));
        }
        Ok(())
    };
    for row in prepared {
        let row = row.map_err(IndexError::Build)?;
        let row_bytes = size_of::<DiscoveredFile>().saturating_add(row.relative.as_os_str().len());
        if !FILE_TRANSFER_LIMIT.accepts(OP, batch.len(), bytes, row_bytes)? {
            write(&mut batch)?;
            FILE_TRANSFER_LIMIT.accepts(OP, 0, 0, row_bytes)?;
            bytes = 0;
        }
        batch.push(row);
        bytes += row_bytes;
    }
    if !batch.is_empty() {
        write(&mut batch)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    super::super::tests::discovery_tests!();
}
