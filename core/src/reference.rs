//! Typed query binding and independent, index-free reference evaluation.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;

use unicode_casefold::UnicodeCaseFold;

use crate::inventory::{FileEntry, RecordEntry};
use crate::model::{FieldPath, LogicalType, MetadataField, Number, PathRoot, Value};
use crate::query::{FilterExpr, FtsQuery, Literal, Modifier, Operator, Quantifier, Term};
use crate::source::TextSegment;

#[derive(Clone, Debug)]
pub struct FieldDefinition {
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub required: bool,
    pub nullable: bool,
    pub indexed: bool,
    pub multi: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transform {
    Identity,
    CaseFold,
}

#[derive(Clone, Debug)]
pub struct CustomBinding {
    pub name: String,
    pub version: u32,
    pub transform: Transform,
}

#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub fields: HashMap<Vec<String>, FieldDefinition>,
    pub custom: HashMap<(Vec<String>, String), CustomBinding>,
}

impl Schema {
    pub fn register_field(&mut self, field: FieldDefinition) -> Result<(), String> {
        if self.fields.insert(field.path.clone(), field).is_some() {
            return Err("duplicate field registration".into());
        }
        Ok(())
    }
    pub fn register_custom(
        &mut self,
        path: Vec<String>,
        binding: CustomBinding,
    ) -> Result<(), String> {
        if matches!(
            binding.name.as_str(),
            "lower" | "length" | "each" | "null" | "exists"
        ) {
            return Err("custom name collides with built-in modifier".into());
        }
        if !self.fields.contains_key(&path) {
            return Err("custom binding field is unknown".into());
        }
        if self
            .custom
            .insert((path, binding.name.clone()), binding)
            .is_some()
        {
            return Err("duplicate custom binding".into());
        }
        Ok(())
    }
    pub fn field(&self, path: &FieldPath) -> Result<FieldDefinition, String> {
        match path.root {
            PathRoot::Data => self
                .fields
                .get(&path.segments)
                .cloned()
                .ok_or_else(|| format!("unknown field: {}", path.segments.join("."))),
            PathRoot::Metadata(field) => Ok(FieldDefinition {
                path: Vec::new(),
                kind: metadata_type(field),
                required: true,
                nullable: field == MetadataField::FileCreated,
                indexed: true,
                multi: false,
            }),
        }
    }
}

