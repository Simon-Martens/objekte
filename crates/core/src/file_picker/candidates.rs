//! Conservative posting candidates and schema-declared result order.

use super::*;


impl FilePicker {
    pub(crate) fn has_sort_index(&self, sorts: &[crate::Sort]) -> bool {
        if matches!(sorts, [sort] if sort.field == "@id") {
            return true;
        }
        self.sort.supports(sorts)
    }

    /// Reorder conservative candidates by a matching schema-declared sort index.
    pub(crate) fn order_candidates(
        &self,
        entries: &mut Vec<(String, Stamp)>,
        sorts: &[crate::Sort],
    ) -> bool {
        if let [sort] = sorts
            && sort.field == "@id"
        {
            if sort.direction == Direction::Desc {
                entries.reverse();
            }
            return true;
        }
        if !self.sort.supports(sorts) {
            return false;
        }
        // Sparse matches are cheaper to sort by key. Dense matches are cheaper
        // to place in a slot table and read in the index's existing order.
        if entries.len().saturating_mul(8) < self.len() {
            let mut keyed = entries
                .drain(..)
                .filter_map(|entry| {
                    self.position(&entry.0)
                        .ok()
                        .map(|position| (self.sorted[position], entry))
                })
                .collect::<Vec<_>>();
            keyed.sort_by(|a, b| {
                self.sort
                    .compare_slots(sorts, a.0, b.0, &self.files, &self.id_bytes)
                    .expect("matching sort index")
            });
            entries.extend(keyed.into_iter().map(|(_, entry)| entry));
        } else {
            let order = self.sort.order(sorts).expect("supported sort index");
            let mut slots = std::iter::repeat_with(|| None)
                .take(self.files.len())
                .collect::<Vec<Option<(String, Stamp)>>>();
            for entry in entries.drain(..) {
                if let Ok(position) = self.position(&entry.0) {
                    slots[self.sorted[position]] = Some(entry);
                }
            }
            entries.extend(order.iter().filter_map(|&slot| slots[slot].take()));
        }
        true
    }

