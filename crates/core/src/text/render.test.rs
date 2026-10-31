use super::*;

#[test]
fn markdown_removes_formatting_and_backslash_escapes() {
    assert_eq!(
        render_text(
            "**He**lp\n\nA [link](https://example.com) and \\*star\\*",
            TextFormat::Markdown
        ),
        Some("Help A link and *star*".into())
    );
}

#[test]
fn html_removes_tags_metadata_and_decodes_entities() {
    assert_eq!(
        render_text(
            "<p><strong>He</strong>lp &amp; world</p><script>hidden</script><p>Next</p>",
            TextFormat::Html
        ),
        Some("Help & world Next".into())
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
        Some("Help & world Next".into())
    );
}

#[test]
fn all_unicode_whitespace_is_collapsed() {
    assert_eq!(
        render_text(" \tHe\r\n\nllo\u{2003} world  ", TextFormat::Plain),
        Some("He llo world".into())
    );
}
