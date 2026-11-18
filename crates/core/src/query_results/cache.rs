//! Verified result-cache admission accounting.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::verified::CachedMember;

/// Compact cache membership is budgeted independently of page payloads.
pub(crate) struct CacheBudget {
    pub(crate) initial: usize,
    remaining: AtomicUsize,
    complete: AtomicBool,
}

impl CacheBudget {
    pub(crate) fn new(bytes: usize) -> Self {
        Self {
            initial: bytes,
            remaining: AtomicUsize::new(bytes),
            complete: AtomicBool::new(true),
        }
    }

    pub(crate) fn complete(&self) -> bool {
        self.complete.load(Ordering::Relaxed)
    }

    pub(crate) fn used(&self) -> usize {
        self.initial
            .saturating_sub(self.remaining.load(Ordering::Relaxed))
    }

    pub(crate) fn reserve(&self, fields: &[String]) -> bool {
        if !self.complete() {
            return false;
        }
        let bytes = std::mem::size_of::<CachedMember>()
            + fields
                .iter()
                .map(|field| field.len() + std::mem::size_of::<String>())
                .sum::<usize>();
        if self
            .remaining
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |remaining| {
                remaining.checked_sub(bytes)
            })
            .is_ok()
        {
            true
        } else {
            self.complete.store(false, Ordering::Relaxed);
            false
        }
    }
}
