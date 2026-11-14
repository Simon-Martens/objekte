//! Validate desktop drafts without changing CLI or query semantics.

use std::path::{Path, PathBuf};

use objekte_cli::arguments::{Alphabet, Arguments};

use crate::backend::{OpenSpec, QuerySpec};
use crate::ui::model::{IndexOpenSource, IndexStorageLocation, OpenDraft, OpenMode, SearchDraft};

/// Build the existing CLI opening arguments from visible controls.
pub fn open_spec(draft: &OpenDraft, cwd: &Path) -> Result<OpenSpec, String> {
    let paths: Vec<PathBuf> = draft
        .paths
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();
    let recipe_only_load = draft.index_source == IndexOpenSource::Load
        && paths.is_empty()
        && draft.index_location == IndexStorageLocation::Custom;
    if paths.is_empty() && !recipe_only_load {
        return Err("Choose a folder before opening data".into());
    }
    let mut args = Arguments {
        auto_index: draft.auto_index,
        // Loading with explicit source paths confirms a saved recipe; omission
        // lets the recipe supply the recorded alphabet.
        index_alphabet: if draft.index_source == IndexOpenSource::Load {
            draft.unicode_index.then_some(Alphabet::Unicode)
        } else if draft.unicode_index {
            Some(Alphabet::Unicode)
        } else {
            Some(Alphabet::Ascii)
        },
        diagnostic: draft.diagnostic,
        ..Arguments::default()
    };
    if !recipe_only_load {
        match draft.mode {
            OpenMode::Folder => {
                if paths.len() > 1 {
                    return Err("Choose one folder in single-folder mode".into());
                }
                args.path = paths;
                args.no_recurse = !draft.recursive;
                if !draft.schema.trim().is_empty() {
                    args.schema = Some(PathBuf::from(draft.schema.trim()));
                }
            }
            OpenMode::MultipleFolders => {
                if paths.len() < 2 {
                    return Err("Choose at least two folders for multiple-folder mode".into());
                }
                args.path = paths;
                args.no_recurse = !draft.recursive;
            }
            OpenMode::ConfiguredRepository => {
                if paths.len() != 1 {
                    return Err("Choose one configured repository root".into());
                }
                args.repo = paths.into_iter().next();
            }
        }
    }
    if draft.index_source == IndexOpenSource::Load {
        args.check_index = draft.check_after_load;
        args.load_index = Some(match draft.index_location {
            IndexStorageLocation::SystemCache => PathBuf::from("cache"),
            IndexStorageLocation::Repository => PathBuf::from("repo"),
            IndexStorageLocation::Custom => {
                if draft.custom_index.trim().is_empty() {
                    return Err("Choose a custom index folder".into());
                }
                PathBuf::from(draft.custom_index.trim())
            }
        });
    }
    args.validate_open_options()?;
    Ok(OpenSpec {
        args,
        cwd: cwd.to_path_buf(),
    })
}

/// Build an unbounded desktop query. Ordering is parsed only from the visible
/// structured expression by the core query builder.
pub fn query_spec(draft: &SearchDraft) -> Result<QuerySpec, String> {
    let structured = if draft.filter_enabled {
        draft.structured.trim()
    } else {
        ""
    };
    let (full_text, filter) = if structured.starts_with('"') {
        if !draft.full_text.trim().is_empty() {
            return Err(
                "A quoted CLI text literal cannot be combined with the Full Text field".into(),
            );
        }
        let decoded = serde_json::from_str::<String>(structured)
            .map_err(|error| format!("invalid FTS JSON string literal: {error}"))?;
        (decoded, String::new())
    } else {
        (draft.full_text.clone(), structured.to_owned())
    };
    Ok(QuerySpec {
        full_text,
        filter,
        scope: draft.collection.clone(),
        directory: draft.directory.clone(),
        case_sensitive: draft.case_sensitive,
        regex: draft.regex,
        offset: 0,
        limit: None,
        count: false,
    })
}

#[cfg(test)]
#[path = "adapt.test.rs"]
mod tests;
