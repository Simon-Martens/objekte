//! Colours for the filter line: fields, operators, values, keywords, and
//! the position a parse error points at.

use super::theme::*;
use ratatui::style::{Modifier, Style};

const KEYWORDS: [&str; 9] = [
    "SORT", "BY", "ASC", "DESC", "exists", "missing", "true", "false", "null",
];

/// One style per character of `text`.
pub(super) fn filter_styles(text: &str) -> Vec<Style> {
    let chars = text.chars().collect::<Vec<_>>();
    let mut styles = vec![Style::new().fg(TEXT); chars.len()];
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index];
        let start = index;
        let style = if ch == '\'' || ch == '"' {
            // A string runs to the matching unescaped quote.
            index += 1;
            while index < chars.len() && chars[index] != ch {
                index += if chars[index] == '\\' { 2 } else { 1 };
            }
            index = (index + 1).min(chars.len());
            Style::new().fg(GREEN)
        } else if matches!(ch, '=' | '!' | '<' | '>' | '~' | '?') {
            while index < chars.len() && matches!(chars[index], '=' | '!' | '<' | '>' | '~' | '?') {
                index += 1;
            }
            Style::new().fg(VIOLET)
        } else if ch == '&' || ch == '|' || ch == '(' || ch == ')' {
            index += 1;
            Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
        } else if ch.is_ascii_digit()
            || (ch == '-' && chars.get(index + 1).is_some_and(char::is_ascii_digit))
        {
            index += 1;
            while index < chars.len()
                && (chars[index].is_ascii_alphanumeric()
                    || matches!(chars[index], '.' | ':' | '-' | '+'))
            {
                index += 1;
            }
            Style::new().fg(AMBER)
        } else if ch == '@' || ch.is_alphabetic() || ch == '_' {
            while index < chars.len()
                && (chars[index].is_alphanumeric()
                    || matches!(chars[index], '_' | '@' | '.' | '[' | ']' | '*' | ':'))
            {
                index += 1;
            }
            let word = chars[start..index].iter().collect::<String>();
            if KEYWORDS.contains(&word.as_str())
                || KEYWORDS.iter().any(|keyword| {
                    keyword.eq_ignore_ascii_case(&word) && keyword.chars().all(char::is_uppercase)
                })
            {
                Style::new().fg(ACCENT).add_modifier(Modifier::BOLD)
            } else if word.starts_with('@') {
                Style::new().fg(TEAL)
            } else {
                Style::new().fg(TEXT)
            }
        } else {
            index += 1;
            Style::new().fg(TEXT)
        };
        for slot in &mut styles[start..index.min(chars.len())] {
            *slot = style;
        }
    }
    styles
}

/// The character an error message points at, from `… at byte N: …`.
#[cfg(test)]
pub(super) fn error_position(text: &str, error: &str) -> Option<usize> {
    let rest = &error[error.find("at byte ")? + "at byte ".len()..];
    let digits = rest
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    let byte = digits.parse::<usize>().ok()?.min(text.len());
    Some(
        text.char_indices()
            .take_while(|(index, _)| *index < byte)
            .count(),
    )
}

#[cfg(test)]
#[path = "syntax.test.rs"]
mod tests;
