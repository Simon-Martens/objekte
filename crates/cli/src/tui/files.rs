//! Working with the files behind records: the clipboard, opening a record
//! in an editor, and picking up changes made by other programs. Changes
//! are only read when asked for; there is no file watcher.

use super::app::App;
use objekte_core::{CollectionChanges, SourceChange};
use std::collections::BTreeMap;
use std::path::Path;


/// The OSC 52 sequence that sets the system clipboard. Terminals, tmux
/// (with `set-clipboard on`), and SSH sessions pass it on.
pub(super) fn osc52(text: &str) -> String {
    format!("\x1b]52;c;{}\x07", base64(text.as_bytes()))
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (index, byte)| {
            value | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(TABLE[(value >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// The editor command from `$VISUAL` or `$EDITOR`, split into words.
pub(super) fn editor() -> Vec<String> {
    std::env::var("VISUAL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            std::env::var("EDITOR")
                .ok()
                .filter(|value| !value.trim().is_empty())
        })
        .unwrap_or_else(|| "vi".into())
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

fn summary(report: &BTreeMap<String, CollectionChanges>) -> (usize, String) {
    let (mut created, mut changed, mut deleted) = (0, 0, 0);
    for outcome in report.values() {
        created += outcome.changes.created.len();
        changed += outcome.changes.changed.len();
        deleted += outcome.changes.deleted.len();
    }
    let total = created + changed + deleted;
    let mut parts = Vec::new();
    for (count, word) in [(changed, "changed"), (created, "new"), (deleted, "deleted")] {
        if count > 0 {
            parts.push(format!("{count} {word}"));
        }
    }
    (total, parts.join(", "))
}


impl App<'_> {
    /// After an editor closed: index the edited file and show the result.
    pub(super) fn apply_edit(&mut self, path: &Path) {
        match self
            .repo
            .apply_source_changes(&[SourceChange::Changed(path.to_owned())])
        {
            Ok(report) => {
                let (total, text) = summary(&report);
                if total == 0 {
                    self.notice("No changes saved");
                } else {
                    // Re-running the search clears notices, so report after.
                    self.reload_shown();
                    self.notice(format!("Record updated ({text})"));
                }
            }
            Err(error) => self.error(format!("Cannot read the edited file: {error}")),
        }
    }

    /// F5: compare every collection with its files and apply the changes.
    pub(super) fn refresh_files(&mut self) {
        match self.repo.diff_all() {
            Ok(report) => {
                let (total, text) = summary(&report);
                if total == 0 {
                    self.notice("No files changed");
                } else {
                    self.reload_shown();
                    self.notice(format!("Files updated: {text}"));
                }
            }
            Err(error) => self.error(format!("Cannot check files: {error}")),
        }
    }

    /// Show the current versions of what is on screen, keeping the selection.
    fn reload_shown(&mut self) {
        // Records changed, so their value counts may have too.
        self.value_counts.borrow_mut().clear();
        if let Some(linked) = self
            .links
            .override_hit
            .as_ref()
            .map(|hit| hit.record().clone())
        {
            match self.repo.get_record(&linked.collection, &linked.id) {
                Ok(Some(record)) => {
                    self.links.override_hit = Some(super::app::Hit::Filter(record));
                }
                _ => self.leave_detail(),
            }
        }
        if self.last_request.is_some() {
            self.rerun();
        }
        if self.view == super::app::View::Detail {
            self.prepare_detail_links();
        }
    }
}


#[cfg(test)]
#[path = "files.test.rs"]
mod tests;
