//! Table header edits of the authoritative structured expression.

use objekte_search_core::query::{parse_structured_filter, structured_sort_start};

/// Change the top-level ordering clause without rewriting predicate text.
/// Manual multi-key ordering becomes a single ascending key on a header click.
pub fn cycle_header_sort(source: &str, column: &str) -> Result<String, String> {
    let parsed = if source.trim().is_empty() {
        None
    } else {
        Some(parse_structured_filter(source)?)
    };
    let start = if parsed.is_some() {
        structured_sort_start(source)?
    } else {
        None
    };
    let base = source[..start.unwrap_or(source.len())].trim_end();
    let next_direction = match parsed.as_ref().and_then(|parsed| parsed.sort.as_deref()) {
        Some([field]) if field.field == column && !field.descending => Some("DESC"),
        Some([field]) if field.field == column && field.descending => None,
        _ => Some("ASC"),
    };
    let next = match next_direction {
        Some(direction) if base.is_empty() => format!("SORT BY {column} {direction}"),
        Some(direction) => format!("{base} SORT BY {column} {direction}"),
        None => base.to_owned(),
    };
    if !next.is_empty() {
        parse_structured_filter(&next)?;
    }
    Ok(next)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycles_sort_and_preserves_quoted_words() {
        let source = "title = 'SORT BY year' && year >= 1900";
        let asc = cycle_header_sort(source, "year").unwrap();
        assert_eq!(asc, format!("{source} SORT BY year ASC"));
        let desc = cycle_header_sort(&asc, "year").unwrap();
        assert_eq!(desc, format!("{source} SORT BY year DESC"));
        assert_eq!(cycle_header_sort(&desc, "year").unwrap(), source);
        assert_eq!(
            cycle_header_sort(&asc, "title").unwrap(),
            format!("{source} SORT BY title ASC")
        );
    }

    #[test]
    fn empty_and_multi_key_sort_have_one_source() {
        assert_eq!(cycle_header_sort("", "year").unwrap(), "SORT BY year ASC");
        assert_eq!(
            cycle_header_sort("SORT BY year DESC, title ASC", "year").unwrap(),
            "SORT BY year ASC"
        );
        assert!(cycle_header_sort("year >= ", "year").is_err());
    }
}
