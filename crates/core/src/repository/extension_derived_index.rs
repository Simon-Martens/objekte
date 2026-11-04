//! Optional derived-value postings over selected provider records. Storage is
//! the same typed B-tree used by the JSON picker, with a separate source table.

use super::*;
use crate::{
    Direction, JsonPath,
    extensions::{Extraction, TypedValue},
    file_picker::{TypedIndex, TypedKey},
    json_filter::decimal_cmp,
};
use std::{cmp::Ordering, sync::Arc};


#[derive(Clone, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct SelectedDerivedKey {
    pub collection: String,
    pub path: JsonPath,
    pub selection: String,
    pub semantics: String,
}


impl SelectedDerivedKey {
    pub(super) fn new(
        collection: &Collection,
        path: &JsonPath,
        selection: &str,
        extensions: &crate::extensions::Extensions,
    ) -> Self {
        Self {
            collection: collection.name.clone(),
            path: path.clone(),
            selection: selection.into(),
            semantics: crate::config::fingerprint(
                "selected-derived-posting/v1",
                &(
                    collection.mapping_hash.as_str(),
                    collection.schema_hash.as_deref(),
                    extensions.fingerprint(),
                ),
            ),
        }
    }
}


pub(super) struct SelectedDerivedPosting {
    pub index: TypedIndex,
    slots: usize,
    scalar_safe: bool,
    order_asc: Option<Arc<Vec<u32>>>,
    order_desc: Option<Arc<Vec<u32>>>,
}


fn sort_rank(key: Option<&TypedKey>) -> u8 {
    match key {
        Some(TypedKey::Bool(_)) => 0,
        Some(TypedKey::Number(_)) => 1,
        Some(TypedKey::String(_)) => 2,
        Some(TypedKey::Null) => 3,
        Some(TypedKey::Missing) | None => 4,
    }
}

fn compare_sort_keys(
    left: Option<&TypedKey>,
    right: Option<&TypedKey>,
    direction: Direction,
) -> Ordering {
    let left_rank = sort_rank(left);
    let right_rank = sort_rank(right);
    if left_rank != right_rank {
        return left_rank.cmp(&right_rank);
    }
    let order = match (left, right) {
        (Some(TypedKey::Bool(left)), Some(TypedKey::Bool(right))) => left.cmp(right),
        (Some(TypedKey::Number(left)), Some(TypedKey::Number(right))) => decimal_cmp(left, right),
        (Some(TypedKey::String(left)), Some(TypedKey::String(right))) => left.cmp(right),
        _ => Ordering::Equal,
    };
    if direction == Direction::Desc && left_rank < 3 {
        order.reverse()
    } else {
        order
    }
}


impl SelectedDerivedPosting {
    pub(super) fn from_rows(
        path: JsonPath,
        rows: Vec<(TypedKey, usize)>,
        present: Vec<u64>,
        scalar_safe: bool,
        slots: usize,
    ) -> Self {
        let mut index = TypedIndex::new(Vec::new());
        index.replace_path(path, rows, present, scalar_safe, true, true);
        Self {
            index,
            slots,
            scalar_safe,
            order_asc: None,
            order_desc: None,
        }
    }

    pub(super) fn order(&mut self, path: &JsonPath, direction: Direction) -> Option<Arc<Vec<u32>>> {
        if !self.scalar_safe {
            return None;
        }
        let missing = match direction {
            Direction::Asc => self.order_asc.is_none(),
            Direction::Desc => self.order_desc.is_none(),
        };
        if missing {
            let order = {
                let mut slot_values = vec![None; self.slots];
                self.index.visit_derived(path, |slot, value| {
                    slot_values[slot] = Some(value);
                })?;
                let mut order = (0..self.slots as u32).collect::<Vec<_>>();
                order.sort_by(|&left, &right| {
                    compare_sort_keys(
                        slot_values[left as usize],
                        slot_values[right as usize],
                        direction,
                    )
                    .then_with(|| left.cmp(&right))
                });
                Arc::new(order)
            };
            match direction {
                Direction::Asc => self.order_asc = Some(order),
                Direction::Desc => self.order_desc = Some(order),
            }
        }
        match direction {
            Direction::Asc => self.order_asc.as_ref().map(Arc::clone),
            Direction::Desc => self.order_desc.as_ref().map(Arc::clone),
        }
    }
}


