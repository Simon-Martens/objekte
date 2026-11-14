//! Versioned saved opening recipe stored in manifest frontend metadata.
//!
//! A recipe captures only the semantic source-selection choices needed to
//! reconstruct collection definitions without recursively discovering source
//! records. Native paths are encoded as hex so non-UTF-8 paths round-trip.

use std::path::{Path, PathBuf};

use objekte_search_core::StoredIndexInfo;
use objekte_search_core::index_storage::manifest::hex as hex_encode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::arguments::{Alphabet, Arguments, ResolvedPaths};
use crate::index_location::IndexLocation;

/// Current opening-recipe schema version.
pub const OPENING_RECIPE_VERSION: u32 = 1;
/// Top-level frontend metadata key holding the recipe.
pub const OPENING_RECIPE_KEY: &str = "opening_recipe";


/// Source-selection family saved with the index.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeSourceMode {
    /// One folder collection named `files`.
    Folder,
    /// Several sibling folders, one collection per folder name.
    Folders,
    /// A configured `.objekte/collections` repository.
    Repository,
}


/// Alphabet spelling stored independently of CLI clap types.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecipeAlphabet {
    Ascii,
    Unicode,
}


impl RecipeAlphabet {
    pub fn from_alphabet(alphabet: Alphabet) -> Self {
        match alphabet {
            Alphabet::Ascii => Self::Ascii,
            Alphabet::Unicode => Self::Unicode,
        }
    }

    pub fn alphabet(self) -> Alphabet {
        match self {
            Self::Ascii => Alphabet::Ascii,
            Self::Unicode => Alphabet::Unicode,
        }
    }
}


/// One saved source-selection recipe.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpeningRecipe {
    pub version: u32,
    pub source_mode: RecipeSourceMode,
    /// Canonical source root, hex-encoded native path bytes.
    pub root: String,
    /// Canonical selected folders, hex-encoded native path bytes.
    pub collection_paths: Vec<String>,
    /// Canonical explicit schema file, hex-encoded native path bytes.
    pub schema_file: Option<String>,
    pub recursive: bool,
    pub alphabet: RecipeAlphabet,
    pub auto_index: bool,
}


impl OpeningRecipe {
    /// Capture the effective opening selected by the CLI/frontends.
    pub fn from_opening(args: &Arguments, paths: &ResolvedPaths) -> Self {
        let source_mode = if args.repo.is_some() {
            RecipeSourceMode::Repository
        } else if paths.collection_paths.len() > 1 {
            RecipeSourceMode::Folders
        } else {
            RecipeSourceMode::Folder
        };
        Self {
            version: OPENING_RECIPE_VERSION,
            source_mode,
            root: path_to_hex(&paths.root),
            collection_paths: paths
                .collection_paths
                .iter()
                .map(|path| path_to_hex(path))
                .collect(),
            schema_file: paths.schema_file.as_deref().map(path_to_hex),
            recursive: !args.no_recurse,
            alphabet: RecipeAlphabet::from_alphabet(args.effective_alphabet()),
            auto_index: args.auto_index,
        }
    }

    /// Encode this recipe into a `frontend_metadata` JSON object.
    pub fn to_metadata(&self) -> Value {
        serde_json::json!({ OPENING_RECIPE_KEY: self })
    }

    /// Decode a recipe from manifest frontend metadata.
    pub fn from_metadata(metadata: Option<&Value>) -> Result<Option<Self>, String> {
        let Some(value) = metadata.and_then(|value| value.get(OPENING_RECIPE_KEY)) else {
            return Ok(None);
        };
        let recipe: Self = serde_json::from_value(value.clone())
            .map_err(|error| format!("saved opening recipe is invalid: {error}"))?;
        if recipe.version != OPENING_RECIPE_VERSION {
            return Err(format!(
                "saved opening recipe version {} is not supported (expected {OPENING_RECIPE_VERSION})",
                recipe.version
            ));
        }
        if recipe.source_mode == RecipeSourceMode::Repository {
            if !recipe.collection_paths.is_empty() {
                return Err("saved repository recipe has selected collection folders".into());
            }
        } else if recipe.collection_paths.is_empty() {
            return Err("saved folder recipe has no selected collection folders".into());
        }
        Ok(Some(recipe))
    }

    /// Read the recipe from an inspected store.
    pub fn from_store(info: &StoredIndexInfo) -> Result<Option<Self>, String> {
        Self::from_metadata(info.manifest.frontend_metadata.as_ref())
    }

    /// Effective alphabet chosen by the recipe.
    pub fn effective_alphabet(&self) -> Alphabet {
        self.alphabet.alphabet()
    }

