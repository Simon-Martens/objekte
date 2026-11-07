//! Per-repository TUI settings in `.objekte/.cache/tui.json`: colour theme,
//! query history, saved queries, and column choices. They are personal, so
//! they live with the other local caches that version control ignores. The
//! file is optional; a missing file starts with defaults.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

/// Most recent queries kept in the history.
const HISTORY: usize = 200;

fn is_false(value: &bool) -> bool {
    !value
}


/// The two query lines and the collection they searched.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Query {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) search: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) filter: String,
    /// Collection name; none searches all collections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) collection: Option<String>,
    /// The current TUI default is case-sensitive. Older settings omit this.
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) case_insensitive: bool,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(super) regex: bool,
}


#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SavedQuery {
    pub(super) name: String,
    #[serde(flatten)]
    pub(super) query: Query,
}


/// Hidden result columns for one collection, or `*` for all collections.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Columns {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) hidden: Vec<String>,
}


#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct Settings {
    /// `light` or `dark`; unset follows the terminal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) theme: Option<String>,
    /// Newest last.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) history: Vec<Query>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) saved: Vec<SavedQuery>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) columns: BTreeMap<String, Columns>,
}


impl Settings {
    pub(super) fn path(root: &Path) -> PathBuf {
        root.join(".objekte/.cache/tui.json")
    }

    /// Read the settings; a missing file gives defaults.
    pub(super) fn load(path: &Path) -> Result<Self, String> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .map_err(|error| format!("{}: {error}", path.display())),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }

    /// Write the settings atomically next to the repository configuration.
    pub(super) fn save(&self, path: &Path) -> Result<(), String> {
        let describe = |error: &dyn std::fmt::Display| format!("{}: {error}", path.display());
        let parent = path
            .parent()
            .ok_or_else(|| describe(&"no parent directory"))?;
        fs::create_dir_all(parent).map_err(|error| describe(&error))?;
        let mut bytes = serde_json::to_vec_pretty(self).map_err(|error| describe(&error))?;
        bytes.push(b'\n');
        let temporary = path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|error| describe(&error))?;
        fs::rename(&temporary, path).map_err(|error| describe(&error))
    }

    /// Remember a query, moving a repeated one to the end.
    pub(super) fn remember(&mut self, query: Query) {
        if query.search.trim().is_empty() && query.filter.trim().is_empty() {
            return;
        }
        self.history.retain(|old| old != &query);
        self.history.push(query);
        let excess = self.history.len().saturating_sub(HISTORY);
        self.history.drain(..excess);
    }

    pub(super) fn hidden(&self, collection: Option<&str>) -> &[String] {
        self.columns
            .get(collection.unwrap_or("*"))
            .map_or(&[], |columns| columns.hidden.as_slice())
    }

    pub(super) fn toggle_column(&mut self, collection: Option<&str>, key: &str) {
        let hidden = &mut self
            .columns
            .entry(collection.unwrap_or("*").to_owned())
            .or_default()
            .hidden;
        if let Some(index) = hidden.iter().position(|name| name == key) {
            hidden.remove(index);
        } else {
            hidden.push(key.to_owned());
        }
        self.columns.retain(|_, columns| !columns.hidden.is_empty());
    }
}


#[cfg(test)]
#[path = "settings.test.rs"]
mod tests;