impl Repository {
    /// Build a process-local typed posting for one extractor over the selected
    /// source table, including registered providers and format decoders. The
    /// existing B-tree implementation owns keys and slot lists. An error or a
    /// changed request inventory leaves no partial posting visible.
    pub fn build_selected_derived_posting(
        &mut self,
        collection: &str,
        extractor: &str,
    ) -> Result<usize> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return Err(Error::Request(
                "selected derived postings require memory cache and non-hash freshness".into(),
            ));
        }
        let selected_collection = self.model.collection(collection)?.clone();
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no value extractors are registered".into()))?;
        let binding = extensions.bind_value(
            extractor,
            collection,
            extensions.schema(collection).map(Arc::as_ref),
        )?;
        if !binding.cacheable {
            return Err(Error::Request(format!(
                "volatile extractor {extractor} cannot be indexed"
            )));
        }
        let selected = if self.options.freshness == Freshness::MetadataOnOpen {
            if let Some(snapshot) = self.custom_source_snapshot.get(collection) {
                Arc::clone(&snapshot.sources)
            } else {
                self.metrics.enumerations += 1;
                Arc::new(self.source_descriptors(collection)?)
            }
        } else {
            self.metrics.enumerations += 1;
            Arc::new(self.source_descriptors(collection)?)
        };
        if selected.len() > u32::MAX as usize {
            return Err(Error::Request(
                "selected derived posting exceeds 2^32 source slots".into(),
            ));
        }
        let selection = crate::config::fingerprint("selected-sources/v1", selected.as_ref());
        let key =
            SelectedDerivedKey::new(&selected_collection, &binding.path, &selection, extensions);
        if self.selected_derived_indexes.contains_key(&key) {
            return Ok(selected.len());
        }
        let mut rows = Vec::new();
        let mut present = vec![0_u64; selected.len().div_ceil(64)];
        let mut scalar_safe = true;
        let mut bytes = Vec::new();
        let mut output = Vec::<TypedValue>::new();
        for (slot, source) in selected.iter().enumerate() {
            let decoder = extensions.decoder(source)?;
            if !decoder.descriptor().cacheable() {
                return Err(Error::Request(format!(
                    "volatile decoder {} cannot be indexed",
                    decoder.descriptor().identity
                )));
            }
            if let Some(provider) = extensions
                .source_providers()
                .find(|provider| provider.descriptor().identity == source.provider)
                && !provider.descriptor().cacheable()
            {
                return Err(Error::Request(format!(
                    "volatile source provider {} cannot be indexed",
                    provider.descriptor().identity
                )));
            }
            self.read_selected_source_bytes_into(source, &mut bytes)?;
            self.metrics.source_reads += 1;
            self.metrics.source_bytes += bytes.len() as u64;
            let mut records = 0;
            self.metrics.parses += 1;
            decoder.decode(&bytes, &mut |record| {
                records += 1;
                if records > 1 {
                    return Err(Error::Request(
                        "format decoder emitted multiple records".into(),
                    ));
                }
                output.clear();
                let state = binding.extractor.extract(record, &mut output)?;
                if state == Extraction::Missing && !output.is_empty() {
                    return Err(Error::Request(format!(
                        "extractor {} emitted values for a missing field",
                        binding.identity
                    )));
                }
                if state == Extraction::Present {
                    present[slot / 64] |= 1 << (slot % 64);
                } else {
                    rows.push((TypedKey::Missing, slot));
                }
                scalar_safe &= output.len() <= 1;
                for value in output.drain(..) {
                    rows.push((TypedKey::from_extracted(value)?, slot));
                }
                Ok(())
            })?;
            if records == 0 {
                return Err(Error::Request("format decoder emitted no record".into()));
            }
        }
        if self.options.freshness != Freshness::MetadataOnOpen {
            let latest = self.source_descriptors(collection)?;
            self.metrics.enumerations += 1;
            if latest != *selected {
                return Err(Error::Unstable(selected_collection.path));
            }
        }
        let posting = SelectedDerivedPosting::from_rows(
            binding.path,
            rows,
            present,
            scalar_safe,
            selected.len(),
        );
        self.selected_derived_indexes.insert(key, posting);
        Ok(selected.len())
    }

    pub(super) fn selected_derived_candidates(
        &self,
        key: &SelectedDerivedKey,
        predicate: &crate::repository::DerivedPredicate,
        comparison: Option<&TypedKey>,
        source_count: usize,
    ) -> Option<Vec<u64>> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return None;
        }
        let posting = self.selected_derived_indexes.get(key)?;
        let mut bits = vec![0_u64; source_count.div_ceil(64)];
        posting.index.visit_derived(&key.path, |slot, value| {
            if predicate.matches_one(value, comparison) {
                bits[slot / 64] |= 1 << (slot % 64);
            }
        })?;
        Some(bits)
    }
}
