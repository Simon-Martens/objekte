//! Cache destination checks and exact discovery exclusions for explicit index opening.

use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::discovery::SymlinkPolicy;
use crate::lifecycle::PublishedIndex;
use crate::model::FileId;
use crate::repository::CollectionDefinition;

/// Resolve a destination that may not exist yet through its nearest existing
/// ancestor. This keeps a symlinked parent visible even when nested cache
/// directories have not been created.
fn resolved_destination(path: &Path) -> Result<PathBuf, String> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()
            .map_err(|error| error.to_string())?
            .join(path)
    };
    let mut ancestor = absolute.as_path();
    let mut suffix = Vec::new();
    loop {
        match fs::canonicalize(ancestor) {
            Ok(mut resolved) => {
                for part in suffix.iter().rev() {
                    resolved.push(part);
                }
                return Ok(resolved);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let name = ancestor.file_name().ok_or_else(|| {
                    format!(
                        "cache destination has no existing ancestor: {}",
                        path.display()
                    )
                })?;
                suffix.push(name.to_os_string());
                ancestor = ancestor
                    .parent()
                    .ok_or("cache destination needs a parent")?;
            }
            Err(error) => return Err(format!("cache destination {}: {error}", path.display())),
        }
    }
}

#[cfg(unix)]
fn same_file(left: &fs::Metadata, right: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    left.dev() == right.dev() && left.ino() == right.ino()
}


/// Prepared checks for one published view. Construct once for a flush, then
/// call `ensure_safe` before writing and immediately before rename.
/// Setup examines collection roots once; each ordinary check is constant time
/// plus the number of protected configuration paths. Supply both configured
/// layout paths and their canonical targets so deleted symlink names stay
/// protected for the lifetime of the publication.
pub(crate) struct CacheDestinationGuard<'a> {
    publication: &'a PublishedIndex,
    protected_files: &'a [PathBuf],
    source_symlinks_possible: bool,
}


impl<'a> CacheDestinationGuard<'a> {
    pub(crate) fn new(publication: &'a PublishedIndex, protected_files: &'a [PathBuf]) -> Self {
        let root = &publication.repository.files().root;
        let source_symlinks_possible =
            publication
                .repository
                .collections()
                .iter()
                .any(|collection| {
                    if collection.definition.discovery.symlinks == SymlinkPolicy::FollowWithinRoot {
                        return true;
                    }
                    let directory = root.join(&collection.definition.source_path);
                    match fs::canonicalize(&directory) {
                        Ok(resolved) => resolved != directory,
                        Err(_) => true,
                    }
                });
        Self {
            publication,
            protected_files,
            source_symlinks_possible,
        }
    }

    /// Refuse an explicit cache target that aliases an active indexed source or
    /// a configuration file. The catalog path lookup is constant time; active
    /// files are scanned only for possible symlink or hardlink aliases.
    pub(crate) fn ensure_safe(&self, path: &Path) -> Result<(), String> {
        let publication = self.publication;
        let protected_files = self.protected_files;
        let destination = resolved_destination(path)?;
        let root = &publication.repository.files().root;
        if let Ok(relative) = destination.strip_prefix(root)
            && publication.catalog.id_for(relative).is_some()
        {
            return Err(format!(
                "cache destination aliases indexed source file: {}",
                destination.display()
            ));
        }

        #[cfg(unix)]
        let destination_metadata = fs::metadata(path).ok();
        #[cfg(unix)]
        let multiple_links = destination_metadata.as_ref().is_some_and(|metadata| {
            use std::os::unix::fs::MetadataExt;
            metadata.nlink() > 1
        });
        #[cfg(not(unix))]
        let multiple_links = false;

        for protected in protected_files {
            if destination == resolved_destination(protected)? {
                return Err(format!(
                    "cache destination aliases configuration file: {}",
                    protected.display()
                ));
            }

            #[cfg(unix)]
            if multiple_links
                && let (Some(destination_metadata), Ok(protected_metadata)) =
                    (&destination_metadata, fs::metadata(protected))
                && same_file(destination_metadata, &protected_metadata)
            {
                return Err(format!(
                    "cache destination aliases configuration file: {}",
                    protected.display()
                ));
            }
        }

        if !multiple_links && !self.source_symlinks_possible {
            return Ok(());
        }

        for number in 0..publication.catalog.allocated_files() {
            let file = FileId(number as u32);
            let Some(entry) = publication.catalog.entry(file) else {
                continue;
            };
            let source = root.join(&entry.relative_path);
            let resolved_source = match fs::canonicalize(&source) {
                Ok(resolved) => resolved,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // A deleted source remains protected while its slot is active.
                    if let Ok(target) = fs::read_link(&source) {
                        resolved_destination(
                            &source.parent().ok_or("source needs parent")?.join(target),
                        )?
                    } else {
                        resolved_destination(&source)?
                    }
                }
                Err(error) => return Err(format!("indexed source {}: {error}", source.display())),
            };
            if destination == resolved_source {
                return Err(format!(
                    "cache destination aliases indexed source file: {}",
                    source.display()
                ));
            }

            #[cfg(unix)]
            if multiple_links
                && let (Some(destination_metadata), Ok(source_metadata)) =
                    (&destination_metadata, fs::metadata(&source))
                && same_file(destination_metadata, &source_metadata)
            {
                return Err(format!(
                    "cache destination aliases indexed source file: {}",
                    source.display()
                ));
            }
        }
        Ok(())
    }
}


/// Exclude only the selected cache file and snapshot writer temporary names
/// from supplied collection definitions. Other files in the same directory
/// remain discoverable. The caller applies this before building the index.
pub(crate) fn exclude_cache_artifacts(
    root: &Path,
    definitions: &mut [CollectionDefinition],
    path: &Path,
) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("{}: {error}", root.display()))?;
    let cache = resolved_destination(path)?;
    let lexical_cache = std::path::absolute(path).map_err(|error| error.to_string())?;
    for definition in definitions {
        let source_directory = root.join(&definition.source_path);
        let source_root = source_directory
            .canonicalize()
            .map_err(|error| format!("{}: {error}", definition.source_path.display()))?;
        // Include both the resolved target and the selected path. A cache
        // symlink may be encountered by discovery under its lexical name.
        for (candidate, directory) in [(&cache, &source_root), (&lexical_cache, &source_directory)]
        {
            let Ok(relative) = candidate.strip_prefix(directory) else {
                continue;
            };
            if relative.as_os_str().is_empty()
                || relative
                    .components()
                    .any(|part| !matches!(part, Component::Normal(_)))
            {
                return Err("cache destination is not a normal source-relative path".into());
            }
            if !definition
                .discovery
                .exclude_files
                .iter()
                .any(|excluded| excluded == relative)
            {
                definition.discovery.exclude_files.push(relative.to_owned());
            }
            let relative_parent = relative
                .parent()
                .ok_or("cache destination needs a parent")?;
            let pattern = if relative_parent.as_os_str().is_empty() {
                ".objekte-snapshot-*.tmp".to_owned()
            } else {
                let parent_text = relative_parent
                    .to_str()
                    .ok_or("cache parent is not UTF-8")?;
                format!("{}/.objekte-snapshot-*.tmp", globset::escape(parent_text))
            };
            if !definition.discovery.exclude.contains(&pattern) {
                definition.discovery.exclude.push(pattern);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "lifecycle_cache_guard.test.rs"]
mod tests;
