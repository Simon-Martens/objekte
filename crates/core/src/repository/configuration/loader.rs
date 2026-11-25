//! Generic source containment and generated-index path exclusions.
//!
//! This module resolves definitions and protected files. It never opens or
//! chooses an index cache and does not build an index.

use std::path::{Component, Path, PathBuf};

use super::CollectionDefinition;

/// Prepare collection definitions for the live configuration identity and
/// source traversal. Selected source paths are validated, the generated index
/// namespace is rejected as source data, and established index locations are
/// pruned. The function never stats, opens or creates an index file.
pub(crate) fn prepare_source_definitions(
    root: &Path,
    definitions: &[CollectionDefinition],
) -> Result<Vec<CollectionDefinition>, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("repository root {}: {error}", root.display()))?;
    if !root.is_dir() {
        return Err(format!(
            "repository root is not a directory: {}",
            root.display()
        ));
    }
    // Kept in the same order and spelling as the previous live identity so
    // unchanged inputs keep their configuration hash byte-for-byte.
    let owned_namespaces = [
        root.join(".objekte").join(".indexes"),
        root.join(".objekte").join("indexes"),
    ];
    let mut prepared = definitions.to_vec();
    for definition in &mut prepared {
        if crate::inventory::discovery::contains_generated_directory(&definition.source_path) {
            return Err(format!(
                "collection {} source selection {} selects or lies beneath a generated index namespace",
                definition.name,
                definition.source_path.display()
            ));
        }
        let selected = root.join(normalized_selection(&definition.source_path)?);
        let selected = match selected.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => selected,
            Err(error) => {
                return Err(format!(
                    "collection {} source selection {}: {error}",
                    definition.name,
                    selected.display()
                ));
            }
        };
        if selected
            .strip_prefix(&root)
            .is_ok_and(crate::inventory::discovery::contains_generated_directory)
        {
            return Err(format!(
                "collection {} source selection {} resolves inside a generated index namespace",
                definition.name,
                definition.source_path.display()
            ));
        }
        for owned in &owned_namespaces {
            if selected.starts_with(owned) {
                return Err(format!(
                    "collection {} selects the generated index namespace as source data",
                    definition.name
                ));
            }
            if let Ok(relative) = owned.strip_prefix(&selected) {
                if relative.as_os_str().is_empty() {
                    return Err("generated index namespace is the collection source root".into());
                }
                if !definition
                    .discovery
                    .exclude_directories
                    .iter()
                    .any(|existing| {
                        !existing.as_os_str().is_empty()
                            && existing
                                .components()
                                .all(|part| matches!(part, Component::Normal(_)))
                            && relative.starts_with(existing)
                    })
                {
                    definition
                        .discovery
                        .exclude_directories
                        .push(relative.to_path_buf());
                }
            }
        }
    }
    Ok(prepared)
}

fn normalized_selection(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Err(format!(
            "source selection must be relative: {}",
            path.display()
        ));
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => result.push(part),
            Component::ParentDir => {
                if !result.pop() {
                    return Err(format!("source selection escapes root: {}", path.display()));
                }
            }
            _ => return Err(format!("invalid source selection: {}", path.display())),
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests;

#[cfg(test)]
#[path = "../../../../loader-json/src/configuration.rs"]
mod json_configuration;
#[cfg(test)]
pub(crate) use json_configuration::load_repository_configuration;
