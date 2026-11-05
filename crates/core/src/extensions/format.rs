//! Borrowed logical records and format decoding.

use super::*;
use std::borrow::Cow;


/// A recoverable syntax diagnostic for the original source bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatDiagnostic {
    pub code: String,
    pub message: String,
    pub span: Option<crate::SourceSpan>,
}


impl FormatDiagnostic {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            span: None,
        }
    }
}


/// Successful logical decoding or a source syntax failure. Operational and
/// callback failures remain `Err` and cannot become a malformed source.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeOutcome {
    Decoded,
    Malformed(Vec<FormatDiagnostic>),
}


/// Borrowed logical scalar delivered during one decoder callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarValue<'a> {
    Null,
    Bool(bool),
    Number(&'a str),
    Text(&'a str),
}


/// A logical JSON-compatible node used only when an owning API needs a tree.
/// Decoders with empty objects, arrays or numeric object keys should visit
/// parent containers before children. Array members must arrive in index
/// order without gaps. Ordinary leaf-only decoders can keep the default.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogicalNode<'a> {
    Object,
    Array,
    Scalar(ScalarValue<'a>),
}


/// A decoded record lends both path presence and logical leaves for one call.
/// Implementations may borrow the source bytes or own a format-specific tree.
pub trait DecodedRecord: RecordAccess {
    /// Visit scalar leaves at JSON-pointer paths. Borrowed values may not be
    /// retained after the callback returns.
    fn visit_leaves(
        &self,
        visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
    ) -> Result<()>;

    fn visit_nodes(
        &self,
        visit: &mut dyn FnMut(&str, LogicalNode<'_>) -> Result<()>,
    ) -> Result<()> {
        self.visit_leaves(&mut |path, value| visit(path, LogicalNode::Scalar(value)))
    }
}


/// Decode one source and lend its logical record only during the callback.
pub trait FormatDecoder: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn accepts(&self, source: &SourceDescriptor) -> bool;
    fn decode(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
    ) -> Result<()>;

    /// Opt in to recoverable syntax failures. The default keeps legacy
    /// decoder errors unchanged. Syntax must be established before `visit`.
    fn decode_outcome(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
    ) -> Result<DecodeOutcome> {
        self.decode(bytes, visit)?;
        Ok(DecodeOutcome::Decoded)
    }

    /// Original-source text to search only after confirmed malformed input.
    /// Binary formats return `None` by default. Borrow when possible.
    fn malformed_text<'a>(&self, _bytes: &'a [u8]) -> Result<Option<Cow<'a, str>>> {
        Ok(None)
    }

    fn visit_leaves(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
    ) -> Result<()> {
        self.decode(bytes, &mut |record| record.visit_leaves(visit))
    }
}


/// Enforce the one-record and callback-order contract at every core call site.
/// The first callback error wins even if a decoder tries to hide it.
pub(crate) fn decode_checked(
    decoder: &dyn FormatDecoder,
    bytes: &[u8],
    visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
) -> Result<DecodeOutcome> {
    let mut count = 0_usize;
    let mut callback_error = None;
    let outcome = decoder.decode_outcome(bytes, &mut |record| {
        if callback_error.is_some() {
            return Err(Error::Request("decoder callback already failed".into()));
        }
        count += 1;
        let result = if count > 1 {
            Err(Error::Request(
                "format decoder emitted multiple records".into(),
            ))
        } else {
            visit(record)
        };
        if let Err(error) = result {
            callback_error = Some(error);
            return Err(Error::Request("decoder callback failed".into()));
        }
        Ok(())
    });
    if let Some(error) = callback_error {
        return Err(error);
    }
    match outcome? {
        DecodeOutcome::Decoded if count == 1 => Ok(DecodeOutcome::Decoded),
        DecodeOutcome::Decoded => Err(Error::Request("format decoder emitted no record".into())),
        DecodeOutcome::Malformed(mut diagnostics) if count == 0 => {
            if diagnostics.is_empty() {
                diagnostics.push(FormatDiagnostic::new(
                    "format_syntax",
                    "source syntax is malformed",
                ));
            }
            for diagnostic in &diagnostics {
                if diagnostic.code.is_empty() || diagnostic.message.is_empty() {
                    return Err(Error::Request(
                        "format diagnostic has no code or message".into(),
                    ));
                }
                if diagnostic
                    .span
                    .is_some_and(|span| span.start > span.end || span.end > bytes.len())
                {
                    return Err(Error::Request(
                        "format diagnostic span is outside source bytes".into(),
                    ));
                }
            }
            Ok(DecodeOutcome::Malformed(diagnostics))
        }
        DecodeOutcome::Malformed(_) => Err(Error::Request(
            "format decoder reported malformed content after emitting a record".into(),
        )),
    }
}
