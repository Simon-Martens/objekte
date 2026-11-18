//! Explicit physical traversal operations selected before execution.

/// Whole-page relation routes. Per-term relation choices are represented by
/// provider offers; these operations coordinate a complete ordered page.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum RelationPageOperation {
    QualifiedSemijoin,
    DirectTargetPrune,
    ParallelFileScan,
    ReferenceScan,
}

/// Eligibility policy for whole-page relation execution. The caller still
/// performs the operation-specific checks because those depend on runtime
/// query shape, but the ordering and fallback are one shared policy.
pub(crate) fn relation_page_plan(
    prune_segments: bool,
    cache_admission: bool,
    bounded_parallel: bool,
) -> Vec<RelationPageOperation> {
    let mut operations = Vec::with_capacity(4);
    if prune_segments && !cache_admission {
        operations.push(RelationPageOperation::QualifiedSemijoin);
    }
    if prune_segments {
        operations.push(RelationPageOperation::DirectTargetPrune);
    }
    if prune_segments && bounded_parallel {
        operations.push(RelationPageOperation::ParallelFileScan);
    }
    operations.push(RelationPageOperation::ReferenceScan);
    operations
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relation_fallbacks_remain_ordered_and_always_end_in_a_complete_scan() {
        assert_eq!(
            relation_page_plan(true, false, true),
            vec![
                RelationPageOperation::QualifiedSemijoin,
                RelationPageOperation::DirectTargetPrune,
                RelationPageOperation::ParallelFileScan,
                RelationPageOperation::ReferenceScan,
            ]
        );
        assert_eq!(
            relation_page_plan(false, true, false),
            vec![RelationPageOperation::ReferenceScan]
        );
    }
}
