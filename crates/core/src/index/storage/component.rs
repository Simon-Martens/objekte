//! Logical index-component descriptors.

use crate::model::CollectionId;

use super::segment::SegmentId;

/// Logical family of an indexed component.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComponentKind {
    DeclaredPosting,
    InferredPosting,
    CustomPosting,
    RawText,
    FallbackText,
    FieldText,
    Sorting,
    Relations,
}

/// Component identity within a pinned segment.
pub struct ComponentDescriptor<'a> {
    pub collection: CollectionId,
    pub segment: SegmentId,
    pub kind: ComponentKind,
    pub path: Option<&'a [String]>,
    pub binding: Option<&'a str>,
}