    pub(crate) fn order_candidate_refs<'a>(
        &self,
        entries: &mut Vec<(&'a str, &'a Stamp)>,
        sorts: &[crate::Sort],
    ) -> bool {
        if let [sort] = sorts
            && sort.field == "@id"
        {
            if sort.direction == Direction::Desc {
                entries.reverse();
            }
            return true;
        }
        if !self.sort.supports(sorts) {
            return false;
        }
        // The borrowed version uses the same two paths without cloning IDs.
        if entries.len().saturating_mul(8) < self.len() {
            let mut keyed = entries
                .drain(..)
                .filter_map(|entry| {
                    self.position(entry.0)
                        .ok()
                        .map(|position| (self.sorted[position], entry))
                })
                .collect::<Vec<_>>();
            keyed.sort_by(|a, b| {
                self.sort
                    .compare_slots(sorts, a.0, b.0, &self.files, &self.id_bytes)
                    .expect("matching sort index")
            });
            entries.extend(keyed.into_iter().map(|(_, entry)| entry));
        } else {
            let order = self.sort.order(sorts).expect("supported sort index");
            let mut slots = vec![None; self.files.len()];
            for entry in entries.drain(..) {
                if let Ok(position) = self.position(entry.0) {
                    slots[self.sorted[position]] = Some(entry);
                }
            }
            entries.extend(order.iter().filter_map(|&slot| slots[slot].take()));
        }
        true
    }

    /// Conservative candidates for an OR of necessary literal fragments.
    /// `None` means no safe prefilter; a missing posting also means scan all.
    fn changed_matches_literal(&self, slot: usize, literal: &str) -> bool {
        let Some(keys) = self.changed_bigrams.get(&slot) else {
            return true;
        };
        let required = extract_bigrams(literal.as_bytes());
        required.iter().all(|key| keys.binary_search(key).is_ok())
    }

    pub(super) fn candidates(&self, literals: Option<&[String]>) -> Vec<usize> {
        let Some(literals) = literals.filter(|terms| !terms.is_empty()) else {
            return self.sorted.clone();
        };
        let mut bitsets = Vec::with_capacity(literals.len());
        for literal in literals {
            let Some(bits) = self.index.query(literal.as_bytes()) else {
                return self.sorted.clone();
            };
            bitsets.push(bits);
        }
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                if slot >= self.indexed_len {
                    true
                } else if self.changed.contains(&slot) {
                    literals
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
        let bits = regex_plan::candidates(&self.index, pattern);
        let overlay = regex_plan::OverlayQuery::new(pattern);
        self.sorted
            .iter()
            .copied()
            .filter(|&slot| {
                if slot >= self.indexed_len {
                    true
                } else if self.changed.contains(&slot) {
                    self.changed_bigrams
                        .get(&slot)
                        .is_none_or(|keys| overlay.matches(keys))
                } else {
                    bits.as_ref()
                        .is_none_or(|bits| BigramFilter::is_candidate(bits, slot))
                }
            })
            .collect()
    }

    pub(crate) fn candidate_records(
        &self,
        pattern: Option<&str>,
        typed_plan: Option<&crate::json_filter::TypedPlan>,
        required_fields: &[String],
    ) -> Vec<(&str, &Stamp)> {
        self.candidate_records_ordered(pattern, typed_plan, required_fields, false)
    }

    pub(crate) fn candidate_records_ordered(
        &self,
        pattern: Option<&str>,
        typed_plan: Option<&crate::json_filter::TypedPlan>,
        required_fields: &[String],
        newest_first: bool,
    ) -> Vec<(&str, &Stamp)> {
        let bits = pattern.and_then(|regex| regex_plan::candidates(&self.index, regex));
        let overlay = pattern.map(regex_plan::OverlayQuery::new);
        let typed_bits = self
            .typed
            .candidate_bits(typed_plan, self.files.len(), &self.changed);
        let presence_bits = self.presence.candidates(required_fields);
        let count_bits =
            |bits: &[u64]| -> usize { bits.iter().map(|word| word.count_ones() as usize).sum() };
        let mut candidate_bits = None;
        let mut needs_overlay = false;
        let mut smallest = usize::MAX;
        for (next, overlay) in [
            (bits.as_deref(), true),
            (typed_bits.as_deref(), false),
            (presence_bits.as_deref(), false),
        ] {
            if let Some(next) = next {
                let count =
                    count_bits(next).saturating_add(if overlay { self.changed.len() } else { 0 });
                if count < smallest {
                    candidate_bits = Some(next);
                    needs_overlay = overlay;
                    smallest = count;
                }
            }
        }
        let selected = candidate_bits.and_then(|candidate_bits| {
            // Sorting a dense candidate set costs more than checking the
            // already sorted ID table. Enumerate set bits only when selective.
            if smallest.saturating_mul(8) >= self.sorted.len() {
                return None;
            }
            let mut slots = Vec::new();
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
                // New and changed files bypass immutable text postings. They
                // may already have an old posting, so deduplicate below.
                slots.extend(
                    self.changed
                        .iter()
                        .copied()
                        .filter(|&slot| self.files[slot].is_some()),
                );
            }
            // Slots follow index insertion order; choose the requested result
            // order after including any appended changed-file slots.
            if newest_first {
                slots.sort_unstable_by(|&left, &right| {
                    ModifiedOrder::compare_slots(&self.files, &self.id_bytes, left, right)
                });
            } else {
                slots.sort_unstable_by(|&left, &right| {
                    self.id(left)
                        .expect("live candidate slot")
                        .as_bytes()
                        .cmp(self.id(right).expect("live candidate slot").as_bytes())
                });
            }
            slots.dedup();
            Some(slots)
        });
        let slots: Box<dyn Iterator<Item = usize> + '_> = if let Some(selected) = selected.as_ref()
        {
            Box::new(selected.iter().copied())
        } else if newest_first {
            Box::new(
                self.modified_order
                    .iter(&self.files, &self.id_bytes)
                    .map(|slot| slot as usize),
            )
        } else {
            Box::new(self.sorted.iter().copied())
        };
        slots
            .filter(|&slot| {
                let text_match = if slot >= self.indexed_len {
                    true
                } else if self.changed.contains(&slot) {
                    self.changed_bigrams
                        .get(&slot)
                        .is_none_or(|keys| overlay.as_ref().is_none_or(|query| query.matches(keys)))
                } else {
                    bits.as_ref()
                        .is_none_or(|bits| BigramFilter::is_candidate(bits, slot))
                };
                let typed_match = typed_bits
                    .as_ref()
                    .is_none_or(|bits| bits[slot / 64] & (1 << (slot % 64)) != 0);
                let presence_match = presence_bits
                    .as_ref()
                    .is_none_or(|bits| bits[slot / 64] & (1 << (slot % 64)) != 0);
                text_match && typed_match && presence_match
            })
            .map(|slot| {
                (
                    self.id(slot).expect("live candidate slot"),
                    &self.files[slot].as_ref().expect("live candidate").stamp,
                )
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
                        key_summary: None,
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
