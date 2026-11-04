//! Use field postings to narrow text and JSON candidate slots.

use super::*;
use sonic_rs::Value as SonicValue;
use std::io::Read;


impl FilePicker {
    pub(crate) fn has_text_field_path(&self, path: &crate::JsonPath) -> bool {
        self.matching_field_filter(path).next().is_some()
    }

    fn matching_field_filter<'a>(
        &'a self,
        path: &crate::JsonPath,
    ) -> impl Iterator<Item = &'a FieldTextIndex> {
        self.text_index
            .iter()
            .flat_map(|text| &text.fields)
            .filter(move |field| path_matches_registered(path, &field.path))
    }

    pub(in crate::file_picker) fn field_candidate_bits(
        &self,
        pattern: &str,
        path: &crate::JsonPath,
        clean: bool,
        literal: bool,
    ) -> Option<Vec<u64>> {
        let mut filters = self.matching_field_filter(path).peekable();
        if filters.peek().is_none() {
            return if clean {
                self.clean_candidate_bits(pattern, literal)
            } else {
                self.raw_candidate_bits(pattern, literal)
            };
        }
        let mut bits = vec![0; self.indexed_len.div_ceil(64)];
        for field in filters {
            let (filter, present, observed) = if clean {
                (&field.clean, field.clean_present, &field.clean_observed)
            } else {
                (&field.normal, field.normal_present, &field.normal_observed)
            };
            if !present {
                continue;
            }
            if literal && filter.excludes_ascii_literal(pattern, observed) {
                continue;
            }
            let next = if literal {
                filter.query(pattern.as_bytes())
            } else {
                regex_plan::candidates_with_observed(filter, pattern, Some(observed))
            };
            let Some(next) = next else {
                // An omitted field column is unknown, not an empty match.
                // The rendered partitions cannot rule this field out.
                return if clean {
                    self.clean_candidate_bits(pattern, literal)
                } else {
                    None
                };
            };
            for (word, extra) in bits.iter_mut().zip(next) {
                *word |= extra;
            }
        }
        for &slot in &self.malformed {
            if slot < self.indexed_len {
                bits[slot / 64] |= 1 << (slot % 64);
            }
        }
        Some(bits)
    }

    pub(crate) fn text_field_candidate_bits(
        &self,
        pattern: &str,
        path: &crate::JsonPath,
        clean: bool,
        literal: bool,
    ) -> Option<Vec<u64>> {
        let bits = self.field_candidate_bits(pattern, path, clean, literal)?;
        Some(self.with_live_overlays(bits))
    }

    /// Decoded source values are partitioned between the general filter and
    /// one filter per declared full-text leaf. A missing constraint in any
    /// partition requires a conservative scan of that partition.
    pub(in crate::file_picker) fn raw_candidate_bits(
        &self,
        pattern: &str,
        literal: bool,
    ) -> Option<Vec<u64>> {
        if !self.text_paths.is_empty() && self.text_index.is_none() {
            return None;
        }
        let query = |filter: &BigramFilter, observed: Option<&[u64]>| {
            if literal {
                filter.query(pattern.as_bytes())
            } else {
                regex_plan::candidates_with_observed(filter, pattern, observed)
            }
        };
        let mut bits = query(&self.index, None)?;
        if let Some(text) = &self.text_index {
            for field in &text.fields {
                if !field.normal_present {
                    continue;
                }
                if literal
                    && field
                        .normal
                        .excludes_ascii_literal(pattern, &field.normal_observed)
                {
                    continue;
                }
                // Observed pairs tell an omitted column from an absent one.
                let next = query(&field.normal, Some(&field.normal_observed))?;
                for (word, extra) in bits.iter_mut().zip(next) {
                    *word |= extra;
                }
            }
        }
        Some(bits)
    }

    fn clean_candidate_bits(&self, pattern: &str, literal: bool) -> Option<Vec<u64>> {
        let Some(text) = &self.text_index else {
            return self.raw_candidate_bits(pattern, literal);
        };
        let query = |filter: &BigramFilter, observed: &[u64]| {
            if literal {
                filter.query(pattern.as_bytes())
            } else {
                regex_plan::candidates_with_observed(filter, pattern, Some(observed))
            }
        };
        let mut bits = vec![0; text.indexed_len.div_ceil(64)];
        if text.global_clean_present
            && !(literal
                && text
                    .global_clean
                    .excludes_ascii_literal(pattern, &text.global_clean_observed))
        {
            let next = query(&text.global_clean, &text.global_clean_observed)?;
            for (word, extra) in bits.iter_mut().zip(next) {
                *word |= extra;
            }
        }
        for field in &text.fields {
            if !field.clean_present {
                continue;
            }
            if literal
                && field
                    .clean
                    .excludes_ascii_literal(pattern, &field.clean_observed)
            {
                continue;
            }
            // Each field is a separate partition. Evaluate the full pattern
            // within each filter before unioning its complete candidates.
            let next = query(&field.clean, &field.clean_observed)?;
            for (word, extra) in bits.iter_mut().zip(next) {
                *word |= extra;
            }
        }
        Some(bits)
    }

    /// A missing posting is unknown, represented by `None`. Changed slots and
    /// slots appended after index construction remain conservative candidates.
    pub(crate) fn text_candidate_bits(
        &self,
        pattern: Option<&str>,
        raw: bool,
        literal: bool,
    ) -> Option<Vec<u64>> {
        let bits = pattern.and_then(|pattern| {
            if raw {
                self.raw_candidate_bits(pattern, literal)
            } else {
                self.clean_candidate_bits(pattern, literal)
            }
        })?;
        Some(self.with_live_overlays(bits))
    }

    fn with_live_overlays(&self, mut bits: Vec<u64>) -> Vec<u64> {
        let indexed_len = self
            .text_index
            .as_ref()
            .map_or(self.indexed_len, |index| index.indexed_len);
        bits.resize(self.files.len().div_ceil(64), 0);
        for &slot in &self.changed {
            if self.files[slot].is_some() {
                bits[slot / 64] |= 1 << (slot % 64);
            }
        }
        for slot in indexed_len..self.files.len() {
            if self.files[slot].is_some() {
                bits[slot / 64] |= 1 << (slot % 64);
            }
        }
        bits
    }

    /// A simple top-level field name cannot collide with a nested field's
    /// display path. For those restrictions, use only that field's postings.
    fn selected_field_bits(
        &self,
        pattern: &str,
        raw: bool,
        literal: bool,
        selected: &std::collections::BTreeSet<&str>,
    ) -> Option<Option<Vec<u64>>> {
        let text = self.text_index.as_ref()?;
        let mut bits = vec![0; text.indexed_len.div_ceil(64)];
        for &name in selected {
            let simple = name
                .chars()
                .next()
                .is_some_and(|first| first == '_' || first.is_alphabetic())
                && name.chars().all(|ch| ch == '_' || ch.is_alphanumeric());
            if !simple {
                return None;
            }
            let field = text.fields.iter().find(|field| {
                matches!(field.path.as_slice(), [crate::JsonPathStep::Key(key)] if key == name)
            })?;
            let (filter, present, observed) = if raw {
                (&field.normal, field.normal_present, &field.normal_observed)
            } else {
                (&field.clean, field.clean_present, &field.clean_observed)
            };
            if !present {
                continue;
            }
            if literal && filter.excludes_ascii_literal(pattern, observed) {
                continue;
            }
            let next = if literal {
                filter.query(pattern.as_bytes())
            } else {
                regex_plan::candidates_with_observed(filter, pattern, Some(observed))
            };
            let Some(next) = next else {
                return Some(None);
            };
            for (word, extra) in bits.iter_mut().zip(next) {
                *word |= extra;
            }
        }
        Some(Some(self.with_live_overlays(bits)))
    }

    pub(crate) fn text_candidate_selection(
        &self,
        pattern: Option<&str>,
        raw: bool,
        literal: bool,
        selected: Option<&std::collections::BTreeSet<&str>>,
        matches: impl FnMut(&str, crate::TextFormat) -> bool,
    ) -> super::candidates::SlotSelection {
        let mut bits = match (pattern, selected) {
            (Some(pattern), Some(selected)) => self
                .selected_field_bits(pattern, raw, literal, selected)
                .unwrap_or_else(|| self.text_candidate_bits(Some(pattern), raw, literal)),
            _ => self.text_candidate_bits(pattern, raw, literal),
        };
        if let Some(base) = bits.as_mut() {
            let typed = self.typed.text_matching_bits(
                self.files.len(),
                selected,
                &self.text_rules,
                matches,
            );
            if let Some(typed) = typed {
                for (word, additional) in base.iter_mut().zip(typed) {
                    *word |= additional;
                }
            }
        }
        super::candidates::SlotSelection::from_bits(self, bits)
    }

    pub(crate) fn text_index_bytes(&self) -> Option<usize> {
        self.text_index.as_ref().map(TextIndex::heap_bytes)
    }

    /// Ensure the separate postings required by markup or exclusion rules.
    pub(crate) fn ensure_text_index(&mut self) -> Result<bool> {
        if !self.eager_text_index {
            return Ok(false);
        }
        if self.text_index.is_some() {
            return Ok(false);
        }
        let source = Source::open_scoped(&self.root, self.recursive)?;
        self.build_text_index_from_source(&source)?;
        source.finish()?;
        Ok(true)
    }

    pub(crate) fn rebuild_text_index(&mut self) -> Result<()> {
        self.refresh()?;
        self.text_index = None;
        self.ensure_text_index()?;
        Ok(())
    }

    pub(super) fn build_text_index_from_source(&mut self, source: &Source) -> Result<()> {
        let builders = TextIndexBuilders::new(self.files.len(), self.encoding, &self.text_paths);
        // FFF writes each posting word with a plain OR. A worker must own
        // all 64 slots in that word or simultaneous writes can lose bits.
        self.files.par_chunks(64).enumerate().try_for_each(
            |(chunk_index, entries)| -> Result<()> {
                let mut bytes = Vec::new();
                for (within, entry) in entries.iter().enumerate() {
                    let Some(entry) = entry else { continue };
                    let slot = chunk_index * 64 + within;
                    let id = self.id(slot).expect("live text-index slot");
                    let path = source.path.join(format!("{id}.json"));
                    let mut file = source.file(id)?;
                    let before =
                        Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
                    if before != entry.stamp {
                        return Err(Error::Unstable(path));
                    }
                    bytes.clear();
                    file.read_to_end(&mut bytes).map_err(|e| io(&path, e))?;
                    let after =
                        Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
                    if before != after || bytes.len() as u64 != after.size {
                        return Err(Error::Unstable(path));
                    }
                    let content = if self.malformed.contains(&slot) {
                        project_malformed_text(
                            &crate::text::malformed_search_text(&bytes),
                            self.text_paths.len(),
                        )
                    } else {
                        match sonic_rs::from_slice::<SonicValue>(&bytes) {
                            Ok(parsed) => project_text(
                                &parsed,
                                &self.text_rules,
                                self.typed.fields(),
                                &self.text_paths,
                            ),
                            Err(_) => project_malformed_text(
                                &crate::text::malformed_search_text(&bytes),
                                self.text_paths.len(),
                            ),
                        }
                    };
                    builders.add(slot, &content);
                }
                Ok(())
            },
        )?;
        source.finish()?;
        self.text_index = Some(builders.finish());
        Ok(())
    }
}


fn path_matches_registered(query: &crate::JsonPath, registered: &crate::JsonPath) -> bool {
    use crate::JsonPathStep;
    let (mut query_at, mut registered_at) = (0, 0);
    while let Some(step) = registered.get(registered_at) {
        match step {
            JsonPathStep::Each => {
                if query_at == query.len() && registered_at + 1 == registered.len() {
                    return true;
                }
                if matches!(
                    query.get(query_at),
                    Some(JsonPathStep::Each | JsonPathStep::Index(_))
                ) {
                    query_at += 1;
                } else if !matches!(query.get(query_at), Some(JsonPathStep::Key(_))) {
                    return false;
                }
            }
            JsonPathStep::Key(name) => {
                if !matches!(query.get(query_at), Some(JsonPathStep::Key(other)) if other == name) {
                    return false;
                }
                query_at += 1;
            }
            JsonPathStep::Index(_) => return false,
        }
        registered_at += 1;
    }
    query_at == query.len()
}
