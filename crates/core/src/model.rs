//! Format-independent values, paths, and stable record identities.

use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::hash::{Hash, Hasher};

use serde::{Deserialize, Deserializer, Serialize, de};
use time::{Date, OffsetDateTime, format_description::well_known::Rfc3339};

use crate::decimal::ExactDecimal;


/// Exact numeric value; finite floats may be compared across representations.
/// Direct non-finite `Float` variants have IEEE total order and are rejected
/// by deserialization and repository build admission.
#[derive(Clone, Debug, Serialize)]
pub enum Number {
    Integer(i128),
    Float(f64),
    ExactDecimal(ExactDecimal),
}


#[derive(Deserialize)]
enum NumberRepresentation {
    Integer(i128),
    Float(f64),
    ExactDecimal(ExactDecimal),
}


impl<'de> Deserialize<'de> for Number {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Ok(match NumberRepresentation::deserialize(deserializer)? {
            NumberRepresentation::Integer(value) => Self::Integer(value),
            NumberRepresentation::Float(value) if value.is_finite() => Self::Float(value),
            NumberRepresentation::Float(_) => return Err(de::Error::custom("non-finite number")),
            NumberRepresentation::ExactDecimal(value) => Self::ExactDecimal(value),
        })
    }
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
            return Ok(match text.parse::<i128>() {
                Ok(integer) => Self::Integer(integer),
                Err(_) => Self::ExactDecimal(ExactDecimal::parse(&text)?),
            });
        }
        let exact = ExactDecimal::parse(&text)?;
        if let Some(float) = value
            .as_f64()
            .filter(|float| float.is_finite() && *float != 0.0)
        {
            Self::float(float)
        } else if exact.digits == "0" {
            Ok(Self::Integer(0))
        } else {
            Ok(Self::ExactDecimal(exact))
        }
    }

    pub fn parse_json_literal(text: &str) -> Result<Self, String> {
        let parsed = serde_json::from_str::<serde_json::Number>(text)
            .map_err(|error| format!("invalid numeric literal: {error}"))?;
        Self::from_json(&parsed)
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
        match (self, other) {
            (Self::Integer(a), Self::Integer(b)) => a.cmp(b),
            (Self::Float(a), Self::Float(b)) if *a == 0.0 && *b == 0.0 => Ordering::Equal,
            (Self::Float(a), Self::Float(b)) => a.total_cmp(b),
            (Self::Integer(i), Self::Float(f)) => compare_int_float(*i, *f),
            (Self::Float(f), Self::Integer(i)) => compare_int_float(*i, *f).reverse(),
            (Self::ExactDecimal(a), Self::ExactDecimal(b)) => a.cmp_value(b),
            (Self::ExactDecimal(a), Self::Integer(b)) => a.cmp_value(&ExactDecimal::from_i128(*b)),
            (Self::Integer(a), Self::ExactDecimal(b)) => ExactDecimal::from_i128(*a).cmp_value(b),
            (Self::ExactDecimal(a), Self::Float(b)) if !b.is_finite() => 0.0f64.total_cmp(b),
            (Self::Float(a), Self::ExactDecimal(_)) if !a.is_finite() => {
                0.0f64.total_cmp(a).reverse()
            }
            (Self::ExactDecimal(a), Self::Float(b)) => a.cmp_value(&ExactDecimal::from_f64(*b)),
            (Self::Float(a), Self::ExactDecimal(b)) => ExactDecimal::from_f64(*a).cmp_value(b),
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
            Self::ExactDecimal(decimal) => {
                if let Some(integer) = decimal.as_i128() {
                    0u8.hash(state);
                    integer.hash(state);
                } else if let Some(float) = decimal.as_exact_f64() {
                    Self::Float(float).hash(state);
                } else {
                    2u8.hash(state);
                    decimal.hash(state);
                }
            }
        }
    }
}


