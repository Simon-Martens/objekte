//! The one rule for whether text matches a full-text pattern. Full-text
//! search and `:fts` filter comparisons both use it, so a mode means the
//! same thing everywhere.

use crate::{Error, Result, TextFormat, TextMode};
use regex::Regex;
use std::borrow::Cow;
use unicode_casefold::UnicodeCaseFold;

/// Longest accepted pattern, in bytes.
const MAX_PATTERN: usize = 16 * 1024;


/// A compiled full-text pattern.
///
/// - `Word` reads the normalized text (rendered by format, punctuation as
///   a space, whitespace collapsed) and looks for a substring.
/// - `WordRaw` reads the stored text and looks for a substring.
/// - `Regex` runs a Rust regex over the stored text.
///
/// The word modes fold case unless `case_sensitive`; a regex sets its own.
#[derive(Debug)]
pub(crate) struct TextPattern {
    source: String,
    mode: TextMode,
    folds: bool,
    /// Word modes: the text to find, normalized and folded as the values are.
    needle: Option<String>,
    /// ASCII needle compared without allocating, for folded word modes.
    ascii_needle: Option<Vec<u8>>,
    regex: Regex,
}


impl TextPattern {
    pub(crate) fn compile(pattern: &str, mode: TextMode, case_sensitive: bool) -> Result<Self> {
        if pattern.is_empty() || pattern.len() > MAX_PATTERN {
            return Err(Error::Request(
                "text pattern must contain 1–16384 bytes".into(),
            ));
        }
        let folds = mode != TextMode::Regex && !case_sensitive;
        let needle = match mode {
            TextMode::Regex => None,
            TextMode::WordRaw => Some(pattern.to_owned()),
            TextMode::Word => Some(
                crate::text::render_text(pattern, TextFormat::Plain)
                    .expect("plain rendering always succeeds"),
            ),
        };
        if needle
            .as_deref()
            .is_some_and(|needle| needle.trim().is_empty())
        {
            return Err(Error::Request(
                "word search needs letters or digits; punctuation and whitespace are ignored"
                    .into(),
            ));
        }
        let needle = needle.map(|needle| {
            if folds {
                needle.case_fold().collect::<String>()
            } else {
                needle
            }
        });
        let regex_source = needle
            .as_deref()
            .map_or_else(|| pattern.to_owned(), regex::escape);
        let regex = Regex::new(&regex_source)
            .map_err(|error| Error::Request(format!("invalid text regex: {error}")))?;
        let ascii_needle = needle
            .as_deref()
            .filter(|needle| folds && needle.is_ascii())
            .map(|needle| needle.as_bytes().to_vec());
        Ok(Self {
            source: pattern.to_owned(),
            mode,
            folds,
            needle,
            ascii_needle,
            regex,
        })
    }

    pub(crate) fn mode(&self) -> TextMode {
        self.mode
    }

    pub(crate) fn folds(&self) -> bool {
        self.folds
    }

    /// The pattern as given.
    pub(crate) fn source(&self) -> &str {
        &self.source
    }

    /// Word modes: the normalized (and folded) text being looked for.
    pub(crate) fn needle(&self) -> Option<&str> {
        self.needle.as_deref()
    }

    /// The text a value is read as: rendered and normalized in word mode,
    /// as stored otherwise. `None` for markup that cannot be rendered.
    fn read<'a>(&self, value: &'a str, format: TextFormat) -> Option<Cow<'a, str>> {
        if self.mode != TextMode::Word
            || (format == TextFormat::Plain && !crate::text::needs_normalizing(value))
        {
            return Some(Cow::Borrowed(value));
        }
        crate::text::render_text(value, format).map(Cow::Owned)
    }

    /// Whether the pattern occurs in `value`: a substring in the word modes,
    /// a regex match otherwise.
    pub(crate) fn matches(&self, value: &str, format: TextFormat) -> bool {
        let Some(text) = self.read(value, format) else {
            return false;
        };
        if let Some(needle) = &self.ascii_needle
            && text.is_ascii()
        {
            return text
                .as_bytes()
                .windows(needle.len())
                .any(|part| part.eq_ignore_ascii_case(needle));
        }
        if self.folds {
            self.regex.is_match(&text.case_fold().collect::<String>())
        } else {
            self.regex.is_match(&text)
        }
    }

    /// Whether `value`, read like the pattern, is the whole pattern text.
    /// A regex compares the stored text with the pattern exactly.
    pub(crate) fn equals(&self, value: &str, format: TextFormat) -> bool {
        let Some(needle) = &self.needle else {
            return value == self.source;
        };
        let Some(text) = self.read(value, format) else {
            return false;
        };
        if self.folds {
            text.case_fold().eq(needle.chars())
        } else {
            text == needle.as_str()
        }
    }
}


#[cfg(test)]
#[path = "matcher.test.rs"]
mod tests;
