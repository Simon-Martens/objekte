// PocketBase parameter wrapping and SQLite LIKE matching (ASCII case folding).
#[derive(Clone, Debug)]
enum Token {
    Any,
    One,
    Char(char),
}
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
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn matching_agrees_with_sqlite_like_after_pocketbase_wrapping() {
        let db = rusqlite::Connection::open_in_memory().unwrap();
        for (input, field, sql_pattern) in [
            ("abc", false, "%abc%"),
            ("abc%", false, "abc%"),
            ("a_c", false, r"%a\_c%"),
            ("%a_c%", false, "%a_c%"),
            (r"\%", false, r"%\%%"),
            (r"\\", false, r"%\\%"),
            (r"a\b", false, r"%a\\b%"),
            (r"a\%", false, r"%a\%%"),
            (r"%a\_%", false, r"%a\_%"),
            (r"%a\", false, r"%a\"),
            ("abc%", true, "%abc%%"),
            ("a_c", true, "%a_c%"),
            ("", false, "%%"),
            ("%_%_%", false, "%_%_%"),
        ] {
            for text in [
                "", "abc", "ABC", "xxabc", "abcxy", "abcc", "a_c", "aXc", "a🐈c", "a%c", "a\\b",
                "a%b", "ÄBC", "äbc",
            ] {
                let expected: bool = db
                    .query_row(
                        "SELECT ?1 LIKE ?2 ESCAPE '\\'",
                        rusqlite::params![text, sql_pattern],
                        |r| r.get(0),
                    )
                    .unwrap();
                assert_eq!(
                    Like::new(input, field).matches(text),
                    expected,
                    "pattern={input:?} field={field} text={text:?}"
                );
            }
        }
    }
}
