//! Typed B-tree dictionaries with record-slot postings and special states.

use std::collections::{BTreeMap, HashMap, HashSet};

use roaring::RoaringBitmap;
use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime};

use crate::model::{LogicalType, Number, RecordSlot, Value};
use crate::reference::{FieldDefinition, Schema, data_values};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
pub enum ScalarKey {
    String(String),
    Number(Number),
    Bool(bool),
    Date(Date),
    Datetime(OffsetDateTime),
    Unixtime(i64),
}

impl ScalarKey {
    pub fn from_value(value: &Value) -> Option<Self> {
        Some(match value {
            Value::String(s) => Self::String(s.clone()),
            Value::Number(n) => Self::Number(*n),
            Value::Bool(b) => Self::Bool(*b),
            Value::Date(d) => Self::Date(*d),
            Value::Datetime(t) => Self::Datetime(*t),
            Value::Unixtime(t) => Self::Unixtime(*t),
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostingIndex {
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub multi: bool,
    pub presence: RoaringBitmap,
    pub nulls: RoaringBitmap,
    pub values: BTreeMap<ScalarKey, RoaringBitmap>,
    pub lengths: BTreeMap<Number, RoaringBitmap>,
    pub covered_records: u32,
}

impl PostingIndex {
    pub fn new(field: &FieldDefinition) -> Self {
        Self {
            path: field.path.clone(),
            kind: field.kind.clone(),
            multi: field.multi,
            presence: RoaringBitmap::new(),
            nulls: RoaringBitmap::new(),
            values: BTreeMap::new(),
            lengths: BTreeMap::new(),
            covered_records: 0,
        }
    }
    pub fn add(&mut self, slot: RecordSlot, record: &Value) -> Result<(), String> {
        self.covered_records = self
            .covered_records
            .checked_add(1)
            .ok_or("posting coverage overflow")?;
        let Some(selected) = data_values(record, &self.path) else {
            return Ok(());
        };
        self.presence.insert(slot.0);
        if selected.len() == 1 && matches!(selected[0], Value::Null) {
            self.nulls.insert(slot.0);
            return Ok(());
        }
        let mut unique = HashSet::new();
        for value in selected {
            if let Value::Array(array) = value {
                self.lengths
                    .entry(Number::integer(array.values.len() as i128))
                    .or_default()
                    .insert(slot.0);
                for item in &array.values {
                    if let Some(key) = ScalarKey::from_value(item) {
                        unique.insert(key);
                    }
                }
            } else if let Some(key) = ScalarKey::from_value(value) {
                unique.insert(key);
            }
        }
        for key in unique {
            self.values.entry(key).or_default().insert(slot.0);
        }
        Ok(())
    }
    pub fn validate(&self, record_count: usize) -> Result<(), String> {
        if self.covered_records as usize != record_count {
            return Err("incomplete posting coverage".into());
        }
        let valid = |set: &RoaringBitmap| set.max().is_none_or(|s| (s as usize) < record_count);
        if !valid(&self.presence)
            || !valid(&self.nulls)
            || !self.values.values().all(valid)
            || !self.lengths.values().all(valid)
        {
            return Err("posting contains out-of-domain record slot".into());
        }
        if !self.nulls.is_subset(&self.presence) {
            return Err("null posting is not present".into());
        }
        Ok(())
    }
    pub fn distinct_values(&self) -> usize {
        self.values.len()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PostingCollection {
    pub fields: HashMap<Vec<String>, PostingIndex>,
}
impl PostingCollection {
    pub fn new(schema: &Schema) -> Self {
        let fields = schema
            .fields
            .values()
            .filter(|f| f.indexed)
            .map(|f| (f.path.clone(), PostingIndex::new(f)))
            .collect();
        Self { fields }
    }
    pub fn add(&mut self, slot: RecordSlot, record: &Value) -> Result<(), String> {
        for index in self.fields.values_mut() {
            index.add(slot, record)?;
        }
        Ok(())
    }
    pub fn validate(&self, record_count: usize) -> Result<(), String> {
        for index in self.fields.values() {
            index.validate(record_count)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_null_array_dedup_and_cardinality() {
        let field = FieldDefinition {
            path: vec!["values".into()],
            kind: LogicalType::Array(Box::new(LogicalType::String)),
            required: false,
            nullable: true,
            indexed: true,
            multi: true,
        };
        let mut index = PostingIndex::new(&field);
        for (slot, json) in [
            serde_json::json!({}),
            serde_json::json!({"values":null}),
            serde_json::json!({"values":[]}),
            serde_json::json!({"values":["a","a","b"]}),
        ]
        .into_iter()
        .enumerate()
        {
            index
                .add(RecordSlot(slot as u32), &Value::from_json(json).unwrap())
                .unwrap();
        }
        index.validate(4).unwrap();
        assert_eq!(index.presence.iter().collect::<Vec<_>>(), [1, 2, 3]);
        assert_eq!(index.nulls.iter().collect::<Vec<_>>(), [1]);
        assert_eq!(
            index
                .lengths
                .get(&Number::integer(0))
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [2]
        );
        assert_eq!(
            index
                .lengths
                .get(&Number::integer(3))
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [3]
        );
        assert_eq!(
            index
                .values
                .get(&ScalarKey::String("a".into()))
                .unwrap()
                .iter()
                .collect::<Vec<_>>(),
            [3]
        );
    }
}
