//! Stored ICU string keys and typed sort columns.

use std::cmp::Ordering;
use std::collections::HashMap;

use icu_collator::{
    CollatorBorrowed, CollatorPreferences,
    options::{CollatorOptions, Strength},
    preferences::CollationNumericOrdering,
};
use icu_locale_core::Locale;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::inventory::{CollectionGeneration, FileEntry, FileInventory, RecordEntry};
use crate::model::{FieldPath, LogicalType, MetadataField, Number, PathRoot, RecordSlot, Value};
use crate::reference::{RecordContext, Schema, data_values_small, metadata_value};
use crate::workload::SortField;

pub const COLLATION_VERSION: &str =
    "icu_collator-2.4.0/icu_collator_data-2.4.0/CLDR-49.0.0/Unicode-18.0.0";


#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum CollationStrength {
    #[default]
    LocaleDefault,
    Primary,
    Secondary,
    Tertiary,
    Quaternary,
    Identical,
}


#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub enum Placement {
    First,
    #[default]
    Last,
}


#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SortOptions {
    pub strength: CollationStrength,
    pub numeric: bool,
    pub nulls: Placement,
    pub missing: Placement,
}


#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SortConfig {
    pub locale: String,
    pub options: SortOptions,
}


#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SortKey {
    Missing,
    Null,
    String(Vec<u8>),
    Number(Number),
    Bool(bool),
    Date(Date),
    Datetime(OffsetDateTime),
    Unixtime(i64),
}


impl SortKey {
    fn rank(&self, options: SortOptions) -> i8 {
        match self {
            Self::Missing if options.missing == Placement::First => -1,
            Self::Missing => 2,
            Self::Null if options.nulls == Placement::First => -2,
            Self::Null => 1,
            _ => 0,
        }
    }

    pub fn compare(&self, other: &Self, descending: bool) -> Ordering {
        self.compare_with(other, descending, SortOptions::default())
    }

    pub fn compare_with(&self, other: &Self, descending: bool, options: SortOptions) -> Ordering {
        let rank = self.rank(options).cmp(&other.rank(options));
        if !rank.is_eq() {
            return rank;
        }
        let order = match (self, other) {
            (Self::String(a), Self::String(b)) => a.cmp(b),
            (Self::Number(a), Self::Number(b)) => a.cmp(b),
            (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
            (Self::Date(a), Self::Date(b)) => a.cmp(b),
            (Self::Datetime(a), Self::Datetime(b)) => a.cmp(b),
            (Self::Unixtime(a), Self::Unixtime(b)) => a.cmp(b),
            _ => Ordering::Equal,
        };
        if descending && self.rank(options) == 0 {
            order.reverse()
        } else {
            order
        }
    }
}


/// Precomputed sort keys for one configured field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SortColumn {
    pub path: FieldPath,
    pub kind: LogicalType,
    pub keys: Vec<SortKey>,
    pub ascending: Vec<RecordSlot>,
    pub descending: Vec<RecordSlot>,
    pub locale: String,
    pub options: SortOptions,
    pub collation_version: String,
}


/// All sort columns available for a collection.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SortCollection {
    pub columns: HashMap<FieldPath, SortColumn>,
}


/// Mutable keys accumulated during index construction.
pub struct SortBuilder {
    columns: HashMap<FieldPath, SortColumn>,
    collators: HashMap<FieldPath, CollatorBorrowed<'static>>,
}


impl SortBuilder {
    pub(crate) fn into_keys(self) -> HashMap<FieldPath, Vec<SortKey>> {
        self.columns.into_iter().map(|(path, column)| (path, column.keys)).collect()
    }

    pub(crate) fn append_keys(
        &mut self,
        keys: HashMap<FieldPath, Vec<SortKey>>,
        base: u32,
        batch_count: u32,
    ) -> Result<(), String> {
        if self.columns.len() != keys.len() {
            return Err("sort batch column coverage mismatch".into());
        }
        for (path, mut values) in keys {
            let column = self.columns.get_mut(&path).ok_or("sort batch column missing")?;
            if column.keys.len() != base as usize || values.len() != batch_count as usize {
                return Err("sort batch key coverage mismatch".into());
            }
            column.keys.append(&mut values);
        }
        Ok(())
    }