fn metadata_type(field: MetadataField) -> LogicalType {
    match field {
        MetadataField::RecordId
        | MetadataField::FilePath
        | MetadataField::CollectionName
        | MetadataField::RepositoryRoot => LogicalType::String,
        MetadataField::RecordByteStart
        | MetadataField::RecordByteEnd
        | MetadataField::RecordByteLength
        | MetadataField::FileId
        | MetadataField::FileSize => LogicalType::Number,
        MetadataField::FileCreated | MetadataField::FileModified => LogicalType::Datetime,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Derived {
    None,
    Length,
    Null,
    Exists,
}

#[derive(Clone, Debug)]
pub struct BoundTerm {
    pub path: FieldPath,
    pub kind: LogicalType,
    pub source_kind: LogicalType,
    pub operator: Operator,
    pub quantifier: Quantifier,
    pub literal: Value,
    pub derived: Derived,
    pub transform: Transform,
    pub binding_name: String,
    pub binding_version: u32,
    pub indexed: bool,
    pub multi: bool,
    pub span: (usize, usize),
}

#[derive(Clone, Debug)]
pub enum BoundExpr {
    Term(BoundTerm),
    And(Vec<BoundExpr>),
    Or(Vec<BoundExpr>),
    Not(Box<BoundExpr>),
}

pub fn bind_filter(expr: &FilterExpr, schema: &Schema) -> Result<BoundExpr, String> {
    Ok(match expr {
        FilterExpr::Term(term) => BoundExpr::Term(bind_term(term, schema)?),
        FilterExpr::And(parts) => BoundExpr::And(
            parts
                .iter()
                .map(|p| bind_filter(p, schema))
                .collect::<Result<_, _>>()?,
        ),
        FilterExpr::Or(parts) => BoundExpr::Or(
            parts
                .iter()
                .map(|p| bind_filter(p, schema))
                .collect::<Result<_, _>>()?,
        ),
        FilterExpr::Not(inner) => BoundExpr::Not(Box::new(bind_filter(inner, schema)?)),
    })
}

fn bind_term(term: &Term, schema: &Schema) -> Result<BoundTerm, String> {
    let field = schema.field(&term.path)?;
    let source_kind = field.kind.clone();
    let mut derived = Derived::None;
    let mut transform = Transform::Identity;
    let mut binding_name = "default".to_string();
    let mut binding_version = 1;
    let mut each = false;
    for modifier in &term.modifiers {
        match modifier {
            Modifier::Length | Modifier::Null | Modifier::Exists => {
                if derived != Derived::None {
                    return Err("incompatible derived modifiers".into());
                }
                derived = match modifier {
                    Modifier::Length => Derived::Length,
                    Modifier::Null => Derived::Null,
                    _ => Derived::Exists,
                };
            }
            Modifier::Each => {
                if each {
                    return Err("duplicate each modifier".into());
                }
                each = true;
            }
            Modifier::Lower => {
                if binding_name != "default" {
                    return Err("multiple string bindings".into());
                }
                transform = Transform::CaseFold;
                binding_name = "lower".into();
            }
            Modifier::Custom(name) => {
                if binding_name != "default" {
                    return Err("multiple string bindings".into());
                }
                let binding = schema
                    .custom
                    .get(&(term.path.segments.clone(), name.clone()))
                    .ok_or_else(|| format!("unknown binding {name}"))?;
                transform = binding.transform;
                binding_name = name.clone();
                binding_version = binding.version;
            }
        }
    }
    if derived != Derived::None
        && (transform != Transform::Identity || each || term.quantifier == Quantifier::Any)
    {
        return Err("derived modifier cannot be combined with quantifier or text binding".into());
    }
    if each && term.quantifier == Quantifier::Any {
        return Err("each and existential operator conflict".into());
    }
    let multi = field.multi || matches!(source_kind, LogicalType::Array(_));
    if each && !multi {
        return Err("each requires a multi-valued field".into());
    }
    if term.quantifier == Quantifier::Any && !multi {
        return Err("existential comparison requires a multi-valued field".into());
    }
    let kind = match derived {
        Derived::Length => {
            if !matches!(source_kind, LogicalType::Array(_)) {
                return Err("length requires an array field".into());
            }
            LogicalType::Number
        }
        Derived::Null | Derived::Exists => LogicalType::Bool,
        Derived::None => match &source_kind {
            LogicalType::Array(inner) => *inner.clone(),
            other => other.clone(),
        },
    };
    if transform != Transform::Identity && kind != LogicalType::String {
        return Err("text binding requires string values".into());
    }
    if matches!(term.operator, Operator::Contains | Operator::NotContains)
        && kind != LogicalType::String
    {
        return Err("contains requires string values".into());
    }
    if matches!(
        term.operator,
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
    ) && matches!(kind, LogicalType::Object | LogicalType::Array(_))
    {
        return Err("ordered comparison requires scalar values".into());
    }
    let literal = prepare_literal(&term.literal, &kind, transform)?;
    if matches!(literal, Value::Null) && !matches!(term.operator, Operator::Eq | Operator::Ne) {
        return Err("null permits only equality operators".into());
    }
    if derived != Derived::None && !matches!(literal, Value::Number(_) | Value::Bool(_)) {
        return Err("invalid derived literal".into());
    }
    Ok(BoundTerm {
        path: term.path.clone(),
        kind,
        source_kind,
        operator: term.operator,
        quantifier: term.quantifier,
        literal,
        derived,
        transform,
        binding_name,
        binding_version,
        indexed: field.indexed,
        multi,
        span: term.span,
    })
}

fn prepare_literal(
    literal: &Literal,
    kind: &LogicalType,
    transform: Transform,
) -> Result<Value, String> {
    let value = match (literal, kind) {
        (Literal::Null, _) => Value::Null,
        (Literal::String(s), LogicalType::String) => Value::String(s.clone()),
        (Literal::String(s), LogicalType::Date | LogicalType::Datetime) => {
            Value::parse_typed_string(s, kind)?
        }
        (Literal::Number(n), LogicalType::Number) => Value::Number(*n),
        (Literal::Number(Number::Integer(n)), LogicalType::Unixtime) => {
            Value::Unixtime(i64::try_from(*n).map_err(|_| "unixtime out of range")?)
        }
        (Literal::Bool(b), LogicalType::Bool) => Value::Bool(*b),
        _ => return Err(format!("literal is incompatible with {kind:?}")),
    };
    Ok(match value {
        Value::String(s) if transform == Transform::CaseFold => {
            Value::String(s.case_fold().collect())
        }
        other => other,
    })
}

pub struct RecordContext<'a> {
    pub value: &'a Value,
    pub record: &'a RecordEntry,
    pub file: &'a FileEntry,
    pub collection_name: &'a str,
    pub repository_root: &'a Path,
}

pub(crate) fn metadata_value(
    field: MetadataField,
    ctx: &RecordContext<'_>,
) -> Result<Value, String> {
    let number = |n: u64| Value::Number(Number::integer(n as i128));
    Ok(match field {
        MetadataField::RecordId => Value::String(ctx.record.id.0.clone()),
        MetadataField::RecordByteStart => number(ctx.record.range.start),
        MetadataField::RecordByteEnd => number(ctx.record.range.end),
        MetadataField::RecordByteLength => number(ctx.record.range.len()),
        MetadataField::FileId => number(ctx.file.id.0 as u64),
        MetadataField::FilePath => {
            Value::String(ctx.file.relative_path.to_string_lossy().replace('\\', "/"))
        }
        MetadataField::FileSize => number(ctx.file.revision.size),
        MetadataField::FileCreated => ctx
            .file
            .created_ns
            .map(timestamp_value)
            .transpose()?
            .unwrap_or(Value::Null),
        MetadataField::FileModified => timestamp_value(ctx.file.revision.modified_ns)?,
        MetadataField::CollectionName => Value::String(ctx.collection_name.into()),
        MetadataField::RepositoryRoot => {
            Value::String(ctx.repository_root.to_string_lossy().into())
        }
    })
}

fn timestamp_value(ns: i128) -> Result<Value, String> {
    time::OffsetDateTime::from_unix_timestamp_nanos(ns)
        .map(Value::Datetime)
        .map_err(|e| e.to_string())
}

fn walk_path<'a>(value: &'a Value, parts: &[String], output: &mut Vec<&'a Value>) -> bool {
    if parts.is_empty() {
        output.push(value);
        return true;
    }
    match value {
        Value::Object(map) => map
            .get(&parts[0])
            .map(|v| walk_path(v, &parts[1..], output))
            .unwrap_or(false),
        Value::Array(a) => {
            let mut found = a.values.is_empty();
            for item in &a.values {
                found |= walk_path(item, parts, output);
            }
            found
        }
        _ => false,
    }
}

