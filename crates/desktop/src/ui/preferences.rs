//! Versioned, desktop-only preferences. Legacy files are read once and written
//! in the new shape on the next preference change; TUI settings are untouched.

use std::{
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use objekte_tui::settings::{Query as LegacyQuery, Settings as LegacySettings};
use objekte_search_core::query_path::QueryPath;
use serde::{Deserialize, Serialize};

use super::model::{QueryEntry, SearchDraft};

const VERSION: u32 = 2;
const HISTORY: usize = 200;


/// Only the restorable definition belongs in bookmarks and history. Delivery
/// offsets, count requests, and view selection are deliberately excluded.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct QueryDefinition {
    full_text: String,
    structured: String,
    filter_enabled: bool,
    collection: Option<String>,
    case_sensitive: bool,
    regex: bool,
}


impl From<&SearchDraft> for QueryDefinition {
    fn from(draft: &SearchDraft) -> Self {
        Self {
            full_text: draft.full_text.clone(),
            structured: draft.structured.clone(),
            filter_enabled: draft.filter_enabled,
            collection: draft.collection.clone(),
            case_sensitive: draft.case_sensitive,
            regex: draft.regex,
        }
    }
}


impl QueryDefinition {
    fn into_draft(self) -> SearchDraft {
        SearchDraft {
            full_text: self.full_text,
            structured: self.structured,
            filter_enabled: self.filter_enabled,
            collection: self.collection,
            case_sensitive: self.case_sensitive,
            regex: self.regex,
            ..SearchDraft::default()
        }
    }
}


#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
struct Bookmark {
    id: u64,
    /// A renamed title is independent of the query definition.
    custom_title: Option<String>,
    query: QueryDefinition,
}


#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
struct ColumnPreferences {
    #[serde(default)]
    hidden: Vec<String>,
    /// Nested paths start hidden; this list records explicit choices to show them.
    #[serde(default)]
    shown_nested: Vec<String>,
    #[serde(default)]
    widths: BTreeMap<String, u16>,
    #[serde(default)]
    wrapped: Vec<String>,
}

pub(crate) fn nested_column(column: &str) -> bool {
    QueryPath::parse(column).is_ok_and(|path| path.steps.len() > 1)
}


#[derive(Clone, Debug, Serialize, Deserialize)]
struct DesktopSettings {
    version: u32,
    #[serde(default)]
    next_bookmark_id: u64,
    #[serde(default)]
    theme: Option<String>,
    /// History is oldest first so repeat executions move to the end.
    #[serde(default)]
    history: Vec<QueryDefinition>,
    #[serde(default)]
    saved: Vec<Bookmark>,
    #[serde(default)]
    columns: BTreeMap<String, ColumnPreferences>,
}


impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            version: VERSION,
            next_bookmark_id: 1,
            theme: None,
            history: Vec::new(),
            saved: Vec::new(),
            columns: BTreeMap::new(),
        }
    }
}


/// Desktop preferences stored separately from terminal preferences.
#[derive(Clone)]
pub struct DesktopPreferences {
    path: PathBuf,
    settings: DesktopSettings,
}


impl DesktopPreferences {
    pub fn path(root: &Path) -> PathBuf {
        root.join(".objekte/.cache/obj-desktop.json")
    }

