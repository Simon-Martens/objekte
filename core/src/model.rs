//! Format-independent values, paths, and stable record identities.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Serialize};
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub enum Number {
    Integer(i128),
    Float(f64),
}

impl Number {
    pub fn integer(value: i128) -> Self {
        Self::Integer(value)
    }

    pub fn float(value: f64) -> Result<Self, String> {
        if !value.is_finite() {
            return Err("non-finite number".into());
        }
        if value == 0.0 {
            return Ok(Self::Integer(0));
        }
        if value.fract() == 0.0 && value >= i128::MIN as f64 && value < 2f64.powi(127) {
            return Ok(Self::Integer(value as i128));
        }
        Ok(Self::Float(value))
    }

    pub fn from_json(value: &serde_json::Number) -> Result<Self, String> {
        if let Some(n) = value.as_i64() {
            return Ok(Self::Integer(n as i128));
        }
        if let Some(n) = value.as_u64() {
            return Ok(Self::Integer(n as i128));
        }
        value
            .as_f64()
            .ok_or_else(|| "unsupported JSON number".into())
            .and_then(Self::float)
    }
}

impl PartialEq for Number {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}
impl Eq for Number {}
impl PartialOrd for Number {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Number {
    fn cmp(&self, other: &Self) -> Ordering {
        match (*self, *other) {
            (Self::Integer(a), Self::Integer(b)) => a.cmp(&b),
            (Self::Float(a), Self::Float(b)) => a.total_cmp(&b),
            (Self::Integer(i), Self::Float(f)) => compare_int_float(i, f),
            (Self::Float(f), Self::Integer(i)) => compare_int_float(i, f).reverse(),
        }
    }
}
impl Hash for Number {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            Self::Integer(i) => {
                0u8.hash(state);
                i.hash(state);
            }
            Self::Float(f) => {
                // Keep Hash compatible with cross-representation numeric Eq,
                // even when a caller constructs Float directly.
                if f.is_finite()
                    && f.fract() == 0.0
                    && *f >= i128::MIN as f64
                    && *f < 2f64.powi(127)
                {
                    0u8.hash(state);
                    (*f as i128).hash(state);
                } else {
                    1u8.hash(state);
                    f.to_bits().hash(state);
                }
            }
        }
    }
}

