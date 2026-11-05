//! Conservative posting candidates and schema-declared result order.

use super::*;
use crate::json_filter::MetadataIdHint;
use std::borrow::Cow;


enum SlotMembership {
    All,
    Bits(Vec<u64>),
    Sparse(Vec<usize>),
}


/// Candidate membership is independent of the picker's stable result order.
/// Sparse candidates keep slots; broad queries keep one bitset or no table.
pub(crate) struct SlotSelection {
    membership: SlotMembership,
    count: usize,
}


#[derive(Default)]
pub(crate) struct TranslationWork {
    pub order_entries: u64,
    pub bitmap_words: u64,
}


impl SlotSelection {
    /// Count the current native-order traversal before replacing it with
    /// membership enumeration. This remains an internal diagnostic boundary.
    pub(crate) fn visit_translation_members(
        &self,
        picker: &FilePicker,
        mut visit: impl FnMut(usize) -> bool,
    ) -> TranslationWork {
        let mut work = TranslationWork::default();
        match &self.membership {
            SlotMembership::Sparse(slots) => {
                for &slot in slots {
                    if !visit(slot) {
                        break;
                    }
                }
            }
            SlotMembership::All => {
                for &slot in picker.sorted_slots() {
                    work.order_entries += 1;
                    if picker.files[slot].is_some() && !visit(slot) {
                        break;
                    }
                }
            }
            SlotMembership::Bits(bits) => {
                for &slot in picker.sorted_slots() {
                    work.order_entries += 1;
                    if picker.files[slot].is_none() {
                        continue;
                    }
                    work.bitmap_words += 1;
                    if bits
                        .get(slot / 64)
                        .is_some_and(|word| word & (1 << (slot % 64)) != 0)
                        && !visit(slot)
                    {
                        break;
                    }
                }
            }
        }
        work
    }

    pub(crate) fn clear(&mut self) {
        self.membership = SlotMembership::Sparse(Vec::new());
        self.count = 0;
    }

    pub(crate) fn retain(&mut self, picker: &FilePicker, mut keep: impl FnMut(usize) -> bool) {
        match &mut self.membership {
            SlotMembership::All => {
                let mut bits = vec![0_u64; picker.slot_count().div_ceil(64)];
                for &slot in &picker.sorted {
                    if keep(slot) {
                        bits[slot / 64] |= 1_u64 << (slot % 64);
                    }
                }
                self.membership = SlotMembership::Bits(bits);
            }
            SlotMembership::Sparse(slots) => slots.retain(|&slot| keep(slot)),
            SlotMembership::Bits(bits) => {
                for (word_index, word) in bits.iter_mut().enumerate() {
                    let mut remaining = *word;
                    while remaining != 0 {
                        let slot = word_index * 64 + remaining.trailing_zeros() as usize;
                        remaining &= remaining - 1;
                        if !keep(slot) {
                            *word &= !(1_u64 << (slot % 64));
                        }
                    }
                }
            }
        }
        self.count = match &self.membership {
            SlotMembership::All => picker.len(),
            SlotMembership::Sparse(slots) => slots.len(),
            SlotMembership::Bits(bits) => picker.count_live_bits(bits),
        };
    }

    pub(crate) fn from_bits(picker: &FilePicker, bits: Option<Vec<u64>>) -> Self {
        let mut selection = Self {
            membership: SlotMembership::All,
            count: picker.len(),
        };
        selection.intersect_bits(picker, bits);
        if selection.count.saturating_mul(8) < picker.len()
            && let SlotMembership::Bits(bits) = &selection.membership
        {
            let mut slots = Vec::with_capacity(selection.count);
            for (word_index, &word) in bits.iter().enumerate() {
                let mut remaining = word;
                while remaining != 0 {
                    let slot = word_index * 64 + remaining.trailing_zeros() as usize;
                    remaining &= remaining - 1;
                    if picker.files.get(slot).is_some_and(Option::is_some) {
                        slots.push(slot);
                    }
                }
            }
            selection.membership = SlotMembership::Sparse(slots);
        }
        selection
    }

    pub(crate) fn len(&self) -> usize {
        self.count
    }

    pub(crate) fn is_all(&self) -> bool {
        matches!(self.membership, SlotMembership::All)
    }

