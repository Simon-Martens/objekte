//! A single-line text editor with a character cursor.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};


/// Text and a cursor counted in characters, not bytes.
#[derive(Default, Clone)]
pub(super) struct Input {
    pub(super) text: String,
    pub(super) cursor: usize,
}


impl Input {
    pub(super) fn with_text(text: &str) -> Self {
        Self {
            text: text.into(),
            cursor: text.chars().count(),
        }
    }

    pub(super) fn len(&self) -> usize {
        self.text.chars().count()
    }

    pub(super) fn byte_at(&self, character: usize) -> usize {
        self.text
            .char_indices()
            .nth(character)
            .map_or(self.text.len(), |(index, _)| index)
    }

    pub(super) fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// Replace characters `start..end` and put the cursor after `with`.
    pub(super) fn replace(&mut self, start: usize, end: usize, with: &str) {
        let (from, to) = (self.byte_at(start), self.byte_at(end));
        self.text.replace_range(from..to, with);
        self.cursor = start + with.chars().count();
    }

    fn word_start(&self) -> usize {
        let chars = self.text.chars().collect::<Vec<_>>();
        let mut at = self.cursor;
        while at > 0 && chars[at - 1].is_whitespace() {
            at -= 1;
        }
        while at > 0 && !chars[at - 1].is_whitespace() {
            at -= 1;
        }
        at
    }

    fn word_end(&self) -> usize {
        let chars = self.text.chars().collect::<Vec<_>>();
        let mut at = self.cursor;
        while at < chars.len() && chars[at].is_whitespace() {
            at += 1;
        }
        while at < chars.len() && !chars[at].is_whitespace() {
            at += 1;
        }
        at
    }

    /// Apply an editing key. Returns whether the text changed; cursor-only
    /// moves return `false`. Keys the editor does not use return `None`.
    pub(super) fn key(&mut self, key: KeyEvent) -> Option<bool> {
        let control = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        match key.code {
            // Ctrl+Alt is AltGr on many international layouts.
            KeyCode::Char(ch) if !ch.is_control() && (!control || alt) => {
                // Some CSI-u terminals omit the shifted codepoint. Recover
                // ASCII letters without assuming a layout for punctuation.
                let ch = if key.modifiers.contains(KeyModifiers::SHIFT) {
                    ch.to_ascii_uppercase()
                } else {
                    ch
                };
                self.text.insert(self.byte_at(self.cursor), ch);
                self.cursor += 1;
                Some(true)
            }
            KeyCode::Char('w') | KeyCode::Backspace
                if control || (alt && key.code == KeyCode::Backspace) =>
            {
                let start = self.word_start();
                let changed = start < self.cursor;
                self.replace(start, self.cursor, "");
                Some(changed)
            }
            KeyCode::Char('u') if control => {
                let changed = self.cursor > 0;
                self.replace(0, self.cursor, "");
                Some(changed)
            }
            KeyCode::Char('k') if control => {
                let changed = self.cursor < self.len();
                self.replace(self.cursor, self.len(), "");
                self.cursor = self.len();
                Some(changed)
            }
            KeyCode::Char('a') if control => {
                self.cursor = 0;
                Some(false)
            }
            KeyCode::Char('e') if control => {
                self.cursor = self.len();
                Some(false)
            }
            KeyCode::Backspace if self.cursor > 0 => {
                self.replace(self.cursor - 1, self.cursor, "");
                Some(true)
            }
            KeyCode::Delete if self.cursor < self.len() => {
                let at = self.cursor;
                self.replace(at, at + 1, "");
                self.cursor = at;
                Some(true)
            }
            KeyCode::Backspace | KeyCode::Delete => Some(false),
            KeyCode::Left if control || alt => {
                self.cursor = self.word_start();
                Some(false)
            }
            KeyCode::Right if control || alt => {
                self.cursor = self.word_end();
                Some(false)
            }
            KeyCode::Left => {
                self.cursor = self.cursor.saturating_sub(1);
                Some(false)
            }
            KeyCode::Right => {
                self.cursor = (self.cursor + 1).min(self.len());
                Some(false)
            }
            KeyCode::Home => {
                self.cursor = 0;
                Some(false)
            }
            KeyCode::End => {
                self.cursor = self.len();
                Some(false)
            }
            _ => None,
        }
    }
}


#[cfg(test)]
#[path = "input.test.rs"]
mod tests;
