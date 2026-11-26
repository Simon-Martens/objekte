//! Caret-local completion parsing. An invalid whole expression can still have
//! a well-defined field or literal at the caret.

use std::ops::Range;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionTarget {
    Field { sort: bool },
    Modifier,
    BooleanValue,
    Value { field: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompletionContext {
    pub replace: Range<usize>,
    pub prefix: String,
    pub target: CompletionTarget,
}

pub fn context(text: &str, caret: usize) -> Option<CompletionContext> {
    if caret > text.len() || !text.is_char_boundary(caret) {
        return None;
    }
    let mut quoted_start = None;
    let mut escaped = false;
    for (at, ch) in text[..caret].char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' && quoted_start.is_some() {
            escaped = true;
        } else if quoted_start.is_some_and(|(_, quote)| quote == ch) {
            quoted_start = None;
        } else if quoted_start.is_none() && matches!(ch, '\'' | '"') {
            quoted_start = Some((at, ch));
        }
    }
    let quoted_path = quoted_start.is_some_and(|(quote, _)| text[..quote].ends_with('['));
    let start = token_start(text, caret);
    let end = token_end(text, caret);
    let (start, prefix) = if let Some((quote, _)) = quoted_start.filter(|_| !quoted_path) {
        (quote, text[quote + 1..caret].to_owned())
    } else {
        (start, text[start..caret].to_owned())
    };
    let before = text[..start].trim_end();
    let scalar_string = !quoted_path
        && text[start..]
            .chars()
            .next()
            .is_some_and(|ch| matches!(ch, '\'' | '"'));
    if scalar_string && quoted_start.is_none() && caret == end {
        return None;
    }
    if let Some(sort_start) = sort_clause_start(before) {
        if scalar_string {
            return None;
        }
        let clause = before[sort_start..].trim_end();
        if !sort_field_position(clause) {
            return None;
        }
        return Some(CompletionContext {
            replace: start..end,
            prefix,
            target: CompletionTarget::Field { sort: true },
        });
    }
    let operator = [
        "?>=", "?<=", "?!~", "?!=", ">=", "<=", "!=", "?=", "?>", "?<", "?~", "=", ">", "<", "~",
    ]
    .into_iter()
    .find(|operator| before.ends_with(operator));
    if let Some(operator) = operator {
        let operand = before[..before.len() - operator.len()].trim_end();
        let field = &operand[token_start(operand, operand.len())..];
        if !field.is_empty() {
            return Some(CompletionContext {
                replace: start..end,
                prefix,
                target: if matches!(operator, "=" | "!=") && boolean_modifier(field) {
                    CompletionTarget::BooleanValue
                } else {
                    CompletionTarget::Value {
                        field: field.to_owned(),
                    }
                },
            });
        }
    }
    if !scalar_string
        && !quoted_path
        && filter_field_position(before)
        && let Some(colon) = modifier_colon(&text[start..caret])
    {
        let modifier_start = start + colon + 1;
        return Some(CompletionContext {
            replace: modifier_start..end,
            prefix: text[modifier_start..caret].to_owned(),
            target: CompletionTarget::Modifier,
        });
    }
    if filter_field_position(before) {
        if scalar_string {
            return None;
        }
        return Some(CompletionContext {
            replace: start..end,
            prefix,
            target: CompletionTarget::Field { sort: false },
        });
    }
    None
}

fn boolean_modifier(field: &str) -> bool {
    [
        ":?", ":string", ":number", ":array", ":null", ":object", ":exists",
    ]
    .iter()
    .any(|suffix| field.ends_with(suffix))
}

