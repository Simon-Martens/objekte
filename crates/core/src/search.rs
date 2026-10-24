//! One matching engine for collection search and its highlighted excerpts.
use crate::{Error, Result};
use grep_matcher::Matcher;
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use std::ops::Range;

pub struct SearchMatcher(Option<RegexMatcher>);

impl SearchMatcher {
    pub fn new(query: &str, regex: bool) -> Result<Self> {
        Self::with_case_sensitivity(query, regex, false)
    }

    /// Match case in literal mode; regex patterns control case with inline flags.
    pub fn with_case_sensitivity(query: &str, regex: bool, case_sensitive: bool) -> Result<Self> {
        // Preserve significant whitespace in patterns; retain the existing literal UX.
        let query = if regex { query } else { query.trim() };
        if query.is_empty() {
            return Ok(Self(None));
        }
        RegexMatcherBuilder::new()
            .fixed_strings(!regex)
            .case_insensitive(regex || !case_sensitive)
            .multi_line(true)
            // CLI defaults allow a 1 GiB DFA cache; bound each web search instead.
            .size_limit(10 * 1024 * 1024)
            .dfa_size_limit(8 * 1024 * 1024)
            .build(query)
            .map(|matcher| Self(Some(matcher)))
            .map_err(|error| Error::Data(format!("Invalid search pattern: {error}")))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_none()
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.0.as_ref().is_none_or(|matcher| {
            matcher
                .is_match(text.as_bytes())
                .expect("infallible regex matcher")
        })
    }

    /// UTF-8 byte ranges in the original text, shared by literal and regex highlighting.
    pub fn ranges(&self, text: &str, limit: usize) -> Vec<Range<usize>> {
        let mut ranges = Vec::new();
        if limit == 0 {
            return ranges;
        }
        if let Some(matcher) = &self.0 {
            matcher
                .find_iter(text.as_bytes(), |m| {
                    if m.start() != m.end()
                        && text.is_char_boundary(m.start())
                        && text.is_char_boundary(m.end())
                    {
                        ranges.push(m.start()..m.end());
                    }
                    ranges.len() < limit
                })
                .expect("infallible regex matcher");
        }
        ranges
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn case_sensitive_literals_preserve_unicode_offsets_and_regex_flags() {
        let text = "üBER · Über · ÜBER";
        let literal = SearchMatcher::with_case_sensitivity("Über", false, true).unwrap();
        assert!(literal.is_match(text));
        assert!(!literal.is_match("über"));
        let ranges = literal.ranges(text, 10);
        assert_eq!(ranges.len(), 1);
        assert_eq!(&text[ranges[0].clone()], "Über");
        let literal = SearchMatcher::with_case_sensitivity("B.*", false, true).unwrap();
        assert!(literal.is_match("B.*"));
        assert!(!literal.is_match("b.*"));
        assert!(!literal.is_match("Berlin"));
        // A remembered literal preference must not affect regex case semantics.
        for case_sensitive in [false, true] {
            assert!(
                SearchMatcher::with_case_sensitivity("über", true, case_sensitive)
                    .unwrap()
                    .is_match("Über")
            );
            assert!(
                !SearchMatcher::with_case_sensitivity("(?-i)über", true, case_sensitive)
                    .unwrap()
                    .is_match("Über")
            );
        }
    }

    #[test]
    fn literal_and_regex_modes_share_unicode_matching_and_offsets() {
        let literal = SearchMatcher::new("Ber.*", false).unwrap();
        assert!(!literal.is_match("Berlin"));
        assert!(literal.is_match("BER.*"));
        let regex = SearchMatcher::new(r"\b(?:berlin|über)\b", true).unwrap();
        let text = "α Über, BERLIN & <tags>";
        assert!(regex.is_match(text));
        assert_eq!(
            regex
                .ranges(text, 10)
                .iter()
                .map(|range| &text[range.clone()])
                .collect::<Vec<_>>(),
            ["Über", "BERLIN"]
        );
        assert_eq!(regex.ranges(text, 1).len(), 1);
        assert!(
            SearchMatcher::new("^second$", true)
                .unwrap()
                .is_match("first\nsecond\nthird")
        );
        assert!(
            !SearchMatcher::new(" Berlin ", true)
                .unwrap()
                .is_match("Berlin")
        );
        assert!(SearchMatcher::new("[", true).is_err());
        assert!(SearchMatcher::new("[", false).unwrap().is_match("["));
        assert!(SearchMatcher::new("(?=Berlin)", true).is_err());
        assert!(
            SearchMatcher::new("^", true)
                .unwrap()
                .ranges("Über", 10)
                .is_empty()
        );
    }
}
