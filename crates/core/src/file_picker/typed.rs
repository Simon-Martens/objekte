//! In-memory ordered postings for top-level numeric, boolean, and enum fields.
//! Immutable postings cover the indexed snapshot; changed slots are included
//! conservatively until the next rebuild and verified against source JSON.
use crate::json_filter::{IndexOp, TypedPlan, decimal_cmp};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ops::Bound::{Excluded, Included, Unbounded},
};


#[derive(Clone, Debug)]
pub(super) enum TypedKey {
    Null,
    Bool(bool),
    Number(String),
    String(String),
}


impl TypedKey {
    pub(super) fn from_value(value: &Value) -> Option<Self> {
        match value {
            Value::Null => Some(Self::Null),
            Value::Bool(value) => Some(Self::Bool(*value)),
            Value::Number(value) => Some(Self::Number(value.as_str().to_owned())),
            Value::String(value) => Some(Self::String(value.clone())),
            _ => None,
        }
    }

    fn same_kind(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (Self::Null, Self::Null)
                | (Self::Bool(_), Self::Bool(_))
                | (Self::Number(_), Self::Number(_))
                | (Self::String(_), Self::String(_))
        )
    }
}


impl Ord for TypedKey {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Null, Self::Null) => Ordering::Equal,
            (Self::Null, _) => Ordering::Less,
            (_, Self::Null) => Ordering::Greater,
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
        self.postings = rows.into_iter().map(postings_for_rows).collect();
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
            TypedPlan::Term(path, operator, value) => {
                self.term_bits(path, *operator, value, slot_count)
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

    pub(super) fn term_bits(
        &self,
        field: &str,
        operator: IndexOp,
        value: &Value,
        slot_count: usize,
    ) -> Option<Vec<u64>> {
        let index = self
            .fields
            .binary_search_by(|name| name.as_str().cmp(field))
            .ok()?;
        let key = TypedKey::from_value(value)?;
        if matches!(key, TypedKey::Bool(_) | TypedKey::Null)
            && !matches!(operator, IndexOp::Eq | IndexOp::Ne)
        {
            return None;
        }
        let mut bits = vec![0u64; slot_count.div_ceil(64)];
        let postings = &self.postings[index];
        match operator {
            IndexOp::Eq => {
                if let Some(slots) = postings.get(&key) {
                    for &slot in slots {
                        bits[slot / 64] |= 1 << (slot % 64);
                    }
                }
            }
            IndexOp::Ne => {
                // Missing fields and containers have no typed posting. Null and
                // other scalar kinds do compare unequal to a numeric value.
                for (candidate, slots) in postings {
                    if candidate != &key {
                        for &slot in slots {
                            bits[slot / 64] |= 1 << (slot % 64);
                        }
                    }
                }
            }
            IndexOp::Lt | IndexOp::Le | IndexOp::Gt | IndexOp::Ge => {
                // TypedKey orders numeric lexemes by exact decimal value. Range
                // bounds therefore avoid visiting unrelated numeric postings.
                let bounds = match operator {
                    IndexOp::Lt => (Unbounded, Excluded(key.clone())),
                    IndexOp::Le => (Unbounded, Included(key.clone())),
                    IndexOp::Gt => (Excluded(key.clone()), Unbounded),
                    IndexOp::Ge => (Included(key.clone()), Unbounded),
                    _ => unreachable!(),
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
        }
        Some(bits)
    }
}


fn postings_for_rows(rows: Vec<(TypedKey, usize)>) -> BTreeMap<TypedKey, Vec<usize>> {
    let mut postings = BTreeMap::<TypedKey, Vec<usize>>::new();
    if !should_group_rows(&rows) {
        for (key, slot) in rows {
            postings.entry(key).or_default().push(slot);
        }
        return postings;
    }

    let mut numbers = HashMap::<String, Vec<usize>>::new();
    let mut strings = HashMap::<String, Vec<usize>>::new();
    let mut booleans = [Vec::new(), Vec::new()];
    let mut nulls = Vec::new();
    for (key, slot) in rows {
        match key {
            TypedKey::Null => nulls.push(slot),
            TypedKey::Bool(value) => booleans[usize::from(value)].push(slot),
            TypedKey::Number(raw) => numbers.entry(raw).or_default().push(slot),
            TypedKey::String(raw) => strings.entry(raw).or_default().push(slot),
        }
    }
    if !nulls.is_empty() {
        postings.insert(TypedKey::Null, nulls);
    }
    for (value, slots) in booleans.into_iter().enumerate() {
        if !slots.is_empty() {
            postings.insert(TypedKey::Bool(value != 0), slots);
        }
    }
    for (raw, slots) in numbers {
        // Equal JSON numbers can have different spellings (1, 1.0, 1e0).
        // The ordered map merges those groups using exact decimal equality.
        postings
            .entry(TypedKey::Number(raw))
            .or_default()
            .extend(slots);
    }
    for (raw, slots) in strings {
        postings.insert(TypedKey::String(raw), slots);
    }
    postings
}

fn should_group_rows(rows: &[(TypedKey, usize)]) -> bool {
    if rows.len() < 2048 {
        return false;
    }
    // Mostly unique fields would pay for a hash table and the ordered map.
    // Sample raw spellings before choosing the grouping path.
    let sample = rows.iter().take(1024);
    let mut distinct = HashSet::new();
    for (key, _) in sample {
        let raw = match key {
            TypedKey::Null => (0, "null"),
            TypedKey::Bool(false) => (1, "false"),
            TypedKey::Bool(true) => (1, "true"),
            TypedKey::Number(raw) => (2, raw.as_str()),
            TypedKey::String(raw) => (3, raw.as_str()),
        };
        distinct.insert(raw);
    }
    distinct.len() * 4 < 1024 * 3
}

#[cfg(test)]
#[path = "typed.test.rs"]
mod tests;
