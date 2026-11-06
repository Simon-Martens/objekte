//! Bounded, query-scoped regular-expression compilation.

use std::sync::Arc;

use regex::{Regex, RegexBuilder};

use crate::query::{CaseSensitivity, RegexOptions};

const MAX_PATTERN_BYTES: usize = 4 * 1024;
const MAX_NESTING: u32 = 128;
const COMPILED_SIZE_LIMIT: usize = 8 * 1024 * 1024;
const DFA_SIZE_LIMIT: usize = 2 * 1024 * 1024;


/// A compiled Boolean matcher shared by the selected text representations.
#[derive(Clone, Debug)]
pub(crate) struct CompiledRegex {
    matcher: Arc<Regex>,
}


impl CompiledRegex {
    pub fn compile(
        pattern: &str,
        options: RegexOptions,
        case_sensitivity: CaseSensitivity,
    ) -> Result<Self, String> {
        if pattern.len() > MAX_PATTERN_BYTES {
            return Err(format!(
                "regex pattern exceeds {MAX_PATTERN_BYTES} byte limit"
            ));
        }
        let matcher = RegexBuilder::new(pattern)
            .unicode(true)
            .case_insensitive(case_sensitivity == CaseSensitivity::Insensitive)
            .multi_line(options.multi_line)
            .dot_matches_new_line(options.dot_matches_new_line)
            .nest_limit(MAX_NESTING)
            .size_limit(COMPILED_SIZE_LIMIT)
            .dfa_size_limit(DFA_SIZE_LIMIT)
            .build()
            .map_err(|error| format!("invalid regex: {error}"))?;
        Ok(Self {
            matcher: Arc::new(matcher),
        })
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.matcher.is_match(text)
    }
}


#[cfg(test)]
#[path = "regex_query.test.rs"]
mod tests;
