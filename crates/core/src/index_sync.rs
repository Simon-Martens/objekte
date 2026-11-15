//! Where a repository's index is kept besides RAM.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

/// Where a repository's index is kept besides RAM.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexSync {
    Memory,
    Repository,
    Cache,
    Custom(PathBuf),
}

fn hex_prefix(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(text, "{byte:02x}");
    }
    text
}

fn root_key(root: &Path) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.as_os_str().as_encoded_bytes());
    hex_prefix(&hasher.finalize()[..8])
}

/// The store file for this repository and configuration, or `None` for
/// `Memory`. Performs no file system access.
pub fn store_file(
    sync: &IndexSync,
    root: &Path,
    configuration_identity: &[u8; 32],
) -> Result<Option<PathBuf>, String> {
    let directory = match sync {
        IndexSync::Memory => return Ok(None),
        IndexSync::Repository => root.join(".objekte").join(".index"),
        IndexSync::Cache => system_cache_dir()?
            .join("objekte-indexes")
            .join(root_key(root)),
        IndexSync::Custom(directory) => {
            if !directory.is_absolute() {
                return Err("custom index sync directory must be absolute".into());
            }
            directory
                .join("objekte-indexes")
                .join(root_key(root))
        }
    };
    let name = format!("{}.redb", hex_prefix(&configuration_identity[..8]));
    Ok(Some(directory.join(name)))
}

/// The per-user cache directory. Performs no file system access.
pub fn system_cache_dir() -> Result<PathBuf, String> {
    if let Some(value) = std::env::var_os("OBJEKTE_CACHE_DIR") {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    if cfg!(target_os = "macos") {
        if let Some(home) = std::env::var_os("HOME") {
            return Ok(PathBuf::from(home).join("Library").join("Caches"));
        }
        return Err("no cache directory is available".into());
    }
    if cfg!(windows) {
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            return Ok(PathBuf::from(local));
        }
        return Err("no cache directory is available".into());
    }
    if let Some(value) = std::env::var_os("XDG_CACHE_HOME") {
        let path = PathBuf::from(value);
        if path.is_absolute() {
            return Ok(path);
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        return Ok(PathBuf::from(home).join(".cache"));
    }
    Err("no cache directory is available".into())
}

#[cfg(test)]
#[path = "index_sync.test.rs"]
mod tests;
