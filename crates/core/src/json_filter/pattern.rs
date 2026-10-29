//! LIKE-style string patterns with ASCII case folding.
// PocketBase parameter wrapping and LIKE-style matching (ASCII case folding).


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
}


impl Like {
    pub fn new(pattern: &str, field: bool) -> Self {
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
                    tokens.push(Token::Char('\\'));
                } else if let Some(c) = chars.next() {
                    tokens.push(Token::Char(c.to_ascii_lowercase()));
                } else {
                    valid = false;
                }
            } else {
                tokens.push(match c {
                    '%' if !literal => Token::Any,
                    '_' if !literal => Token::One,
                    _ => Token::Char(c.to_ascii_lowercase()),
                });
            }
        }
        if literal || field {
            tokens.push(Token::Any);
        }
        Self { tokens, valid }
    }

    pub fn matches(&self, text: &str) -> bool {
        if !self.valid {
            return false;
        }
        // Iterative glob matching; bounded stack even for adversarial patterns.
        let chars: Vec<_> = text.chars().map(|c| c.to_ascii_lowercase()).collect();
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
