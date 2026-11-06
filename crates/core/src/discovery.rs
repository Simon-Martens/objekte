//! Deterministic collection file selection shared by construction and restore.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};

use crate::source::PreparedSource;


#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SymlinkPolicy {
    #[default]
    Skip,
    FollowWithinRoot,
}


#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Discovery {
    pub recursive: bool,
    pub include: Vec<String>,
    pub exclude: Vec<String>,
    pub hidden: bool,
    pub symlinks: SymlinkPolicy,
}


impl Default for Discovery {
    fn default() -> Self {
        Self {
            recursive: false,
            include: Vec::new(),
            exclude: Vec::new(),
            hidden: false,
            symlinks: SymlinkPolicy::Skip,
        }
    }
}


fn compile(patterns: &[String]) -> Result<GlobSet, String> {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        builder
            .add(Glob::new(pattern).map_err(|e| format!("invalid discovery glob {pattern}: {e}"))?);
    }
    builder.build().map_err(|e| e.to_string())
}

fn hidden(path: &Path) -> bool {
    path.components().any(|part| match part {
        Component::Normal(name) => name.to_string_lossy().starts_with('.'),
        _ => false,
    })
}

/// Returns repository-native lexical paths. Symlink aliases to one physical
/// source are reduced to the first path in lexical order.
pub fn discover(
    root: &Path,
    source_path: &Path,
    rules: &Discovery,
    source: &PreparedSource,
) -> Result<Vec<PathBuf>, String> {
    if source_path.is_absolute()
        || source_path
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err("collection source path must be a contained relative path".into());
    }
    let source_directory = root.join(source_path);
    let physical_root = source_directory
        .canonicalize()
        .map_err(|e| format!("{}: {e}", source_directory.display()))?;
    if !physical_root.starts_with(root) || !physical_root.is_dir() {
        return Err(
            "collection source directory escapes repository root or is not a directory".into(),
        );
    }
    let includes = compile(&rules.include)?;
    let excludes = compile(&rules.exclude)?;
    let mut pending = vec![(source_directory.clone(), vec![physical_root.clone()])];
    let mut direct_files = Vec::new();
    let mut aliases = BTreeMap::<PathBuf, PathBuf>::new();
    while let Some((directory, ancestors)) = pending.pop() {
        let entries =
            std::fs::read_dir(&directory).map_err(|e| format!("{}: {e}", directory.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            let selected_path = path
                .strip_prefix(&source_directory)
                .map_err(|e| e.to_string())?;
            if !rules.hidden && hidden(selected_path) {
                continue;
            }
            let kind = entry.file_type().map_err(|e| e.to_string())?;
            if rules.symlinks == SymlinkPolicy::Skip {
                if kind.is_dir() && rules.recursive {
                    pending.push((path, ancestors.clone()));
                } else if kind.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext == source.extension())
                    && (rules.include.is_empty() || includes.is_match(selected_path))
                    && !excludes.is_match(selected_path)
                {
                    direct_files.push(path);
                }
                continue;
            }
            let physical = path
                .canonicalize()
                .map_err(|e| format!("{}: {e}", path.display()))?;
            if !physical.starts_with(&physical_root) {
                continue;
            }
            if physical.is_dir() {
                if rules.recursive && !ancestors.contains(&physical) {
                    let mut child_ancestors = ancestors.clone();
                    child_ancestors.push(physical);
                    pending.push((path, child_ancestors));
                }
            } else if physical.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext == source.extension())
                && (rules.include.is_empty() || includes.is_match(selected_path))
                && !excludes.is_match(selected_path)
            {
                aliases
                    .entry(physical)
                    .and_modify(|existing| {
                        if path < *existing {
                            *existing = path.clone();
                        }
                    })
                    .or_insert(path);
            }
        }
    }
    let mut paths = if rules.symlinks == SymlinkPolicy::Skip {
        direct_files
    } else {
        aliases.into_values().collect()
    };
    paths.sort();
    Ok(paths)
}
