//! Resident estimates for the native component families.

use crate::lifecycle::IndexFamilyEstimate;

#[cfg(test)]
thread_local! {
    static INDEX_FAMILY_ESTIMATE_CALLS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn index_family_estimate_calls() -> usize {
    INDEX_FAMILY_ESTIMATE_CALLS.with(std::cell::Cell::get)
}

#[cfg(test)]
pub(crate) fn reset_index_family_estimate_calls() {
    INDEX_FAMILY_ESTIMATE_CALLS.with(|calls| calls.set(0));
}

/// Cached narrow index-family total for one native contribution. It counts
/// gram/posting bitmap payloads and sort keys only, not segment metadata.
pub(crate) fn index_family_total(
    raw: &crate::index::bigram::BigramIndex,
    fallback: &crate::index::bigram::BigramIndex,
    custom: &std::collections::HashMap<(Vec<String>, String), crate::index::bigram::BigramIndex>,
    postings: &crate::index::posting::PostingCollection,
    sorting: Option<&crate::index::sorting::SortCollection>,
) -> usize {
    index_family_estimate(raw, fallback, custom, postings, sorting).total()
}

/// Native retained-memory estimate for one published contribution. It counts
/// posting and gram bitmap payloads plus estimated sort keys. It is not an
/// allocator measurement or a strict peak bound; it orders maintenance.
pub(crate) fn index_family_estimate(
    raw: &crate::index::bigram::BigramIndex,
    fallback: &crate::index::bigram::BigramIndex,
    custom: &std::collections::HashMap<(Vec<String>, String), crate::index::bigram::BigramIndex>,
    postings: &crate::index::posting::PostingCollection,
    sorting: Option<&crate::index::sorting::SortCollection>,
) -> IndexFamilyEstimate {
    #[cfg(test)]
    INDEX_FAMILY_ESTIMATE_CALLS.with(|calls| calls.set(calls.get() + 1));
    IndexFamilyEstimate {
        raw_bytes: raw.estimated_heap_bytes(),
        fallback_bytes: fallback.estimated_heap_bytes(),
        custom_text_bytes: custom
            .values()
            .map(|index| index.estimated_heap_bytes())
            .sum(),
        postings_bytes: postings
            .fields
            .values()
            .map(|posting| {
                posting
                    .values
                    .values()
                    .map(|bitmap| bitmap.serialized_size() as usize)
                    .sum::<usize>()
                    + posting
                        .lengths
                        .values()
                        .map(|bitmap| bitmap.serialized_size() as usize)
                        .sum::<usize>()
            })
            .sum(),
        sorting_bytes: sorting.map_or(0, |sorting| {
            sorting
                .columns
                .values()
                .map(|column| {
                    column
                        .keys
                        .iter()
                        .map(|key| match key {
                            crate::index::sorting::SortKey::String(value) => value.len(),
                            _ => 24,
                        })
                        .sum::<usize>()
                })
                .sum::<usize>()
                .saturating_add(sorting.placed_bytes())
        }),
    }
}
