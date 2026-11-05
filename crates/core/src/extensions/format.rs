//! Borrowed logical records and format decoding.

use super::*;


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

    fn visit_leaves(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
    ) -> Result<()> {
        self.decode(bytes, &mut |record| record.visit_leaves(visit))
    }
}