    pub fn load(root: &Path) -> Result<Self, String> {
        let path = Self::path(root);
        let settings = match fs::read(&path) {
            Ok(bytes) => {
                let value: serde_json::Value = serde_json::from_slice(&bytes)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                if value.get("version").is_some() {
                    let settings: DesktopSettings = serde_json::from_value(value)
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                    if settings.version != VERSION {
                        return Err(format!(
                            "{}: unsupported desktop settings version {}",
                            path.display(),
                            settings.version
                        ));
                    }
                    settings
                } else {
                    let legacy: LegacySettings = serde_json::from_value(value)
                        .map_err(|error| format!("{}: {error}", path.display()))?;
                    migrate_legacy(legacy)
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => DesktopSettings::default(),
            Err(error) => return Err(format!("{}: {error}", path.display())),
        };
        Ok(Self { path, settings })
    }

    pub fn history(&self) -> Vec<QueryEntry> {
        self.settings
            .history
            .iter()
            .map(|query| QueryEntry {
                id: None,
                name: None,
                draft: query.clone().into_draft(),
            })
            .collect()
    }

    pub fn saved(&self) -> Vec<QueryEntry> {
        self.settings
            .saved
            .iter()
            .map(|bookmark| QueryEntry {
                id: Some(bookmark.id),
                name: Some(
                    bookmark
                        .custom_title
                        .clone()
                        .unwrap_or_else(|| generated_title(&bookmark.query.clone().into_draft())),
                ),
                draft: bookmark.query.clone().into_draft(),
            })
            .collect()
    }

    pub fn remember(&mut self, draft: &SearchDraft) -> Result<(), String> {
        self.remember_local(draft);
        self.persist()
    }

    /// Record a successful execution, including zero-result and collection-only
    /// queries. The caller serializes writes on the preference worker.
    pub fn remember_local(&mut self, draft: &SearchDraft) {
        let query = QueryDefinition::from(draft);
        self.settings.history.retain(|old| old != &query);
        self.settings.history.push(query);
        let excess = self.settings.history.len().saturating_sub(HISTORY);
        self.settings.history.drain(..excess);
    }

    /// Save immediately under an automatic title. Repeated definitions keep
    /// their existing bookmark ID and any custom title.
    pub fn save_query_local(&mut self, draft: &SearchDraft) -> u64 {
        let query = QueryDefinition::from(draft);
        if let Some(saved) = self
            .settings
            .saved
            .iter()
            .find(|saved| saved.query == query)
        {
            return saved.id;
        }
        let id = self.settings.next_bookmark_id.max(1);
        self.settings.next_bookmark_id = id.saturating_add(1);
        self.settings.saved.push(Bookmark {
            id,
            custom_title: None,
            query,
        });
        id
    }

    pub fn save_query(&mut self, draft: &SearchDraft) -> Result<u64, String> {
        let id = self.save_query_local(draft);
        self.persist()?;
        Ok(id)
    }

    pub fn rename_local(&mut self, id: u64, title: &str) -> Result<(), String> {
        let title = title.trim();
        if title.is_empty() {
            return Err("Saved query title cannot be empty".into());
        }
        let saved = self
            .settings
            .saved
            .iter_mut()
            .find(|saved| saved.id == id)
            .ok_or_else(|| "Saved query no longer exists".to_string())?;
        saved.custom_title = Some(title.to_owned());
        Ok(())
    }

    pub fn remove_local(&mut self, id: u64) -> bool {
        let before = self.settings.saved.len();
        self.settings.saved.retain(|saved| saved.id != id);
        self.settings.saved.len() != before
    }

    pub fn bookmark_id(&self, draft: &SearchDraft) -> Option<u64> {
        let query = QueryDefinition::from(draft);
        self.settings
            .saved
            .iter()
            .find(|saved| saved.query == query)
            .map(|saved| saved.id)
    }

    pub fn restore_history(&self, newest_first_index: usize) -> Option<SearchDraft> {
        self.settings
            .history
            .iter()
            .rev()
            .nth(newest_first_index)
            .cloned()
            .map(QueryDefinition::into_draft)
    }

    pub fn restore_saved(&self, id: u64) -> Option<SearchDraft> {
        self.settings
            .saved
            .iter()
            .find(|saved| saved.id == id)
            .map(|saved| saved.query.clone().into_draft())
    }

    pub fn hidden(&self, collection: Option<&str>) -> &[String] {
        self.settings
            .columns
            .get(collection.unwrap_or("*"))
            .map_or(&[], |columns| columns.hidden.as_slice())
    }

    pub fn shown_nested(&self, collection: Option<&str>) -> &[String] {
        self.settings
            .columns
            .get(collection.unwrap_or("*"))
            .map_or(&[], |columns| columns.shown_nested.as_slice())
    }

    pub fn toggle_column_local(&mut self, collection: Option<&str>, key: &str) {
        let columns = self
            .settings
            .columns
            .entry(collection.unwrap_or("*").to_owned())
            .or_default();
        if nested_column(key) {
            if let Some(index) = columns.shown_nested.iter().position(|name| name == key) {
                columns.shown_nested.remove(index);
            } else {
                columns.hidden.retain(|name| name != key);
                columns.shown_nested.push(key.to_owned());
            }
        } else if let Some(index) = columns.hidden.iter().position(|name| name == key) {
            columns.hidden.remove(index);
        } else {
            columns.hidden.push(key.to_owned());
        }
    }

    pub fn table_presentation(
        &self,
        collection: Option<&str>,
    ) -> (BTreeMap<String, u16>, Vec<String>) {
        let columns = self.settings.columns.get(collection.unwrap_or("*"));
        columns.map_or_else(
            || (BTreeMap::new(), Vec::new()),
            |columns| (columns.widths.clone(), columns.wrapped.clone()),
        )
    }

    pub fn set_column_width_local(&mut self, collection: Option<&str>, key: &str, width: f32) {
        let columns = self
            .settings
            .columns
            .entry(collection.unwrap_or("*").to_owned())
            .or_default();
        columns
            .widths
            .insert(key.to_owned(), width.clamp(80., 600.).round() as u16);
    }

    pub fn toggle_wrap_local(&mut self, collection: Option<&str>, key: &str) {
        let columns = self
            .settings
            .columns
            .entry(collection.unwrap_or("*").to_owned())
            .or_default();
        if let Some(index) = columns.wrapped.iter().position(|name| name == key) {
            columns.wrapped.remove(index);
        } else {
            columns.wrapped.push(key.to_owned());
        }
    }

    pub fn dark(&self) -> Option<bool> {
        self.settings.theme.as_deref().map(|theme| theme == "dark")
    }

    pub fn set_dark_local(&mut self, dark: bool) {
        self.settings.theme = Some(if dark { "dark" } else { "light" }.into());
    }

    /// The preference worker writes snapshots in channel order.
    pub fn persist(&self) -> Result<(), String> {
        let describe = |error: &dyn std::fmt::Display| format!("{}: {error}", self.path.display());
        let parent = self
            .path
            .parent()
            .ok_or_else(|| describe(&"no parent directory"))?;
        fs::create_dir_all(parent).map_err(|error| describe(&error))?;
        let mut bytes =
            serde_json::to_vec_pretty(&self.settings).map_err(|error| describe(&error))?;
        bytes.push(b'\n');
        let temporary = self.path.with_extension("json.tmp");
        fs::write(&temporary, bytes).map_err(|error| describe(&error))?;
        fs::rename(&temporary, &self.path).map_err(|error| describe(&error))
    }
}


/// A display-only description shared by history and default bookmarks.
pub fn generated_title(draft: &SearchDraft) -> String {
    let collection = draft.collection.as_deref().unwrap_or("All collections");
    let fts = draft
        .full_text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let filter = if draft.filter_enabled {
        draft
            .structured
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        String::new()
    };
    let mut parts = vec![collection.to_owned()];
    if !fts.is_empty() {
        parts.push(fts);
    }
    if !filter.is_empty() {
        parts.push(filter);
    }
    if parts.len() == 1 {
        parts.push("All records".into());
    }
    parts.join(" · ")
}

fn migrate_legacy(legacy: LegacySettings) -> DesktopSettings {
    let mut settings = DesktopSettings {
        theme: legacy.theme,
        history: legacy.history.iter().map(from_legacy).collect(),
        ..DesktopSettings::default()
    };
    for saved in legacy.saved {
        let id = settings.next_bookmark_id;
        settings.next_bookmark_id += 1;
        settings.saved.push(Bookmark {
            id,
            custom_title: Some(saved.name),
            query: from_legacy(&saved.query),
        });
    }
    for (scope, columns) in legacy.columns {
        settings.columns.insert(
            scope,
            ColumnPreferences {
                hidden: columns.hidden,
                ..ColumnPreferences::default()
            },
        );
    }
    settings
}

fn from_legacy(query: &LegacyQuery) -> QueryDefinition {
    QueryDefinition {
        full_text: query.search.clone(),
        structured: query.filter.clone(),
        // The old format had no explicit flag, so derive the old behavior once.
        filter_enabled: !query.filter.trim().is_empty(),
        collection: query.collection.clone(),
        case_sensitive: !query.case_insensitive,
        regex: query.regex,
    }
}

#[cfg(test)]
#[path = "preferences.test.rs"]
mod tests;