pub(crate) fn data_values<'a>(value: &'a Value, parts: &[String]) -> Option<Vec<&'a Value>> {
    let mut selected = Vec::new();
    walk_path(value, parts, &mut selected).then_some(selected)
}

fn select_values<'a>(
    term: &BoundTerm,
    ctx: &'a RecordContext<'a>,
) -> Result<Option<Vec<&'a Value>>, String> {
    match term.path.root {
        PathRoot::Data => {
            let mut selected = Vec::new();
            if walk_path(ctx.value, &term.path.segments, &mut selected) {
                Ok(Some(selected))
            } else {
                Ok(None)
            }
        }
        PathRoot::Metadata(field) => {
            // Metadata is evaluated from inventory without source parsing.
            // The owned value is handled separately in `eval_term`.
            let _ = field;
            Ok(Some(Vec::new()))
        }
    }
}

fn compare_scalar(lhs: &Value, rhs: &Value, op: Operator, transform: Transform) -> bool {
    if matches!(rhs, Value::Null) {
        return match op {
            Operator::Eq => matches!(lhs, Value::Null),
            Operator::Ne => !matches!(lhs, Value::Null),
            _ => false,
        };
    }
    if matches!(lhs, Value::Null) {
        return false;
    }
    let transformed;
    let lhs = if transform == Transform::CaseFold {
        if let Value::String(s) = lhs {
            transformed = Value::String(s.case_fold().collect());
            &transformed
        } else {
            return false;
        }
    } else {
        lhs
    };
    let order = match (lhs, rhs) {
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Number(a), Value::Number(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        (Value::Date(a), Value::Date(b)) => Some(a.cmp(b)),
        (Value::Datetime(a), Value::Datetime(b)) => Some(a.cmp(b)),
        (Value::Unixtime(a), Value::Unixtime(b)) => Some(a.cmp(b)),
        _ => None,
    };
    match op {
        Operator::Eq => order == Some(Ordering::Equal),
        Operator::Ne => order.is_some_and(|o| o != Ordering::Equal),
        Operator::Gt => order == Some(Ordering::Greater),
        Operator::Ge => order.is_some_and(|o| o != Ordering::Less),
        Operator::Lt => order == Some(Ordering::Less),
        Operator::Le => order.is_some_and(|o| o != Ordering::Greater),
        Operator::Contains | Operator::NotContains => {
            let (Value::String(text), Value::String(pattern)) = (lhs, rhs) else {
                return false;
            };
            let found = contains_pattern(text, pattern);
            if op == Operator::Contains {
                found
            } else {
                !found
            }
        }
    }
}

/// `%` spans any sequence; `\%` means a literal percent. No other wildcard.
fn contains_pattern(text: &str, pattern: &str) -> bool {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut chars = pattern.chars().peekable();
    let mut wildcard = false;
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'%') {
            current.push(chars.next().unwrap());
        } else if ch == '%' {
            pieces.push(std::mem::take(&mut current));
            wildcard = true;
        } else {
            current.push(ch);
        }
    }
    pieces.push(current);
    if !wildcard {
        return text.contains(&pieces[0]);
    }
    let mut rest = text;
    for piece in pieces {
        if piece.is_empty() {
            continue;
        }
        let Some(index) = rest.find(&piece) else {
            return false;
        };
        rest = &rest[index + piece.len()..];
    }
    true
}

