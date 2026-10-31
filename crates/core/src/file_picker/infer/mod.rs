//! Explicit, on-demand JSON field inference and one-pass posting construction.

mod links;
mod markup;
mod profile;

pub(crate) use links::ReferenceInventory;

use super::{FilePicker, Source, Stamp, TypedKey};
use crate::{
    CollectionInference, Error, InferredKind, JsonPath, JsonPathStep, Result, config::fingerprint,
    error::io,
};
use profile::{FieldStats, PerDocument, scalar_kind};
use sonic_rs::{Value as SonicValue, ValueRef};
use std::{collections::BTreeMap, io::Read};


impl FilePicker {
    /// Scan the current source snapshot again and optionally replace inferred postings.
    pub(crate) fn infer_fields(
        &mut self,
        collection: &str,
        inventory: &ReferenceInventory,
        auto_index: bool,
    ) -> Result<CollectionInference> {
        self.refresh()?;
        let source = Source::open_scoped(&self.root, self.recursive)?;
        let entries = self
            .sorted
            .iter()
            .map(|&slot| {
                (
                    slot,
                    self.id(slot).expect("live inference slot").to_owned(),
                    self.files[slot]
                        .as_ref()
                        .expect("live inference slot")
                        .stamp
                        .clone(),
                )
            })
            .collect::<Vec<_>>();
        let source_snapshot = fingerprint(
            "inference-source/v1",
            &entries
                .iter()
                .map(|(_, id, stamp)| (id, stamp))
                .collect::<Vec<_>>(),
        );
        let mut fields = BTreeMap::<JsonPath, FieldStats>::new();
        let mut parsed_bits = vec![0u64; self.files.len().div_ceil(64)];
        let mut parsed = 0;
        let mut malformed = 0;
        for (slot, id, stamp) in &entries {
            let path = self.root.join(format!("{id}.json"));
            let mut file = source.file(id)?;
            let before = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
            if &before != stamp {
                return Err(Error::Unstable(path));
            }
            let mut raw = Vec::new();
            file.read_to_end(&mut raw).map_err(|e| io(&path, e))?;
            let after = Stamp::from_metadata(&file.metadata().map_err(|e| io(&path, e))?, &path)?;
            if before != after || raw.len() as u64 != after.size {
                return Err(Error::Unstable(path));
            }
            let Ok(document) = sonic_rs::from_slice::<SonicValue>(&raw) else {
                malformed += 1;
                continue;
            };
            parsed += 1;
            parsed_bits[slot / 64] |= 1u64 << (slot % 64);
            let mut observed = BTreeMap::new();
            visit(&document, &mut Vec::new(), &mut observed, 0);
            for (field_path, observation) in observed {
                fields
                    .entry(field_path.clone())
                    .or_insert_with(|| FieldStats::new(field_path))
                    .add(*slot, observation, inventory, &path, &raw, auto_index);
            }
        }
        source.finish()?;

        if auto_index {
            for path in std::mem::take(&mut self.inferred_paths) {
                self.typed.remove_dynamic_path(&path);
            }
        }
        let mut reported = Vec::with_capacity(fields.len());
        let mut indexed_paths = 0;
        for (_, stats) in fields {
            let (mut field, mut rows) = stats.finish(parsed);
            let indexable = matches!(
                field.kind,
                InferredKind::Boolean
                    | InferredKind::Integer
                    | InferredKind::Number
                    | InferredKind::Enum
            ) && !rows.is_empty();
            field.recommended_index = auto_index && indexable;
            if auto_index && indexable && !self.typed.has_posting(&field.path) {
                let mut present = vec![0u64; parsed_bits.len()];
                for (_, slot) in &rows {
                    present[slot / 64] |= 1u64 << (slot % 64);
                }
                for (word_index, &parsed_word) in parsed_bits.iter().enumerate() {
                    let mut missing = parsed_word & !present[word_index];
                    while missing != 0 {
                        let bit = missing.trailing_zeros() as usize;
                        rows.push((TypedKey::Missing, word_index * 64 + bit));
                        missing &= missing - 1;
                    }
                }
                self.typed.replace_path(
                    field.path.clone(),
                    rows,
                    present,
                    !field.multiple_values,
                    true,
                );
                self.inferred_paths.insert(field.path.clone());
            }
            field.indexed = self.typed.has_posting(&field.path);
            indexed_paths += usize::from(field.indexed);
            reported.push(field);
        }
        Ok(CollectionInference {
            collection: collection.to_owned(),
            files: entries.len(),
            parsed,
            malformed,
            fields: reported,
            indexed_paths,
            schema_written: false,
            auto_index_requested: auto_index,
            source_snapshot,
            schema_snapshot: None,
        })
    }

    pub(crate) fn inferred_paths(&self) -> Vec<JsonPath> {
        self.inferred_paths.iter().cloned().collect()
    }

    pub(crate) fn restore_inferred_paths(&mut self, paths: &std::collections::BTreeSet<JsonPath>) {
        self.inferred_paths = paths.clone();
    }
}


fn visit(
    value: &SonicValue,
    path: &mut JsonPath,
    observed: &mut BTreeMap<JsonPath, PerDocument>,
    depth: usize,
) {
    if depth == 128 {
        return;
    }
    match value.as_ref() {
        ValueRef::Object(fields) => {
            // Last duplicate key wins, matching JSON filter evaluation.
            let mut selected = BTreeMap::new();
            for (name, child) in fields {
                selected.insert(name, child);
            }
            for (name, child) in selected {
                path.push(JsonPathStep::Key(name.to_owned()));
                let (kind, scalar) = scalar_kind(child);
                observed
                    .entry(path.clone())
                    .or_default()
                    .observe(kind, scalar);
                visit(child, path, observed, depth + 1);
                path.pop();
            }
        }
        ValueRef::Array(items) => {
            for item in items {
                path.push(JsonPathStep::Each);
                let (kind, scalar) = scalar_kind(item);
                observed
                    .entry(path.clone())
                    .or_default()
                    .observe(kind, scalar);
                visit(item, path, observed, depth + 1);
                path.pop();
            }
        }
        _ => {}
    }
}
