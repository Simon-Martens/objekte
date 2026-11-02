//! Project normal and rendered values from one parsed source document.

use super::*;
use crate::text::{render_text_into, visit_sonic_text};
use sonic_rs::Value as SonicValue;
use unicode_casefold::UnicodeCaseFold;

pub(in crate::file_picker) fn project_text(
    value: &SonicValue,
    rules: &TextRules,
    indexed_fields: &[String],
    paths: &[crate::JsonPath],
) -> TextContent {
    let mut content = TextContent::new(paths.len());
    project_text_into(value, rules, indexed_fields, paths, &mut content);
    content
}

pub(in crate::file_picker) fn project_text_into(
    value: &SonicValue,
    rules: &TextRules,
    indexed_fields: &[String],
    paths: &[crate::JsonPath],
    content: &mut TextContent,
) {
    content.clear();
    let global_clean = &mut content.global_clean;
    let fields = &mut content.fields;
    visit_sonic_text(
        value,
        rules,
        &mut |path, steps, fallback, value, format, top_level| {
            if top_level
                && indexed_fields
                    .binary_search_by(|field| field.as_str().cmp(path))
                    .is_ok()
                && paths.binary_search(steps).is_err()
            {
                return;
            }
            // A malformed container in a declared scalar retains its
            // descendants in that field's candidate index.
            let field = paths
                .binary_search(steps)
                .ok()
                .or_else(|| fallback.and_then(|path| paths.binary_search(path).ok()));
            if let Some(index) = field {
                fields[index].normal.push_str(value);
                fields[index].normal.push('\u{1f}');
                let clean = &mut fields[index].clean;
                let start = clean.len();
                if render_text_into(value, format, clean) {
                    global_clean.push_str(&clean[start..]);
                    clean.push('\u{1f}');
                    global_clean.push('\u{1f}');
                }
            } else if render_text_into(value, format, global_clean) {
                global_clean.push('\u{1f}');
            }
        },
    );
    append_folded(global_clean);
    for field in fields {
        append_folded(&mut field.normal);
        append_folded(&mut field.clean);
    }
}

/// Malformed files have no trustworthy field paths or markup declarations.
pub(in crate::file_picker) fn project_malformed_text(
    text: &str,
    field_count: usize,
) -> TextContent {
    let mut content = TextContent::new(field_count);
    project_malformed_text_into(text, &mut content);
    content
}

pub(in crate::file_picker) fn project_malformed_text_into(text: &str, content: &mut TextContent) {
    content.clear();
    render_text_into(text, crate::TextFormat::Plain, &mut content.global_clean);
    append_folded(&mut content.global_clean);
}

fn append_folded(text: &mut String) {
    if text.is_ascii() {
        return;
    }
    let folded = text.case_fold().collect::<String>();
    if folded != *text {
        text.push('\u{1e}');
        text.push_str(&folded);
    }
}

#[cfg(test)]
mod tests {
    use super::append_folded;

    #[test]
    fn ascii_uses_builder_normalization_and_unicode_keeps_folded_projection() {
        let mut ascii = String::from("AbC");
        append_folded(&mut ascii);
        assert_eq!(ascii, "AbC");

        let mut unicode = String::from("Straße");
        append_folded(&mut unicode);
        assert_eq!(unicode, "Straße\u{1e}strasse");
    }
}
