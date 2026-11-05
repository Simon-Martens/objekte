//! In-memory B-tree postings for explicitly indexed top-level scalar fields
//! and runtime paths. Refreshed files are patched in place: each slot moves
//! from its old keys to its new ones, as a database updates its indexes.
use crate::json_filter::{IndexOp, JsonPath, JsonPathStep, TypedPlan, decimal_cmp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ops::Bound::{Excluded, Included, Unbounded},
};

mod derived;


#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum TypedKey {
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

    /// Retain only the final posting key from the validated Sonic tree.
    pub(super) fn from_sonic(value: &SonicValue) -> Option<Self> {
        if let Some(raw) = value.as_raw_number() {
            let number: serde_json::Number = raw
                .as_str()
                .parse()
                .expect("projection validated every JSON number");
            return Some(Self::Number(number.as_str().to_owned()));
        }
        match value.as_ref() {
            ValueRef::Null => Some(Self::Null),
            ValueRef::Bool(value) => Some(Self::Bool(value)),
            ValueRef::String(value) => Some(Self::String(value.to_owned())),
            ValueRef::Number(_) => unreachable!("raw numbers enabled"),
            ValueRef::Array(_) | ValueRef::Object(_) => None,
        }
    }

    pub(crate) fn same_kind(&self, other: &Self) -> bool {
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


/// Typed postings for the collection, kept current by patching.
#[derive(Default, Serialize, Deserialize)]
pub(crate) struct TypedIndex {
    fields: Vec<String>,
    postings: Vec<BTreeMap<TypedKey, Vec<usize>>>,
    #[serde(with = "path_map")]
    dynamic: BTreeMap<JsonPath, DynamicPosting>,
    /// Current keys of every slot per static field, built on the first patch.
    #[serde(skip)]
    slot_keys: Option<Vec<SlotKeys>>,
}


pub(crate) struct ExportedPosting {
    pub rows: Vec<(TypedKey, usize)>,
    pub present: Vec<u64>,
    pub scalar_safe: bool,
    pub implicit_equivalent: bool,
}


/// One document's values at a runtime posting path.
pub(crate) struct PathValues {
    pub(super) keys: Vec<TypedKey>,
    pub(super) present: bool,
    pub(super) scalar_safe: bool,
    pub(super) complete_values: bool,
    pub(super) implicit_equivalent: bool,
}


const NO_KEY: u32 = u32::MAX;
const MANY_KEYS: u32 = u32::MAX - 1;


/// The keys each slot currently has in one posting map. Source files can
/// change outside objekte, so the old value of an edited file must come from
/// here: it is no longer on disk.
#[derive(Default)]
struct SlotKeys {
    keys: Vec<TypedKey>,
    /// A key ID per slot, `NO_KEY`, or `MANY_KEYS` for slots in `many`.
    single: Vec<u32>,
    many: HashMap<usize, Vec<u32>>,
}


impl SlotKeys {
    /// Index the current postings. Parallel builds append slots in worker
    /// order, so each list is sorted and deduplicated for binary search.
    fn build(postings: &mut BTreeMap<TypedKey, Vec<usize>>, slots: usize) -> Self {
        let mut index = Self {
            single: vec![NO_KEY; slots],
            ..Self::default()
        };
        for (key, list) in postings.iter_mut() {
            list.sort_unstable();
            list.dedup();
            let id = index.intern(key.clone());
            for &slot in list.iter() {
                index.add(slot, id);
            }
        }
        index
    }

    fn intern(&mut self, key: TypedKey) -> u32 {
        self.keys.push(key);
        u32::try_from(self.keys.len() - 1).expect("typed key IDs fit u32")
    }

    fn add(&mut self, slot: usize, id: u32) {
        if slot >= self.single.len() {
            self.single.resize(slot + 1, NO_KEY);
        }
        match self.single[slot] {
            NO_KEY => self.single[slot] = id,
            MANY_KEYS => self.many.get_mut(&slot).expect("listed slot").push(id),
            first => {
                self.many.insert(slot, vec![first, id]);
                self.single[slot] = MANY_KEYS;
            }
        }
    }

    fn take(&mut self, slot: usize) -> Vec<u32> {
        match self.single.get(slot).copied().unwrap_or(NO_KEY) {
            NO_KEY => Vec::new(),
            MANY_KEYS => {
                self.single[slot] = NO_KEY;
                self.many.remove(&slot).unwrap_or_default()
            }
            id => {
                self.single[slot] = NO_KEY;
                vec![id]
            }
        }
    }

    /// Move `slot` from its current keys to `keys` within `postings`.
    fn patch(
        &mut self,
        postings: &mut BTreeMap<TypedKey, Vec<usize>>,
        slot: usize,
        keys: Vec<TypedKey>,
    ) {
        for id in self.take(slot) {
            let key = &self.keys[id as usize];
            if let Some(list) = postings.get_mut(key) {
                if let Ok(at) = list.binary_search(&slot) {
                    list.remove(at);
                }
                if list.is_empty() {
                    postings.remove(key);
                }
            }
        }
        for key in keys {
            let list = postings.entry(key.clone()).or_default();
            // Repeated array values share one posting entry.
            if let Err(at) = list.binary_search(&slot) {
                list.insert(at, slot);
                let id = self.intern(key);
                self.add(slot, id);
            }
        }
    }
}


mod path_map {
    use super::*;
    use serde::{Deserializer, Serializer, ser::SerializeSeq};

    pub(super) fn serialize<S>(
        map: &BTreeMap<JsonPath, DynamicPosting>,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut sequence = serializer.serialize_seq(Some(map.len()))?;
        for (path, posting) in map {
            sequence.serialize_element(&(crate::json_filter::format_path(path), posting))?;
        }
        sequence.end()
    }

    pub(super) fn deserialize<'de, D>(
        deserializer: D,
    ) -> std::result::Result<BTreeMap<JsonPath, DynamicPosting>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let entries = Vec::<(String, DynamicPosting)>::deserialize(deserializer)?;
        let mut map = BTreeMap::new();
        for (name, posting) in entries {
            let path = crate::JsonFilter::parse_path(&name).map_err(serde::de::Error::custom)?;
            if map.insert(path, posting).is_some() {
                return Err(serde::de::Error::custom("duplicate posting path"));
            }
        }
        Ok(map)
    }
}