    /// Rebuild the opening arguments from recipe values.
    pub fn apply(&self, args: &Arguments) -> Result<Arguments, String> {
        let root = path_from_hex(&self.root)?;
        let collection_paths = self
            .collection_paths
            .iter()
            .map(|path| path_from_hex(path))
            .collect::<Result<Vec<_>, _>>()?;
        let schema_file = self.schema_file.as_deref().map(path_from_hex).transpose()?;
        let mut effective = args.clone();
        effective.path = match self.source_mode {
            RecipeSourceMode::Repository => Vec::new(),
            RecipeSourceMode::Folder | RecipeSourceMode::Folders => collection_paths,
        };
        effective.repo = (self.source_mode == RecipeSourceMode::Repository).then_some(root);
        effective.schema = schema_file;
        effective.no_recurse = !self.recursive;
        effective.auto_index = self.auto_index;
        effective.index_alphabet = Some(self.effective_alphabet());
        Ok(effective)
    }

    /// Build resolved paths directly from stored canonical values. Loading a
    /// fully packed index does not require the source directory to still exist.
    pub fn resolved_paths(&self) -> Result<ResolvedPaths, String> {
        Ok(ResolvedPaths {
            root: path_from_hex(&self.root)?,
            collection_paths: self
                .collection_paths
                .iter()
                .map(|path| path_from_hex(path))
                .collect::<Result<Vec<_>, _>>()?,
            schema_file: self.schema_file.as_deref().map(path_from_hex).transpose()?,
        })
    }

    /// Confirm the source root and selected folders of explicit arguments.
    pub fn validate_source_selection(&self, paths: &ResolvedPaths) -> Result<(), String> {
        let root = path_from_hex(&self.root)?;
        if paths.root != root {
            return Err(format!(
                "explicit source root {} does not match the saved index root {}",
                paths.root.display(),
                root.display()
            ));
        }
        let expected_paths = self
            .collection_paths
            .iter()
            .map(|path| path_from_hex(path))
            .collect::<Result<Vec<_>, _>>()?;
        if paths.collection_paths != expected_paths {
            return Err(
                "explicit collection folders do not match the saved index's source selection"
                    .into(),
            );
        }
        Ok(())
    }

    /// Confirm every resolved path, including the explicit schema.
    pub fn validate_paths(&self, paths: &ResolvedPaths) -> Result<(), String> {
        self.validate_source_selection(paths)?;
        let expected_schema = self.schema_file.as_deref().map(path_from_hex).transpose()?;
        if paths.schema_file != expected_schema {
            return Err("explicit schema does not match the saved index's schema".into());
        }
        Ok(())
    }
}


/// One resolved effective opening after applying any saved recipe.
pub struct EffectiveOpening {
    pub args: Arguments,
    pub paths: ResolvedPaths,
    pub recipe: Option<OpeningRecipe>,
    pub load_directory: Option<PathBuf>,
}


impl EffectiveOpening {
    /// Save metadata for the effective opening.
    pub fn save_metadata(&self) -> Value {
        OpeningRecipe::from_opening(&self.args, &self.paths).to_metadata()
    }

    /// Save options carrying the effective opening recipe.
    pub fn save_options(&self) -> objekte_search_core::SaveOptions {
        objekte_search_core::SaveOptions {
            frontend_metadata: Some(self.save_metadata()),
        }
    }
}


