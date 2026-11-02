//! Project normal and rendered values from one parsed source document.

use super::*;
use crate::text::{render_text, visit_sonic_text};
use sonic_rs::Value as SonicValue;
use unicode_casefold::UnicodeCaseFold;

pub(in crate::file_picker) fn project_text(
    value: &SonicValue,
    rules: &TextRules,
    indexed_fields: &[String],
    paths: &[crate::JsonPath],
) -> TextContent {
    let mut global_clean = String::new();
    let mut fields = (0..paths.len())
        .map(|_| (String::new(), String::new()))
        .collect::<Vec<_>>();
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
                fields[index].0.push_str(value);
                fields[index].0.push('\u{1f}');
                if let Some(rendered) = render_text(value, format) {
                    fields[index].1.push_str(&rendered);
                    fields[index].1.push('\u{1f}');
                    global_clean.push_str(&rendered);
                    global_clean.push('\u{1f}');
                }
            } else if let Some(rendered) = render_text(value, format) {
                global_clean.push_str(&rendered);
                global_clean.push('\u{1f}');
            }
        },
    );
    append_folded(&mut global_clean);
    for (normal, clean) in &mut fields {
        append_folded(normal);
        append_folded(clean);
    }
    TextContent {
        global_clean: global_clean.into_bytes(),
        fields: fields
            .into_iter()
            .map(|(normal, clean)| FieldTextContent {
                normal: normal.into_bytes(),
                clean: clean.into_bytes(),
            })
            .collect(),
    }
}

/// Malformed files have no trustworthy field paths or markup declarations.
pub(in crate::file_picker) fn project_malformed_text(
    text: &str,
    field_count: usize,
) -> TextContent {
    let mut global_clean = render_text(text, crate::TextFormat::Plain).expect("plain text renders");
    append_folded(&mut global_clean);
    TextContent {
        global_clean: global_clean.into_bytes(),
        fields: (0..field_count)
            .map(|_| FieldTextContent {
                normal: Vec::new(),
                clean: Vec::new(),
            })
            .collect(),
    }
}

fn append_folded(text: &mut String) {
    let folded = text.case_fold().collect::<String>();
    if folded != *text {
        text.push('\u{1e}');
        text.push_str(&folded);
    }
}
