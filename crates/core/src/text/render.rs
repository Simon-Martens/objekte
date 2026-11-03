//! Visible-text extraction and normalization for declared formats.
//!
//! Normalized full-text search reads every string the same way: Unicode
//! punctuation becomes a space, and runs of whitespace collapse to one
//! space, trimmed at both ends. `Goethe, Johann` and `Goethe,Johann` both
//! read `Goethe Johann`.

use crate::TextFormat;
use html5gum::{Token, Tokenizer};
use pulldown_cmark::{Event as MarkdownEvent, Parser as MarkdownParser, Tag, TagEnd};
use quick_xml::{escape::unescape, events::Event as XmlEvent, reader::Reader};

pub(crate) fn render_text(source: &str, format: TextFormat) -> Option<String> {
    let visible = match format {
        TextFormat::Plain => return Some(normalize_whitespace(source)),
        TextFormat::Markdown => markdown_text(source)?,
        TextFormat::Html => html_text(source)?,
        TextFormat::Xml => xml_text(source)?,
    };
    Some(normalize_whitespace(&visible))
}

/// Append one independently normalized leaf, retaining the destination's capacity.
pub(crate) fn render_text_into(source: &str, format: TextFormat, output: &mut String) -> bool {
    if format == TextFormat::Plain {
        normalize_whitespace_into(source, output);
        return true;
    }
    if let Some(rendered) = render_text(source, format) {
        output.push_str(&rendered);
        true
    } else {
        false
    }
}

fn normalize_whitespace(source: &str) -> String {
    let mut output = String::with_capacity(source.len());
    normalize_whitespace_into(source, &mut output);
    output
}

/// Whether a character is Unicode punctuation (general category `P*`:
/// connectors, dashes, brackets, quotes, and other marks). Symbols such as
/// `$`, `+`, `€`, or `©` are not punctuation.
pub(crate) fn is_punctuation(ch: char) -> bool {
    use icu_properties::{
        CodePointMapData,
        props::{GeneralCategory, GeneralCategoryGroup},
    };
    if ch.is_ascii() {
        // The ASCII members of `P*`, without a table lookup.
        return matches!(
            ch,
            '!' | '"'
                | '#'
                | '%'
                | '&'
                | '\''
                | '('
                | ')'
                | '*'
                | ','
                | '-'
                | '.'
                | '/'
                | ':'
                | ';'
                | '?'
                | '@'
                | '['
                | '\\'
                | ']'
                | '_'
                | '{'
                | '}'
        );
    }
    GeneralCategoryGroup::Punctuation.contains(CodePointMapData::<GeneralCategory>::new().get(ch))
}

/// Whether normalizing would change `text`: it has whitespace to collapse
/// or punctuation to drop.
pub(crate) fn needs_normalizing(text: &str) -> bool {
    text.chars()
        .any(|ch| ch.is_whitespace() || is_punctuation(ch))
}

fn normalize_whitespace_into(source: &str, output: &mut String) {
    let start = output.len();
    let mut pending_space = false;
    for ch in source.chars() {
        if ch.is_whitespace() || is_punctuation(ch) {
            pending_space = output.len() > start;
        } else {
            if pending_space {
                output.push(' ');
                pending_space = false;
            }
            output.push(ch);
        }
    }
}

fn boundary(output: &mut String) {
    if !output.ends_with(' ') && !output.is_empty() {
        output.push(' ');
    }
}

fn block_tag(name: &[u8]) -> bool {
    [
        b"p".as_slice(),
        b"div",
        b"br",
        b"li",
        b"ul",
        b"ol",
        b"hr",
        b"section",
        b"article",
        b"header",
        b"footer",
        b"blockquote",
        b"pre",
        b"tr",
        b"td",
        b"th",
        b"table",
        b"h1",
        b"h2",
        b"h3",
        b"h4",
        b"h5",
        b"h6",
    ]
    .iter()
    .any(|tag| name.eq_ignore_ascii_case(tag))
}

fn html_text(source: &str) -> Option<String> {
    let mut output = String::new();
    let mut hidden_tag: Option<Vec<u8>> = None;
    for token in Tokenizer::new(source) {
        match token.ok()? {
            Token::StartTag(tag) => {
                if [b"script".as_slice(), b"style", b"head"]
                    .iter()
                    .any(|name| tag.name.eq_ignore_ascii_case(name))
                    && hidden_tag.is_none()
                {
                    hidden_tag = Some(tag.name.to_vec());
                } else if block_tag(&tag.name) && hidden_tag.is_none() {
                    boundary(&mut output);
                }
            }
            Token::EndTag(tag) => {
                if hidden_tag
                    .as_ref()
                    .is_some_and(|name| tag.name.eq_ignore_ascii_case(name))
                {
                    hidden_tag = None;
                } else if block_tag(&tag.name) && hidden_tag.is_none() {
                    boundary(&mut output);
                }
            }
            Token::String(value) if hidden_tag.is_none() => {
                output.push_str(&String::from_utf8_lossy(&value));
            }
            _ => {}
        }
    }
    Some(output)
}

fn markdown_text(source: &str) -> Option<String> {
    let mut output = String::new();
    for event in MarkdownParser::new(source) {
        match event {
            MarkdownEvent::Text(text)
            | MarkdownEvent::Code(text)
            | MarkdownEvent::InlineMath(text)
            | MarkdownEvent::DisplayMath(text)
            | MarkdownEvent::FootnoteReference(text) => output.push_str(&text),
            MarkdownEvent::Html(html) | MarkdownEvent::InlineHtml(html) => {
                output.push_str(&html_text(&html)?);
            }
            MarkdownEvent::SoftBreak | MarkdownEvent::HardBreak | MarkdownEvent::Rule => {
                boundary(&mut output);
            }
            MarkdownEvent::Start(Tag::Paragraph | Tag::Heading { .. } | Tag::Item)
            | MarkdownEvent::End(TagEnd::Paragraph | TagEnd::Heading(_) | TagEnd::Item) => {
                boundary(&mut output);
            }
            _ => {}
        }
    }
    Some(output)
}

fn xml_text(source: &str) -> Option<String> {
    let mut reader = Reader::from_str(source);
    let mut output = String::new();
    loop {
        match reader.read_event().ok()? {
            XmlEvent::Start(tag) | XmlEvent::Empty(tag) => {
                if block_tag(tag.name().as_ref()) {
                    boundary(&mut output);
                }
            }
            XmlEvent::End(tag) => {
                if block_tag(tag.name().as_ref()) {
                    boundary(&mut output);
                }
            }
            XmlEvent::Text(text) => {
                let decoded = text.xml10_content().ok()?;
                output.push_str(&unescape(&decoded).ok()?);
            }
            XmlEvent::GeneralRef(reference) => {
                let entity = format!("&{};", reference.xml10_content().ok()?);
                output.push_str(&unescape(&entity).ok()?);
            }
            XmlEvent::CData(text) => output.push_str(&text.decode().ok()?),
            XmlEvent::Eof => break,
            _ => {}
        }
    }
    Some(output)
}

#[cfg(test)]
#[path = "render.test.rs"]
mod tests;
