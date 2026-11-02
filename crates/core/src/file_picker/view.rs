//! In-memory ordered view keys for explicitly indexed scalar fields.
use super::FileEntry;
#[cfg(test)]
use crate::query::scalar;
use crate::{
    Direction, Field,
    query::{Operator, Scalar, scalar_value},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::{cmp::Ordering, collections::BTreeMap};


/// Current sortable keys and live slot order for each typed field.
pub(super) struct ViewIndex {
    fields: Vec<String>,
    definitions: Vec<Option<Field>>,
    keys: Vec<Vec<Scalar>>,
    ordered: Vec<Vec<usize>>,
}


#[derive(Serialize)]
pub(super) struct SavedViewIndex<'a> {
    fields: &'a [String],
    keys: &'a [Vec<Scalar>],
    ordered: &'a [Vec<usize>],
}


/// Owned values read from a secondary-index snapshot.
#[derive(Deserialize)]
pub(super) struct RestoredViewIndex {
    pub(super) fields: Vec<String>,
    pub(super) keys: Vec<Vec<Scalar>>,
    pub(super) ordered: Vec<Vec<usize>>,
}


impl ViewIndex {
    pub(super) fn field_count(&self) -> usize {
        self.fields.len()
    }

    pub(super) fn saved(&self) -> SavedViewIndex<'_> {
        SavedViewIndex {
            fields: &self.fields,
            keys: &self.keys,
            ordered: &self.ordered,
        }
    }

    pub(super) fn can_restore(&self, saved: &RestoredViewIndex, slots: usize) -> bool {
        if saved.fields != self.fields
            || saved.keys.len() != self.fields.len()
            || saved.ordered.len() != self.fields.len()
            || saved.keys.iter().any(|keys| keys.len() != slots)
            || saved.ordered.iter().any(|order| order.len() != slots)
        {
            return false;
        }
        for order in &saved.ordered {
            let mut seen = vec![false; slots];
            for &slot in order {
                if slot >= slots || std::mem::replace(&mut seen[slot], true) {
                    return false;
                }
            }
        }
        true
    }

    pub(super) fn apply_restored(&mut self, saved: RestoredViewIndex) {
        self.keys = saved.keys;
        self.ordered = saved.ordered;
    }

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

    #[cfg(test)]
    pub(super) fn project(&self, id: &str, data: &Value) -> Vec<Scalar> {
        self.fields
            .iter()
            .zip(&self.definitions)
            .map(|(name, definition)| scalar(id, data, name, definition.as_ref()))
            .collect()
    }

    /// Project retained view scalars directly from the validated source tree.
    pub(super) fn project_sonic(
        &self,
        id: &str,
        fields: &BTreeMap<&str, &SonicValue>,
    ) -> Vec<Scalar> {
        self.fields
            .iter()
            .zip(&self.definitions)
            .map(|(name, definition)| scalar_sonic(id, name, definition.as_ref(), fields))
            .collect()
    }

    /// Each worker owns disjoint 64-slot slices of the final field columns.
    pub(super) fn project_sonic_into_columns(
        &self,
        id: &str,
        fields: &BTreeMap<&str, &SonicValue>,
        columns: &mut [&mut [Scalar]],
        within: usize,
    ) {
        debug_assert_eq!(columns.len(), self.fields.len());
        for ((name, definition), column) in self
            .fields
            .iter()
            .zip(&self.definitions)
            .zip(columns.iter_mut())
        {
            column[within] = scalar_sonic(id, name, definition.as_ref(), fields);
        }
    }

    pub(super) fn missing_keys(&self) -> Vec<Scalar> {
        vec![Scalar::Missing; self.fields.len()]
    }

    pub(super) fn ordered_columns(
        &self,
        columns: &[Vec<Scalar>],
        files: &[Option<FileEntry>],
        id_bytes: &[u8],
    ) -> Vec<Vec<usize>> {
        debug_assert_eq!(columns.len(), self.fields.len());
        (0..self.fields.len())
            .map(|field| {
                let mut slots = (0..files.len()).collect::<Vec<_>>();
                slots.sort_by(|&a, &b| compare_slots(&columns[field], a, b, files, id_bytes));
                slots
            })
            .collect()
    }

    pub(super) fn apply_columns(&mut self, columns: Vec<Vec<Scalar>>, ordered: Vec<Vec<usize>>) {
        self.keys = columns;
        self.ordered = ordered;
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


fn scalar_sonic(
    id: &str,
    name: &str,
    definition: Option<&Field>,
    fields: &BTreeMap<&str, &SonicValue>,
) -> Scalar {
    if name == "@id" {
        return Scalar::String(id.to_owned());
    }
    let Some(value) = fields.get(name) else {
        return Scalar::Missing;
    };
    if let Some(raw) = value.as_raw_number() {
        let number: serde_json::Number = raw
            .as_str()
            .parse()
            .expect("projection validated every JSON number");
        let number = Value::Number(number);
        if definition.is_some_and(|field| !crate::schema::indexable(field, &number)) {
            return Scalar::Invalid;
        }
        return scalar_value(Some(&number));
    }
    match value.as_ref() {
        ValueRef::Null => Scalar::Null,
        ValueRef::Bool(value) => {
            if definition.is_some_and(|field| field.kind != crate::ValueType::Boolean) {
                Scalar::Invalid
            } else {
                Scalar::Bool(value)
            }
        }
        ValueRef::String(value) => {
            if definition.is_some_and(|field| field.kind != crate::ValueType::String) {
                Scalar::Invalid
            } else {
                Scalar::String(value.to_owned())
            }
        }
        ValueRef::Number(_) => unreachable!("raw numbers enabled"),
        ValueRef::Array(_) | ValueRef::Object(_) => Scalar::Invalid,
    }
}

#[cfg(test)]
#[path = "view.test.rs"]
mod tests;

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
