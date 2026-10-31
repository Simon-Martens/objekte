//! Exact match sets for covered scalar and field-presence predicates.

use super::*;
use crate::json_filter::ExactPlan;


/// Snapshot match count and only the rows needed for the requested page.
pub(crate) struct ExactSelection<'a> {
    pub count: usize,
    pub page: Vec<(&'a str, &'a Stamp)>,
}


impl FilePicker {
    /// Count exact snapshot matches and select only the requested page in ID
    /// order. Changed slots still require source verification.
    pub(crate) fn exact_selection(
        &self,
        plan: &ExactPlan,
        offset: usize,
        limit: usize,
    ) -> Option<ExactSelection<'_>> {
        if !self.changed.is_empty() {
            return None;
        }
        let bits = self.exact_bits(plan)?;
        let count: usize = bits.iter().map(|word| word.count_ones() as usize).sum();
        let mut page = Vec::with_capacity(limit.min(count.saturating_sub(offset)));
        if limit == 0 || offset >= count {
            return Some(ExactSelection { count, page });
        }
        // With no changed overlay, live slots remain in rebuild ID order.
        // The bits cover live slots, so a popcount gets the full snapshot count
        // without visiting every file or checking per-file metadata.
        let mut skipped = 0;
        for (word_index, word) in bits.iter().enumerate() {
            let mut remaining = *word;
            while remaining != 0 {
                let slot = word_index * 64 + remaining.trailing_zeros() as usize;
                remaining &= remaining - 1;
                if skipped < offset {
                    skipped += 1;
                    continue;
                }
                let entry = self.files[slot].as_ref().expect("live exact slot");
                page.push((self.id(slot).expect("live exact slot"), &entry.stamp));
                if page.len() == limit {
                    return Some(ExactSelection { count, page });
                }
            }
        }
        Some(ExactSelection { count, page })
    }

    fn exact_bits(&self, plan: &ExactPlan) -> Option<Vec<u64>> {
        match plan {
            ExactPlan::Present(path) => Some(self.observed.presence_bits(path).to_vec()),
            ExactPlan::Term(path, operator, value) => {
                if value.is_null() {
                    return match operator {
                        crate::json_filter::IndexOp::Eq => {
                            Some(self.observed.null_bits(path).to_vec())
                        }
                        crate::json_filter::IndexOp::Ne => {
                            Some(self.observed.non_null_scalar_bits(path))
                        }
                        _ => None,
                    };
                }
                self.typed
                    .term_bits(path, *operator, value, self.files.len())
            }
            ExactPlan::And(parts) => {
                let (first, rest) = parts.split_first()?;
                let mut bits = self.exact_bits(first)?;
                for part in rest {
                    let next = self.exact_bits(part)?;
                    for (word, next) in bits.iter_mut().zip(next) {
                        *word &= next;
                    }
                }
                Some(bits)
            }
            ExactPlan::Or(parts) => {
                let (first, rest) = parts.split_first()?;
                let mut bits = self.exact_bits(first)?;
                for part in rest {
                    let next = self.exact_bits(part)?;
                    for (word, next) in bits.iter_mut().zip(next) {
                        *word |= next;
                    }
                }
                Some(bits)
            }
        }
    }
}
