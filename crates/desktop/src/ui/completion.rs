//! Caret-local completion parsing. An invalid whole expression can still have
//! a well-defined field or literal at the caret.

use std::ops::Range;


#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CompletionTarget {
    Field { sort: bool },
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
        } else if ch == '"' {
            quoted_start = quoted_start.map_or(Some(at), |_| None);
        }
    }
    let (start, end, prefix) = if let Some(quote) = quoted_start {
        let end = text[caret..]
            .find('"')
            .map_or(text.len(), |offset| caret + offset + 1);
        (quote, end, text[quote + 1..caret].to_owned())
    } else {
        let start = text[..caret]
            .char_indices()
            .rev()
            .find(|(_, ch)| token_boundary(*ch))
            .map_or(0, |(at, ch)| at + ch.len_utf8());
        let end = text[caret..]
            .char_indices()
            .find(|(_, ch)| token_boundary(*ch))
            .map_or(text.len(), |(offset, _)| caret + offset);
        (start, end, text[start..caret].to_owned())
    };
    let before = text[..start].trim_end();
    let upper = before.to_ascii_uppercase();
    let sort = upper.rfind("SORT BY").is_some_and(|at| {
        // The sort expression is the final clause; preceding literal words
        // cannot become the completion context for an earlier predicate.
        !before[at + 7..].contains(['&', '|', '(', ')'])
    });
    if sort {
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
        let field = before[..before.len() - operator.len()]
            .trim_end()
            .rsplit(|ch: char| ch.is_whitespace() || matches!(ch, '(' | ')' | '&' | '|'))
            .next()
            .unwrap_or("");
        if !field.is_empty() {
            return Some(CompletionContext {
                replace: start..end,
                prefix,
                target: CompletionTarget::Value {
                    field: field.to_owned(),
                },
            });
        }
    }
    Some(CompletionContext {
        replace: start..end,
        prefix,
        target: CompletionTarget::Field { sort: false },
    })
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
    use super::*;

    #[test]
    fn detects_partial_field_and_value_without_valid_whole_expression() {
        assert_eq!(
            context("na >=", 2).unwrap().target,
            CompletionTarget::Field { sort: false }
        );
        assert_eq!(
            context("year >= 19", 10).unwrap().target,
            CompletionTarget::Value {
                field: "year".into()
            }
        );
        assert_eq!(context("year >= \"Hei", 12).unwrap().replace, 8..12);
    }

    #[test]
    fn insertion_keeps_surrounding_expression_and_unicode_offsets() {
        let text = "name = \"Häi\" && age > 1";
        let position = text.find("Häi").unwrap() + "Hä".len();
        let completion = context(text, position).unwrap();
        assert_eq!(
            completion.target,
            CompletionTarget::Value {
                field: "name".into()
            }
        );
        let (result, _) = insert(text, completion.replace, "\"Heidelberg\"").unwrap();
        assert_eq!(result, "name = \"Heidelberg\" && age > 1");
    }
}