/// One caller-built path posting and its complete presence set.
#[derive(Serialize, Deserialize)]
struct DynamicPosting {
    postings: BTreeMap<TypedKey, Vec<usize>>,
    present: Vec<u64>,
    scalar_safe: bool,
    complete_values: bool,
    fresh: bool,
    /// Every file reaches the same values through this path's implicit
    /// spelling without `[*]`, so comparisons may use either spelling.
    implicit_equivalent: bool,
    #[serde(skip)]
    slot_keys: Option<SlotKeys>,
}


impl TypedIndex {
    pub(crate) fn new(fields: Vec<String>) -> Self {
        let postings = (0..fields.len()).map(|_| BTreeMap::new()).collect();
        Self {
            fields,
            postings,
            dynamic: BTreeMap::new(),
            slot_keys: None,
        }
    }

    pub(super) fn fields(&self) -> &[String] {
        &self.fields
    }

    /// Replace a slot's indexed values after its source file was read again.
    /// `fields` holds static field keys and `paths` the runtime paths in
    /// `dynamic_paths` order; empty inputs remove the slot, as for a deleted
    /// or malformed file. Flags can only become more conservative.
    pub(super) fn patch(
        &mut self,
        slot: usize,
        slots: usize,
        fields: &[(usize, TypedKey)],
        paths: Vec<PathValues>,
    ) {
        let postings = &mut self.postings;
        let slot_keys = self.slot_keys.get_or_insert_with(|| {
            postings
                .iter_mut()
                .map(|posting| SlotKeys::build(posting, slots))
                .collect()
        });
        for (field, (posting, keys)) in postings.iter_mut().zip(slot_keys).enumerate() {
            let new = fields
                .iter()
                .filter(|(index, _)| *index == field)
                .map(|(_, key)| key.clone())
                .collect();
            keys.patch(posting, slot, new);
        }
        let mut paths = paths.into_iter();
        for (path, posting) in &mut self.dynamic {
            if crate::extensions::is_derived_path(path) {
                // The refresh caller patches active definitions separately;
                // inactive persisted definitions cannot keep old postings.
                posting.fresh = false;
                continue;
            }
            let values = paths.next().unwrap_or_else(PathValues::missing);
            let keys = posting
                .slot_keys
                .get_or_insert_with(|| SlotKeys::build(&mut posting.postings, slots));
            keys.patch(&mut posting.postings, slot, values.keys);
            posting.present.resize(slots.div_ceil(64), 0);
            if values.present {
                posting.present[slot / 64] |= 1 << (slot % 64);
            } else {
                posting.present[slot / 64] &= !(1 << (slot % 64));
            }
            posting.scalar_safe &= values.scalar_safe;
            posting.complete_values &= values.complete_values;
            posting.implicit_equivalent &= values.implicit_equivalent;
        }
    }

