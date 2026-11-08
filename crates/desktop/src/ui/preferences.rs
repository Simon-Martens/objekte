//! Desktop settings with the TUI's history, saved-query, and column semantics.

use std::path::{Path, PathBuf};

use objekte_tui::settings::{Query, SavedQuery, Settings};

use super::model::{QueryEntry, SearchDraft};


/// Desktop preferences stored separately from terminal preferences.
#[derive(Clone)]
pub struct DesktopPreferences {
    path: PathBuf,
    settings: Settings,
}


impl DesktopPreferences {
    pub fn path(root: &Path) -> PathBuf {
        root.join(".objekte/.cache/obj-desktop.json")
    }

    pub fn load(root: &Path) -> Result<Self, String> {
        let path = Self::path(root);
        let settings = Settings::load(&path)?;
        Ok(Self { path, settings })
    }

    pub fn history(&self) -> Vec<QueryEntry> {
        self.settings
            .history
            .iter()
            .map(|query| QueryEntry {
                name: None,
                draft: from_query(query),
            })
            .collect()
    }

    pub fn saved(&self) -> Vec<QueryEntry> {
        self.settings
            .saved
            .iter()
            .map(|saved| QueryEntry {
                name: Some(saved.name.clone()),
                draft: from_query(&saved.query),
            })
            .collect()
    }

    pub fn remember(&mut self, draft: &SearchDraft) -> Result<(), String> {
        self.remember_local(draft);
        self.persist()
    }

    /// Update resident history without filesystem work on the window thread.
    pub fn remember_local(&mut self, draft: &SearchDraft) {
        self.settings.remember(to_query(draft));
    }

    pub fn save_query(&mut self, name: &str, draft: &SearchDraft) -> Result<(), String> {
        self.save_query_local(name, draft)?;
        self.persist()
    }

    /// Apply named-query semantics in memory; callers queue `persist` separately.
    pub fn save_query_local(&mut self, name: &str, draft: &SearchDraft) -> Result<(), String> {
        let name = name.trim();
        if name.is_empty() {
            return Err("Enter a name for the saved query".into());
        }
        if draft.full_text.trim().is_empty() && draft.structured.trim().is_empty() {
            return Err("Enter a search or filter before saving".into());
        }
        self.settings.saved.retain(|saved| saved.name != name);
        self.settings.saved.push(SavedQuery {
            name: name.to_owned(),
            query: to_query(draft),
        });
        Ok(())
    }

    pub fn restore_history(&self, newest_first_index: usize) -> Option<SearchDraft> {
        self.settings
            .history
            .iter()
            .rev()
            .nth(newest_first_index)
            .map(from_query)
    }

    pub fn restore_saved(&self, name: &str) -> Option<SearchDraft> {
        self.settings
            .saved
            .iter()
            .find(|saved| saved.name == name)
            .map(|saved| from_query(&saved.query))
    }

    pub fn hidden(&self, collection: Option<&str>) -> &[String] {
        self.settings.hidden(collection)
    }

    pub fn toggle_column(&mut self, collection: Option<&str>, key: &str) -> Result<(), String> {
        self.toggle_column_local(collection, key);
        self.persist()
    }

    /// Change a visible column without touching disk in a render callback.
    pub fn toggle_column_local(&mut self, collection: Option<&str>, key: &str) {
        self.settings.toggle_column(collection, key);
    }

    pub fn dark(&self) -> Option<bool> {
        self.settings.theme.as_deref().map(|theme| theme == "dark")
    }

    pub fn set_dark(&mut self, dark: bool) -> Result<(), String> {
        self.set_dark_local(dark);
        self.persist()
    }

    /// Change the resident theme before a queued settings write.
    pub fn set_dark_local(&mut self, dark: bool) {
        self.settings.theme = Some(if dark { "dark" } else { "light" }.into());
    }

    /// Write one immutable settings snapshot from a serialized worker queue.
    pub fn persist(&self) -> Result<(), String> {
        self.settings.save(&self.path)
    }
}


fn to_query(draft: &SearchDraft) -> Query {
    Query {
        search: draft.full_text.clone(),
        filter: draft.structured.clone(),
        collection: draft.collection.clone(),
        case_insensitive: !draft.case_sensitive,
        regex: draft.regex,
    }
}

fn from_query(query: &Query) -> SearchDraft {
    SearchDraft {
        full_text: query.search.clone(),
        structured: query.filter.clone(),
        filter_enabled: !query.filter.trim().is_empty(),
        collection: query.collection.clone(),
        case_sensitive: !query.case_insensitive,
        regex: query.regex,
        ..SearchDraft::default()
    }
}

#[cfg(test)]
#[path = "preferences.test.rs"]
mod tests;
