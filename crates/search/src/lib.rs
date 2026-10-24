//! The search matcher shared by the editor library and browser validation.
use regex::{Regex, RegexBuilder};
use serde::{Deserialize, Serialize};

pub const MAX_PATTERN_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchOptions {
    pub regex: bool,
    pub case_sensitive: bool,
}

#[derive(Clone, Debug)]
pub struct SearchMatcher(Option<Regex>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct MatchRange {
    /// UTF-8 byte offsets into the normalized field, never Markdown or HTML.
    pub start: usize,
    pub end: usize,
}

impl SearchMatcher {
    pub fn new(query: &str, options: SearchOptions) -> Result<Self, String> {
        if query.len() > MAX_PATTERN_BYTES {
            return Err(format!("Search pattern exceeds {MAX_PATTERN_BYTES} bytes"));
        }
        let query = if options.regex { query } else { query.trim() };
        if query.is_empty() {
            return Ok(Self(None));
        }
        let pattern = if options.regex {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        RegexBuilder::new(&pattern)
            .case_insensitive(!options.case_sensitive)
            .size_limit(4 * 1024 * 1024)
            .dfa_size_limit(2 * 1024 * 1024)
            .nest_limit(128)
            .build()
            .map(|regex| Self(Some(regex)))
            .map_err(|error| error.to_string())
    }

    pub fn is_active(&self) -> bool {
        self.0.is_some()
    }

    pub fn is_match(&self, text: &str) -> bool {
        self.0.as_ref().is_none_or(|regex| regex.is_match(text))
    }

    /// At most 64 matches, only requested for the visible page. Zero-width
    /// matches select records but cannot highlight characters.
    pub fn ranges(&self, text: &str) -> Vec<MatchRange> {
        self.0
            .as_ref()
            .map(|regex| {
                regex
                    .find_iter(text)
                    .take(64)
                    .filter(|m| !m.is_empty())
                    .map(|m| MatchRange {
                        start: m.start(),
                        end: m.end(),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

// A small buffer ABI keeps browser validation on this exact compiler without a
// second regex parser or a JavaScript regex dialect. The JS adapter owns buffers.
#[cfg(target_arch = "wasm32")]
mod browser {
    use super::*;

    #[unsafe(no_mangle)]
    pub extern "C" fn search_alloc(len: usize) -> *mut u8 {
        Box::into_raw(vec![0u8; len].into_boxed_slice()) as *mut u8
    }

    /// `ptr` must be an allocation returned by search_alloc with this length.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn search_free(ptr: *mut u8, len: usize) {
        unsafe {
            drop(Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr, len)));
        }
    }

    /// Compile the input prefix, then replace the buffer with an error message.
    /// Returns its byte length, or zero when valid. The caller supplies the buffer.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn search_validate(
        ptr: *mut u8,
        len: usize,
        capacity: usize,
        case_sensitive: bool,
    ) -> usize {
        let buffer = unsafe { std::slice::from_raw_parts_mut(ptr, capacity) };
        let error = std::str::from_utf8(&buffer[..len.min(capacity)])
            .map_err(|e| e.to_string())
            .and_then(|query| {
                SearchMatcher::new(
                    query,
                    SearchOptions {
                        regex: true,
                        case_sensitive,
                    },
                )
            })
            .err()
            .unwrap_or_default();
        let mut end = error.len().min(capacity);
        while !error.is_char_boundary(end) {
            end -= 1;
        }
        buffer[..end].copy_from_slice(&error.as_bytes()[..end]);
        end
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literals_regex_unicode_and_limits() {
        let literal = SearchMatcher::new("a.b", SearchOptions::default()).unwrap();
        assert!(literal.is_match("A.B"));
        assert!(!literal.is_match("axb"));
        let options = SearchOptions {
            regex: true,
            case_sensitive: false,
        };
        let regex = SearchMatcher::new(r"(?m)^über\s+\p{Greek}+$", options).unwrap();
        assert!(regex.is_match("ÜBER Σς"));
        assert_eq!(regex.ranges("ÜBER Σς")[0], MatchRange { start: 0, end: 10 });
        for invalid in ["[", "(?=a)", r"(a)\1"] {
            assert!(SearchMatcher::new(invalid, options).is_err());
        }
        assert!(SearchMatcher::new(&"x".repeat(MAX_PATTERN_BYTES + 1), options).is_err());
        assert!(!SearchMatcher::new("^ a $", options).unwrap().is_match("a"));
        assert!(
            SearchMatcher::new("^ a $", options)
                .unwrap()
                .is_match(" a ")
        );
        let empty = SearchMatcher::new("^", options).unwrap();
        assert!(empty.is_match("hello"));
        assert!(empty.ranges("hello").is_empty());
    }
}
