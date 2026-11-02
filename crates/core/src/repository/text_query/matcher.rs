//! Compile full-text patterns and identify matching document paths.

use super::*;
use crate::{
    TextMode,
    text::{render_text, visit_json_text},
};
use regex::Regex;
use std::borrow::Cow;
use unicode_casefold::UnicodeCaseFold;


/// Compiled matcher and conservative bigram prefilter for one request.
pub(super) struct TextMatcher {
    regex: Regex,
    pub(super) candidate_pattern: Option<String>,
    pub(super) candidate_literal: bool,
    pub(super) raw: bool,
    fold_word: bool,
    ascii_word: Option<Vec<u8>>,
}


/// Canonical query fields and, for returned hits, concrete indexed paths.
pub(super) struct TextMatches {
    pub(super) fields: Vec<String>,
    pub(super) matched_paths: Vec<String>,
}


fn indexed_path(path: &str, indices: &[(usize, usize)]) -> String {
    if path.is_empty() {
        return "$value".into();
    }
    let mut result = String::with_capacity(path.len() + indices.len() * 3);
    if indices.first().is_some_and(|(offset, _)| *offset == 0) {
        result.push_str("$value");
    }
    let mut start = 0;
    for &(offset, index) in indices {
        // Offsets locate traversal-inserted [] markers. Splitting on the text
        // "[]" would mistake the same characters inside a JSON field name.
        result.push_str(&path[start..offset]);
        result.push('[');
        result.push_str(&index.to_string());
        result.push(']');
        start = offset + 2;
    }
    result.push_str(&path[start..]);
    result
}


impl TextMatcher {
    fn ascii_contains(text: &[u8], word: &[u8]) -> bool {
        text.windows(word.len())
            .any(|part| part.eq_ignore_ascii_case(word))
    }

    pub(super) fn matches_value(&self, value: &str, format: crate::TextFormat) -> bool {
        let text = if self.raw
            || (format == crate::TextFormat::Plain && !value.chars().any(char::is_whitespace))
        {
            Some(Cow::Borrowed(value))
        } else {
            render_text(value, format).map(Cow::Owned)
        };
        text.is_some_and(|text| {
            if let Some(word) = &self.ascii_word
                && text.is_ascii()
            {
                return Self::ascii_contains(text.as_bytes(), word);
            }
            if self.fold_word {
                self.regex.is_match(&text.case_fold().collect::<String>())
            } else {
                self.regex.is_match(&text)
            }
        })
    }

    pub(super) fn compile(request: &TextQuery) -> Result<Self> {
        if request.pattern.len() > 16 * 1024 || request.pattern.is_empty() {
            return Err(Error::Request(
                "text pattern must contain 1–16384 bytes".into(),
            ));
        }
        let raw = matches!(request.mode, TextMode::WordRaw | TextMode::RegexRaw);
        let word_mode = matches!(request.mode, TextMode::Word | TextMode::WordRaw);
        let fold_word = word_mode && !request.case_sensitive;
        let mut candidate_base = None;
        let pattern = if word_mode {
            let word = if raw {
                request.pattern.clone()
            } else {
                render_text(&request.pattern, crate::TextFormat::Plain)
                    .expect("plain rendering always succeeds")
            };
            if word.trim().is_empty() {
                return Err(Error::Request(
                    "word search needs non-whitespace text".into(),
                ));
            }
            let word = if fold_word {
                word.case_fold().collect::<String>()
            } else {
                word
            };
            let literal = regex::escape(&word);
            candidate_base = Some(word);
            literal
        } else {
            request.pattern.clone()
        };
        let regex = regex::RegexBuilder::new(&pattern)
            .case_insensitive(!request.case_sensitive && !fold_word)
            .build()
            .map_err(|error| Error::Request(format!("invalid text regex: {error}")))?;
        let candidate_base = candidate_base.as_deref().unwrap_or(&pattern);
        let ascii_word = (word_mode && fold_word && candidate_base.is_ascii())
            .then(|| candidate_base.as_bytes().to_vec());
        let candidate_pattern = if word_mode {
            Some(candidate_base.to_owned())
        } else if !candidate_base.is_ascii() && !request.case_sensitive {
            // Unicode regex folding may select byte sequences absent from a
            // case-folded projection. Keep those searches conservative.
            None
        } else if !request.case_sensitive {
            Some(format!("(?i:{candidate_base})"))
        } else {
            Some(candidate_base.to_owned())
        };
        Ok(Self {
            regex,
            candidate_pattern,
            candidate_literal: word_mode,
            raw,
            fold_word,
            ascii_word,
        })
    }

    pub(super) fn matching_fields(
        &self,
        document: &Value,
        rules: &TextRules,
        selected: Option<&BTreeSet<&str>>,
        include_indices: bool,
    ) -> TextMatches {
        let mut fields = BTreeSet::new();
        let mut matched_paths = Vec::new();
        visit_json_text(
            document,
            rules,
            include_indices,
            &mut |name, indices, value, format| {
                if selected.is_some_and(|selected| !selected.contains(name)) {
                    return;
                }
                if self.matches_value(value, format) {
                    fields.insert(name.to_owned());
                    if include_indices {
                        matched_paths.push(indexed_path(name, indices));
                    }
                }
            },
        );
        TextMatches {
            fields: fields.into_iter().collect(),
            matched_paths,
        }
    }

    pub(super) fn matches_malformed(&self, bytes: &[u8]) -> bool {
        let text = crate::text::malformed_search_text(bytes);
        let text = if self.raw {
            text
        } else {
            render_text(&text, crate::TextFormat::Plain).expect("plain text renders")
        };
        if self.fold_word {
            self.regex.is_match(&text.case_fold().collect::<String>())
        } else {
            self.regex.is_match(&text)
        }
    }

    /// Normal postings retain scalar values but do not normalize whitespace.
    /// Broad or whitespace-sensitive patterns therefore scan all file slots.
    pub(super) fn regular_candidate_pattern(&self) -> Option<&str> {
        let pattern = self.candidate_pattern.as_deref()?;
        if pattern.chars().any(char::is_whitespace)
            || [r"\s", r"\n", r"\r", r"\t", r"\x", r"\u"]
                .iter()
                .any(|escape| pattern.contains(escape))
        {
            None
        } else {
            Some(pattern)
        }
    }
}