    fn contains(&self, slot: usize) -> bool {
        match &self.membership {
            SlotMembership::All => true,
            SlotMembership::Bits(bits) => bits
                .get(slot / 64)
                .is_some_and(|word| word & (1 << (slot % 64)) != 0),
            SlotMembership::Sparse(slots) => slots.contains(&slot),
        }
    }

    pub(crate) fn intersect_bits(&mut self, picker: &FilePicker, bits: Option<Vec<u64>>) {
        let Some(bits) = bits else { return };
        match &mut self.membership {
            SlotMembership::All => self.membership = SlotMembership::Bits(bits),
            SlotMembership::Bits(current) => {
                for (word, next) in current.iter_mut().zip(bits) {
                    *word &= next;
                }
            }
            SlotMembership::Sparse(slots) => slots.retain(|&slot| {
                bits.get(slot / 64)
                    .is_some_and(|word| word & (1 << (slot % 64)) != 0)
            }),
        }
        self.count = match &self.membership {
            SlotMembership::All => picker.len(),
            SlotMembership::Sparse(slots) => slots.len(),
            SlotMembership::Bits(bits) => picker.count_live_bits(bits),
        };
    }

    pub(crate) fn intersect(&mut self, picker: &FilePicker, other: Self) {
        if matches!(self.membership, SlotMembership::All) {
            *self = other;
            return;
        }
        match other.membership {
            SlotMembership::All => {}
            SlotMembership::Bits(bits) => self.intersect_bits(picker, Some(bits)),
            SlotMembership::Sparse(slots) => {
                let slots = match &self.membership {
                    SlotMembership::Sparse(current) => slots
                        .into_iter()
                        .filter(|slot| current.binary_search(slot).is_ok())
                        .collect::<Vec<_>>(),
                    _ => slots
                        .into_iter()
                        .filter(|&slot| self.contains(slot))
                        .collect::<Vec<_>>(),
                };
                self.count = slots.len();
                self.membership = SlotMembership::Sparse(slots);
            }
        }
    }

    pub(crate) fn order_sparse(
        &mut self,
        picker: &FilePicker,
        newest_first: bool,
        sorts: &[crate::Sort],
    ) -> bool {
        let indexed = matches!(sorts, [sort] if sort.field == "@id") || picker.sort.supports(sorts);
        let SlotMembership::Sparse(slots) = &mut self.membership else {
            return indexed;
        };
        if indexed {
            if let [sort] = sorts
                && sort.field == "@id"
            {
                slots.sort_unstable_by(|&a, &b| picker.id(a).cmp(&picker.id(b)));
                if sort.direction == Direction::Desc {
                    slots.reverse();
                }
            } else {
                slots.sort_unstable_by(|&a, &b| {
                    picker
                        .sort
                        .compare_slots(sorts, a, b, &picker.files, &picker.id_bytes)
                        .expect("supported sort")
                });
            }
        } else if newest_first {
            slots.sort_unstable_by(|&a, &b| {
                ModifiedOrder::compare_slots(&picker.files, &picker.id_bytes, a, b)
            });
        } else {
            slots.sort_unstable_by(|&a, &b| picker.id(a).cmp(&picker.id(b)));
        }
        indexed
    }

    pub(crate) fn ordered<'a>(
        &'a self,
        picker: &'a FilePicker,
        newest_first: bool,
        sorts: &[crate::Sort],
    ) -> Box<dyn Iterator<Item = usize> + 'a> {
        if let SlotMembership::Sparse(slots) = &self.membership {
            return Box::new(slots.iter().copied());
        }
        if matches!(self.membership, SlotMembership::All) && newest_first && sorts.is_empty() {
            return Box::new(
                picker
                    .modified_order
                    .iter(&picker.files, &picker.id_bytes)
                    .map(|slot| slot as usize),
            );
        }
        let order: Box<dyn Iterator<Item = usize> + 'a> = if let [sort] = sorts
            && sort.field == "@id"
        {
            if sort.direction == Direction::Desc {
                Box::new(picker.sorted.iter().rev().copied())
            } else {
                Box::new(picker.sorted.iter().copied())
            }
        } else if let Some(order) = picker.sort.order(sorts) {
            match order {
                Cow::Borrowed(slots) => Box::new(slots.iter().copied()),
                Cow::Owned(slots) => Box::new(slots.into_iter()),
            }
        } else if newest_first {
            Box::new(
                picker
                    .modified_order
                    .iter(&picker.files, &picker.id_bytes)
                    .map(|slot| slot as usize),
            )
        } else {
            Box::new(picker.sorted.iter().copied())
        };
        Box::new(order.filter(|&slot| picker.files[slot].is_some() && self.contains(slot)))
    }
}