pub fn eval_term(term: &BoundTerm, ctx: &RecordContext<'_>) -> Result<bool, String> {
    let owned;
    let mut selected = if let PathRoot::Metadata(field) = term.path.root {
        owned = metadata_value(field, ctx)?;
        Some(vec![&owned])
    } else {
        select_values(term, ctx)?
    };
    if term.derived == Derived::Exists || term.derived == Derived::Null {
        let result = match term.derived {
            Derived::Exists => selected.is_some(),
            Derived::Null => selected
                .as_ref()
                .is_some_and(|v| v.len() == 1 && matches!(v[0], Value::Null)),
            _ => unreachable!(),
        };
        return Ok(compare_scalar(
            &Value::Bool(result),
            &term.literal,
            term.operator,
            Transform::Identity,
        ));
    }
    let Some(mut values) = selected.take() else {
        return Ok(false);
    };
    if term.derived == Derived::Length {
        let Some(Value::Array(a)) = values.first().copied() else {
            return Ok(false);
        };
        return Ok(compare_scalar(
            &Value::Number(Number::integer(a.values.len() as i128)),
            &term.literal,
            term.operator,
            Transform::Identity,
        ));
    }
    if values.len() == 1 {
        if let Value::Array(a) = values[0] {
            values = a.values.iter().collect();
        }
    }
    if values.len() == 1 && matches!(values[0], Value::Null) && !matches!(term.literal, Value::Null)
    {
        return Ok(false);
    }
    if term.multi {
        if term.quantifier == Quantifier::Any {
            Ok(values
                .into_iter()
                .any(|v| compare_scalar(v, &term.literal, term.operator, term.transform)))
        } else {
            Ok(values
                .into_iter()
                .all(|v| compare_scalar(v, &term.literal, term.operator, term.transform)))
        }
    } else {
        Ok(values
            .first()
            .is_some_and(|v| compare_scalar(v, &term.literal, term.operator, term.transform)))
    }
}