fn compare_int_float(i: i128, f: f64) -> Ordering {
    // Outside the integer domain, floating-point values have a known order.
    if f >= 2f64.powi(127) {
        return Ordering::Less;
    }
    if f < i128::MIN as f64 {
        return Ordering::Greater;
    }
    let truncated = f.trunc() as i128;
    match i.cmp(&truncated) {
        Ordering::Equal => {
            if f.fract() > 0.0 {
                Ordering::Less
            } else if f.fract() < 0.0 {
                Ordering::Greater
            } else {
                Ordering::Equal
            }
        }
        result => result,
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum LogicalType {
    String,
    Number,
    Bool,
    Date,
    Datetime,
    Unixtime,
    Object,
    Array(Box<LogicalType>),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArrayValue {
    pub element_type: Option<LogicalType>,
    pub nullable_elements: bool,
    pub values: Vec<Value>,
}

impl ArrayValue {
    pub fn new(
        values: Vec<Value>,
        hint: Option<LogicalType>,
        nullable_elements: bool,
    ) -> Result<Self, String> {
        let mut element_type = hint;
        for value in &values {
            if matches!(value, Value::Null) {
                if !nullable_elements {
                    return Err("null array element is not allowed".into());
                }
                continue;
            }
            let kind = value.logical_type();
            if let Some(expected) = &element_type {
                if expected != &kind {
                    return Err(format!("mixed array types: {expected:?} and {kind:?}"));
                }
            } else {
                element_type = Some(kind);
            }
        }
        Ok(Self {
            element_type,
            nullable_elements,
            values,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    String(String),
    Number(Number),
    Bool(bool),
    Date(Date),
    Datetime(OffsetDateTime),
    Unixtime(i64),
    Object(BTreeMap<String, Value>),
    Array(ArrayValue),
}

impl Value {
    pub fn logical_type(&self) -> LogicalType {
        match self {
            Self::Null => panic!("null has no non-null logical type"),
            Self::String(_) => LogicalType::String,
            Self::Number(_) => LogicalType::Number,
            Self::Bool(_) => LogicalType::Bool,
            Self::Date(_) => LogicalType::Date,
            Self::Datetime(_) => LogicalType::Datetime,
            Self::Unixtime(_) => LogicalType::Unixtime,
            Self::Object(_) => LogicalType::Object,
            Self::Array(a) => LogicalType::Array(Box::new(
                a.element_type.clone().unwrap_or(LogicalType::Object),
            )),
        }
    }

    pub fn from_json(value: serde_json::Value) -> Result<Self, String> {
        Ok(match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::String(s) => Self::String(s),
            serde_json::Value::Number(n) => Self::Number(Number::from_json(&n)?),
            serde_json::Value::Bool(b) => Self::Bool(b),
            serde_json::Value::Array(a) => Self::Array(ArrayValue::new(
                a.into_iter()
                    .map(Self::from_json)
                    .collect::<Result<_, _>>()?,
                None,
                true,
            )?),
            serde_json::Value::Object(o) => Self::Object(
                o.into_iter()
                    .map(|(k, v)| Ok((k, Self::from_json(v)?)))
                    .collect::<Result<_, String>>()?,
            ),
        })
    }

    pub fn parse_typed_string(text: &str, kind: &LogicalType) -> Result<Self, String> {
        match kind {
            LogicalType::String => Ok(Self::String(text.into())),
            LogicalType::Date => {
                Date::parse(text, &time::format_description::well_known::Iso8601::DATE)
                    .map(Self::Date)
                    .map_err(|e| e.to_string())
            }
            LogicalType::Datetime => OffsetDateTime::parse(text, &Rfc3339)
                .map(Self::Datetime)
                .map_err(|e| e.to_string()),
            _ => Err(format!("string cannot be parsed as {kind:?}")),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CollectionId(pub u32);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct FileId(pub u32);
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecordSlot(pub u32);
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecordId(pub String);
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecordIdentity {
    pub collection: CollectionId,
    pub record: RecordId,
}

impl RecordId {
    pub fn intrinsic(text: impl Into<String>) -> Result<Self, String> {
        let text = text.into();
        if text.is_empty() || text.starts_with("~generated:") {
            return Err("invalid intrinsic record ID".into());
        }
        Ok(Self(text))
    }
    pub fn generated(file: FileId, modified_ns: i128, start: u64) -> Self {
        Self(format!("~generated:{}:{modified_ns}:{start}", file.0))
    }
}

#[derive(Default)]
pub struct IdentitySet(HashSet<RecordId>);
impl IdentitySet {
    pub fn insert(&mut self, id: RecordId) -> Result<(), String> {
        if !self.0.insert(id.clone()) {
            return Err(format!("duplicate record ID: {}", id.0));
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum PathRoot {
    Data,
    Metadata(MetadataField),
}

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct FieldPath {
    pub root: PathRoot,
    pub segments: Vec<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub enum MetadataField {
    RecordId,
    RecordByteStart,
    RecordByteEnd,
    RecordByteLength,
    FileId,
    FilePath,
    FileSize,
    FileCreated,
    FileModified,
    CollectionName,
    RepositoryRoot,
}

impl MetadataField {
    pub fn parse(name: &str) -> Result<Self, String> {
        match name {
            "@record.id" => Ok(Self::RecordId),
            "@record.byte_start" => Ok(Self::RecordByteStart),
            "@record.byte_end" => Ok(Self::RecordByteEnd),
            "@record.byte_length" => Ok(Self::RecordByteLength),
            "@file.id" => Ok(Self::FileId),
            "@file.path" => Ok(Self::FilePath),
            "@file.size" => Ok(Self::FileSize),
            "@file.created" => Ok(Self::FileCreated),
            "@file.modified" => Ok(Self::FileModified),
            "@collection.name" => Ok(Self::CollectionName),
            "@repository.root" => Ok(Self::RepositoryRoot),
            _ => Err(format!("unknown metadata field: {name}")),
        }
    }
}

impl FieldPath {
    pub fn parse(input: &str) -> Result<Self, String> {
        if input.starts_with('@') {
            return Ok(Self {
                root: PathRoot::Metadata(MetadataField::parse(input)?),
                segments: Vec::new(),
            });
        }
        let mut segments = Vec::new();
        let mut rest = input;
        while !rest.is_empty() {
            if rest.starts_with("[\"") || rest.starts_with("['") {
                let quote = rest.as_bytes()[1] as char;
                let end = rest[2..].find(quote).ok_or("unterminated quoted path")? + 2;
                let name = &rest[2..end];
                if rest.as_bytes().get(end + 1) != Some(&b']') {
                    return Err("missing closing bracket".into());
                }
                segments.push(name.to_owned());
                rest = &rest[end + 2..];
            } else {
                let end = rest.find(['.', '[']).unwrap_or(rest.len());
                if end == 0 {
                    return Err("empty path segment".into());
                }
                segments.push(rest[..end].to_owned());
                rest = &rest[end..];
            }
            if rest.starts_with("[*]") {
                rest = &rest[3..];
            }
            if rest.starts_with('.') {
                rest = &rest[1..];
            } else if !rest.is_empty() && !rest.starts_with('[') {
                return Err("invalid path separator".into());
            }
        }
        if segments.is_empty() {
            return Err("empty field path".into());
        }
        Ok(Self {
            root: PathRoot::Data,
            segments,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn precise_numbers_and_temporal_distinctions() {
        let a = Number::integer(9_007_199_254_740_993);
        let b = Number::integer(9_007_199_254_740_992);
        assert!(a > b);
        assert!(a > Number::float(9_007_199_254_740_992.0).unwrap());
        assert_eq!(Number::float(1.0).unwrap(), Number::integer(1));
        assert!(Number::float(f64::NAN).is_err());
        assert!(matches!(
            Value::parse_typed_string("2026-01-01", &LogicalType::Date).unwrap(),
            Value::Date(_)
        ));
        assert!(matches!(
            Value::parse_typed_string("2026-01-01T00:00:00Z", &LogicalType::Datetime).unwrap(),
            Value::Datetime(_)
        ));
        assert!(Value::parse_typed_string("2026-01-01T00:00:00", &LogicalType::Datetime).is_err());
        assert_ne!(
            Value::Unixtime(0).logical_type(),
            Value::Number(Number::integer(0)).logical_type()
        );
    }

    #[test]
    fn arrays_paths_and_id_namespace() {
        assert!(
            ArrayValue::new(
                vec![Value::String("x".into()), Value::Bool(true)],
                None,
                false
            )
            .is_err()
        );
        assert!(ArrayValue::new(vec![Value::Null], Some(LogicalType::String), false).is_err());
        assert_eq!(
            ArrayValue::new(vec![], None, false).unwrap().element_type,
            None
        );
        assert_eq!(
            FieldPath::parse("data.gnd[\"@context\"]").unwrap().segments,
            ["data", "gnd", "@context"]
        );
        assert_eq!(
            FieldPath::parse("[\"@file\"].size").unwrap().root,
            PathRoot::Data
        );
        assert_eq!(
            FieldPath::parse("@file.size").unwrap().root,
            PathRoot::Metadata(MetadataField::FileSize)
        );
        assert!(FieldPath::parse("@file.unknown").is_err());
        let mut ids = IdentitySet::default();
        ids.insert(RecordId::intrinsic("a").unwrap()).unwrap();
        assert!(ids.insert(RecordId::intrinsic("a").unwrap()).is_err());
        assert_ne!(
            RecordId::generated(FileId(1), 2, 3),
            RecordId::intrinsic("a").unwrap()
        );
    }
}
