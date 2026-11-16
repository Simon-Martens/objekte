use objekte_search_core::lifecycle::{
    CompactionMode, CompactionOptions, CompactionReport, Index, IndexError,
};

#[allow(dead_code)]
fn public_compaction_surface(
    index: &Index,
) -> Result<(CompactionReport, CompactionReport), IndexError> {
    let bounded = index.compact()?;
    let full = index.compact_with_options(CompactionOptions {
        mode: CompactionMode::Full,
        merge_small_segments: true,
    })?;
    Ok((bounded, full))
}

#[test]
fn public_compaction_options_construct_with_documented_defaults() {
    let defaults = CompactionOptions::default();
    assert_eq!(defaults.mode, CompactionMode::Bounded);
    assert!(!defaults.merge_small_segments);
}
