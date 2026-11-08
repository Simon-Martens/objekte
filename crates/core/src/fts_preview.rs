//! Bounded source-text spans for presenting verified full-text evidence.
//! This uses the same literal case folding and regex compiler as execution.

use std::ops::Range;
use unicode_casefold::{Locale, UnicodeCaseFold, Variant};

use crate::bigram::fold;
use crate::query::{CaseSensitivity, FtsMode, FtsQuery};
use crate::regex_query::CompiledRegex;


pub struct PreviewMatcher {
    mode: PreviewMode,
}


enum PreviewMode {
    Literal { needle: String, sensitive: bool },
    Regex(CompiledRegex),
}


impl PreviewMatcher {
    pub fn new(query: &FtsQuery) -> Result<Self, String> {
        let mode = match query.mode {
            FtsMode::Literal => PreviewMode::Literal {
                needle: if query.case_sensitivity == CaseSensitivity::Sensitive {
                    query.literal.clone()
                } else {
                    fold(&query.literal)
                },
                sensitive: query.case_sensitivity == CaseSensitivity::Sensitive,
            },
            FtsMode::Regex(options) => PreviewMode::Regex(CompiledRegex::compile(
                &query.literal,
                options,
                query.case_sensitivity,
            )?),
        };
        Ok(Self { mode })
    }

    pub fn find(&self, text: &str) -> Option<Range<usize>> {
        match &self.mode {
            PreviewMode::Literal {
                needle,
                sensitive: true,
            } => text.find(needle).map(|start| start..start + needle.len()),
            PreviewMode::Literal {
                needle,
                sensitive: false,
            } => folded_span(text, needle),
            PreviewMode::Regex(regex) => regex.find(text),
        }
    }
}


fn folded_span(text: &str, needle: &str) -> Option<Range<usize>> {
    if needle.is_empty() {
        return None;
    }
    let mut folded = String::new();
    let mut mapping = Vec::new();
    for (start, ch) in text.char_indices() {
        let end = start + ch.len_utf8();
        let folded_start = folded.len();
        folded.extend(
            ch.to_string()
                .case_fold_with(Variant::Full, Locale::NonTurkic),
        );
        mapping.push((folded_start..folded.len(), start..end));
    }
    let at = folded.find(needle)?;
    let end = at + needle.len();
    let original_start = mapping.iter().find(|(folded, _)| folded.end > at)?.1.start;
    let original_end = mapping.iter().find(|(folded, _)| folded.end >= end)?.1.end;
    Some(original_start..original_end)
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::RegexOptions;

    fn query(literal: &str, case_sensitivity: CaseSensitivity, mode: FtsMode) -> FtsQuery {
        FtsQuery {
            literal: literal.into(),
            case_sensitivity,
            mode,
            pattern_parameter: None,
            fields: Vec::new(),
        }
    }

    #[test]
    fn spans_cover_unicode_casefold_expansions() {
        let matcher = PreviewMatcher::new(&query(
            "STRASSE",
            CaseSensitivity::Insensitive,
            FtsMode::Literal,
        ))
        .unwrap();
        assert_eq!(matcher.find("Die Straße endet"), Some(4..11));
    }

    #[test]
    fn regex_uses_query_compiler_and_returns_exact_span() {
        let matcher = PreviewMatcher::new(&query(
            "ab+c",
            CaseSensitivity::Insensitive,
            FtsMode::Regex(RegexOptions::default()),
        ))
        .unwrap();
        assert_eq!(matcher.find("xABBCy"), Some(1..5));
    }
}
