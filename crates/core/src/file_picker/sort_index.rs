//! Schema-declared locale-aware orders over stable file slots.

use super::FileEntry;
use crate::{
    Direction, Field, Result, Sort,
    sorting::{SortPlan, SortValue},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{borrow::Cow, cmp::Ordering, collections::BTreeMap};

pub(super) struct SortIndex {
    definitions: Vec<Vec<Sort>>,
    plans: Vec<SortPlan>,
    keys: Vec<Vec<Vec<SortValue>>>,
    ordered: Vec<Vec<usize>>,
}

#[derive(Serialize)]
pub(super) struct SavedSortIndex<'a> {
    definitions: &'a [Vec<Sort>],
    keys: &'a [Vec<Vec<SortValue>>],
    ordered: &'a [Vec<usize>],
}

#[derive(Deserialize)]
pub(super) struct RestoredSortIndex {
    definitions: Vec<Vec<Sort>>,
    keys: Vec<Vec<Vec<SortValue>>>,
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
        Ok(Self {
            definitions: definitions.to_vec(),
            keys: vec![Vec::new(); plans.len()],
            ordered: vec![Vec::new(); plans.len()],
            plans,
        })
    }

    pub(super) fn is_empty(&self) -> bool {
        self.plans.is_empty()
    }

    pub(super) fn project(&self, id: &str, data: &Value) -> Result<Vec<Vec<SortValue>>> {
        self.plans
            .iter()
            .map(|plan| plan.project(id, data))
            .collect()
    }

    pub(super) fn missing_keys(&self) -> Vec<Vec<SortValue>> {
        self.plans.iter().map(SortPlan::missing_keys).collect()
    }

    pub(super) fn replace(
        &mut self,
        rows: Vec<(usize, Vec<Vec<SortValue>>)>,
        files: &[Option<FileEntry>],
        ids: &[u8],
    ) {
        self.keys = self
            .plans
            .iter()
            .map(|plan| vec![plan.missing_keys(); files.len()])
            .collect();
        for (slot, values) in rows {
            for (index, key) in values.into_iter().enumerate() {
                self.keys[index][slot] = key;
            }
        }
        self.ordered = self
            .plans
            .iter()
            .enumerate()
            .map(|(index, plan)| {
                let mut order = (0..files.len()).collect::<Vec<_>>();
                order.sort_by(|&a, &b| compare(plan, &self.keys[index], a, b, files, ids));
                order
            })
            .collect();
    }

    pub(super) fn update(
        &mut self,
        slot: usize,
        values: Vec<Vec<SortValue>>,
        files: &[Option<FileEntry>],
        ids: &[u8],
    ) {
        for (index, value) in values.into_iter().enumerate() {
            let keys = &mut self.keys[index];
            if keys.len() <= slot {
                keys.resize(slot + 1, self.plans[index].missing_keys());
            }
            keys[slot] = value;
            let order = &mut self.ordered[index];
            if let Some(at) = order.iter().position(|&current| current == slot) {
                order.remove(at);
            }
            let position = order
                .binary_search_by(|&current| {
                    compare(&self.plans[index], keys, current, slot, files, ids)
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
        let keys = &self.keys[index];
        let mut descending = Vec::with_capacity(ascending.len());
        for rank in 0..=2 {
            let start = ascending.partition_point(|&slot| keys[slot][0].rank() < rank);
            let mut end = ascending.partition_point(|&slot| keys[slot][0].rank() <= rank);
            while end > start {
                let mut group = end - 1;
                while group > start
                    && keys[ascending[group - 1]][0]
                        .compare(&keys[ascending[end - 1]][0], Direction::Asc)
                        .is_eq()
                {
                    group -= 1;
                }
                descending.extend_from_slice(&ascending[group..end]);
                end = group;
            }
        }
        let valid_end = ascending.partition_point(|&slot| keys[slot][0].rank() < 3);
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
                &self.keys[index],
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
        let order = self.keys[index][a][0].compare(&self.keys[index][b][0], requested.direction);
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

    pub(super) fn restore(&mut self, saved: RestoredSortIndex, slots: usize) -> bool {
        if saved.definitions != self.definitions
            || saved.keys.len() != self.plans.len()
            || saved.ordered.len() != self.plans.len()
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
        self.keys = saved.keys;
        self.ordered = saved.ordered;
        true
    }
}

fn compare(
    plan: &SortPlan,
    keys: &[Vec<SortValue>],
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
    plan.compare(&keys[a], id(a), &keys[b], id(b))
}