/// Find a modifier separator, excluding colons inside quoted path segments.
fn modifier_colon(token: &str) -> Option<usize> {
    let mut bracket = 0usize;
    let mut quote = None;
    let mut escaped = false;
    let mut found = None;
    for (index, ch) in token.char_indices() {
        if escaped {
            escaped = false;
        } else if quote.is_some() && ch == '\\' {
            escaped = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() {
            match ch {
                '\'' | '"' if bracket > 0 => quote = Some(ch),
                '[' => bracket += 1,
                ']' => bracket = bracket.saturating_sub(1),
                ':' if bracket == 0 && index > 0 => found = Some(index),
                _ => {}
            }
        }
    }
    found
}

fn filter_field_position(mut before: &str) -> bool {
    loop {
        if before.is_empty() || before.ends_with("&&") || before.ends_with("||") {
            return true;
        }
        let Some(parent) = before.strip_suffix('(') else {
            return false;
        };
        let parent = parent.trim_end();
        before = parent.strip_suffix('!').unwrap_or(parent).trim_end();
    }
}

fn sort_field_position(clause: &str) -> bool {
    if clause.is_empty() || clause.ends_with(',') {
        return true;
    }
    let Some(reducer) = clause.strip_suffix('(') else {
        return false;
    };
    let reducer = reducer.trim_end();
    let name = reducer
        .rsplit_once(',')
        .map_or(reducer, |(_, name)| name)
        .trim_start();
    ["MIN", "MAX", "FIRST", "LAST"]
        .iter()
        .any(|candidate| name.eq_ignore_ascii_case(candidate))
}

fn token_start(text: &str, caret: usize) -> usize {
    let mut start = 0;
    let mut bracket: usize = 0;
    let mut quote = None;
    let mut escaped = false;
    for (at, ch) in text[..caret].char_indices() {
        if escaped {
            escaped = false;
        } else if quote.is_some_and(|current| current == ch) {
            quote = None;
        } else if ch == '\\' && quote.is_some() {
            escaped = true;
        } else if quote.is_none() {
            match ch {
                '\'' | '"' => quote = Some(ch),
                '[' => bracket += 1,
                ']' => bracket = bracket.saturating_sub(1),
                _ if bracket == 0 && token_boundary(ch) => start = at + ch.len_utf8(),
                _ => {}
            }
        }
    }
    start
}

fn token_end(text: &str, caret: usize) -> usize {
    let mut bracket: usize = 0;
    let mut quote = None;
    let mut escaped = false;
    for ch in text[..caret].chars() {
        if escaped {
            escaped = false;
        } else if quote.is_some_and(|current| current == ch) {
            quote = None;
        } else if ch == '\\' && quote.is_some() {
            escaped = true;
        } else if quote.is_none() {
            match ch {
                '\'' | '"' => quote = Some(ch),
                '[' => bracket += 1,
                ']' => bracket = bracket.saturating_sub(1),
                _ => {}
            }
        }
    }
    for (offset, ch) in text[caret..].char_indices() {
        if escaped {
            escaped = false;
        } else if quote.is_some_and(|current| current == ch) {
            quote = None;
        } else if ch == '\\' && quote.is_some() {
            escaped = true;
        } else if quote.is_none() {
            match ch {
                '\'' | '"' => quote = Some(ch),
                '[' => bracket += 1,
                ']' => bracket = bracket.saturating_sub(1),
                _ if bracket == 0 && token_boundary(ch) => return caret + offset,
                _ => {}
            }
        }
    }
    text.len()
}

fn sort_clause_start(text: &str) -> Option<usize> {
    let mut bracket: usize = 0;
    let mut paren: usize = 0;
    let mut quote = None;
    let mut escaped = false;
    let mut sort = None;
    for (at, ch) in text.char_indices() {
        if escaped {
            escaped = false;
        } else if quote.is_some_and(|current| current == ch) {
            quote = None;
        } else if ch == '\\' && quote.is_some() {
            escaped = true;
        } else if quote.is_none() {
            if bracket == 0
                && paren == 0
                && (at == 0 || text[..at].chars().last().is_some_and(char::is_whitespace))
                && let Some(end) = sort_by_end(text, at)
            {
                sort = Some(end);
            }
            match ch {
                '\'' | '"' => quote = Some(ch),
                '[' => bracket += 1,
                ']' => bracket = bracket.saturating_sub(1),
                '(' if bracket == 0 => paren += 1,
                ')' if bracket == 0 => paren = paren.saturating_sub(1),
                _ => {}
            }
        }
    }
    sort
}

fn sort_by_end(text: &str, at: usize) -> Option<usize> {
    let keyword = text[at..].get(..4)?;
    if !keyword.eq_ignore_ascii_case("SORT") {
        return None;
    }
    let after_sort = &text[at + 4..];
    let spaces = after_sort.len() - after_sort.trim_start_matches(char::is_whitespace).len();
    if spaces == 0 || !text[at + 4 + spaces..].get(..2)?.eq_ignore_ascii_case("BY") {
        return None;
    }
    let end = at + 6 + spaces;
    text[end..]
        .chars()
        .next()
        .is_none_or(char::is_whitespace)
        .then_some(end)
}

fn token_boundary(ch: char) -> bool {
    ch.is_whitespace()
        || matches!(
            ch,
            '(' | ')' | '&' | '|' | ',' | '=' | '!' | '<' | '>' | '~'
        )
}

pub fn insert(text: &str, replace: Range<usize>, completion: &str) -> Option<(String, usize)> {
    if replace.start > replace.end
        || replace.end > text.len()
        || !text.is_char_boundary(replace.start)
        || !text.is_char_boundary(replace.end)
    {
        return None;
    }
    let mut result = String::with_capacity(text.len() - replace.len() + completion.len());
    result.push_str(&text[..replace.start]);
    result.push_str(completion);
    let caret = result.len();
    result.push_str(&text[replace.end..]);
    Some((result, caret))
}

#[cfg(test)]
mod tests {
    super::super::tests::completion_tests!();
}
