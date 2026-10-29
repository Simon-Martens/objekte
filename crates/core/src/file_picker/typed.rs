//! In-memory ordered postings for top-level numeric, boolean, and enum fields.
//! Immutable postings cover the indexed snapshot; changed slots are included
//! conservatively until the next rebuild and verified against source JSON.
use crate::{
    json_filter::{TypedPlan, decimal_cmp},
    query::Operator,
};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
    ops::Bound::{Excluded, Included, Unbounded},
};


#[derive(Clone, Debug)]
pub(super) enum TypedKey {
    Bool(bool),
    Number(String),
    String(String),
}


impl TypedKey {
    pub(super) fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Bool(value) => Some(Self::Bool(*value)),
            Value::Number(value) => Some(Self::Number(value.as_str().to_owned())),
            Value::String(value) => Some(Self::String(value.clone())),
            _ => None,
        }
    }

    fn same_kind(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Bool(_), Self::Bool(_))
                | (Self::Number(_), Self::Number(_))
                | (Self::String(_), Self::String(_))
        )
    }
}


impl Ord for TypedKey {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Number(a), Self::Number(b)) => decimal_cmp(a, b),
            (Self::String(a), Self::String(b)) => a.cmp(b),
            (Self::Bool(_), _) => Ordering::Less,
            (Self::Number(_), Self::Bool(_)) => Ordering::Greater,
            (Self::Number(_), Self::String(_)) => Ordering::Less,
            (Self::String(_), _) => Ordering::Greater,
        }
    }
}


impl PartialOrd for TypedKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


impl PartialEq for TypedKey {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}


impl Eq for TypedKey {}


/// Immutable typed postings for the indexed collection snapshot.
#[derive(Default)]
pub(super) struct TypedIndex {
    fields: Vec<String>,
    postings: Vec<BTreeMap<TypedKey, Vec<usize>>>,
}


impl TypedIndex {
    pub(super) fn new(fields: Vec<String>) -> Self {
        let postings = (0..fields.len()).map(|_| BTreeMap::new()).collect();
        Self { fields, postings }
    }

    pub(super) fn fields(&self) -> &[String] {
        &self.fields
    }

    pub(super) fn replace_rows(&mut self, rows: Vec<Vec<(TypedKey, usize)>>) {
        self.postings = rows
            .into_iter()
            .map(|rows| {
                let mut map = BTreeMap::<TypedKey, Vec<usize>>::new();
                for (key, slot) in rows {
                    map.entry(key).or_default().push(slot);
                }
                map
            })
            .collect();
    }

    /// Evaluate necessary typed postings. Unknown AND children are skipped;
    /// any unknown OR child makes the union unbounded. Changed slots always
    /// bypass the immutable postings and are verified against source JSON.
    pub(super) fn candidate_bits(
        &self,
        plan: Option<&TypedPlan>,
        slot_count: usize,
        changed: &BTreeSet<usize>,
    ) -> Option<Vec<u64>> {
        let mut bits = self.plan_bits(plan?, slot_count)?;
        for &slot in changed {
            bits[slot / 64] |= 1 << (slot % 64);
        }
        Some(bits)
    }

    fn plan_bits(&self, plan: &TypedPlan, slot_count: usize) -> Option<Vec<u64>> {
        match plan {
            TypedPlan::Term(field, operator, value) => {
                self.term_bits(field, *operator, value, slot_count)
            }
            TypedPlan::And(parts) => {
                let mut combined: Option<Vec<u64>> = None;
                for part in parts {
                    if let Some(bits) = self.plan_bits(part, slot_count) {
                        combined = Some(match combined {
                            Some(mut old) => {
                                old.iter_mut()
                                    .zip(bits)
                                    .for_each(|(left, right)| *left &= right);
                                old
                            }
                            None => bits,
                        });
                    }
                }
                combined
            }
            TypedPlan::Or(parts) => {
                let mut combined = vec![0u64; slot_count.div_ceil(64)];
                for part in parts {
                    let bits = self.plan_bits(part, slot_count)?;
                    combined
                        .iter_mut()
                        .zip(bits)
                        .for_each(|(left, right)| *left |= right);
                }
                Some(combined)
            }
        }
    }

    fn term_bits(
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
        let key = TypedKey::from_value(value)?;
        if matches!(key, TypedKey::Bool(_)) && !matches!(operator, Operator::Eq) {
            return None;
        }
        let mut bits = vec![0u64; slot_count.div_ceil(64)];
        let postings = &self.postings[index];
        if matches!(operator, Operator::Eq) {
            if let Some(slots) = postings.get(&key) {
                for &slot in slots {
                    bits[slot / 64] |= 1 << (slot % 64);
                }
            }
        } else {
            // TypedKey orders numeric lexemes by exact decimal value. Range
            // bounds therefore avoid visiting unrelated numeric postings.
            let bounds = match operator {
                Operator::Lt => (Unbounded, Excluded(key.clone())),
                Operator::Le => (Unbounded, Included(key.clone())),
                Operator::Gt => (Excluded(key.clone()), Unbounded),
                Operator::Ge => (Included(key.clone()), Unbounded),
                Operator::Eq => unreachable!(),
            };
            for (candidate, slots) in postings.range(bounds) {
                if !candidate.same_kind(&key) {
                    continue;
                }
                for &slot in slots {
                    bits[slot / 64] |= 1 << (slot % 64);
                }
            }
        }
        Some(bits)
    }
}
