//! Bounded per-path counts and candidate posting rows.

use super::{
    links::{LinkVotes, ReferenceInventory},
    markup::MarkupVotes,
};
use crate::file_picker::TypedKey;
use crate::{FieldInference, InferredKind, JsonPath};
use std::{collections::BTreeMap, path::Path};

const BOOLEAN: u8 = 1;
const NUMBER: u8 = 2;
const STRING: u8 = 4;
const ARRAY: u8 = 8;
const OBJECT: u8 = 16;
const NULL: u8 = 32;
const MAX_ENUM_VALUES: usize = 64;
const MAX_ENUM_KEY_BYTES: usize = 8 * 1024;
const MAX_ENUM_VALUE_BYTES: usize = 64;
const MAX_POSTING_ROWS: usize = 500_000;
const MAX_DOCUMENT_SCALARS: usize = 4_096;


/// All values reached at one path in one parseable file.
#[derive(Default)]
pub(super) struct PerDocument {
    pub kinds: u8,
    pub occurrences: usize,
    pub decimal_number: bool,
    pub scalars: Vec<TypedKey>,
    pub null_values: usize,
    pub truncated: bool,
}


impl PerDocument {
    pub(super) fn observe(&mut self, kind: u8, scalar: Option<TypedKey>) {
        self.kinds |= kind;
        self.occurrences += 1;
        if let Some(TypedKey::Number(value)) = &scalar {
            self.decimal_number |= value.contains(['.', 'e', 'E']);
        }
        self.null_values += usize::from(matches!(scalar, Some(TypedKey::Null)));
        if let Some(value) = scalar {
            if self.scalars.len() == MAX_DOCUMENT_SCALARS {
                self.truncated = true;
            } else if !self.truncated {
                self.scalars.push(value);
            }
        }
    }
}


/// Aggregate observations and still-provisional ordered posting rows.
pub(super) struct FieldStats {
    path: JsonPath,
    kinds: u8,
    present_records: usize,
    null_values: usize,
    decimal_number: bool,
    multiple_values: bool,
    enum_counts: BTreeMap<String, usize>,
    enum_nonblank_values_seen: usize,
    enum_nonblank_records: usize,
    enum_key_bytes: usize,
    enum_rejected: bool,
    links: LinkVotes,
    link_records: usize,
    markup: MarkupVotes,
    rows: Vec<(TypedKey, usize)>,
    rows_rejected: bool,
    semantic_rejected: bool,
}


impl FieldStats {
    pub(super) fn new(path: JsonPath) -> Self {
        Self {
            path,
            kinds: 0,
            present_records: 0,
            null_values: 0,
            decimal_number: false,
            multiple_values: false,
            enum_counts: BTreeMap::new(),
            enum_nonblank_values_seen: 0,
            enum_nonblank_records: 0,
            enum_key_bytes: 0,
            enum_rejected: false,
            links: LinkVotes::default(),
            link_records: 0,
            markup: MarkupVotes::default(),
            rows: Vec::new(),
            rows_rejected: false,
            semantic_rejected: false,
        }
    }

    pub(super) fn add(
        &mut self,
        slot: usize,
        observation: PerDocument,
        inventory: &ReferenceInventory,
        source_file: &Path,
        raw: &[u8],
        auto_index: bool,
    ) {
        self.present_records += 1;
        self.kinds |= observation.kinds;
        self.decimal_number |= observation.decimal_number;
        self.null_values += observation.null_values;
        if observation.truncated {
            self.rows.clear();
            self.rows_rejected = true;
            self.semantic_rejected = true;
            self.reject_enum();
        }
        if observation.occurrences > 1 {
            self.multiple_values = true;
        }
        if !self.semantic_rejected {
            let mut link_in_record = false;
            let mut enum_nonblank_in_record = false;
            for value in &observation.scalars {
                if let TypedKey::String(text) = value {
                    self.observe_enum(text);
                    enum_nonblank_in_record |= !text.trim().is_empty();
                    if !text.is_empty() {
                        link_in_record = true;
                    }
                    self.links.observe(text, inventory, source_file, raw);
                }
            }
            if link_in_record {
                self.link_records += 1;
            }
            if enum_nonblank_in_record {
                self.enum_nonblank_records += 1;
            }
            for value in observation
                .scalars
                .iter()
                .filter_map(|value| {
                    if let TypedKey::String(text) = value {
                        Some(text.as_str())
                    } else {
                        None
                    }
                })
                .take(4)
            {
                self.markup.observe(value);
            }
        }
        if auto_index && !self.rows_rejected {
            for scalar in &observation.scalars {
                if self.rows.len() == MAX_POSTING_ROWS {
                    self.rows.clear();
                    self.rows_rejected = true;
                    break;
                }
                self.rows.push((scalar.clone(), slot));
            }
            if self.enum_rejected
                && self.kinds & STRING != 0
                && self.kinds & (BOOLEAN | NUMBER) == 0
            {
                self.rows.clear();
            }
        }
    }