impl FilePicker {
    /// Refreshes keep each live slot's ID and stamp current, including slots
    /// in the changed-file overlay, so only an unverified inventory is unsafe.
    pub(crate) fn metadata_exact_ready(&self) -> bool {
        self.verified()
    }

    pub(crate) fn metadata_id_selection(&self, hint: &MetadataIdHint) -> Option<SlotSelection> {
        let slots: Vec<usize> = match hint {
            MetadataIdHint::Exact(id) => self
                .position(id)
                .ok()
                .map(|pos| self.sorted[pos])
                .into_iter()
                .collect(),
            MetadataIdHint::Filename(id) => {
                if self.recursive
                    && self
                        .sorted
                        .iter()
                        .any(|&slot| self.id(slot).is_some_and(|id| id.contains('/')))
                {
                    return None;
                }
                self.position(id)
                    .ok()
                    .map(|pos| self.sorted[pos])
                    .into_iter()
                    .collect()
            }
            MetadataIdHint::Prefix(prefix) => {
                let start = self
                    .sorted
                    .partition_point(|&slot| self.id(slot).is_some_and(|id| id < prefix.as_str()));
                self.sorted[start..]
                    .iter()
                    .copied()
                    .take_while(|&slot| self.id(slot).is_some_and(|id| id.starts_with(prefix)))
                    .collect()
            }
        };
        let count = slots.len();
        Some(SlotSelection {
            membership: SlotMembership::Sparse(slots),
            count,
        })
    }

    pub(crate) fn count_live_bits(&self, bits: &[u64]) -> usize {
        let mut count = 0;
        for (word_index, &word) in bits.iter().enumerate() {
            let mut remaining = word;
            while remaining != 0 {
                let slot = word_index * 64 + remaining.trailing_zeros() as usize;
                remaining &= remaining - 1;
                count += usize::from(self.files.get(slot).is_some_and(Option::is_some));
            }
        }
        count
    }

    /// Select conservative JSON candidates without materializing an ID/stamp
    /// pair per file. Selective postings enumerate set bits; broad postings
    /// scan the existing order once and retain only membership bits.
    #[cfg(test)]
    pub(crate) fn candidate_slot_selection(
        &self,
        pattern: Option<&str>,
        typed_plan: Option<&crate::json_filter::TypedPlan>,
        required_fields: &[String],
    ) -> SlotSelection {
        self.candidate_slot_selection_for_path(pattern, None, typed_plan, required_fields)
    }

