//! Schema-declared locale-aware orders over stable file slots.

use super::FileEntry;
use crate::{
    Direction, Field, Result, Sort,
    sorting::{SortPlan, SortValue},
};
use serde::{Deserialize, Serialize};
#[cfg(test)]
use serde_json::Value;
use sonic_rs::Value as SonicValue;
use std::{borrow::Cow, cmp::Ordering, collections::BTreeMap};


/// Locale-aware sort keys stored in slot-major, fixed-width rows.
pub(super) struct SortIndex {
    definitions: Vec<Vec<Sort>>,
    plans: Vec<SortPlan>,
    offsets: Vec<usize>,
    width: usize,
    /// All fixed-width sort terms for a slot are adjacent in this vector.
    keys: Vec<SortValue>,
    ordered: Vec<Vec<usize>>,
}


/// Borrowed sidecar representation of the retained sort keys and slot orders.
#[derive(Serialize)]
pub(super) struct SavedSortIndex<'a> {
    definitions: &'a [Vec<Sort>],
    keys: &'a [SortValue],
    ordered: &'a [Vec<usize>],
}


/// Owned sidecar representation checked before it replaces live sort data.
#[derive(Deserialize)]
pub(super) struct RestoredSortIndex {
    definitions: Vec<Vec<Sort>>,
    keys: Vec<SortValue>,
    ordered: Vec<Vec<usize>>,
}


impl SortIndex {
    pub(super) fn supports(&self, sorts: &[Sort]) -> bool {
        self.definitions
            .iter()
            .any(|definition| definition == sorts)
            || matches!(sorts, [requested] if requested.direction == Direction::Desc
                && self.definitions.iter().any(|definition| definition.len() == 1
                    && definition[0].field == requested.field
                    && definition[0].direction == Direction::Asc))
    }

