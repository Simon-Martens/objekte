//! Project normal and rendered values from one parsed source document.

use super::*;
use crate::text::{render_text_into, visit_sonic_index_text};
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
    debug_assert_eq!(content.fields.len(), paths.len());
    content.clear();
    let global_clean = &mut content.global_clean;
    let fields = &mut content.fields;
    visit_sonic_index_text(value, rules, &mut |field, top_level_name, value, format| {
        if top_level_name.is_some_and(|name| {
            indexed_fields
                .binary_search_by(|field| field.as_str().cmp(name))
                .is_ok()
                && field.is_none()
        }) {
            return;
        }
        if let Some(index) = field {
            fields[index].normal.push_str(value);
            fields[index].normal.push('\u{1f}');
            let clean = &mut fields[index].clean;
            if render_text_into(value, format, clean) {
                clean.push('\u{1f}');
            }
        } else if render_text_into(value, format, global_clean) {
            global_clean.push('\u{1f}');
        }
    });
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
    use super::*;
    use crate::{Field, JsonPathStep, ValueType, text::visit_sonic_text};
    use std::collections::BTreeMap;

    #[test]
    fn ascii_uses_builder_normalization_and_unicode_keeps_folded_projection() {
        let mut ascii = String::from("AbC");
        append_folded(&mut ascii);
        assert_eq!(ascii, "AbC");

        let mut unicode = String::from("Straße");
        append_folded(&mut unicode);
        assert_eq!(unicode, "Straße\u{1e}strasse");
    }

    fn field(kind: ValueType, format: Option<crate::TextFormat>) -> Field {
        Field {
            kind,
            nullable: true,
            index: false,
            language: None,
            sort_index: false,
            sort_reduce: None,
            text_format: format,
            text_search: None,
            exclude_fts: false,
            enum_values: None,
            suggested_values: None,
            properties: BTreeMap::new(),
            items: None,
            relation: None,
        }
    }

    fn legacy_projection(
        value: &SonicValue,
        rules: &TextRules,
        indexed_fields: &[String],
        paths: &[crate::JsonPath],
    ) -> TextContent {
        let mut content = TextContent::new(paths.len());
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
                let field = paths
                    .binary_search(steps)
                    .ok()
                    .or_else(|| fallback.and_then(|path| paths.binary_search(path).ok()));
                if let Some(index) = field {
                    content.fields[index].normal.push_str(value);
                    content.fields[index].normal.push('\u{1f}');
                    let clean = &mut content.fields[index].clean;
                    if render_text_into(value, format, clean) {
                        clean.push('\u{1f}');
                    }
                } else if render_text_into(value, format, &mut content.global_clean) {
                    content.global_clean.push('\u{1f}');
                }
            },
        );
        append_folded(&mut content.global_clean);
        for field in &mut content.fields {
            append_folded(&mut field.normal);
            append_folded(&mut field.clean);
        }
        content
    }

    #[test]
    fn compiled_fields_match_path_routing_for_nested_and_invalid_values() {
        let mut schema = BTreeMap::new();
        schema.insert(
            "title".into(),
            field(ValueType::String, Some(crate::TextFormat::Plain)),
        );
        schema.insert(
            "md".into(),
            field(ValueType::String, Some(crate::TextFormat::Markdown)),
        );
        schema.insert("rank".into(), field(ValueType::Integer, None));
        let mut nested = field(ValueType::Object, None);
        nested.properties.insert(
            "body".into(),
            field(ValueType::String, Some(crate::TextFormat::Html)),
        );
        schema.insert("nested".into(), nested);
        let mut item = field(ValueType::Object, None);
        item.properties.insert(
            "note".into(),
            field(ValueType::String, Some(crate::TextFormat::Xml)),
        );
        let mut items = field(ValueType::Array, None);
        items.items = Some(Box::new(item));
        schema.insert("items".into(), items);
        let mut excluded = field(ValueType::String, Some(crate::TextFormat::Plain));
        excluded.exclude_fts = true;
        schema.insert("hidden".into(), excluded);
        let mut rules = TextRules::new(&schema);
        let mut paths = rules.registered_paths();
        paths.sort();
        paths.dedup();
        assert!(paths.iter().any(|path| path.contains(&JsonPathStep::Each)));
        rules.compile_index_fields(&paths);
        let indexed = vec!["rank".to_owned()];
        let documents = [
            r#"{"title":"Straße","md":"**Äpfel**","rank":12,"nested":{"body":"<p>niño</p>"},"items":[{"note":"<p>Łódź</p>"},{"note":"<p>中文</p>"}],"hidden":"secret","unknown":"Привет"}"#,
            r#"{"title":{"unexpected":["Müller",{"deep":"Straße"}]},"rank":5,"md":"plain"}"#,
            r#"{"title":["A","Ä"],"nested":{"body":{"deep":"日本語"}},"items":[{"note":["مرحبا","😀"]}]}"#,
            r#"{"title":"discarded","title":"retained","md":"**first**","md":"*last*"}"#,
            r#"{"title":null,"rank":0,"items":[{"note":false}]}"#,
        ];
        for document in documents {
            let parsed = sonic_rs::from_str::<SonicValue>(document).unwrap();
            let expected = legacy_projection(&parsed, &rules, &indexed, &paths);
            let actual = project_text(&parsed, &rules, &indexed, &paths);
            assert_eq!(actual.global_clean, expected.global_clean, "{document}");
            for (actual, expected) in actual.fields.iter().zip(&expected.fields) {
                assert_eq!(actual.normal, expected.normal, "{document}");
                assert_eq!(actual.clean, expected.clean, "{document}");
            }
        }
    }
}