    pub(crate) fn candidate_slot_selection_for_path(
        &self,
        pattern: Option<&str>,
        path: Option<&crate::JsonPath>,
        typed_plan: Option<&crate::json_filter::TypedPlan>,
        required_fields: &[String],
    ) -> SlotSelection {
        let bits = pattern.and_then(|regex| {
            path.map_or_else(
                || self.raw_candidate_bits(regex, false),
                |path| self.field_candidate_bits(regex, path, false, false),
            )
        });
        let overlay = pattern.map(regex_plan::OverlayQuery::new);
        let typed_bits = self.typed.candidate_bits(typed_plan, self.files.len());
        let presence_bits = self.presence.candidates(required_fields);
        if bits.is_none() && typed_bits.is_none() && presence_bits.is_none() {
            return SlotSelection {
                membership: SlotMembership::All,
                count: self.len(),
            };
        }
        let mut smallest: Option<(&[u64], bool, usize)> = None;
        for (next, changed) in [
            (bits.as_deref(), true),
            (typed_bits.as_deref(), false),
            (presence_bits.as_deref(), false),
        ] {
            if let Some(next) = next {
                let count = next
                    .iter()
                    .map(|word| word.count_ones() as usize)
                    .sum::<usize>()
                    .saturating_add(if changed { self.changed.len() } else { 0 });
                if smallest.is_none_or(|(_, _, previous)| count < previous) {
                    smallest = Some((next, changed, count));
                }
            }
        }
        let matches = |slot: usize| {
            let text_match = if slot >= self.indexed_len {
                true
            } else if self.changed.contains(&slot) {
                !self.text_paths.is_empty()
                    || self
                        .changed_bigrams
                        .get(&slot)
                        .is_none_or(|keys| overlay.as_ref().is_none_or(|query| query.matches(keys)))
            } else {
                bits.as_ref()
                    .is_none_or(|bits| BigramFilter::is_candidate(bits, slot))
            };
            let typed_match = typed_bits.as_ref().is_none_or(|bits| {
                bits.get(slot / 64)
                    .is_some_and(|word| word & (1 << (slot % 64)) != 0)
            });
            let presence_match = presence_bits.as_ref().is_none_or(|bits| {
                bits.get(slot / 64)
                    .is_some_and(|word| word & (1 << (slot % 64)) != 0)
            });
            text_match && typed_match && presence_match
        };
        if let Some((candidate_bits, needs_overlay, count)) = smallest
            && count.saturating_mul(8) < self.len()
        {
            let mut slots = Vec::with_capacity(count);
            for (word_index, &word) in candidate_bits.iter().enumerate() {
                let mut remaining = word;
                while remaining != 0 {
                    let slot = word_index * 64 + remaining.trailing_zeros() as usize;
                    remaining &= remaining - 1;
                    if self.files.get(slot).is_some_and(Option::is_some) {
                        slots.push(slot);
                    }
                }
            }
            if needs_overlay {
                slots.extend(
                    self.changed
                        .iter()
                        .copied()
                        .filter(|&slot| self.files[slot].is_some()),
                );
            }
            slots.sort_unstable();
            slots.dedup();
            slots.retain(|&slot| matches(slot));
            let count = slots.len();
            return SlotSelection {
                membership: SlotMembership::Sparse(slots),
                count,
            };
        }
        let mut selected = vec![0; self.files.len().div_ceil(64)];
        let mut count = 0;
        for &slot in &self.sorted {
            if matches(slot) {
                selected[slot / 64] |= 1 << (slot % 64);
                count += 1;
            }
        }
        SlotSelection {
            membership: SlotMembership::Bits(selected),
            count,
        }
    }

    pub(crate) fn has_sort_index(&self, sorts: &[crate::Sort]) -> bool {
        if matches!(sorts, [sort] if sort.field == "@id") {
            return true;
        }
        self.sort.supports(sorts)
    }

    /// Conservative candidates for an OR of necessary literal fragments.
    /// `None` means no safe prefilter; a missing posting also means scan all.
    fn changed_matches_literal(&self, slot: usize, literal: &str) -> bool {
        let Some(keys) = self.changed_bigrams.get(&slot) else {
            return true;
        };
        let required = extract_bigrams_with_encoding(literal.as_bytes(), self.encoding);
        required.iter().all(|key| keys.binary_search(key).is_ok())
    }

    /// Typed candidates for a top-level field equal to any of `ids`.
    pub(crate) fn typed_id_bits(
        &self,
        path: &crate::JsonPath,
        ids: &std::collections::BTreeSet<String>,
    ) -> Option<Vec<u64>> {
        use crate::json_filter::{IndexOp, TypedPlan};
        let plan = TypedPlan::Or(
            ids.iter()
                .map(|id| {
                    TypedPlan::Term(
                        path.clone(),
                        IndexOp::Eq,
                        serde_json::Value::String(id.clone()),
                    )
                })
                .collect(),
        );
        self.typed.candidate_bits(Some(&plan), self.files.len())
    }

    /// Conservative candidates containing any of `literals`, as a bitset.
    pub(crate) fn literal_union_bits(&self, literals: &[String]) -> Option<Vec<u64>> {
        let mut bits = vec![0_u64; self.slot_count().div_ceil(64)];
        for slot in self.candidates(Some(literals)) {
            bits[slot / 64] |= 1 << (slot % 64);
        }
        Some(bits)
    }

