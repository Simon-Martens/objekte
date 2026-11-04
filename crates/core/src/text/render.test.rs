use super::*;

#[test]
fn markdown_removes_formatting_and_backslash_escapes() {
    assert_eq!(
        render_text(
            "**He**lp\n\nA [link](https://example.com) and \\*star\\*",
            TextFormat::Markdown
        ),
        Some("Help A link and star".into())
    );
}

#[test]
fn html_removes_tags_metadata_and_decodes_entities() {
    assert_eq!(
        render_text(
            "<p><strong>He</strong>lp &amp; world</p><script>hidden</script><p>Next</p>",
            TextFormat::Html
        ),
        Some("Help world Next".into())
    );
    assert_eq!(
        render_text(
            "<head><style>hidden</style>secret</head><p>Visible</p>",
            TextFormat::Html
        ),
        Some("Visible".into())
    );
}

#[test]
fn xml_removes_tags_and_decodes_entities_and_cdata() {
    assert_eq!(
        render_text(
            "<root><p><b>He</b>lp &amp; world</p><p><![CDATA[Next]]></p></root>",
            TextFormat::Xml
        ),
        Some("Help world Next".into())
    );
}

#[test]
fn all_unicode_whitespace_is_collapsed() {
    assert_eq!(
        render_text(" \tHe\r\n\nllo\u{2003} world  ", TextFormat::Plain),
        Some("He llo world".into())
    );
}

#[test]
fn plain_text_large_input_keeps_exact_normalization() {
    let source = format!(
        "  {}\u{2003}{}  ",
        "Abc ".repeat(20_000),
        "Straße".repeat(5_000)
    );
    let rendered = render_text(&source, TextFormat::Plain).unwrap();
    assert!(rendered.starts_with("Abc Abc"));
    assert!(rendered.contains("Abc Straße"));
    assert!(rendered.ends_with("Straße"));
    assert!(!rendered.contains("  "));
    assert_eq!(render_text("", TextFormat::Plain), Some(String::new()));
}

#[test]
fn rendering_into_reused_buffer_preserves_each_leaf_boundary() {
    let mut output = String::from("previous");
    for source in ["  A  b  ", "\tÄpfel\u{2003}Straße  ", " ", "C"] {
        let expected = render_text(source, TextFormat::Plain).unwrap();
        let start = output.len();
        assert!(render_text_into(source, TextFormat::Plain, &mut output));
        assert_eq!(&output[start..], expected);
    }
}

#[test]
fn unicode_punctuation_becomes_one_space_and_symbols_stay() {
    assert_eq!(
        render_text("„Goethe“ – Werke…, Bd. 1/2 (¿Qué?)", TextFormat::Plain),
        Some("Goethe Werke Bd 1 2 Qué".into())
    );
    assert_eq!(
        render_text("Goethe,Johann  W.", TextFormat::Plain),
        Some("Goethe Johann W".into())
    );
    assert_eq!(
        render_text("€ 5 + 3 = 8 © $ ~ |", TextFormat::Plain),
        Some("€ 5 + 3 = 8 © $ ~ |".into())
    );
    assert_eq!(
        render_text("... !!", TextFormat::Plain),
        Some(String::new())
    );
    assert!(!needs_normalizing("Goethe"));
    assert!(needs_normalizing("Goethe,"));
    assert!(needs_normalizing("a b"));
}

#[test]
fn ascii_punctuation_shortcut_agrees_with_the_unicode_category() {
    use icu_properties::{
        CodePointMapData,
        props::{GeneralCategory, GeneralCategoryGroup},
    };
    for byte in 0u8..128 {
        let ch = char::from(byte);
        let unicode = GeneralCategoryGroup::Punctuation
            .contains(CodePointMapData::<GeneralCategory>::new().get(ch));
        assert_eq!(is_punctuation(ch), unicode, "{ch:?}");
    }
}