/// Resolve explicit source arguments or hydrate them from a custom store's
/// saved opening recipe. This performs no source discovery or record reads.
pub fn resolve_opening(args: &Arguments, cwd: &Path) -> Result<EffectiveOpening, String> {
    args.validate_open_options()?;
    let explicit_source = args.repo.is_some() || !args.path.is_empty();
    let Some(load) = &args.load_index else {
        let paths = args.resolve_paths(cwd)?;
        return Ok(EffectiveOpening {
            args: args.clone(),
            paths,
            recipe: None,
            load_directory: None,
        });
    };
    let location = IndexLocation::parse(load);
    if !explicit_source {
        let IndexLocation::Custom(path) = &location else {
            return Err(
                "--load-index cache/repo requires --path or --repo; use an explicit directory to load its saved opening recipe"
                    .into(),
            );
        };
        let directory = absolute_custom(path, cwd);
        let info = objekte_search_core::inspect_store(&directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?;
        let recipe =
            OpeningRecipe::from_store(&info)?.ok_or_else(|| no_recipe_message(&directory))?;
        validate_explicit_conflicts(args, cwd, &recipe)?;
        let effective_args = recipe.apply(args)?;
        let paths = recipe.resolved_paths()?;
        reject_source_root(&directory, &paths.root)?;
        return Ok(EffectiveOpening {
            args: effective_args,
            paths,
            recipe: Some(recipe),
            load_directory: Some(directory),
        });
    }

    let preliminary = args.resolve_paths(cwd)?;
    let directory = match &location {
        IndexLocation::Custom(path) => absolute_custom(path, cwd),
        _ => location.resolve(
            &preliminary.root,
            cwd,
            args.effective_alphabet(),
            args.auto_index,
        )?,
    };
    let info = objekte_search_core::inspect_store(&directory).ok();
    let recipe = info
        .as_ref()
        .map(OpeningRecipe::from_store)
        .transpose()?
        .flatten();
    let effective_args = if let Some(recipe) = &recipe {
        validate_explicit_conflicts(args, cwd, recipe)?;
        recipe.validate_source_selection(&preliminary)?;
        recipe.apply(args)?
    } else {
        args.clone()
    };
    let paths = effective_args.resolve_paths(cwd)?;
    if let Some(recipe) = &recipe {
        recipe.validate_paths(&paths)?;
    }
    reject_source_root(&directory, &paths.root)?;
    Ok(EffectiveOpening {
        args: effective_args,
        paths,
        recipe,
        load_directory: Some(directory),
    })
}

fn validate_explicit_conflicts(
    args: &Arguments,
    cwd: &Path,
    recipe: &OpeningRecipe,
) -> Result<(), String> {
    if let Some(explicit) = args.index_alphabet
        && explicit != recipe.effective_alphabet()
    {
        return Err(format!(
            "--index-alphabet conflicts with the saved index (saved {})",
            alphabet_name(recipe.effective_alphabet())
        ));
    }
    if args.auto_index && !recipe.auto_index {
        return Err(
            "--auto-index conflicts with the saved index (saved without auto-index)".into(),
        );
    }
    if args.no_recurse && recipe.recursive {
        return Err("--no-recurse conflicts with the saved index (saved recursively)".into());
    }
    if let Some(schema) = &args.schema {
        let explicit = explicit_schema(schema, cwd)?;
        let saved = recipe
            .schema_file
            .as_deref()
            .map(path_from_hex)
            .transpose()?;
        if Some(explicit) != saved {
            return Err("--schema conflicts with the saved index's schema".into());
        }
    }
    Ok(())
}

fn explicit_schema(path: &Path, cwd: &Path) -> Result<PathBuf, String> {
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    path.canonicalize()
        .map_err(|error| format!("{}: {error}", path.display()))
}

fn reject_source_root(directory: &Path, source_root: &Path) -> Result<(), String> {
    if directory == source_root {
        return Err("the index directory cannot be the source root".into());
    }
    Ok(())
}

fn absolute_custom(path: &Path, cwd: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    }
}

fn no_recipe_message(directory: &Path) -> String {
    format!(
        "saved index {} has no opening recipe; supply --path or --repo",
        directory.display()
    )
}

fn alphabet_name(alphabet: Alphabet) -> &'static str {
    match alphabet {
        Alphabet::Ascii => "ascii",
        Alphabet::Unicode => "unicode",
    }
}

/// Encode one native path losslessly as lower-case hexadecimal.
pub fn path_to_hex(path: &Path) -> String {
    hex_encode(&path_bytes(path))
}

/// Decode one lower-case or upper-case hexadecimal native path.
pub fn path_from_hex(text: &str) -> Result<PathBuf, String> {
    if !text.len().is_multiple_of(2) {
        return Err("saved opening recipe path has an odd number of hex characters".into());
    }
    let mut bytes = Vec::with_capacity(text.len() / 2);
    for index in (0..text.len()).step_by(2) {
        let high = (text.as_bytes()[index] as char)
            .to_digit(16)
            .ok_or_else(|| "saved opening recipe path is not hexadecimal".to_string())?;
        let low = (text.as_bytes()[index + 1] as char)
            .to_digit(16)
            .ok_or_else(|| "saved opening recipe path is not hexadecimal".to_string())?;
        bytes.push(((high << 4) | low) as u8);
    }
    path_from_bytes(bytes)
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(|unit| unit.to_le_bytes())
        .collect()
}

#[cfg(unix)]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    use std::os::unix::ffi::OsStringExt;
    Ok(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(windows)]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    use std::os::windows::ffi::OsStringExt;
    if bytes.len() % 2 != 0 {
        return Err("saved opening recipe Windows path is not UTF-16 aligned".into());
    }
    let units = bytes
        .chunks_exact(2)
        .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
        .collect::<Vec<_>>();
    Ok(PathBuf::from(std::ffi::OsString::from_wide(&units)))
}

#[cfg(not(any(unix, windows)))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

#[cfg(not(any(unix, windows)))]
fn path_from_bytes(bytes: Vec<u8>) -> Result<PathBuf, String> {
    Ok(PathBuf::from(String::from_utf8_lossy(&bytes).into_owned()))
}

#[cfg(test)]
#[path = "index_recipe.test.rs"]
mod tests;
