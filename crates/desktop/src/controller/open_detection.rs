//! Cheap opening-mode detection from the existing collection configuration.

use std::io::ErrorKind;
use std::path::Path;

use objekte_cli::arguments::Arguments;
use objekte_cli::loader::configure_definitions;


/// Folder classification before any source discovery or index construction.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DetectedSource {
    PlainFolder,
    ConfiguredRepository { collections: usize },
    InvalidConfiguration { message: String },
}


/// Inspect only `.objekte/collections` and compile its configuration if present.
///
/// The result is advisory until the user confirms Open. A malformed
/// configuration remains visible and never silently changes to folder mode.
pub fn detect_source(path: &Path) -> Result<DetectedSource, String> {
    let root = path
        .canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))?;
    if !root.is_dir() {
        return Err(format!("{} is not a directory", root.display()));
    }
    let marker = root.join(".objekte/collections");
    match std::fs::symlink_metadata(&marker) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            return Ok(DetectedSource::PlainFolder);
        }
        Err(error) => {
            return Ok(DetectedSource::InvalidConfiguration {
                message: format!("{}: {error}", marker.display()),
            });
        }
        Ok(_) => {}
    }
    let args = Arguments {
        repo: Some(root.clone()),
        ..Arguments::default()
    };
    let resolved = args.resolve_paths(&root)?;
    match configure_definitions(&args, &resolved) {
        Ok(configured) => Ok(DetectedSource::ConfiguredRepository {
            collections: configured.definitions.len(),
        }),
        Err(message) => Ok(DetectedSource::InvalidConfiguration { message }),
    }
}

#[cfg(test)]
#[path = "open_detection.test.rs"]
mod tests;