    pub(super) fn new(
        definitions: &[Vec<Sort>],
        fields: &BTreeMap<String, Field>,
        language: Option<&str>,
    ) -> Result<Self> {
        let plans = definitions
            .iter()
            .map(|sorts| SortPlan::compile_fields_with_language(fields, sorts, language))
            .collect::<Result<Vec<_>>>()?;
        let mut offsets = Vec::with_capacity(plans.len());
        let mut width = 0usize;
        for plan in &plans {
            offsets.push(width);
            width = width
                .checked_add(plan.term_count())
                .ok_or_else(|| crate::Error::Request("too many sort terms".into()))?;
        }
        Ok(Self {
            definitions: definitions.to_vec(),
            offsets,
            width,
            keys: Vec::new(),
            ordered: vec![Vec::new(); plans.len()],
            plans,
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }

    pub(super) fn key_width(&self) -> usize {
        self.width
    }

    #[cfg(test)]
    pub(super) fn project(&self, id: &str, data: &Value) -> Result<Vec<Vec<SortValue>>> {
        self.plans
            .iter()
            .map(|plan| plan.project(id, data))
            .collect()
    }

    pub(super) fn project_sonic(&self, id: &str, data: &SonicValue) -> Result<Vec<Vec<SortValue>>> {
        self.plans
            .iter()
            .map(|plan| plan.project_sonic(id, data))
            .collect()
    }

    pub(super) fn project_sonic_into(
        &self,
        id: &str,
        data: &SonicValue,
        output: &mut [SortValue],
    ) -> Result<()> {
        debug_assert_eq!(output.len(), self.width);
        for (index, plan) in self.plans.iter().enumerate() {
            let start = self.offsets[index];
            plan.project_sonic_into(id, data, &mut output[start..start + plan.term_count()])?;
        }
        Ok(())
    }

    pub(super) fn missing_keys(&self) -> Vec<Vec<SortValue>> {
        self.plans.iter().map(SortPlan::missing_keys).collect()
    }

    pub(super) fn ordered_flat(
        &self,
        keys: &[SortValue],
        files: &[Option<FileEntry>],
        ids: &[u8],
    ) -> Vec<Vec<usize>> {
        assert_eq!(keys.len(), files.len() * self.width);
        self.plans
            .iter()
            .enumerate()
            .map(|(index, plan)| {
                let mut order = (0..files.len()).collect::<Vec<_>>();
                order.sort_by(|&a, &b| {
                    compare(
                        plan,
                        keys,
                        self.width,
                        self.offsets[index],
                        a,
                        b,
                        files,
                        ids,
                    )
                });
                order
            })
            .collect()
    }

    pub(super) fn apply_flat(&mut self, keys: Vec<SortValue>, ordered: Vec<Vec<usize>>) {
        self.keys = keys;
        self.ordered = ordered;
    }

    pub(super) fn update(
        &mut self,
        slot: usize,
        values: Vec<Vec<SortValue>>,
        files: &[Option<FileEntry>],
        ids: &[u8],
    ) {
        let needed = (slot + 1) * self.width;
        if self.keys.len() < needed {
            self.keys.resize(needed, SortValue::Missing);
        }
        for (index, value) in values.into_iter().enumerate() {
            let start = slot * self.width + self.offsets[index];
            for (target, value) in self.keys[start..start + value.len()].iter_mut().zip(value) {
                *target = value;
            }
            let order = &mut self.ordered[index];
            if let Some(at) = order.iter().position(|&current| current == slot) {
                order.remove(at);
            }
            let position = order
                .binary_search_by(|&current| {
                    compare(
                        &self.plans[index],
                        &self.keys,
                        self.width,
                        self.offsets[index],
                        current,
                        slot,
                        files,
                        ids,
                    )
                })
                .unwrap_or_else(|at| at);
            order.insert(position, slot);
        }
    }

    pub(super) fn remove(&mut self, slot: usize) {
        for order in &mut self.ordered {
            if let Some(at) = order.iter().position(|&current| current == slot) {
                order.remove(at);
            }
        }
    }

    pub(super) fn order(&self, sorts: &[Sort]) -> Option<Cow<'_, [usize]>> {
        if let Some(index) = self
            .definitions
            .iter()
            .position(|definition| definition == sorts)
        {
            return Some(Cow::Borrowed(&self.ordered[index]));
        }
        let [requested] = sorts else { return None };
        if requested.direction != Direction::Desc {
            return None;
        }
        let index = self.definitions.iter().position(|definition| {
            definition.len() == 1
                && definition[0].field == requested.field
                && definition[0].direction == Direction::Asc
        })?;
        let ascending = &self.ordered[index];
        let first_key = |slot: usize| &self.keys[slot * self.width + self.offsets[index]];
        let mut descending = Vec::with_capacity(ascending.len());
        // Reverse value groups, not the entire array: equal keys must keep
        // their ascending ID tie-break, including for DESC queries.
        for rank in 0..=2 {
            let start = ascending.partition_point(|&slot| first_key(slot).rank() < rank);
            let mut end = ascending.partition_point(|&slot| first_key(slot).rank() <= rank);
            while end > start {
                let mut group = end - 1;
                while group > start
                    && first_key(ascending[group - 1])
                        .compare(first_key(ascending[end - 1]), Direction::Asc)
                        .is_eq()
                {
                    group -= 1;
                }
                descending.extend_from_slice(&ascending[group..end]);
                end = group;
            }
        }
        let valid_end = ascending.partition_point(|&slot| first_key(slot).rank() < 3);
        descending.extend_from_slice(&ascending[valid_end..]);
        Some(Cow::Owned(descending))
    }

    pub(super) fn compare_slots(
        &self,
        sorts: &[Sort],
        a: usize,
        b: usize,
        files: &[Option<FileEntry>],
        ids: &[u8],
    ) -> Option<Ordering> {
        if let Some(index) = self
            .definitions
            .iter()
            .position(|definition| definition == sorts)
        {
            return Some(compare(
                &self.plans[index],
                &self.keys,
                self.width,
                self.offsets[index],
                a,
                b,
                files,
                ids,
            ));
        }
        let [requested] = sorts else { return None };
        let index = self.definitions.iter().position(|definition| {
            definition.len() == 1
                && definition[0].field == requested.field
                && definition[0].direction == Direction::Asc
        })?;
        let key = |slot: usize| &self.keys[slot * self.width + self.offsets[index]];
        let order = key(a).compare(key(b), requested.direction);
        if order.is_eq() {
            let id = |slot: usize| {
                let entry = files[slot].as_ref().expect("live sort slot");
                &ids[entry.id_start as usize..(entry.id_start + entry.id_len) as usize]
            };
            Some(id(a).cmp(id(b)))
        } else {
            Some(order)
        }
    }

    pub(super) fn saved(&self) -> SavedSortIndex<'_> {
        SavedSortIndex {
            definitions: &self.definitions,
            keys: &self.keys,
            ordered: &self.ordered,
        }
    }

    pub(super) fn can_restore(&self, saved: &RestoredSortIndex, slots: usize) -> bool {
        if saved.definitions != self.definitions
            || Some(saved.keys.len()) != slots.checked_mul(self.width)
            || saved.ordered.len() != self.plans.len()
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

    pub(super) fn apply_restored(&mut self, saved: RestoredSortIndex) {
        self.keys = saved.keys;
        self.ordered = saved.ordered;
    }
}


#[expect(
    clippy::too_many_arguments,
    reason = "comparison borrows fixed key storage and slot metadata"
)]
fn compare(
    plan: &SortPlan,
    keys: &[SortValue],
    width: usize,
    offset: usize,
    a: usize,
    b: usize,
    files: &[Option<FileEntry>],
    ids: &[u8],
) -> Ordering {
    let id = |slot: usize| {
        let entry = files[slot].as_ref().expect("live sort slot");
        std::str::from_utf8(&ids[entry.id_start as usize..(entry.id_start + entry.id_len) as usize])
            .expect("validated ID")
    };
    let terms = plan.term_count();
    plan.compare(
        &keys[a * width + offset..a * width + offset + terms],
        id(a),
        &keys[b * width + offset..b * width + offset + terms],
        id(b),
    )
}
