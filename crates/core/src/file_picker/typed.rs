//! In-memory B-tree postings for explicitly indexed top-level scalar fields.
//! Immutable postings cover the indexed snapshot; changed slots are included
//! conservatively until the next rebuild and verified against source JSON.
use crate::json_filter::{IndexOp, JsonPath, JsonPathStep, TypedPlan, decimal_cmp};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ops::Bound::{Excluded, Included, Unbounded},
};


#[derive(Clone, Debug)]
pub(super) enum TypedKey {
    Null,
    Missing,
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
                | (Self::Missing, Self::Missing)
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
            (Self::Missing, Self::Missing) => Ordering::Equal,
            (Self::Missing, _) => Ordering::Less,
            (_, Self::Missing) => Ordering::Greater,
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
    dynamic: BTreeMap<JsonPath, DynamicPosting>,
}


/// One caller-built path posting and its complete presence set.
struct DynamicPosting {
    postings: BTreeMap<TypedKey, Vec<usize>>,
    present: Vec<u64>,
    scalar_safe: bool,
    fresh: bool,
}


impl TypedIndex {
    pub(super) fn new(fields: Vec<String>) -> Self {
        let postings = (0..fields.len()).map(|_| BTreeMap::new()).collect();
        Self {
            fields,
            postings,
            dynamic: BTreeMap::new(),
        }
    }

    pub(super) fn fields(&self) -> &[String] {
        &self.fields
    }

    pub(super) fn replace_rows(&mut self, rows: Vec<Vec<(TypedKey, usize)>>) {
        self.postings = rows.into_iter().map(postings_for_rows).collect();
    }

    pub(super) fn dynamic_paths(&self) -> Vec<JsonPath> {
        self.dynamic.keys().cloned().collect()
    }

    pub(super) fn invalidate_dynamic(&mut self) {
        for field in self.dynamic.values_mut() {
            field.fresh = false;
        }
    }

    pub(super) fn scalar_safe(&self, path: &JsonPath) -> bool {
        self.dynamic
            .get(path)
            .is_some_and(|field| field.scalar_safe && field.fresh)
    }

    pub(super) fn replace_path(
        &mut self,
        path: JsonPath,
        rows: Vec<(TypedKey, usize)>,
        present: Vec<u64>,
        scalar_safe: bool,
    ) {
        let postings = postings_for_rows(rows);
        if let Some(index) = self.static_index(&path) {
            self.postings[index] = postings;
        } else {
            self.dynamic.insert(
                path,
                DynamicPosting {
                    postings,
                    present,
                    scalar_safe,
                    fresh: true,
                },
            );
        }
    }

    fn static_index(&self, path: &JsonPath) -> Option<usize> {
        let [JsonPathStep::Key(name)] = path.as_slice() else {
            return None;
        };
        self.fields.binary_search(name).ok()
    }

    fn posting_for(&self, path: &JsonPath) -> Option<&BTreeMap<TypedKey, Vec<usize>>> {
        if let Some(field) = self.dynamic.get(path) {
            return (field.scalar_safe && field.fresh).then_some(&field.postings);
        }
        self.static_index(path).map(|index| &self.postings[index])
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
            TypedPlan::Missing(path) => self.missing_bits(path, slot_count),
            TypedPlan::Present(path) => self.present_bits(path),
            TypedPlan::Prefix(path, prefix) => self.prefix_bits(path, prefix, slot_count),
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
        path: &JsonPath,
        operator: IndexOp,
        value: &Value,
        slot_count: usize,
    ) -> Option<Vec<u64>> {
        let postings = self.posting_for(path)?;
        let key = TypedKey::from_value(value)?;
        if matches!(key, TypedKey::Bool(_) | TypedKey::Null)
            && !matches!(operator, IndexOp::Eq | IndexOp::Ne)
        {
            return None;
        }
        let mut bits = vec![0u64; slot_count.div_ceil(64)];
        match operator {
            IndexOp::Eq => {
                if let Some(slots) = postings.get(&key) {
                    for &slot in slots {
                        bits[slot / 64] |= 1 << (slot % 64);
                    }
                }
            }
            IndexOp::Ne => {
                // Missing fields are recorded for the index but do not match
                // JSON comparisons. Null and other scalar kinds may differ.
                for (candidate, slots) in postings {
                    if candidate != &key && !matches!(candidate, TypedKey::Missing) {
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

    pub(super) fn missing_bits(&self, path: &JsonPath, slot_count: usize) -> Option<Vec<u64>> {
        let postings = self
            .dynamic
            .get(path)
            .filter(|field| field.fresh)
            .map(|field| &field.postings)
            .or_else(|| self.static_index(path).map(|index| &self.postings[index]))?;
        let mut bits = vec![0u64; slot_count.div_ceil(64)];
        for &slot in postings.get(&TypedKey::Missing).into_iter().flatten() {
            bits[slot / 64] |= 1 << (slot % 64);
        }
        Some(bits)
    }

    pub(super) fn present_bits(&self, path: &JsonPath) -> Option<Vec<u64>> {
        self.dynamic
            .get(path)
            .filter(|field| field.fresh)
            .map(|field| field.present.clone())
    }

    pub(super) fn prefix_bits(
        &self,
        path: &JsonPath,
        prefix: &str,
        slot_count: usize,
    ) -> Option<Vec<u64>> {
        let postings = self.posting_for(path)?;
        let mut bits = vec![0u64; slot_count.div_ceil(64)];
        for (key, slots) in postings.range(TypedKey::String(prefix.to_owned())..) {
            let TypedKey::String(value) = key else { break };
            if !value.starts_with(prefix) {
                break;
            }
            for &slot in slots {
                bits[slot / 64] |= 1 << (slot % 64);
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
    let mut missing = Vec::new();
    for (key, slot) in rows {
        match key {
            TypedKey::Null => nulls.push(slot),
            TypedKey::Missing => missing.push(slot),
            TypedKey::Bool(value) => booleans[usize::from(value)].push(slot),
            TypedKey::Number(raw) => numbers.entry(raw).or_default().push(slot),
            TypedKey::String(raw) => strings.entry(raw).or_default().push(slot),
        }
    }
    if !nulls.is_empty() {
        postings.insert(TypedKey::Null, nulls);
    }
    if !missing.is_empty() {
        postings.insert(TypedKey::Missing, missing);
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
            TypedKey::Missing => (4, "missing"),
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
