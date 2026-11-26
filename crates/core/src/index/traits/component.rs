//! Logical index-component descriptors.

use std::borrow::Cow;

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
    pub path: Option<Cow<'a, [String]>>,
    pub binding: Option<Cow<'a, str>>,
}

/// A cheap sealed estimate of resident memory. A backend can identify a shared
/// resident allocation explicitly when several reader handles refer to it.
#[derive(Clone, Copy, Default)]
pub(crate) struct ResidentComponentBytes {
    pub(crate) bytes: usize,
    pub(crate) shared_allocation: Option<usize>,
}

#[derive(Default)]
pub(crate) struct ResidentComponentCounter(std::collections::HashSet<usize>);

impl ResidentComponentCounter {
    pub(crate) fn charge<T: ?Sized>(
        &mut self,
        reader: &std::sync::Arc<T>,
        estimate: ResidentComponentBytes,
    ) -> usize {
        if estimate.bytes == 0 {
            return 0;
        }
        let key = estimate
            .shared_allocation
            .unwrap_or_else(|| std::sync::Arc::as_ptr(reader).cast::<()>().addr());
        if self.0.insert(key) {
            estimate.bytes
        } else {
            0
        }
    }
}
