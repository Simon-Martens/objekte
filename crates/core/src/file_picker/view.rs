//! In-memory ordered view keys for explicitly indexed scalar fields.
use super::FileEntry;
use crate::{
    Direction, Field,
    query::{Operator, Scalar, scalar, scalar_value},
};
use serde_json::Value;
use std::{cmp::Ordering, collections::BTreeMap};


/// Current sortable keys and live slot order for each typed field.
pub(super) struct ViewIndex {
    fields: Vec<String>,
    definitions: Vec<Option<Field>>,
    keys: Vec<Vec<Scalar>>,
    ordered: Vec<Vec<usize>>,
}


impl ViewIndex {
    pub(super) fn new(fields: &[String], definitions: &BTreeMap<String, Field>) -> Self {
        Self {
            fields: fields.to_vec(),
            definitions: fields
                .iter()
                .map(|name| definitions.get(name).cloned())
                .collect(),
            keys: vec![Vec::new(); fields.len()],
            ordered: vec![Vec::new(); fields.len()],
        }
    }

    pub(super) fn project(&self, id: &str, data: &Value) -> Vec<Scalar> {
        self.fields
            .iter()
            .zip(&self.definitions)
            .map(|(name, definition)| scalar(id, data, name, definition.as_ref()))
            .collect()
    }

    pub(super) fn fields(&self) -> &[String] {
        &self.fields
    }

    pub(super) fn missing_keys(&self) -> Vec<Scalar> {
        vec![Scalar::Missing; self.fields.len()]
    }

    pub(super) fn replace(
        &mut self,
        rows: Vec<(usize, Vec<Scalar>)>,
        files: &[Option<FileEntry>],
        id_bytes: &[u8],
    ) {
        self.keys = vec![vec![Scalar::Missing; files.len()]; self.fields.len()];
        for (slot, values) in rows {
            for (field, value) in values.into_iter().enumerate() {
                self.keys[field][slot] = value;
            }
        }
        self.ordered = (0..self.fields.len())
            .map(|field| {
                let mut slots = (0..files.len()).collect::<Vec<_>>();
                slots.sort_by(|&a, &b| compare_slots(&self.keys[field], a, b, files, id_bytes));
                slots
            })
            .collect();
    }

    pub(super) fn update(
        &mut self,
        slot: usize,
        values: Vec<Scalar>,
        files: &[Option<FileEntry>],
        id_bytes: &[u8],
    ) {
        for (field, value) in values.into_iter().enumerate() {
            let order = &mut self.ordered[field];
            if let Some(position) = order.iter().position(|&current| current == slot) {
                order.remove(position);
            }
            if self.keys[field].len() <= slot {
                self.keys[field].resize(slot + 1, Scalar::Missing);
            }
            self.keys[field][slot] = value;
            let position = order
                .binary_search_by(|&current| {
                    compare_slots(&self.keys[field], current, slot, files, id_bytes)
                })
                .unwrap_or_else(|position| position);
            order.insert(position, slot);
        }
    }

    pub(super) fn remove(&mut self, slot: usize) {
        for order in &mut self.ordered {
            if let Some(position) = order.iter().position(|&current| current == slot) {
                order.remove(position);
            }
        }
    }

    pub(super) fn ordered_slots(&self, field: &str, direction: Direction) -> Option<Vec<usize>> {
        let index = self
            .fields
            .binary_search_by(|name| name.as_str().cmp(field))
            .ok()?;
        let order = &self.ordered[index];
        if direction == Direction::Asc {
            return Some(order.clone());
        }
        // States retain their ascending order. Reverse only the order of
        // distinct valid values, keeping IDs ascending within equal keys.
        let valid_end = order.partition_point(|&slot| self.keys[index][slot].state() == 0);
        let mut result = Vec::with_capacity(order.len());
        let mut end = valid_end;
        while end > 0 {
            let mut start = end - 1;
            while start > 0
                && self.keys[index][order[start - 1]].cmp(&self.keys[index][order[end - 1]])
                    == Ordering::Equal
            {
                start -= 1;
            }
            result.extend_from_slice(&order[start..end]);
            end = start;
        }
        result.extend_from_slice(&order[valid_end..]);
        Some(result)
    }

    pub(super) fn filter_bits(
        &self,
        field: &str,
        operator: Operator,
        value: &Value,
        slot_count: usize,
    ) -> Option<Vec<u64>> {
        let index = self
            .fields
            .binary_search_by(|name| name.as_str().cmp(field))
            .ok()?;
        let rhs = scalar_value(Some(value));
        let order = &self.ordered[index];
        let mut bits = vec![0u64; slot_count.div_ceil(64)];
        let selected = if rhs.state() == 1 {
            if !matches!(operator, Operator::Eq) {
                return Some(bits);
            }
            let start = order.partition_point(|&slot| self.keys[index][slot].state() == 0);
            let end = order.partition_point(|&slot| self.keys[index][slot].state() <= 1);
            &order[start..end]
        } else {
            let valid_end = order.partition_point(|&slot| self.keys[index][slot].state() == 0);
            let valid = &order[..valid_end];
            let lower = valid.partition_point(|&slot| self.keys[index][slot].cmp(&rhs).is_lt());
            let upper = valid.partition_point(|&slot| !self.keys[index][slot].cmp(&rhs).is_gt());
            match operator {
                Operator::Eq => &valid[lower..upper],
                Operator::Lt => &valid[..lower],
                Operator::Le => &valid[..upper],
                Operator::Gt => &valid[upper..],
                Operator::Ge => &valid[lower..],
            }
        };
        for &slot in selected {
            bits[slot / 64] |= 1 << (slot % 64);
        }
        Some(bits)
    }
}


fn compare_slots(
    keys: &[Scalar],
    a: usize,
    b: usize,
    files: &[Option<FileEntry>],
    id_bytes: &[u8],
) -> Ordering {
    keys[a].cmp(&keys[b]).then_with(|| {
        let id = |slot: usize| {
            let entry = files[slot].as_ref().expect("live ordered slot");
            &id_bytes[entry.id_start as usize..(entry.id_start + entry.id_len) as usize]
        };
        id(a).cmp(id(b))
    })
}
