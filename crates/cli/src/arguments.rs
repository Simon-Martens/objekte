//! Command arguments and their filesystem interpretation.

use std::path::{Path, PathBuf};

use clap::{ArgGroup, Parser, ValueEnum};
use objekte_search_core::index_storage::IndexStorageTarget;

/// The alphabet used by the shared bigram index.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Alphabet {
    Ascii,
    Unicode,
}

impl Default for Alphabet {
    fn default() -> Self {
        Self::Ascii
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum IndexTargetChoice {
    Temporary,
    Repository,
    Custom,
}

/// One invocation of the general JSON search command.
#[derive(Clone, Debug, Default, Parser)]
#[command(name = "obj", version, about = "Search a directory of JSON documents")]
#[command(group(ArgGroup::new("input").args(["query", "fts", "query_file"]).multiple(false)))]
pub struct Arguments {
    /// One structured query, or a complete JSON string literal for FTS.
    #[arg(group = "input")]
    pub query: Option<String>,

    /// Search this directory; repeat to open separate collections.
    #[arg(
        short = 'p',
        long = "path",
        value_name = "PATH",
        conflicts_with = "repo"
    )]
    pub path: Vec<PathBuf>,

    /// Open a configured repository containing `.objekte/collections`.
    #[arg(long, value_name = "PATH", conflicts_with_all = ["path", "schema", "no_recurse"])]
    pub repo: Option<PathBuf>,

    /// Search only immediate JSON children.
    #[arg(long)]
    pub no_recurse: bool,

    /// Infer fields and build bounded automatic ordered postings.
    #[arg(long)]
    pub auto_index: bool,

    /// Apply one explicitly selected JSON Schema to the files collection.
    #[arg(long, value_name = "FILE")]
    pub schema: Option<PathBuf>,

    /// Open the interactive terminal screen for this collection.
    #[arg(long)]
    pub tui: bool,

    /// Search a literal string without JSON quoting.
    #[arg(long, group = "input", value_name = "TEXT")]
    pub fts: Option<String>,

    /// Read one query per physical line; '-' reads stdin.
    #[arg(long, group = "input", value_name = "FILE")]
    pub query_file: Option<String>,

    /// Deprecated. Use --flush-index.
    #[arg(long, hide = true, num_args = 0..=1, require_equals = true, default_missing_value = "", value_name = "DIR")]
    pub write_index: Option<String>,

    /// Load one selected saved index, falling back to a memory build only on absence.
    #[arg(long, value_enum, value_name = "LOCATION")]
    pub load_index: Option<IndexTargetChoice>,

    /// Existing custom base directory for --load-index custom.
    #[arg(long, value_name = "DIR")]
    pub load_index_directory: Option<PathBuf>,

    /// Save one complete publication to this selected location.
    #[arg(long, value_enum, value_name = "LOCATION")]
    pub flush_index: Option<IndexTargetChoice>,

    /// Existing custom base directory for --flush-index custom.
    #[arg(long, value_name = "DIR")]
    pub flush_index_directory: Option<PathBuf>,

    /// Deprecated combined storage option. Use --load-index or --flush-index.
    #[arg(long = "index-storage", hide = true, value_name = "MODE")]
    pub legacy_index_storage: Option<String>,

    /// Deprecated combined custom base. Use an operation-specific directory.
    #[arg(long = "index-directory", hide = true, value_name = "DIR")]
    pub legacy_index_directory: Option<PathBuf>,

    /// Deprecated full-residency selector; saved indexes are always on demand.
    #[arg(long = "index-residency", hide = true, value_name = "MODE")]
    pub legacy_index_residency: Option<String>,

    /// Maximum returned matches. Omission returns all matches.
    #[arg(long, value_name = "N")]
    pub limit: Option<usize>,

    /// Skip this many matches in final order.
    #[arg(long, default_value_t = 0, value_name = "N")]
    pub offset: usize,

    /// Compute an exact match total for a finite page.
    #[arg(long)]
    pub count: bool,

    /// Index alphabet, independent of exact case sensitivity.
    #[arg(long, value_enum, default_value_t = Alphabet::Ascii)]
    pub index_alphabet: Alphabet,

    /// Report inferred index paths in NDJSON and command status on stderr.
    #[arg(long = "diagnostic", visible_alias = "diagnostics")]
    pub diagnostic: bool,
}

/// Canonical collection root and one resolved optional snapshot directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedPaths {
    pub root: PathBuf,
    pub collection_paths: Vec<PathBuf>,
    pub load_index_directory: Option<PathBuf>,
    pub flush_index_directory: Option<PathBuf>,
    pub schema_file: Option<PathBuf>,
}

impl Arguments {
    pub fn multiple_collections(&self) -> bool {
        self.repo.is_some() || self.path.len() > 1
    }

    pub fn validate_input(&self) -> Result<(), String> {
        self.validate_open_options()?;
        if self.tui && self.query_file.is_some() {
            return Err("--tui cannot be combined with --query-file".into());
        }
        if self.query.is_none()
            && self.fts.is_none()
            && self.query_file.is_none()
            && self.load_index.is_none()
            && self.flush_index.is_none()
            && !self.tui
        {
            return Err(
                "missing query (or select --load-index or --flush-index for index-only use)".into(),
            );
        }
        Ok(())
    }

