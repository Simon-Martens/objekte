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
        let text = value.to_string();
        if !text.contains(['.', 'e', 'E']) {
            return text
                .parse::<i128>()
                .map(Self::Integer)
                .map_err(|_| "JSON integer is outside i128 range".into());
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
    Unknown,
    String,
    Id,
    Number,
    Bool,
    Date,
    Datetime,
    Unixtime,
    Object,
    Array(Box<LogicalType>),
}


/// Homogeneous logical array with its resolved element type.
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
                element_type = Some(
                    merge_array_element_types(expected, &kind)
                        .ok_or_else(|| format!("mixed array types: {expected:?} and {kind:?}"))?,
                );
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


fn merge_array_element_types(left: &LogicalType, right: &LogicalType) -> Option<LogicalType> {
    match (left, right) {
        (LogicalType::Unknown, kind) | (kind, LogicalType::Unknown) => Some(kind.clone()),
        (LogicalType::Array(a), LogicalType::Array(b)) => Some(LogicalType::Array(Box::new(
            merge_array_element_types(a, b)?,
        ))),
        _ if left == right => Some(left.clone()),
        _ => None,
    }
}


#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Value {
    Null,
    String(String),
    Id(String),
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
            Self::Id(_) => LogicalType::Id,
            Self::Number(_) => LogicalType::Number,
            Self::Bool(_) => LogicalType::Bool,
            Self::Date(_) => LogicalType::Date,
            Self::Datetime(_) => LogicalType::Datetime,
            Self::Unixtime(_) => LogicalType::Unixtime,
            Self::Object(_) => LogicalType::Object,
            Self::Array(a) => LogicalType::Array(Box::new(
                a.element_type.clone().unwrap_or(LogicalType::Unknown),
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
                    .map(|(k, v)| {
                        let value = Self::from_json(v).map_err(|error| format!("{k}: {error}"))?;
                        Ok((k, value))
                    })
                    .collect::<Result<_, String>>()?,
            ),
        })
    }

    pub fn parse_typed_string(text: &str, kind: &LogicalType) -> Result<Self, String> {
        match kind {
            LogicalType::String => Ok(Self::String(text.into())),
            LogicalType::Id => Ok(Self::Id(text.into())),
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


/// Compact repository-local collection identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct CollectionId(pub u32);


/// Compact repository-local source-file identifier.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct FileId(pub u32);


/// Dense record position within one collection generation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecordSlot(pub u32);


/// Stable logical identifier within a collection.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecordId(pub String);


/// Collection-scoped identity used outside slot storage.
#[derive(Clone, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
pub struct RecordIdentity {
    pub collection: CollectionId,
    pub record: RecordId,
}


impl RecordId {
    pub fn intrinsic(text: impl Into<String>) -> Result<Self, String> {
        let text = text.into();
        if text.is_empty() || text.starts_with("~generated:") || text.starts_with("~fallback:") {
            return Err("invalid intrinsic record ID".into());
        }
        Ok(Self(text))
    }

    pub fn generated(file: FileId, modified_ns: i128, start: u64) -> Self {
        Self(format!("~generated:{}:{modified_ns}:{start}", file.0))
    }

    pub fn fallback(
        collection: CollectionId,
        source: &str,
        source_version: u32,
        relative_path: &std::path::Path,
        file: FileId,
        modified_ns: i128,
        start: u64,
        end: u64,
    ) -> Self {
        let path = crate::path_repr::query_path(relative_path);
        Self(format!(
            "~fallback:{}:{}:{}:{}:{}:{}:{}:{modified_ns}:{start}:{end}",
            collection.0,
            file.0,
            source_version,
            source.len(),
            source,
            path.len(),
            path
        ))
    }
}


/// Duplicate-detecting set of logical record IDs.
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


/// Resolved path into record data or reserved metadata.
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
    FileStatus,
    RecordClass,
    CollectionName,
    RepositoryRoot,
}


impl MetadataField {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RecordId => "@record.id",
            Self::RecordByteStart => "@record.byte_start",
            Self::RecordByteEnd => "@record.byte_end",
            Self::RecordByteLength => "@record.byte_length",
            Self::FileId => "@file.id",
            Self::FilePath => "@file.path",
            Self::FileSize => "@file.size",
            Self::FileCreated => "@file.created",
            Self::FileModified => "@file.modified",
            Self::FileStatus => "@file.status",
            Self::RecordClass => "@record.class",
            Self::CollectionName => "@collection.name",
            Self::RepositoryRoot => "@repository.root",
        }
    }

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
            "@file.status" => Ok(Self::FileStatus),
            "@record.class" => Ok(Self::RecordClass),
            "@collection.name" => Ok(Self::CollectionName),
            "@repository.root" => Ok(Self::RepositoryRoot),
            _ => Err(format!("unknown metadata field: {name}")),
        }
    }
}


impl FieldPath {
    pub fn display(&self) -> String {
        match self.root {
            PathRoot::Metadata(field) => field.as_str().to_string(),
            PathRoot::Data => {
                let mut output = String::new();
                for (index, segment) in self.segments.iter().enumerate() {
                    let quoted = segment.is_empty()
                        || segment.starts_with('@')
                        || segment
                            .chars()
                            .any(|ch| matches!(ch, '.' | '[' | ']' | '"' | '\'' | '\\'));
                    if quoted {
                        output.push('[');
                        output.push_str(
                            &serde_json::to_string(segment).expect("string serialization"),
                        );
                        output.push(']');
                    } else {
                        if index != 0 {
                            output.push('.');
                        }
                        output.push_str(segment);
                    }
                }
                output
            }
        }
    }

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
                let mut escaped = false;
                let mut end = None;
                for (offset, ch) in rest[2..].char_indices() {
                    if escaped {
                        escaped = false;
                        continue;
                    }
                    if ch == '\\' {
                        escaped = true;
                        continue;
                    }
                    if ch == quote {
                        end = Some(offset + 2);
                        break;
                    }
                }
                let end = end.ok_or("unterminated quoted path")?;
                if rest.as_bytes().get(end + 1) != Some(&b']') {
                    return Err("missing closing bracket".into());
                }
                let name = if quote == '"' {
                    serde_json::from_str::<String>(&rest[1..=end])
                        .map_err(|e| format!("invalid quoted path: {e}"))?
                } else {
                    let mut name = String::new();
                    let mut chars = rest[2..end].chars();
                    while let Some(ch) = chars.next() {
                        if ch == '\\' {
                            let next = chars.next().ok_or("trailing path escape")?;
                            if next != '\\' && next != '\'' {
                                return Err("invalid single-quoted path escape".into());
                            }
                            name.push(next);
                        } else {
                            name.push(ch);
                        }
                    }
                    name
                };
                segments.push(name);
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
#[path = "model.test.rs"]
mod tests;
