pub(crate) const MAX_FOLD_SCRATCH_CAPACITY: usize = 256 * 1024;
const MAX_SHARED_FOLDS: usize = 64;

#[derive(Default)]
struct FoldValueCache {
    entries: Vec<FoldValueEntry>,
    active: usize,
    record_token: Option<(usize, usize)>,
}

struct FoldValueEntry {
    source_ptr: usize,
    source_len: usize,
    folded: String,
}

thread_local! {
    pub(crate) static FOLD_SCRATCH: RefCell<String> = const { RefCell::new(String::new()) };
    static FOLD_RECORD_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FOLD_SHARE_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static FOLD_VALUE_CACHE: RefCell<FoldValueCache> = RefCell::new(FoldValueCache::default());
    #[cfg(feature = "test-scan-instrumentation")]
    static FOLD_GROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    #[cfg(feature = "test-scan-instrumentation")]
    static FOLD_CANDIDATE_TRANSFORMS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    #[cfg(test)]
    static FOLD_VALUE_TRANSFORMS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    #[cfg(test)]
    static FOLD_VALUE_REUSES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Keep exceptional folding capacity until all segments of one record have
/// consumed it. Unwrapped callers retain the previous per-segment bound.
pub(crate) struct FoldRecordGuard {
    sharing: bool,
}

impl FoldRecordGuard {
    pub(crate) fn new() -> Self {
        Self::new_with_sharing(false)
    }

    pub(crate) fn new_with_sharing(sharing: bool) -> Self {
        FOLD_RECORD_DEPTH.with(|depth| depth.set(depth.get() + 1));
        if sharing {
            FOLD_SHARE_DEPTH.with(|depth| depth.set(depth.get() + 1));
        }
        Self { sharing }
    }
}

impl Drop for FoldRecordGuard {
    fn drop(&mut self) {
        if self.sharing {
            FOLD_SHARE_DEPTH.with(|depth| {
                let remaining = depth.get() - 1;
                depth.set(remaining);
                if remaining == 0 {
                    FOLD_VALUE_CACHE.with(|cache| {
                        let mut cache = cache.borrow_mut();
                        cache.active = 0;
                        cache.record_token = None;
                        let mut retained = 0usize;
                        cache.entries.retain(|entry| {
                            retained = retained.saturating_add(entry.folded.capacity());
                            retained <= MAX_FOLD_SCRATCH_CAPACITY
                        });
                    });
                }
            });
        }
        FOLD_RECORD_DEPTH.with(|depth| {
            let remaining = depth.get() - 1;
            depth.set(remaining);
            if remaining == 0 {
                trim_fold_scratch();
            }
        });
    }
}

fn trim_fold_scratch() {
    FOLD_SCRATCH.with(|scratch| {
        let mut buffer = scratch.borrow_mut();
        if buffer.capacity() > MAX_FOLD_SCRATCH_CAPACITY {
            *buffer = String::new();
        }
    });
}

#[cfg(feature = "test-scan-instrumentation")]
pub(crate) fn fold_scratch_stats() -> (usize, usize) {
    (
        FOLD_GROWS.with(std::cell::Cell::get),
        FOLD_SCRATCH.with(|scratch| scratch.borrow().capacity()),
    )
}

/// Reuse one worker thread's UTF-8 folding buffer without retaining large records.
pub(crate) fn with_folded_candidate_str<R>(text: &str, inspect: impl FnOnce(&str) -> R) -> R {
    FOLD_SCRATCH.with(|scratch| {
        let mut buffer = scratch.borrow_mut();
        #[cfg(feature = "test-scan-instrumentation")]
        let old_capacity = buffer.capacity();
        fold_into(text, &mut buffer);
        #[cfg(feature = "test-scan-instrumentation")]
        FOLD_CANDIDATE_TRANSFORMS.with(|count| count.set(count.get() + 1));
        #[cfg(feature = "test-scan-instrumentation")]
        if buffer.capacity() > old_capacity {
            FOLD_GROWS.with(|count| count.set(count.get() + 1));
        }
        let result = inspect(&buffer);
        if FOLD_RECORD_DEPTH.with(|depth| depth.get() == 0)
            && buffer.capacity() > MAX_FOLD_SCRATCH_CAPACITY
        {
            *buffer = String::new();
        }
        result
    })
}

pub(crate) fn with_folded_candidate<R>(text: &str, inspect: impl FnOnce(&[u8]) -> R) -> R {
    with_folded_candidate_str(text, |folded| inspect(folded.as_bytes()))
}

/// Borrow one record's immutable logical string by occurrence identity. The
/// guard clears active keys before the next revision can reuse an address;
/// raw streaming buffers never enter this cache.
pub(crate) fn with_folded_value_str<R>(
    record_token: (usize, usize),
    text: &str,
    inspect: impl FnOnce(&str) -> R,
) -> R {
    if !FOLD_SHARE_DEPTH.with(|depth| depth.get() > 0) || text.len() > MAX_FOLD_SCRATCH_CAPACITY / 4
    {
        return with_folded_candidate_str(text, inspect);
    }
    FOLD_VALUE_CACHE.with(|cache| {
        let mut cache = cache.borrow_mut();
        if cache.record_token != Some(record_token) {
            cache.active = 0;
            cache.record_token = Some(record_token);
        }
        let source_ptr = text.as_ptr() as usize;
        let source_len = text.len();
        if let Some(entry) = cache.entries[..cache.active]
            .iter()
            .find(|entry| entry.source_ptr == source_ptr && entry.source_len == source_len)
        {
            #[cfg(test)]
            FOLD_VALUE_REUSES.with(|count| count.set(count.get() + 1));
            return inspect(&entry.folded);
        }
        if cache.active == MAX_SHARED_FOLDS {
            return with_folded_candidate_str(text, inspect);
        }
        let index = cache.active;
        if index == cache.entries.len() {
            cache.entries.push(FoldValueEntry {
                source_ptr,
                source_len,
                folded: String::new(),
            });
        }
        let entry = &mut cache.entries[index];
        entry.source_ptr = source_ptr;
        entry.source_len = source_len;
        fold_into(text, &mut entry.folded);
        cache.active += 1;
        #[cfg(test)]
        FOLD_VALUE_TRANSFORMS.with(|count| count.set(count.get() + 1));
        inspect(&cache.entries[index].folded)
    })
}

#[cfg(test)]
pub(crate) fn fold_value_cache_stats() -> (usize, usize) {
    (
        FOLD_VALUE_TRANSFORMS.with(std::cell::Cell::get),
        FOLD_VALUE_REUSES.with(std::cell::Cell::get),
    )
}

#[cfg(feature = "test-scan-instrumentation")]
pub(crate) fn fold_candidate_transforms() -> usize {
    FOLD_CANDIDATE_TRANSFORMS.with(std::cell::Cell::get)
}

use super::*;
