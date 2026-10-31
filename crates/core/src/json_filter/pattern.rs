//! LIKE-style string patterns with optional Unicode case folding.
// PocketBase parameter wrapping and LIKE-style matching.

use unicode_casefold::UnicodeCaseFold;


#[derive(Clone, Debug)]
enum Token {
    Any,
    One,
    Char(char),
}


// INFO: Compiled LIKE pattern tokens and validity state.
#[derive(Debug)]
pub(super) struct Like {
    tokens: Vec<Token>,
    valid: bool,
    ignore_case: bool,
}


impl Like {
    /// A case-sensitive LIKE prefix whose full match set an ordered string
    /// index can decide. Other patterns still use source verification.
    pub(super) fn anchored_prefix(pattern: &str) -> Option<String> {
        let mut chars = pattern.chars().peekable();
        let mut prefix = String::new();
        while let Some(ch) = chars.next() {
            match ch {
                '\\' => prefix.push(chars.next()?),
                '%' => return chars.all(|rest| rest == '%').then_some(prefix),
                '_' => return None,
                _ => prefix.push(ch),
            }
        }
        None
    }

    fn literal(tokens: &mut Vec<Token>, c: char, ignore_case: bool) {
        if ignore_case {
            tokens.extend(c.case_fold().map(Token::Char));
        } else {
            tokens.push(Token::Char(c));
        }
    }

    pub fn new(pattern: &str, field: bool, ignore_case: bool) -> Self {
        let mut escaped = false;
        let has_percent = pattern.chars().any(|c| {
            if escaped {
                escaped = false;
                false
            } else if c == '\\' {
                escaped = true;
                false
            } else {
                c == '%'
            }
        });
        let mut tokens = Vec::new();
        let mut chars = pattern.chars().peekable();
        let literal = !field && !has_percent;
        if literal || field {
            tokens.push(Token::Any);
        }
        let mut valid = true;
        while let Some(c) = chars.next() {
            if c == '\\' {
                if literal && !chars.peek().is_some_and(|c| matches!(c, '\\' | '%' | '_')) {
                    Self::literal(&mut tokens, '\\', ignore_case);
                } else if let Some(c) = chars.next() {
                    Self::literal(&mut tokens, c, ignore_case);
                } else {
                    valid = false;
                }
            } else {
                match c {
                    '%' if !literal => tokens.push(Token::Any),
                    '_' if !literal => tokens.push(Token::One),
                    _ => Self::literal(&mut tokens, c, ignore_case),
                }
            }
        }
        if literal || field {
            tokens.push(Token::Any);
        }
        Self {
            tokens,
            valid,
            ignore_case,
        }
    }

    pub fn matches(&self, text: &str) -> bool {
        if !self.valid {
            return false;
        }
        // Iterative glob matching; bounded stack even for adversarial patterns.
        let chars: Vec<_> = if self.ignore_case {
            text.case_fold().collect()
        } else {
            text.chars().collect()
        };
        let (mut i, mut p, mut star) = (0, 0, None);
        while i < chars.len() {
            match self.tokens.get(p) {
                Some(Token::Char(c)) if *c == chars[i] => {
                    i += 1;
                    p += 1;
                }
                Some(Token::One) => {
                    i += 1;
                    p += 1;
                }
                Some(Token::Any) => {
                    star = Some((p, i));
                    p += 1;
                }
                _ => {
                    if let Some((s, n)) = star {
                        i = n + 1;
                        p = s + 1;
                        star = Some((s, i));
                    } else {
                        return false;
                    }
                }
            }
        }
        while matches!(self.tokens.get(p), Some(Token::Any)) {
            p += 1;
        }
        p == self.tokens.len()
    }

    // A run of literal ASCII letters/digits is present in every matching string.
    // Wildcards and punctuation break runs; the source scanner handles JSON escapes.
    pub(super) fn required_ascii_runs(&self) -> Vec<String> {
        let (mut runs, mut current) = (Vec::new(), String::new());
        for token in &self.tokens {
            if let Token::Char(c) = token
                && c.is_ascii_alphanumeric()
            {
                current.push(*c);
            } else {
                if !current.is_empty() {
                    runs.push(std::mem::take(&mut current));
                }
            }
        }
        if !current.is_empty() {
            runs.push(current);
        }
        runs
    }
}


#[cfg(test)]
#[path = "pattern.test.rs"]
mod tests;
