//! Command arguments and their filesystem interpretation.

use std::path::{Component, Path, PathBuf};

use clap::{ArgGroup, Parser, ValueEnum};

/// The alphabet used by the shared bigram index.
#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum Alphabet {
    Ascii,
    Unicode,
}

/// One invocation of the general JSON search command.
#[derive(Clone, Debug, Parser)]
#[command(name = "obj", version, about = "Search a directory of JSON documents")]
#[command(group(ArgGroup::new("input").args(["query", "fts", "query_file"]).multiple(false)))]
pub struct Arguments {
    /// One structured query, or a complete JSON string literal for FTS.
    #[arg(group = "input")]
    pub query: Option<String>,

    /// Search this directory instead of the current directory.
    #[arg(
        short = 'p',
        long = "path",
        value_name = "PATH",
        conflicts_with = "repo"
    )]
    pub path: Option<PathBuf>,

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

    /// Build and save an index; optional directory requires '='.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "", value_name = "DIR")]
    pub write_index: Option<String>,

    /// Load an index; optional directory requires '='.
    #[arg(long, num_args = 0..=1, require_equals = true, default_missing_value = "", value_name = "DIR")]
    pub load_index: Option<String>,

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
    pub index_directory: Option<PathBuf>,
    pub schema_file: Option<PathBuf>,
}

impl Arguments {
    pub fn validate_input(&self) -> Result<(), String> {
        if self.tui && self.query_file.is_some() {
            return Err("--tui cannot be combined with --query-file".into());
        }
        if self.query.is_none()
            && self.fts.is_none()
            && self.query_file.is_none()
            && self.write_index.is_none()
            && self.load_index.is_none()
            && !self.tui
        {
            return Err("missing query (or --write-index/--load-index for index-only use)".into());
        }
        Ok(())
    }

    pub fn resolve_paths(&self, cwd: &Path) -> Result<ResolvedPaths, String> {
        let selected = self.repo.as_deref().or(self.path.as_deref()).unwrap_or(cwd);
        let root = if selected.is_absolute() {
            selected.to_path_buf()
        } else {
            cwd.join(selected)
        };
        let root = root
            .canonicalize()
            .map_err(|error| format!("{}: {error}", root.display()))?;
        if !root.is_dir() {
            return Err(format!("{} is not a directory", root.display()));
        }
        let read = self
            .load_index
            .as_deref()
            .map(|input| index_path(input, cwd, &root));
        let write = self
            .write_index
            .as_deref()
            .map(|input| index_path(input, cwd, &root));
        let read = read.transpose()?;
        let write = write.transpose()?;
        if let (Some(read), Some(write)) = (&read, &write)
            && read != write
        {
            return Err("--load-index and --write-index must name the same directory".into());
        }
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
        Ok(ResolvedPaths {
            root,
            index_directory: write.or(read),
            schema_file,
        })
    }
}

fn index_path(input: &str, cwd: &Path, root: &Path) -> Result<PathBuf, String> {
    let path = if input.is_empty() {
        root.join(".objekte")
    } else if Path::new(input).is_absolute() {
        PathBuf::from(input)
    } else {
        cwd.join(input)
    };
    let path = normalize(&path);
    let mut ancestor = path.clone();
    let mut suffix = Vec::new();
    while !ancestor.exists() {
        let name = ancestor
            .file_name()
            .ok_or_else(|| format!("{} has no existing ancestor", path.display()))?;
        suffix.push(name.to_os_string());
        ancestor.pop();
    }
    let mut resolved = ancestor
        .canonicalize()
        .map_err(|error| format!("{}: {error}", ancestor.display()))?;
    for component in suffix.iter().rev() {
        resolved.push(component);
    }
    Ok(resolved)
}

fn normalize(path: &Path) -> PathBuf {
    let mut parts = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                parts.pop();
            }
            _ => parts.push(part.as_os_str()),
        }
    }
    parts
}

#[cfg(test)]
#[path = "arguments.test.rs"]
mod tests;
