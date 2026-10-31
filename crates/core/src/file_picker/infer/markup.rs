//! Conservative format votes from recognizable markup tokens.

use crate::TextFormat;
use html5gum::{Token, Tokenizer};
use pulldown_cmark::{Event as MarkdownEvent, Parser as MarkdownParser, Tag};
use quick_xml::{events::Event as XmlEvent, reader::Reader};

const MAX_SAMPLES: usize = 512;
const MAX_SAMPLE_BYTES: usize = 64 * 1024;


/// Bounded, per-field evidence for a non-plain text format.
#[derive(Default)]
pub(super) struct MarkupVotes {
    sampled: usize,
    html: usize,
    xml: usize,
    markdown: usize,
}


impl MarkupVotes {
    pub(super) fn observe(&mut self, value: &str) {
        if value.is_empty() || self.sampled == MAX_SAMPLES {
            return;
        }
        self.sampled += 1;
        if value.len() > MAX_SAMPLE_BYTES {
            return;
        }
        match classify(value) {
            Some(TextFormat::Html) => self.html += 1,
            Some(TextFormat::Xml) => self.xml += 1,
            Some(TextFormat::Markdown) => self.markdown += 1,
            Some(TextFormat::Plain) | None => {}
        }
    }

    pub(super) fn format(&self) -> Option<TextFormat> {
        let mut votes = [
            (self.html, TextFormat::Html),
            (self.xml, TextFormat::Xml),
            (self.markdown, TextFormat::Markdown),
        ];
        votes.sort_by_key(|(count, _)| *count);
        let (best, format) = votes[2];
        (self.sampled >= 3 && best >= 3 && best * 5 >= self.sampled && best >= 2 * votes[1].0)
            .then_some(format)
    }
}


fn classify(value: &str) -> Option<TextFormat> {
    if value.contains('<') && value.contains('>') {
        if (value.trim_start().starts_with("<?xml") || value.contains("xmlns")) && valid_xml(value)
        {
            return Some(TextFormat::Xml);
        }
        if has_html_tag(value) {
            return Some(TextFormat::Html);
        }
        if valid_xml(value) {
            return Some(TextFormat::Xml);
        }
    }
    has_markdown_structure(value).then_some(TextFormat::Markdown)
}

fn has_html_tag(value: &str) -> bool {
    for token in Tokenizer::new(value) {
        let token = token.expect("infallible HTML tokenizer");
        match token {
            Token::StartTag(tag)
                if [
                    b"p".as_slice(),
                    b"br",
                    b"div",
                    b"span",
                    b"a",
                    b"em",
                    b"strong",
                    b"ul",
                    b"ol",
                    b"li",
                    b"table",
                    b"tr",
                    b"td",
                    b"h1",
                    b"h2",
                    b"h3",
                    b"h4",
                    b"h5",
                    b"h6",
                    b"section",
                    b"article",
                    b"blockquote",
                ]
                .iter()
                .any(|name| tag.name.eq_ignore_ascii_case(name)) =>
            {
                return true;
            }
            Token::EndTag(tag)
                if [
                    b"p".as_slice(),
                    b"br",
                    b"div",
                    b"span",
                    b"a",
                    b"em",
                    b"strong",
                    b"ul",
                    b"ol",
                    b"li",
                    b"table",
                    b"tr",
                    b"td",
                    b"h1",
                    b"h2",
                    b"h3",
                    b"h4",
                    b"h5",
                    b"h6",
                    b"section",
                    b"article",
                    b"blockquote",
                ]
                .iter()
                .any(|name| tag.name.eq_ignore_ascii_case(name)) =>
            {
                return true;
            }
            Token::Doctype(_) => return true,
            _ => {}
        }
    }
    false
}

fn valid_xml(value: &str) -> bool {
    let mut reader = Reader::from_str(value);
    let mut depth = 0usize;
    let mut tags = 0usize;
    loop {
        match reader.read_event() {
            Ok(XmlEvent::Start(_)) => {
                depth += 1;
                tags += 1;
            }
            Ok(XmlEvent::Empty(_)) => tags += 1,
            Ok(XmlEvent::End(_)) if depth > 0 => depth -= 1,
            Ok(XmlEvent::End(_)) => return false,
            Ok(XmlEvent::Eof) => return tags > 0 && depth == 0,
            Ok(_) => {}
            Err(_) => return false,
        }
    }
}

fn has_markdown_structure(value: &str) -> bool {
    if !value.contains(['#', '*', '_', '[', ']', '`', '-', '>']) {
        return false;
    }
    MarkdownParser::new(value).any(|event| {
        matches!(
            event,
            MarkdownEvent::Start(
                Tag::Heading { .. }
                    | Tag::List(_)
                    | Tag::Link { .. }
                    | Tag::CodeBlock(_)
                    | Tag::Strong
                    | Tag::Emphasis
                    | Tag::BlockQuote(_)
            )
        )
    })
}
