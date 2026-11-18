//! Ordered traversal metadata, independently from predicate membership.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrderCapability {
    None,
    Stored,
    Candidate,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct OrderedPlan {
    pub(crate) capability: OrderCapability,
    pub(crate) early_termination: bool,
    pub(crate) needs_source: bool,
}