    fn observe_enum(&mut self, text: &str) {
        // Blanks remain candidate values for validation, but cannot establish an enum.
        self.enum_nonblank_values_seen += usize::from(!text.trim().is_empty());
        if self.enum_rejected {
            return;
        }
        if text.len() > MAX_ENUM_VALUE_BYTES {
            self.reject_enum();
            return;
        }
        if !self.enum_counts.contains_key(text) {
            self.enum_key_bytes += text.len();
            if self.enum_counts.len() == MAX_ENUM_VALUES || self.enum_key_bytes > MAX_ENUM_KEY_BYTES
            {
                self.reject_enum();
                return;
            }
        }
        *self.enum_counts.entry(text.to_owned()).or_default() += 1;
    }

    fn reject_enum(&mut self) {
        self.enum_rejected = true;
        self.enum_counts.clear();
        self.enum_key_bytes = 0;
        self.rows.clear();
    }

    pub(super) fn finish(self, parsed: usize) -> (FieldInference, Vec<(TypedKey, usize)>) {
        let non_null = self.kinds & !NULL;
        let mut kind = match non_null {
            0 => InferredKind::Null,
            BOOLEAN => InferredKind::Boolean,
            NUMBER if self.decimal_number => InferredKind::Number,
            NUMBER => InferredKind::Integer,
            STRING => InferredKind::String,
            ARRAY => InferredKind::Array,
            OBJECT => InferredKind::Object,
            _ => InferredKind::Mixed,
        };
        let enum_values = if kind == InferredKind::String
            && !self.enum_rejected
            && self.enum_nonblank_values_seen >= 20
            && self.enum_nonblank_records >= 10
            && self
                .enum_counts
                .keys()
                .filter(|value| !value.trim().is_empty())
                .count()
                <= (self.enum_nonblank_values_seen / 10).max(4)
            && self
                .enum_counts
                .iter()
                .filter(|entry| !entry.0.trim().is_empty() && *entry.1 >= 2)
                .map(|entry| *entry.1)
                .sum::<usize>()
                * 5
                >= self.enum_nonblank_values_seen * 4
        {
            kind = InferredKind::Enum;
            Some(self.enum_counts.into_keys().collect())
        } else {
            None
        };
        let link = if !self.semantic_rejected
            && matches!(kind, InferredKind::String | InferredKind::Enum)
        {
            self.links.inferred(self.link_records)
        } else {
            None
        };
        let text_format = if !self.semantic_rejected
            && matches!(kind, InferredKind::String | InferredKind::Enum)
        {
            self.markup.format()
        } else {
            None
        };
        let display_path = crate::json_filter::format_path(&self.path);
        (
            FieldInference {
                path: self.path,
                display_path,
                kind,
                present_records: self.present_records,
                missing_records: parsed.saturating_sub(self.present_records),
                null_values: self.null_values,
                multiple_values: self.multiple_values,
                enum_values,
                link,
                text_format,
                recommended_index: false,
                indexed: false,
            },
            if self.rows_rejected {
                Vec::new()
            } else {
                self.rows
            },
        )
    }
}


pub(super) fn scalar_kind(value: &sonic_rs::Value) -> (u8, Option<TypedKey>) {
    use sonic_rs::{JsonValueTrait, ValueRef};
    if let Some(number) = value.as_raw_number() {
        return (NUMBER, Some(TypedKey::Number(number.as_str().to_owned())));
    }
    match value.as_ref() {
        ValueRef::Null => (NULL, Some(TypedKey::Null)),
        ValueRef::Bool(value) => (BOOLEAN, Some(TypedKey::Bool(value))),
        ValueRef::String(value) => (STRING, Some(TypedKey::String(value.to_owned()))),
        ValueRef::Number(_) => (
            NUMBER,
            value
                .as_number()
                .map(|number| TypedKey::Number(number.to_string())),
        ),
        ValueRef::Array(_) => (ARRAY, None),
        ValueRef::Object(_) => (OBJECT, None),
    }
}