    pub fn new(
        paths: &[FieldPath],
        schema: &Schema,
        locale: &str,
        defaults: SortOptions,
        overrides: &HashMap<FieldPath, SortConfig>,
    ) -> Result<Self, String> {
        let mut columns = HashMap::new();
        let mut collators = HashMap::new();
        if overrides.keys().any(|path| !paths.contains(path)) {
            return Err("sort override is not a configured sort field".into());
        }
        for path in paths {
            let config = overrides.get(path).cloned().unwrap_or_else(|| SortConfig {
                locale: locale.into(),
                options: defaults,
            });
            let parsed: Locale = config
                .locale
                .parse()
                .map_err(|e| format!("invalid sort locale: {e}"))?;
            let mut prefs: CollatorPreferences = parsed.into();
            prefs.numeric_ordering = Some(if config.options.numeric {
                CollationNumericOrdering::True
            } else {
                CollationNumericOrdering::False
            });
            let mut options = CollatorOptions::default();
            options.strength = match config.options.strength {
                CollationStrength::LocaleDefault => None,
                CollationStrength::Primary => Some(Strength::Primary),
                CollationStrength::Secondary => Some(Strength::Secondary),
                CollationStrength::Tertiary => Some(Strength::Tertiary),
                CollationStrength::Quaternary => Some(Strength::Quaternary),
                CollationStrength::Identical => Some(Strength::Identical),
            };
            let collator = CollatorBorrowed::try_new(prefs, options).map_err(|e| e.to_string())?;
            collators.insert(path.clone(), collator);
            let field = schema.field(path)?;
            if field.multi || matches!(field.kind, LogicalType::Object | LogicalType::Array(_)) {
                return Err(format!(
                    "array/object sort needs explicit reduction: {}",
                    path.display()
                ));
            }
            if columns
                .insert(
                    path.clone(),
                    SortColumn {
                        path: path.clone(),
                        kind: field.kind,
                        keys: Vec::new(),
                        ascending: Vec::new(),
                        descending: Vec::new(),
                        locale: config.locale,
                        options: config.options,
                        collation_version: COLLATION_VERSION.into(),
                    },
                )
                .is_some()
            {
                return Err("duplicate sort field".into());
            }
        }
        Ok(Self { columns, collators })
    }

    pub fn add(
        &mut self,
        slot: RecordSlot,
        record: &Value,
        entry: &RecordEntry,
        file: &FileEntry,
        root: &std::path::Path,
        collection_name: &str,
    ) -> Result<(), String> {
        for column in self.columns.values_mut() {
            if column.keys.len() != slot.0 as usize {
                return Err("sort slots out of order".into());
            }
            let owned;
            let selected = match column.path.root {
                PathRoot::Metadata(field) => {
                    let ctx = RecordContext {
                        value: record,
                        source_bytes: None,
                        record: entry,
                        file,
                        collection_name,
                        repository_root: root,
                    };
                    owned = metadata_value(field, &ctx)?;
                    Some(smallvec::smallvec![&owned])
                }
                PathRoot::Data => data_values_small(record, &column.path.segments),
            };
            let key = match selected {
                None => SortKey::Missing,
                Some(values) if values.is_empty() => SortKey::Missing,
                Some(values) if values.len() > 1 => {
                    return Err(format!(
                        "multi-valued sort field: {}",
                        column.path.display()
                    ));
                }
                Some(values) => match values[0] {
                    Value::Null => SortKey::Null,
                    Value::String(text) => {
                        let mut key = Vec::new();
                        self.collators[&column.path]
                            .write_sort_key_to(text, &mut key)
                            .map_err(|e| e.to_string())?;
                        SortKey::String(key)
                    }
                    Value::Number(n) => SortKey::Number(*n),
                    Value::Bool(b) => SortKey::Bool(*b),
                    Value::Date(d) => SortKey::Date(*d),
                    Value::Datetime(t) => SortKey::Datetime(*t),
                    Value::Unixtime(t) => SortKey::Unixtime(*t),
                    _ => return Err(format!("unsupported sort value: {}", column.path.display())),
                },
            };
            column.keys.push(key);
        }
        Ok(())
    }

