//! Shared index-location policy for CLI, TUI, and desktop frontends.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::arguments::Alphabet;


/// One selected index location.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexLocation {
    SystemCache,
    Repository,
    Custom(PathBuf),
}


impl IndexLocation {
    /// Parse the CLI spelling: exactly `cache` and `repo` are presets.
    pub fn parse(value: &Path) -> Self {
        match value.to_str() {
            Some("cache") => Self::SystemCache,
            Some("repo") => Self::Repository,
            _ => Self::Custom(value.to_owned()),
        }
    }

    /// Resolve the location against the selected source root and working
    /// directory. Custom paths are used exactly as selected.
    pub fn resolve(
        &self,
        source_root: &Path,
        cwd: &Path,
        alphabet: Alphabet,
        auto_index: bool,
    ) -> Result<PathBuf, String> {
        match self {
            Self::Repository => Ok(source_root.join(".objekte").join(".index")),
            Self::Custom(path) => {
                let path = if path.is_absolute() {
                    path.clone()
                } else {
                    cwd.join(path)
                };
                if path == source_root {
                    return Err("the index directory cannot be the source root".into());
                }
                Ok(path)
            }
            Self::SystemCache => {
                let cache = dirs::cache_dir()
                    .ok_or("no system cache directory is available")?
                    .join("objekte")
                    .join("indexes");
                let key = source_key(source_root, alphabet, auto_index);
                Ok(cache.join(key))
            }
        }
    }
}


/// Stable source-selection key for the system cache namespace.
pub fn source_key(source_root: &Path, alphabet: Alphabet, auto_index: bool) -> String {
    let mut hasher = Sha256::new();
    let root = path_bytes(source_root);
    hasher.update((root.len() as u64).to_le_bytes());
    hasher.update(&root);
    hasher.update([match alphabet {
        Alphabet::Ascii => 0,
        Alphabet::Unicode => 1,
    }]);
    hasher.update([u8::from(auto_index)]);
    let digest: [u8; 32] = hasher.finalize().into();
    let mut text = String::with_capacity(64);
    for byte in digest {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

fn path_bytes(path: &Path) -> Vec<u8> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        path.as_os_str().as_bytes().to_vec()
    }

    #[cfg(not(unix))]
    {
        path.to_string_lossy().into_owned().into_bytes()
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_are_exact_and_custom_paths_are_not() {
        assert_eq!(
            IndexLocation::parse(Path::new("cache")),
            IndexLocation::SystemCache
        );
        assert_eq!(
            IndexLocation::parse(Path::new("repo")),
            IndexLocation::Repository
        );
        assert_eq!(
            IndexLocation::parse(Path::new("./cache")),
            IndexLocation::Custom(PathBuf::from("./cache"))
        );
        assert_eq!(
            IndexLocation::parse(Path::new("CACHE")),
            IndexLocation::Custom(PathBuf::from("CACHE"))
        );
    }

    #[test]
    fn repository_location_is_root_relative() {
        let root = Path::new("/tmp/archive");
        assert_eq!(
            IndexLocation::Repository
                .resolve(root, Path::new("/tmp"), Alphabet::Ascii, false)
                .unwrap(),
            root.join(".objekte/.index")
        );
    }
}