    /// Match distinct scalar keys once, then expand their immutable postings.
    /// Changed and appended slots are verified from source by the caller.
    pub(super) fn text_matching_bits(
        &self,
        slot_count: usize,
        selected: Option<&BTreeSet<&str>>,
        rules: &crate::text::TextRules,
        mut matches: impl FnMut(&str, crate::TextFormat) -> bool,
    ) -> Option<Vec<u64>> {
        let mut bits: Option<Vec<u64>> = None;
        for (name, postings) in self.fields.iter().zip(&self.postings) {
            if selected.is_some_and(|fields| !fields.contains(name.as_str())) {
                continue;
            }
            let Some(format) = rules.scalar_format(name) else {
                continue;
            };
            let bits = bits.get_or_insert_with(|| vec![0_u64; slot_count.div_ceil(64)]);
            for (key, slots) in postings {
                let value = match key {
                    TypedKey::String(value) | TypedKey::Number(value) => value.as_str(),
                    TypedKey::Bool(true) => "true",
                    TypedKey::Bool(false) => "false",
                    TypedKey::Null => "null",
                    TypedKey::Missing => continue,
                };
                let format = if matches!(key, TypedKey::String(_)) {
                    format
                } else {
                    crate::TextFormat::Plain
                };
                if matches(value, format) {
                    for &slot in slots {
                        bits[slot / 64] |= 1 << (slot % 64);
                    }
                }
            }
        }
        bits
    }

    pub(super) fn valid_for_slots(&self, slots: usize) -> bool {
        self.postings.len() == self.fields.len()
            && self
                .postings
                .iter()
                .flat_map(|postings| postings.values())
                .chain(
                    self.dynamic
                        .values()
                        .flat_map(|posting| posting.postings.values()),
                )
                .all(|positions| positions.iter().all(|&slot| slot < slots))
            && self
                .dynamic
                .values()
                .all(|posting| posting.present.len() == slots.div_ceil(64))
    }

    pub(super) fn replace_rows(&mut self, rows: Vec<Vec<(TypedKey, usize)>>) {
        self.postings = rows.into_iter().map(postings_for_rows).collect();
        self.slot_keys = None;
    }

    pub(super) fn dynamic_paths(&self) -> Vec<JsonPath> {
        self.dynamic
            .keys()
            .filter(|path| !crate::extensions::is_derived_path(path))
            .cloned()
            .collect()
    }

    pub(crate) fn has_posting(&self, path: &JsonPath) -> bool {
        if crate::extensions::is_derived_path(path) {
            return false;
        }
        self.static_index(path).is_some() || self.dynamic.get(path).is_some_and(|field| field.fresh)
    }

