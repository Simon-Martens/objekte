//! Parse field-query and standalone `SORT BY` clauses.

use super::*;
use std::collections::BTreeSet;

/// Split a trailing `SORT BY` clause from a field filter or UI input.
pub fn split_sort_clause(input: &str) -> Result<(String, Vec<Sort>)> {
    let mut quote = None;
    let mut escaped = false;
    let mut candidate = None;
    for (at, ch) in input.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            continue;
        }
        if input[at..]
            .get(..7)
            .is_some_and(|token| token.eq_ignore_ascii_case("SORT BY"))
            && (at == 0 || input[..at].chars().last().is_some_and(char::is_whitespace))
            && input[at + 7..]
                .chars()
                .next()
                .is_none_or(char::is_whitespace)
        {
            candidate = Some(at);
        }
    }
    let Some(at) = candidate else {
        return Ok((input.to_owned(), Vec::new()));
    };
    let before = input[..at].trim_end();
    let clause = input[at + 7..].trim();
    Ok((before.to_owned(), parse_sort_terms(clause)?))
}

/// Parse sort terms supplied separately from a query, such as a CLI `--sort` value.
pub fn parse_sort_terms(clause: &str) -> Result<Vec<Sort>> {
    if clause.is_empty() {
        return Err(Error::Request("SORT BY needs at least one field".into()));
    }
    let mut boundaries = vec![0];
    let mut quote = None;
    let mut escaped = false;
    for (at, ch) in clause.char_indices() {
        if escaped {
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if let Some(delimiter) = quote {
            if ch == delimiter {
                quote = None;
            }
        } else if ch == '\'' || ch == '"' {
            quote = Some(ch);
        } else if clause[at..].starts_with("&&") {
            boundaries.push(at + 2);
        }
    }
    let mut seen = BTreeSet::new();
    let mut terms = Vec::new();
    for (index, &start) in boundaries.iter().enumerate() {
        let end = boundaries
            .get(index + 1)
            .map_or(clause.len(), |next| next - 2);
        let raw = &clause[start..end];
        let raw = raw.trim();
        if raw.is_empty() {
            return Err(Error::Request("empty SORT BY term".into()));
        }
        let (field, direction) = match raw.rsplit_once(char::is_whitespace) {
            Some((field, word)) if word.eq_ignore_ascii_case("asc") => {
                (field.trim_end(), Direction::Asc)
            }
            Some((field, word)) if word.eq_ignore_ascii_case("desc") => {
                (field.trim_end(), Direction::Desc)
            }
            _ => (raw, Direction::Asc),
        };
        if field != "@id" {
            JsonFilter::parse_path(field)?;
        }
        if !seen.insert(field) {
            return Err(Error::Request(format!("duplicate SORT BY field {field}")));
        }
        terms.push(Sort {
            field: field.into(),
            direction,
        });
    }
    Ok(terms)
}
