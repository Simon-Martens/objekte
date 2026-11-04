//! Compile full-text patterns and identify matching document paths.

use super::*;
use crate::{
    TextMode,
    text::{TextPattern, visit_json_text},
};


/// A request's compiled pattern and conservative bigram prefilter. Whether
/// text matches is decided by the shared [`TextPattern`].
pub(super) struct TextMatcher {
    pattern: TextPattern,
    pub(super) candidate_pattern: Option<String>,
    pub(super) candidate_literal: bool,
    pub(super) raw: bool,
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
    pub(super) fn matches_value(&self, value: &str, format: crate::TextFormat) -> bool {
        self.pattern.matches(value, format)
    }

    pub(super) fn compile(request: &TextQuery) -> Result<Self> {
        let pattern = TextPattern::compile(&request.pattern, request.mode, request.case_sensitive)?;
        let raw = pattern.mode() != TextMode::Word;
        let word_mode = pattern.mode() != TextMode::Regex;
        // Word modes narrow by the needle as the values are indexed; a regex
        // is planned as given, with its own case flags.
        let candidate_base = pattern.needle().unwrap_or(pattern.source());
        let candidate_pattern = Some(candidate_base.to_owned());
        Ok(Self {
            candidate_pattern,
            candidate_literal: word_mode,
            raw,
            pattern,
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
        self.pattern.matches(&text, crate::TextFormat::Plain)
    }

    /// Normal postings retain scalar values but do not normalize whitespace.
    /// Broad or whitespace-sensitive patterns therefore scan all file slots.
    pub(super) fn regular_candidate_pattern(&self) -> Option<&str> {
        let pattern = self.candidate_pattern.as_deref()?;
        // A normalized word pattern has no punctuation and single spaces.
        // Each of its words appears unchanged in the source text, so the
        // raw index can narrow by the longest one; verification checks the
        // whole phrase.
        if self.candidate_literal && !self.raw {
            return pattern
                .split(' ')
                .max_by_key(|word| word.len())
                .filter(|word| !word.is_empty());
        }
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