    /// Export a complete dynamic posting for remapping into a new selected
    /// source generation. No old source content is reread.
    pub(crate) fn export_dynamic(&self, path: &JsonPath) -> Option<ExportedPosting> {
        let posting = self.dynamic.get(path).filter(|posting| posting.fresh)?;
        let rows = posting
            .postings
            .iter()
            .flat_map(|(key, slots)| slots.iter().map(move |&slot| (key.clone(), slot)))
            .collect();
        Some(ExportedPosting {
            rows,
            present: posting.present.clone(),
            scalar_safe: posting.scalar_safe,
            implicit_equivalent: posting.implicit_equivalent,
        })
    }

    /// Every non-null value a usable posting holds for `path`, with the
    /// number of distinct slots holding it, in key order.
    pub(super) fn value_counts(&self, path: &JsonPath) -> Option<Vec<(TypedKey, usize)>> {
        let postings = self.posting_for(path)?;
        Some(
            postings
                .iter()
                .filter(|(key, _)| !matches!(key, TypedKey::Null | TypedKey::Missing))
                .map(|(key, slots)| (key.clone(), distinct_slots(slots)))
                .filter(|(_, count)| *count > 0)
                .collect(),
        )
    }

    pub(super) fn remove_dynamic_path(&mut self, path: &JsonPath) {
        self.dynamic.remove(path);
    }

    pub(crate) fn scalar_safe(&self, path: &JsonPath) -> bool {
        self.comparison_posting(path)
            .is_some_and(|field| field.scalar_safe && field.fresh)
    }

    /// The runtime posting a comparison on `path` can use: one built for the
    /// same spelling, or a `[*]` spelling whose values equal the implicit
    /// traversal in every file. Presence tests use only the same spelling,
    /// because `exists(tags)` holds for an empty array and `exists(tags[*])`
    /// does not.
    fn comparison_posting(&self, path: &JsonPath) -> Option<&DynamicPosting> {
        if crate::extensions::is_derived_path(path) {
            return None;
        }
        self.dynamic.get(path).or_else(|| {
            if self.static_index(path).is_some() {
                return None;
            }
            self.dynamic.iter().find_map(|(built, posting)| {
                (posting.implicit_equivalent
                    && posting.fresh
                    && built
                        .iter()
                        .filter(|step| !matches!(step, JsonPathStep::Each))
                        .eq(path.iter()))
                .then_some(posting)
            })
        })
    }

