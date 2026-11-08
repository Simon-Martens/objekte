//! Validate desktop drafts without changing CLI or query semantics.

use std::path::{Path, PathBuf};

use objekte_cli::arguments::{Alphabet, Arguments};
use objekte_search_core::query::parse_structured_filter;

use crate::backend::{OpenSpec, QuerySpec};
use crate::ui::model::{OpenDraft, OpenMode, SearchDraft, SnapshotMode};

/// Build the existing CLI opening arguments from visible controls.
pub fn open_spec(draft: &OpenDraft, cwd: &Path) -> Result<OpenSpec, String> {
    let mut args = Arguments {
        auto_index: draft.auto_index,
        index_alphabet: if draft.unicode_index {
            Alphabet::Unicode
        } else {
            Alphabet::Ascii
        },
        diagnostic: draft.diagnostic,
        ..Arguments::default()
    };
    let paths: Vec<PathBuf> = draft
        .paths
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();
    if paths.is_empty() {
        return Err("Choose a folder before opening data".into());
    }
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
    let snapshot_path = draft.snapshot_directory.trim().to_owned();
    match draft.snapshot {
        SnapshotMode::None => {}
        SnapshotMode::Write => args.write_index = Some(snapshot_path),
        SnapshotMode::Load => args.load_index = Some(snapshot_path),
        SnapshotMode::LoadAndWrite => {
            args.load_index = Some(snapshot_path.clone());
            args.write_index = Some(snapshot_path);
        }
    }
    args.validate_open_options()?;
    Ok(OpenSpec {
        args,
        cwd: cwd.to_path_buf(),
    })
}

/// Parse ranges and the independent sort control using the core grammar.
pub fn query_spec(draft: &SearchDraft) -> Result<QuerySpec, String> {
    let offset = draft
        .offset
        .trim()
        .parse::<usize>()
        .map_err(|_| "Offset must be a nonnegative integer")?;
    let limit = if draft.limit.trim().is_empty() {
        None
    } else {
        Some(
            draft
                .limit
                .trim()
                .parse::<usize>()
                .map_err(|_| "Limit must be a nonnegative integer")?,
        )
    };
    let sort = if draft.sort.trim().is_empty() {
        Vec::new()
    } else {
        let parsed = parse_structured_filter(&format!("SORT BY {}", draft.sort.trim()))?;
        let sort = parsed.sort.ok_or("Enter a valid sort field")?;
        if sort.len() != 1 {
            return Err("Choose one sort field in the sort control".into());
        }
        sort
    };
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
        (draft.full_text.clone(), draft.structured.clone())
    };
    Ok(QuerySpec {
        full_text,
        filter,
        scope: draft.collection.clone(),
        case_sensitive: draft.case_sensitive,
        regex: draft.regex,
        sort,
        offset,
        limit,
        count: draft.count,
    })
}

#[cfg(test)]
#[path = "adapt.test.rs"]
mod tests;
