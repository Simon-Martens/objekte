//! Derived-value accessors kept separate from the native typed posting codec.

use super::*;


impl TypedKey {
    pub(crate) fn from_extracted(value: crate::extensions::TypedValue) -> crate::Result<Self> {
        Ok(match value {
            crate::extensions::TypedValue::Null => Self::Null,
            crate::extensions::TypedValue::Bool(value) => Self::Bool(value),
            crate::extensions::TypedValue::Text(value) => Self::String(value),
            crate::extensions::TypedValue::Number(value) => {
                let number: serde_json::Number = value.parse().map_err(|_| {
                    crate::Error::Request(format!(
                        "extractor emitted invalid JSON number {value:?}"
                    ))
                })?;
                Self::Number(number.as_str().to_owned())
            }
        })
    }
}


impl PathValues {
    pub(crate) fn missing() -> Self {
        Self {
            keys: Vec::new(),
            present: false,
            scalar_safe: true,
            complete_values: true,
            implicit_equivalent: true,
        }
    }
}


impl TypedIndex {
    pub(crate) fn has_derived_posting(&self, path: &JsonPath) -> bool {
        crate::extensions::is_derived_path(path)
            && self.dynamic.get(path).is_some_and(|posting| posting.fresh)
    }

    pub(crate) fn patch_derived(
        &mut self,
        path: &JsonPath,
        slot: usize,
        slots: usize,
        values: PathValues,
    ) {
        let Some(posting) = self.dynamic.get_mut(path) else {
            return;
        };
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
        posting.fresh = true;
    }

    pub(crate) fn derived_value_counts(&self, path: &JsonPath) -> Option<Vec<(TypedKey, usize)>> {
        let posting = self.dynamic.get(path).filter(|posting| posting.fresh)?;
        Some(
            posting
                .postings
                .iter()
                .filter(|(key, _)| !matches!(key, TypedKey::Null | TypedKey::Missing))
                .map(|(key, slots)| (key.clone(), distinct_slots(slots)))
                .filter(|(_, count)| *count > 0)
                .collect(),
        )
    }

    pub(crate) fn visit_derived<'a>(
        &'a self,
        path: &JsonPath,
        mut visit: impl FnMut(usize, &'a TypedKey),
    ) -> Option<()> {
        let posting = self
            .dynamic
            .get(path)
            .filter(|posting| posting.fresh && posting.complete_values)?;
        for (key, positions) in &posting.postings {
            if matches!(key, TypedKey::Missing) {
                continue;
            }
            for &slot in positions {
                visit(slot, key);
            }
        }
        Some(())
    }
}