fn compare_int_float(i: i128, f: f64) -> Ordering {
    if !f.is_finite() {
        return 0.0f64.total_cmp(&f);
    }
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


/// Logical array with an optional homogeneous element summary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ArrayValue {
    pub element_type: Option<LogicalType>,
    /// A conflict between non-null element types is sticky, including nested arrays.
    pub mixed_elements: bool,
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
            mixed_elements: false,
            nullable_elements,
            values,
        })
    }

    /// Preserve every parsed JSON member without imposing a declared element type.
    pub fn from_unrestricted(values: Vec<Value>) -> Self {
        let mut element_type: Option<LogicalType> = None;
        let mut mixed_elements = false;
        let mut nullable_elements = false;
        for value in &values {
            if matches!(value, Value::Null) {
                nullable_elements = true;
                continue;
            }
            if matches!(value, Value::Array(array) if array.mixed_elements) {
                mixed_elements = true;
                element_type = None;
                continue;
            }
            if mixed_elements {
                continue;
            }
            let kind = value.logical_type();
            element_type = match element_type.take() {
                None => Some(kind),
                Some(previous) => merge_array_element_types(&previous, &kind),
            };
            if element_type.is_none() {
                mixed_elements = true;
            }
        }
        Self {
            element_type,
            mixed_elements,
            nullable_elements,
            values,
        }
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
    /// Reject non-finite values supplied by custom readers before indexing.
    pub(crate) fn validate_finite_numbers(&self) -> Result<(), String> {
        match self {
            Self::Number(Number::Float(value)) if !value.is_finite() => {
                Err("non-finite number".into())
            }
            Self::Object(fields) => {
                for (field, value) in fields {
                    value
                        .validate_finite_numbers()
                        .map_err(|error| format!("{field}: {error}"))?;
                }
                Ok(())
            }
            Self::Array(array) => {
                for (index, value) in array.values.iter().enumerate() {
                    value
                        .validate_finite_numbers()
                        .map_err(|error| format!("[{index}]: {error}"))?;
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

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
            serde_json::Value::Array(a) => Self::Array(ArrayValue::from_unrestricted(
                a.into_iter()
                    .map(Self::from_json)
                    .collect::<Result<_, _>>()?,
            )),
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
pub struct RecordId(pub std::sync::Arc<str>);


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
        Ok(Self(text.into()))
    }

    pub fn generated(file: FileId, modified_ns: i128, start: u64) -> Self {
        Self(format!("~generated:{}:{modified_ns}:{start}", file.0).into())
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
        Self(
            format!(
                "~fallback:{}:{}:{}:{}:{}:{}:{}:{modified_ns}:{start}:{end}",
                collection.0,
                file.0,
                source_version,
                source.len(),
                source,
                path.len(),
                path
            )
            .into(),
        )
    }
}


impl std::borrow::Borrow<str> for RecordId {
    fn borrow(&self) -> &str {
        &self.0
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
    Backlinks,
}


/// Schema/index field address into data or reserved metadata. Occurrence
/// selectors belong to `QueryPath` and are not retained here.
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
            PathRoot::Backlinks => format!(
                "@backlinks.{}",
                Self {
                    root: PathRoot::Data,
                    segments: self.segments.clone(),
                }
                .display()
            ),
            PathRoot::Data => {
                if self.segments.is_empty() {
                    return "@root".into();
                }
                let mut output = String::new();
                for (index, segment) in self.segments.iter().enumerate() {
                    let quoted = segment.is_empty()
                        || segment.starts_with('@')
                        || segment
                            .chars()
                            .any(|ch| ch.is_whitespace() || ".[]\"'\\:,()!<>=~?&|".contains(ch));
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
        if input == "@root" {
            return Ok(Self {
                root: PathRoot::Data,
                segments: Vec::new(),
            });
        }
        if let Some(rest) = input.strip_prefix("@root.") {
            return Self::parse(rest);
        }
        if input.starts_with("@root[") {
            return crate::query_path::QueryPath::parse(input).map(|path| path.field);
        }
        if let Some(rest) = input.strip_prefix("@backlinks.") {
            let mut path = Self::parse(rest)?;
            if path.segments.is_empty() || path.root != PathRoot::Data {
                return Err("backlink path requires an alias".into());
            }
            path.root = PathRoot::Backlinks;
            return Ok(path);
        }
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
                if rest.is_empty() {
                    return Err("trailing path separator".into());
                }
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
