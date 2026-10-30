//! Exact match sets for covered scalar and field-presence predicates.

use super::*;
use crate::json_filter::ExactPlan;


impl FilePicker {
    /// Return matching source rows in ID order when the immutable snapshot can
    /// decide the entire predicate. Changed slots require source verification.
    pub(crate) fn exact_records(&self, plan: &ExactPlan) -> Option<Vec<(&str, &Stamp)>> {
        if !self.changed.is_empty() {
            return None;
        }
        let bits = self.exact_bits(plan)?;
        let mut records = Vec::new();
        // With no changed overlay, live slots remain in rebuild ID order.
        // Enumerate set bits directly so a sparse match avoids an ID-table scan.
        for (word_index, word) in bits.iter().enumerate() {
            let mut remaining = *word;
            while remaining != 0 {
                let slot = word_index * 64 + remaining.trailing_zeros() as usize;
                remaining &= remaining - 1;
                if let Some(entry) = self.files.get(slot).and_then(Option::as_ref) {
                    records.push((self.id(slot).expect("live exact slot"), &entry.stamp));
                }
            }
        }
        Some(records)
    }

    fn exact_bits(&self, plan: &ExactPlan) -> Option<Vec<u64>> {
        match plan {
            ExactPlan::Present(field) => Some(self.presence.bits(field)?.to_vec()),
            ExactPlan::Term(field, operator, value) => {
                self.typed
                    .term_bits(field, *operator, value, self.files.len())
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
