//! Stored ICU string keys and typed sort columns.

use std::cmp::Ordering;
use std::collections::HashMap;

use icu_collator::{CollatorBorrowed, options::CollatorOptions};
use icu_locale_core::Locale;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::inventory::{CollectionGeneration, FileEntry, RecordEntry};
use crate::model::{FieldPath, LogicalType, Number, PathRoot, RecordSlot, Value};
use crate::reference::{RecordContext, Schema, data_values, metadata_value};
use crate::workload::SortField;

pub const COLLATION_VERSION: &str = "icu_collator-2.4.0/compiled-data";


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
    fn rank(&self) -> u8 {
        match self {
            Self::Missing => 2,
            Self::Null => 1,
            _ => 0,
        }
    }

    pub fn compare(&self, other: &Self, descending: bool) -> Ordering {
        let rank = self.rank().cmp(&other.rank());
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
        if descending && self.rank() == 0 {
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
    pub locale: String,
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
    collator: CollatorBorrowed<'static>,
}


impl SortBuilder {
    pub fn new(paths: &[FieldPath], schema: &Schema, locale: &str) -> Result<Self, String> {
        let parsed: Locale = locale
            .parse()
            .map_err(|e| format!("invalid sort locale: {e}"))?;
        let collator = CollatorBorrowed::try_new(parsed.into(), CollatorOptions::default())
            .map_err(|e| e.to_string())?;
        let mut columns = HashMap::new();
        for path in paths {
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
                        locale: locale.into(),
                        collation_version: COLLATION_VERSION.into(),
                    },
                )
                .is_some()
            {
                return Err("duplicate sort field".into());
            }
        }
        Ok(Self { columns, collator })
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
                        record: entry,
                        file,
                        collection_name,
                        repository_root: root,
                    };
                    owned = metadata_value(field, &ctx)?;
                    Some(vec![&owned])
                }
                PathRoot::Data => data_values(record, &column.path.segments),
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
                        self.collator
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
                    .compare(&column.keys[b.0 as usize], false)
                    .then_with(|| {
                        generation.records[a.0 as usize]
                            .id
                            .cmp(&generation.records[b.0 as usize].id)
                    })
            });
        }
        Ok(SortCollection {
            columns: self.columns,
        })
    }
}


impl SortCollection {
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
