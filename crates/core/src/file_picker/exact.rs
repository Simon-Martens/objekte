//! Exact match sets for covered scalar and field-presence predicates.

use super::*;
use crate::json_filter::ExactPlan;


/// Snapshot match count and only the rows needed for the requested page.
pub(crate) struct ExactSelection<'a> {
    pub count: usize,
    pub page: Vec<(&'a str, &'a Stamp)>,
}


impl FilePicker {
    /// Count exact snapshot matches and select only the requested page in
    /// modification-time order. Changed slots still require source verification.
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
        // Sparse matches are cheaper to gather and sort; dense matches are
        // cheaper to intersect with the persistent modification-time order.
        let slots: Box<dyn Iterator<Item = usize> + '_> = if count.saturating_mul(8) < self.len() {
            let mut slots = Vec::with_capacity(count);
            for (word_index, word) in bits.iter().enumerate() {
                let mut remaining = *word;
                while remaining != 0 {
                    slots.push(word_index * 64 + remaining.trailing_zeros() as usize);
                    remaining &= remaining - 1;
                }
            }
            slots.sort_unstable_by(|&a, &b| {
                ModifiedOrder::compare_slots(&self.files, &self.id_bytes, a, b)
            });
            Box::new(slots.into_iter())
        } else {
            Box::new(
                self.modified_order
                    .iter(&self.files, &self.id_bytes)
                    .map(|slot| slot as usize)
                    .filter(|&slot| bits[slot / 64] & (1_u64 << (slot % 64)) != 0),
            )
        };
        for slot in slots.skip(offset).take(limit) {
            let entry = self.files[slot].as_ref().expect("live exact slot");
            page.push((self.id(slot).expect("live exact slot"), &entry.stamp));
        }
        Some(ExactSelection { count, page })
    }

    fn exact_bits(&self, plan: &ExactPlan) -> Option<Vec<u64>> {
        match plan {
            ExactPlan::Present(path) => {
                if let [crate::JsonPathStep::Key(name)] = path.as_slice()
                    && let Some(bits) = self.presence.bits(name)
                {
                    return Some(bits.to_vec());
                }
                self.typed.present_bits(path)
            }
            ExactPlan::Missing(path) => self.typed.missing_bits(path, self.files.len()),
            ExactPlan::Prefix(path, prefix) => {
                self.typed.prefix_bits(path, prefix, self.files.len())
            }
            ExactPlan::Term(field, operator, value) => {
                self.typed
                    .term_bits(field, *operator, value, self.files.len())
            }
            ExactPlan::AnyTerm(field, operator, value) => {
                self.typed
                    .any_term_bits(field, *operator, value, self.files.len())
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
