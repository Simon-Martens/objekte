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
        && text[start..].chars().next().is_some_and(|ch| matches!(ch, '\'' | '"'));
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
                target: CompletionTarget::Value {
                    field: field.to_owned(),
                },
            });
        }
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
            if bracket == 0 && paren == 0
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
    text[end..].chars().next().is_none_or(char::is_whitespace).then_some(end)
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

    #[test]
    fn quoted_nested_path_replaces_one_complete_token() {
        let text = "foo[\"ba\"] = 1";
        let at = text.find("ba").unwrap() + 2;
        let context = context(text, at).unwrap();
        assert_eq!(context.replace, 0..9);
        assert_eq!(context.prefix, "foo[\"ba");
        assert_eq!(context.target, CompletionTarget::Field { sort: false });
        assert_eq!(
            insert(text, context.replace, "foo[\"bar\"]").unwrap().0,
            "foo[\"bar\"] = 1"
        );
    }

    #[test]
    fn offers_fields_only_where_a_field_can_begin() {
        for text in [
            "name ", "name:lower ", "name = 'Ada' ", "name = 'Ada' SORT ",
            "!", "name = 'Ada' !", "name = 'Ada' (", "name = 'Ada' && !",
        ] {
            assert!(context(text, text.len()).is_none(), "{text}");
        }
        for text in [
            "", "(", "! (", "name = 'Ada' && ", "name = 'Ada' || (",
            "name = 'Ada' && !( ",
        ] {
            assert_eq!(
                context(text, text.len()).unwrap().target,
                CompletionTarget::Field { sort: false },
                "{text}"
            );
        }
    }

    #[test]
    fn sort_completion_stops_after_a_field_or_direction() {
        for text in [
            "SORT BY ", "SORT  BY ", "name = 'Ada' SORT BY title, ",
            "SORT BY MIN(", "SORT BY MIN (", "SORT BY title, MAX (",
        ] {
            assert_eq!(
                context(text, text.len()).unwrap().target,
                CompletionTarget::Field { sort: true },
                "{text}"
            );
        }
        for text in ["SORT BY title ", "SORT BY title ASC", "SORT BY MIN(title) "] {
            assert!(context(text, text.len()).is_none(), "{text}");
        }
        let quoted = "name = 'SORT BY' && title";
        assert_eq!(
            context(quoted, quoted.len()).unwrap().target,
            CompletionTarget::Field { sort: false }
        );
    }

    #[test]
    fn single_quoted_values_and_spaced_nested_paths_keep_their_token() {
        let value = "name ~ 'A%'";
        let caret = value.find('%').unwrap() + 1;
        let found = context(value, caret).unwrap();
        assert_eq!(found.replace, 7..11);
        assert_eq!(found.prefix, "A%");
        assert_eq!(found.target, CompletionTarget::Value { field: "name".into() });

        let field = "foo['a b'] = 1";
        let caret = field.find('b').unwrap() + 1;
        let found = context(field, caret).unwrap();
        assert_eq!(found.replace, 0..10);
        assert_eq!(found.target, CompletionTarget::Field { sort: false });

        let nested_value = "name = 'x' && foo['a b'] ~ 'Al";
        let found = context(nested_value, nested_value.len()).unwrap();
        assert_eq!(
            found.target,
            CompletionTarget::Value {
                field: "foo['a b']".into()
            }
        );
    }

    #[test]
    fn standalone_and_finished_strings_do_not_offer_fields() {
        assert!(context("\"Alpha\"", 1).is_none());
        assert!(context("name = 'Ada'", "name = 'Ada'".len()).is_none());
        assert!(context("SORT BY 'x", "SORT BY 'x".len()).is_none());
    }

    #[test]
    fn metadata_paths_complete_in_filters_and_sorts() {
        for prefix in ["@record.", "@file.", "@collection."] {
            for before in ["", "year > 0 && ", "SORT BY "] {
                let text = format!("{before}{prefix}");
                let found = context(&text, text.len()).unwrap();
                assert_eq!(found.prefix, prefix);
                assert_eq!(
                    found.target,
                    CompletionTarget::Field { sort: before == "SORT BY " }
                );
                assert_eq!(found.replace, before.len()..text.len());
            }
        }
        let text = "@file.size >= 02GiB";
        let found = context(text, text.len()).unwrap();
        assert_eq!(
            found.target,
            CompletionTarget::Value { field: "@file.size".into() }
        );
        assert_eq!(found.prefix, "02GiB");
    }
}