    pub fn finish(mut self, generation: &CollectionGeneration) -> Result<SortCollection, String> {
        for column in self.columns.values_mut() {
            if column.keys.len() != generation.records.len() {
                return Err("sort column coverage mismatch".into());
            }
            column.ascending = generation.records.iter().map(|r| r.slot).collect();
            column.ascending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], false, column.options)
                    .then_with(|| {
                        generation.records[a.0 as usize]
                            .id
                            .cmp(&generation.records[b.0 as usize].id)
                    })
            });
            column.descending = descending_order(&column.ascending, &column.keys, column.options);
        }
        Ok(SortCollection {
            columns: self.columns,
        })
    }
}


fn descending_order(
    ascending: &[RecordSlot],
    keys: &[SortKey],
    options: SortOptions,
) -> Vec<RecordSlot> {
    let mut result = Vec::with_capacity(ascending.len());
    let mut start = 0;
    while start < ascending.len() {
        let rank = keys[ascending[start].0 as usize].rank(options);
        let mut end = start + 1;
        while end < ascending.len() && keys[ascending[end].0 as usize].rank(options) == rank {
            end += 1;
        }
        if rank == 0 {
            let mut group_end = end;
            while group_end > start {
                let mut group_start = group_end - 1;
                while group_start > start
                    && keys[ascending[group_start - 1].0 as usize]
                        .compare_with(&keys[ascending[group_end - 1].0 as usize], false, options)
                        .is_eq()
                {
                    group_start -= 1;
                }
                result.extend_from_slice(&ascending[group_start..group_end]);
                group_end = group_start;
            }
        } else {
            result.extend_from_slice(&ascending[start..end]);
        }
        start = end;
    }
    result
}


impl SortCollection {
    pub fn refresh_file_status(
        &mut self,
        generation: &CollectionGeneration,
        inventory: &FileInventory,
    ) {
        for column in self.columns.values_mut() {
            if column.path.root != PathRoot::Metadata(MetadataField::FileStatus) {
                continue;
            }
            for record in &generation.records {
                let status = inventory.files[record.file.0 as usize].status.code();
                column.keys[record.slot.0 as usize] =
                    SortKey::Number(Number::integer(status as i128));
            }
            column.ascending = generation.records.iter().map(|r| r.slot).collect();
            column.ascending.sort_by(|a, b| {
                column.keys[a.0 as usize]
                    .compare_with(&column.keys[b.0 as usize], false, column.options)
                    .then_with(|| {
                        generation.records[a.0 as usize]
                            .id
                            .cmp(&generation.records[b.0 as usize].id)
                    })
            });
            column.descending = descending_order(&column.ascending, &column.keys, column.options);
        }
    }

    pub fn column(&self, field: &str) -> Result<&SortColumn, String> {
        let path = FieldPath::parse(field)?;
        self.columns
            .get(&path)
            .ok_or_else(|| format!("sort field is not registered: {field}"))
    }

    pub fn key(&self, field: &str, slot: RecordSlot) -> Result<&SortKey, String> {
        self.column(field)?
            .keys
            .get(slot.0 as usize)
            .ok_or("sort slot out of bounds".into())
    }

    pub fn validate_request(&self, fields: &[SortField]) -> Result<(), String> {
        for field in fields {
            self.key(&field.field, RecordSlot(0)).or_else(|e| {
                // An empty collection has no slot zero; the column still exists.
                if self.columns.contains_key(&FieldPath::parse(&field.field)?) {
                    Ok(&SortKey::Missing)
                } else {
                    Err(e)
                }
            })?;
        }
        Ok(())
    }
}


#[cfg(test)]
#[path = "sorting.test.rs"]
mod tests;