    pub(crate) fn replace_path(
        &mut self,
        path: JsonPath,
        rows: Vec<(TypedKey, usize)>,
        present: Vec<u64>,
        scalar_safe: bool,
        complete_values: bool,
        implicit_equivalent: bool,
    ) {
        let postings = postings_for_rows(rows);
        if let Some(index) = self.static_index(&path) {
            self.postings[index] = postings;
            self.slot_keys = None;
        } else {
            self.dynamic.insert(
                path,
                DynamicPosting {
                    postings,
                    present,
                    scalar_safe,
                    complete_values,
                    fresh: true,
                    implicit_equivalent,
                    slot_keys: None,
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
        if crate::extensions::is_derived_path(path) {
            return None;
        }
        if let Some(field) = self.comparison_posting(path) {
            return (field.complete_values && field.fresh).then_some(&field.postings);
        }
        self.static_index(path).map(|index| &self.postings[index])
    }

    /// Evaluate necessary typed postings. Unknown AND children are skipped;
    /// any unknown OR child makes the union unbounded. Changed slots always
    /// bypass the immutable postings and are verified against source JSON.
    pub(crate) fn candidate_bits(
        &self,
        plan: Option<&TypedPlan>,
        slot_count: usize,
    ) -> Option<Vec<u64>> {
        // Changed slots are patched into their current keys, so the postings
        // are exact for them too.
        self.plan_bits(plan?, slot_count)
    }

    /// Bound on candidate cardinality from posting metadata, without
    /// materializing a bitset. Unknown AND terms cannot raise the minimum;
    /// unknown OR terms make the whole estimate unavailable.
    pub(crate) fn cheap_candidate_estimate(
        &self,
        plan: &TypedPlan,
        slot_count: usize,
    ) -> Option<usize> {
        match plan {
            TypedPlan::Term(path, IndexOp::Eq, value)
            | TypedPlan::AnyTerm(path, IndexOp::Eq, value) => {
                let posting = self.posting_for(path)?;
                let key = TypedKey::from_value(value)?;
                Some(posting.get(&key).map_or(0, Vec::len))
            }
            TypedPlan::Present(path) => {
                self.dynamic
                    .get(path)
                    .filter(|posting| posting.fresh)
                    .map(|posting| {
                        posting
                            .present
                            .iter()
                            .map(|word| word.count_ones() as usize)
                            .sum()
                    })
            }
            TypedPlan::Missing(path) => self
                .dynamic
                .get(path)
                .filter(|posting| posting.fresh)
                .map(|posting| posting.postings.get(&TypedKey::Missing).map_or(0, Vec::len)),
            TypedPlan::And(parts) => parts
                .iter()
                .filter_map(|part| self.cheap_candidate_estimate(part, slot_count))
                .min(),
            TypedPlan::Or(parts) => {
                let mut total = 0_usize;
                for part in parts {
                    total = total.saturating_add(self.cheap_candidate_estimate(part, slot_count)?);
                }
                Some(total.min(slot_count))
            }
            _ => None,
        }
    }

    fn plan_bits(&self, plan: &TypedPlan, slot_count: usize) -> Option<Vec<u64>> {
        match plan {
            TypedPlan::Term(path, operator, value) => {
                self.term_bits(path, *operator, value, slot_count)
            }
            TypedPlan::AnyTerm(path, operator, value) => {
                self.any_term_bits(path, *operator, value, slot_count)
            }
            TypedPlan::Missing(path) => self.missing_bits(path, slot_count),
            TypedPlan::Present(path) => self.present_bits(path),
            TypedPlan::Prefix(path, prefix) => self.prefix_bits(path, prefix, slot_count),
            TypedPlan::Like(path, pattern, ignore_case) => {
                self.like_bits(path, pattern, *ignore_case, slot_count)
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

    pub(super) fn any_term_bits(
        &self,
        path: &JsonPath,
        operator: IndexOp,
        value: &Value,
        slot_count: usize,
    ) -> Option<Vec<u64>> {
        // A schema scalar posting may omit invalid array values. Only a
        // complete runtime posting can decide an existential comparison.
        self.comparison_posting(path)
            .filter(|field| field.fresh && field.complete_values)?;
        self.term_bits(path, operator, value, slot_count)
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

    fn like_bits(
        &self,
        path: &JsonPath,
        pattern: &str,
        ignore_case: bool,
        slot_count: usize,
    ) -> Option<Vec<u64>> {
        let postings = self.posting_for(path)?;
        let matcher = crate::json_filter::like_pattern(pattern, ignore_case);
        let mut bits = vec![0_u64; slot_count.div_ceil(64)];
        for (key, slots) in postings {
            if let TypedKey::String(value) = key
                && matcher.matches(value)
            {
                for &slot in slots {
                    bits[slot / 64] |= 1 << (slot % 64);
                }
            }
        }
        Some(bits)
    }
}


/// Slots in a posting list. Lists are sorted and deduplicated lazily, and a
/// record holding a value twice must count once.
fn distinct_slots(slots: &[usize]) -> usize {
    if slots.windows(2).all(|pair| pair[0] < pair[1]) {
        return slots.len();
    }
    let mut sorted = slots.to_vec();
    sorted.sort_unstable();
    sorted.dedup();
    sorted.len()
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