pub fn eval_filter(
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    evidence: &mut HashSet<String>,
) -> Result<bool, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let match_ = eval_term(term, ctx)?;
            if match_
                && term.derived == Derived::None
                && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            {
                evidence.insert(term.path.display());
            }
            match_
        }
        BoundExpr::And(parts) => {
            let mut result = true;
            for p in parts {
                result &= eval_filter(p, ctx, evidence)?;
            }
            result
        }
        BoundExpr::Or(parts) => {
            let mut result = false;
            for p in parts {
                result |= eval_filter(p, ctx, evidence)?;
            }
            result
        }
        BoundExpr::Not(inner) => !eval_filter(inner, ctx, &mut HashSet::new())?,
    })
}

pub fn eval_fts(
    literal: &str,
    selected: &[FieldPath],
    segments: &[TextSegment],
    evidence: &mut HashSet<String>,
) -> bool {
    let mut matched = false;
    for segment in segments {
        if selected.is_empty()
            || selected
                .iter()
                .any(|p| p.root == PathRoot::Data && p.segments == segment.path)
        {
            if segment.text.contains(literal) {
                matched = true;
                evidence.insert(segment.path.join("."));
            }
        }
    }
    matched
}

pub fn eval_fts_query(
    fts: &FtsQuery,
    schema: &Schema,
    segments: &[TextSegment],
    evidence: &mut HashSet<String>,
) -> Result<bool, String> {
    if fts.fields.is_empty() {
        return Ok(eval_fts(&fts.literal, &[], segments, evidence));
    }
    let mut matched = false;
    for field in &fts.fields {
        let definition = schema.field(&field.path)?;
        if !matches!(definition.kind, LogicalType::String | LogicalType::Array(_)) {
            return Err("FTS field requires string values".into());
        }
        let transform = if let Some(name) = &field.binding {
            schema
                .custom
                .get(&(field.path.segments.clone(), name.clone()))
                .ok_or_else(|| format!("unknown FTS binding {name}"))?
                .transform
        } else {
            Transform::Identity
        };
        let needle = if transform == Transform::CaseFold {
            fts.literal.case_fold().collect::<String>()
        } else {
            fts.literal.clone()
        };
        for segment in segments.iter().filter(|s| s.path == field.path.segments) {
            let value = if transform == Transform::CaseFold {
                segment.text.case_fold().collect::<String>()
            } else {
                segment.text.clone()
            };
            if value.contains(&needle) {
                matched = true;
                evidence.insert(field.path.display());
            }
        }
    }
    Ok(matched)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::{ByteRange, SourceRevision};
    use crate::model::{FileId, RecordId, RecordSlot};
    use crate::query::parse_filter;

    fn schema() -> Schema {
        let mut schema = Schema::default();
        for (path, kind, multi) in [
            ("name", LogicalType::String, false),
            (
                "values",
                LogicalType::Array(Box::new(LogicalType::String)),
                true,
            ),
            ("people.name", LogicalType::String, true),
        ] {
            schema
                .register_field(FieldDefinition {
                    path: path.split('.').map(str::to_owned).collect(),
                    kind,
                    required: false,
                    nullable: true,
                    indexed: true,
                    multi,
                })
                .unwrap();
        }
        schema
            .register_custom(
                vec!["name".into()],
                CustomBinding {
                    name: "folded".into(),
                    version: 1,
                    transform: Transform::CaseFold,
                },
            )
            .unwrap();
        schema
    }
    fn with_record<F: FnOnce(RecordContext<'_>)>(value: Value, f: F) {
        let record = RecordEntry {
            slot: RecordSlot(0),
            id: RecordId("x".into()),
            file: FileId(0),
            range: ByteRange { start: 0, end: 4 },
            revision: SourceRevision {
                modified_ns: 0,
                size: 4,
            },
        };
        let file = FileEntry {
            id: FileId(0),
            relative_path: "x.json".into(),
            revision: record.revision,
            created_ns: None,
            collections: vec![],
        };
        f(RecordContext {
            value: &value,
            record: &record,
            file: &file,
            collection_name: "test",
            repository_root: Path::new("/tmp"),
        });
    }
    #[test]
    fn truth_table_arrays_null_and_case_folding() {
        let value = Value::from_json(serde_json::json!({"name":"Straße", "values":["a","b"], "people":[{"name":"Ada"},{"name":"Bob"}]})).unwrap();
        with_record(value, |ctx| {
            for (input, expected) in [
                ("values ?!= 'a'", true),
                ("!(values ?= 'a')", false),
                ("values:each = 'a'", false),
                ("values:length = 2", true),
                ("name:lower ~ 'STRASSE'", true),
                ("name:folded ~ 'strasse'", true),
                ("people.name ?= 'Ada'", true),
                ("name:null = false", true),
                ("@file.created:null = true", true),
            ] {
                let bound = bind_filter(&parse_filter(input).unwrap(), &schema()).unwrap();
                assert_eq!(
                    eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                    expected,
                    "{input}"
                );
            }
        });
    }
    #[test]
    fn invalid_branches_rejected_before_evaluation() {
        let schema = schema();
        assert!(bind_filter(&parse_filter("name = 'x' || absent = 1").unwrap(), &schema).is_err());
        assert!(bind_filter(&parse_filter("name:missing = 'x'").unwrap(), &schema).is_err());
        assert!(bind_filter(&parse_filter("name ?= 'x'").unwrap(), &schema).is_err());
    }

    #[test]
    fn missing_null_empty_truth_table() {
        for (input, expected) in [
            ("values:exists = true", true),
            ("values:null = false", true),
            ("values:each = 'x'", true),
            ("values ?= 'x'", false),
            ("values:length = 0", true),
            ("name:exists = false", true),
            ("name:null = false", true),
            ("name != 'x'", false),
        ] {
            let value = Value::from_json(serde_json::json!({"values": []})).unwrap();
            with_record(value, |ctx| {
                let bound = bind_filter(&parse_filter(input).unwrap(), &schema()).unwrap();
                assert_eq!(
                    eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                    expected,
                    "{input}"
                );
            });
        }
        let value = Value::from_json(serde_json::json!({"name":null,"values":[null,"x"]})).unwrap();
        with_record(value, |ctx| {
            for (input, expected) in [
                ("name:exists = true", true),
                ("name:null = true", true),
                ("name = null", true),
                ("name != 'x'", false),
                ("values ?!= 'x'", false),
                ("values ?= 'x'", true),
            ] {
                let bound = bind_filter(&parse_filter(input).unwrap(), &schema()).unwrap();
                assert_eq!(
                    eval_filter(&bound, &ctx, &mut HashSet::new()).unwrap(),
                    expected,
                    "{input}"
                );
            }
        });
    }
}