    pub(super) fn candidates(&self, literals: Option<&[String]>) -> Vec<usize> {
        let Some(literals) = literals.filter(|terms| !terms.is_empty()) else {
            return self.sorted.clone();
        };
        let mut bitsets = Vec::with_capacity(literals.len());
        for literal in literals {
            let Some(bits) = self.raw_candidate_bits(literal, true) else {
                return self.sorted.clone();
            };
            bitsets.push(bits);
        }
        if self.changed.is_empty() && self.indexed_len == self.files.len() {
            if let [bits] = bitsets.as_slice() {
                return self
                    .sorted
                    .iter()
                    .copied()
                    .filter(|&slot| BigramFilter::is_candidate(bits, slot))
                    .collect();
            }
            return self
                .sorted
                .iter()
                .copied()
                .filter(|&slot| {
                    bitsets
                        .iter()
                        .any(|bits| BigramFilter::is_candidate(bits, slot))
                })
                .collect();
        }
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                if slot >= self.indexed_len {
                    true
                } else if self.changed.contains(&slot) {
                    !self.text_paths.is_empty()
                        || literals
                            .iter()
                            .any(|literal| self.changed_matches_literal(slot, literal))
                } else {
                    bitsets
                        .iter()
                        .any(|bits| BigramFilter::is_candidate(bits, slot))
                }
            })
            .collect()
    }

    /// Returns byte-sorted candidate IDs for an OR of necessary literal
    /// fragments. The result must still be parsed and checked against the
    /// complete filter. `None` conservatively returns every file.
    pub fn candidate_ids(&self, literals: Option<&[String]>) -> Vec<&str> {
        self.candidates(literals)
            .into_iter()
            .map(|slot| self.id(slot).expect("live candidate slot"))
            .collect()
    }

    /// Candidate IDs for a regex over decoded scalar values. FFF's regex HIR
    /// planner narrows the immutable index; changed files bypass it. This is
    /// only a prefilter and may include false positives.
    pub fn regex_candidate_ids(&self, pattern: &str) -> Vec<&str> {
        self.regex_candidate_slots(pattern)
            .into_iter()
            .map(|slot| self.id(slot).expect("live candidate slot"))
            .collect()
    }

    fn regex_candidate_slots(&self, pattern: &str) -> Vec<usize> {
        let bits = self.raw_candidate_bits(pattern, false);
        let overlay = regex_plan::OverlayQuery::new(pattern);
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                if slot >= self.indexed_len {
                    true
                } else if self.changed.contains(&slot) {
                    !self.text_paths.is_empty()
                        || self
                            .changed_bigrams
                            .get(&slot)
                            .is_none_or(|keys| overlay.matches(keys))
                } else {
                    bits.as_ref()
                        .is_none_or(|bits| BigramFilter::is_candidate(bits, slot))
                }
            })
            .collect()
    }

    /// Complete regex search over decoded values or malformed-file text.
    /// Source bytes are reread for each candidate; the index never decides truth.
    pub fn search_values_regex(&self, pattern: &str) -> Result<Vec<String>> {
        let matcher = regex::bytes::RegexBuilder::new(pattern)
            .unicode(false)
            .build()
            .map_err(|e| Error::Request(format!("invalid values regex: {e}")))?;
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let candidates = self.regex_candidate_ids(pattern);
        let matches = candidates
            .par_iter()
            .map(|&id| -> Result<Option<String>> {
                let slot = self.sorted[self.position(id).expect("candidate is live")];
                let stamp = &self.files[slot].as_ref().expect("live candidate").stamp;
                let text = projected_values_with_text(
                    &source,
                    id,
                    stamp,
                    &[],
                    &[],
                    &self.view,
                    &self.sort,
                    ProjectionOptions {
                        folded_index: false,
                        text_rules: None,
                        text_paths: &[],
                        key_summary: None,
                        text_scratch: None,
                        values_scratch: None,
                        derived_scratch: None,
                        #[cfg(feature = "index-timings")]
                        timings: None,
                    },
                )?;
                let matched = match text {
                    ProjectionOutcome::Valid(text) => matcher.is_match(&text.bytes),
                    ProjectionOutcome::Malformed(text) => matcher.is_match(text.as_bytes()),
                };
                Ok(matched.then(|| id.to_owned()))
            })
            .collect::<Result<Vec<_>>>()?;
        source.finish()?;
        Ok(matches.into_iter().flatten().collect())
    }
}
