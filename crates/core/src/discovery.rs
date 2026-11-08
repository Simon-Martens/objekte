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
    /// Exact source-relative files, including names containing glob syntax.
    pub exclude_files: Vec<PathBuf>,
    /// Exact directories relative to the collection source root to prune before traversal.
    pub exclude_directories: Vec<PathBuf>,
    pub hidden: bool,
    pub symlinks: SymlinkPolicy,
}

impl Default for Discovery {
    fn default() -> Self {
        Self {
            recursive: false,
            include: Vec::new(),
            exclude: Vec::new(),
            exclude_files: Vec::new(),
            exclude_directories: Vec::new(),
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
    discover_bounded(root, source_path, rules, source, usize::MAX)
}

/// The reconciliation path sets an admission ceiling so an unexpectedly
/// large filesystem cannot grow its pending path inventory without bound.
pub(crate) fn discover_bounded(
    root: &Path,
    source_path: &Path,
    rules: &Discovery,
    source: &PreparedSource,
    max_paths: usize,
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
            let directory_excluded = rules
                .exclude_directories
                .iter()
                .any(|excluded| selected_path == excluded);
            if rules.symlinks == SymlinkPolicy::Skip {
                if kind.is_dir() && rules.recursive {
                    if !directory_excluded && !excludes.is_match(selected_path) {
                        if pending.len() >= max_paths {
                            return Err("source discovery directory budget exceeded".into());
                        }
                        pending.push((path, ancestors.clone()));
                    }
                } else if kind.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext == source.extension())
                    && (rules.include.is_empty() || includes.is_match(selected_path))
                    && !excludes.is_match(selected_path)
                    && !rules
                        .exclude_files
                        .iter()
                        .any(|excluded| selected_path == excluded)
                {
                    if direct_files.len() >= max_paths {
                        return Err("source discovery file budget exceeded".into());
                    }
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
                if rules.recursive
                    && !directory_excluded
                    && !excludes.is_match(selected_path)
                    && !ancestors.contains(&physical)
                {
                    let mut child_ancestors = ancestors.clone();
                    child_ancestors.push(physical);
                    if pending.len() >= max_paths {
                        return Err("source discovery directory budget exceeded".into());
                    }
                    pending.push((path, child_ancestors));
                }
            } else if physical.is_file()
                && path
                    .extension()
                    .is_some_and(|ext| ext == source.extension())
                && (rules.include.is_empty() || includes.is_match(selected_path))
                && !excludes.is_match(selected_path)
                && !rules
                    .exclude_files
                    .iter()
                    .any(|excluded| selected_path == excluded)
            {
                if !aliases.contains_key(&physical) && aliases.len() >= max_paths {
                    return Err("source discovery file budget exceeded".into());
                }
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

/// Check one repository-relative path against the source, glob, hidden,
/// recursion and symlink eligibility rules used by full discovery. Global
/// deduplication of multiple symlink aliases still belongs to the mutation
/// coordinator. This direct check avoids enumerating sibling files.
pub fn selects_file(
    root: &Path,
    source_path: &Path,
    rules: &Discovery,
    source: &PreparedSource,
    relative_path: &Path,
) -> Result<bool, String> {
    if source_path.is_absolute()
        || source_path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
        || relative_path.is_absolute()
        || relative_path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("file and source paths must be contained repository-relative paths".into());
    }
    let Ok(selected) = relative_path.strip_prefix(source_path) else {
        return Ok(false);
    };
    if selected.as_os_str().is_empty()
        || (!rules.recursive && selected.components().count() != 1)
        || (!rules.hidden && hidden(selected))
        || relative_path
            .extension()
            .is_none_or(|ext| ext != source.extension())
        || rules
            .exclude_files
            .iter()
            .any(|excluded| selected == excluded)
    {
        return Ok(false);
    }
    let includes = compile(&rules.include)?;
    let excludes = compile(&rules.exclude)?;
    if (!rules.include.is_empty() && !includes.is_match(selected)) || excludes.is_match(selected) {
        return Ok(false);
    }
    let mut prefix = PathBuf::new();
    for part in selected
        .components()
        .take(selected.components().count() - 1)
    {
        prefix.push(part);
        if rules
            .exclude_directories
            .iter()
            .any(|excluded| *excluded == prefix)
            || excludes.is_match(&prefix)
        {
            return Ok(false);
        }
    }
    let source_directory = root.join(source_path);
    let physical_root = source_directory
        .canonicalize()
        .map_err(|error| format!("{}: {error}", source_directory.display()))?;
    if !physical_root.starts_with(root) || !physical_root.is_dir() {
        return Err(
            "collection source directory escapes repository root or is not a directory".into(),
        );
    }
    let path = root.join(relative_path);
    if rules.symlinks == SymlinkPolicy::Skip {
        let mut component_path = source_directory;
        for part in selected.components() {
            component_path.push(part);
            let metadata = match std::fs::symlink_metadata(&component_path) {
                Ok(metadata) => metadata,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
                Err(error) => return Err(format!("{}: {error}", component_path.display())),
            };
            if metadata.file_type().is_symlink() {
                return Ok(false);
            }
        }
        Ok(path.is_file())
    } else {
        let physical = match path.canonicalize() {
            Ok(path) => path,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        Ok(physical.starts_with(&physical_root) && physical.is_file())
    }
}

#[cfg(test)]
#[path = "discovery.test.rs"]
mod tests;