    /// Validate opening choices when a graphical caller constructs arguments directly.
    pub fn validate_open_options(&self) -> Result<(), String> {
        if self.write_index.is_some() {
            return Err(
                "--write-index was removed; use --flush-index temporary|repository|custom".into(),
            );
        }
        if self.legacy_index_storage.is_some()
            || self.legacy_index_directory.is_some()
            || self.legacy_index_residency.is_some()
        {
            return Err("--index-storage, --index-directory and --index-residency were removed; use --load-index/--load-index-directory and --flush-index/--flush-index-directory".into());
        }
        Self::validate_target_choice(
            self.load_index,
            self.load_index_directory.as_deref(),
            "--load-index",
            "--load-index-directory",
        )?;
        Self::validate_target_choice(
            self.flush_index,
            self.flush_index_directory.as_deref(),
            "--flush-index",
            "--flush-index-directory",
        )?;
        if self.repo.is_some() && !self.path.is_empty() {
            return Err("--repo cannot be combined with --path".into());
        }
        if self.repo.is_some() && self.schema.is_some() {
            return Err("--repo cannot be combined with --schema".into());
        }
        if self.repo.is_some() && self.no_recurse {
            return Err("--repo cannot be combined with --no-recurse".into());
        }
        if self.path.len() > 1 && self.schema.is_some() {
            return Err(
                "--schema applies to one collection and cannot be used with repeated --path".into(),
            );
        }
        Ok(())
    }

    fn validate_target_choice(
        choice: Option<IndexTargetChoice>,
        directory: Option<&Path>,
        option: &str,
        directory_option: &str,
    ) -> Result<(), String> {
        match (choice, directory) {
            (Some(IndexTargetChoice::Custom), None) => {
                Err(format!("{option} custom requires {directory_option} DIR"))
            }
            (Some(IndexTargetChoice::Custom), Some(_)) => Ok(()),
            (_, Some(_)) => Err(format!("{directory_option} requires {option} custom")),
            _ => Ok(()),
        }
    }

    pub fn resolved_load_target(
        &self,
        paths: &ResolvedPaths,
    ) -> Result<Option<IndexStorageTarget>, String> {
        self.load_index
            .map(|choice| Self::target(choice, paths.load_index_directory.as_ref()))
            .transpose()
    }

    pub fn resolved_flush_target(
        &self,
        paths: &ResolvedPaths,
    ) -> Result<Option<IndexStorageTarget>, String> {
        self.flush_index
            .map(|choice| Self::target(choice, paths.flush_index_directory.as_ref()))
            .transpose()
    }

    fn target(
        choice: IndexTargetChoice,
        directory: Option<&PathBuf>,
    ) -> Result<IndexStorageTarget, String> {
        Ok(match choice {
            IndexTargetChoice::Temporary => IndexStorageTarget::SystemTemporary,
            IndexTargetChoice::Repository => IndexStorageTarget::RepositoryDirectory,
            IndexTargetChoice::Custom => IndexStorageTarget::CustomDirectory(
                directory
                    .cloned()
                    .ok_or("custom index directory not resolved")?,
            ),
        })
    }

    pub fn resolve_paths(&self, cwd: &Path) -> Result<ResolvedPaths, String> {
        self.validate_open_options()?;
        let selected = if let Some(repo) = self.repo.as_deref() {
            vec![repo]
        } else if self.path.is_empty() {
            vec![cwd]
        } else {
            self.path.iter().map(PathBuf::as_path).collect()
        };
        let mut resolved = Vec::with_capacity(selected.len());
        for selected in selected {
            let directory = if selected.is_absolute() {
                selected.to_path_buf()
            } else {
                cwd.join(selected)
            };
            let directory = directory
                .canonicalize()
                .map_err(|error| format!("{}: {error}", directory.display()))?;
            if !directory.is_dir() {
                return Err(format!("{} is not a directory", directory.display()));
            }
            resolved.push(directory);
        }
        let root = if resolved.len() == 1 {
            resolved[0].clone()
        } else {
            for (index, left) in resolved.iter().enumerate() {
                for right in resolved.iter().skip(index + 1) {
                    if left.starts_with(right) || right.starts_with(left) {
                        return Err(format!(
                            "--path directories must be distinct and non-overlapping: {} and {}",
                            left.display(),
                            right.display()
                        ));
                    }
                }
            }
            let mut root = resolved[0]
                .parent()
                .ok_or("--path has no parent directory")?;
            while !resolved.iter().all(|path| path.starts_with(root)) {
                root = root
                    .parent()
                    .ok_or("--path directories have no common parent")?;
            }
            if root.parent().is_none() {
                return Err("--path directories have no useful common parent".into());
            }
            root.to_path_buf()
        };
        let schema_file = self
            .schema
            .as_ref()
            .map(|path| {
                let path = if path.is_absolute() {
                    path.clone()
                } else {
                    cwd.join(path)
                };
                let path = path
                    .canonicalize()
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                if !path.is_file() {
                    return Err(format!("{} is not a file", path.display()));
                }
                Ok(path)
            })
            .transpose()?;
        let resolve_custom = |custom: &Path| -> Result<PathBuf, String> {
            let base = if custom.is_absolute() {
                custom.to_owned()
            } else {
                cwd.join(custom)
            };
            let base = base
                .canonicalize()
                .map_err(|error| format!("custom index base {}: {error}", base.display()))?;
            if !base.is_dir() {
                return Err(format!(
                    "custom index base is not a directory: {}",
                    base.display()
                ));
            }
            Ok(base)
        };
        let load_index_directory = self
            .load_index_directory
            .as_deref()
            .map(resolve_custom)
            .transpose()?;
        let flush_index_directory = self
            .flush_index_directory
            .as_deref()
            .map(resolve_custom)
            .transpose()?;
        Ok(ResolvedPaths {
            root,
            collection_paths: if self.repo.is_some() {
                Vec::new()
            } else {
                resolved
            },
            load_index_directory,
            flush_index_directory,
            schema_file,
        })
    }
}

#[cfg(test)]
#[path = "arguments.test.rs"]
mod tests;
