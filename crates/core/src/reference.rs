//! Typed query binding and independent, index-free reference evaluation.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use unicode_casefold::UnicodeCaseFold;

use crate::inventory::{FileEntry, RecordEntry};
use crate::model::{FieldPath, LogicalType, MetadataField, Number, PathRoot, Value};
use crate::query::{FilterExpr, FtsQuery, Literal, Modifier, Operator, Quantifier, Term};
use crate::source::TextSegment;


/// Logical type, presence rules, and index policy for one field.
#[derive(Clone, Debug)]
pub struct FieldDefinition {
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub required: bool,
    pub nullable: bool,
    pub indexed: bool,
    pub multi: bool,
    /// Nullability at each array level, from outermost to innermost.
    /// An empty list retains permissive legacy behavior for in-memory hints.
    pub element_nullable: Vec<bool>,
}


#[derive(Clone, Debug, Default)]
struct ValidationNode {
    field: Option<FieldDefinition>,
    children: HashMap<String, ValidationNode>,
}


impl ValidationNode {
    fn requires_presence(&self) -> bool {
        self.field.as_ref().map_or_else(
            || self.children.values().any(Self::requires_presence),
            |field| field.required,
        )
    }
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transform {
    Identity,
    CaseFold,
}


/// Named field transformation used by indexing and scanning.
#[derive(Clone, Debug)]
pub struct CustomBinding {
    pub name: String,
    pub version: u32,
    pub transform: Transform,
}


/// One prepared named operation shared by FILTER, FTS, indexes, and scanners.
/// Implementations work at value or record boundaries, never per character.
pub trait BindingOperation: Send + Sync + std::fmt::Debug {
    fn accepts(&self, kind: &LogicalType, operator: Operator, multi: bool) -> bool;
    fn prepare_literal(&self, literal: &Literal, kind: &LogicalType) -> Result<Value, String>;
    fn map_value(&self, value: &Value) -> Option<Value>;
    fn map_text(&self, text: &str) -> String;
    fn prepare_fts_literal(&self, text: &str) -> Result<String, String> {
        Ok(self.map_text(text))
    }

    fn posting(&self) -> bool {
        false
    }

    fn bigram(&self) -> bool {
        false
    }

    fn complete_coverage(&self) -> bool {
        true
    }
}


/// Stable named binding and semantic/configuration version.
#[derive(Clone, Debug)]
pub struct RegisteredBinding {
    pub name: String,
    pub version: u32,
    pub operation: Arc<dyn BindingOperation>,
}


/// In-memory field definitions and custom query bindings.
#[derive(Clone, Debug, Default)]
pub struct Schema {
    pub(crate) fields: HashMap<Vec<String>, FieldDefinition>,
    pub(crate) custom: HashMap<(Vec<String>, String), CustomBinding>,
    pub(crate) operations: HashMap<(Vec<String>, String), RegisteredBinding>,
    pub(crate) defaults: HashMap<Vec<String>, String>,
    pub(crate) raw: Option<RegisteredBinding>,
    temporal_fields: bool,
    validation: ValidationNode,
    temporal_validation: ValidationNode,
}


impl Schema {
    pub fn fields(&self) -> &HashMap<Vec<String>, FieldDefinition> {
        &self.fields
    }

    pub fn custom_bindings(&self) -> &HashMap<(Vec<String>, String), CustomBinding> {
        &self.custom
    }

    pub fn coerce_record(&self, value: &mut Value) -> Result<(), String> {
        self.coerce_projected_record(value, None)
    }

    pub fn coerce_projected_record(
        &self,
        value: &mut Value,
        selected_top_level: Option<&HashSet<String>>,
    ) -> Result<(), String> {
        let Value::Object(_) = value else {
            return Err("record root must be an object".into());
        };
        validate_children(value, &self.validation, &mut Vec::new(), selected_top_level)
    }

    /// Query records already passed the declared-type contract during build.
    /// Only temporal values need conversion again after a source read.
    pub(crate) fn coerce_query_record(
        &self,
        value: &mut Value,
        selected_top_level: Option<&HashSet<String>>,
    ) -> Result<(), String> {
        if !self.temporal_fields {
            return Ok(());
        }
        validate_children(
            value,
            &self.temporal_validation,
            &mut Vec::new(),
            selected_top_level,
        )
    }

    pub fn has_temporal_fields(&self) -> bool {
        self.temporal_fields
    }

    pub fn register_field(&mut self, field: FieldDefinition) -> Result<(), String> {
        if field.path.is_empty() {
            return Err("field path is empty".into());
        }
        if self.fields.contains_key(&field.path) {
            return Err("duplicate field registration".into());
        }
        self.temporal_fields |= temporal_kind(&field.kind);
        let mut node = &mut self.validation;
        for part in &field.path {
            node = node.children.entry(part.clone()).or_default();
        }
        node.field = Some(field.clone());
        self.fields.insert(field.path.clone(), field);
        if self.temporal_fields {
            self.rebuild_temporal_validation();
        }
        Ok(())
    }

    fn rebuild_temporal_validation(&mut self) {
        let mut root = ValidationNode::default();
        for field in self.fields.values().filter(|field| temporal_kind(&field.kind)) {
            let mut node = &mut root;
            for depth in 0..field.path.len() {
                node = node.children.entry(field.path[depth].clone()).or_default();
                if let Some(ancestor) = self.fields.get(&field.path[..=depth]) {
                    node.field = Some(ancestor.clone());
                }
            }
        }
        self.temporal_validation = root;
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
            .operations
            .contains_key(&(path.clone(), binding.name.clone()))
        {
            return Err("duplicate custom binding".into());
        }
        if self
            .custom
            .contains_key(&(path.clone(), binding.name.clone()))
        {
            return Err("duplicate custom binding".into());
        }
        self.custom.insert((path, binding.name.clone()), binding);
        Ok(())
    }

    pub fn register_operation(
        &mut self,
        path: Vec<String>,
        binding: RegisteredBinding,
    ) -> Result<(), String> {
        if !self.fields.contains_key(&path) {
            return Err("binding field is unknown".into());
        }
        if binding.name.is_empty()
            || binding.version == 0
            || matches!(
                binding.name.as_str(),
                "default" | "lower" | "length" | "each" | "null" | "exists"
            )
        {
            return Err("invalid binding registration".into());
        }
        let key = (path, binding.name.clone());
        if self.custom.contains_key(&key) || self.operations.contains_key(&key) {
            return Err("duplicate binding registration".into());
        }
        self.operations.insert(key, binding);
        Ok(())
    }

    pub fn set_default_binding(&mut self, path: Vec<String>, name: &str) -> Result<(), String> {
        if !self.fields.contains_key(&path)
            || (name != "lower"
                && !self
                    .operations
                    .contains_key(&(path.clone(), name.to_owned()))
                && !self.custom.contains_key(&(path.clone(), name.to_owned())))
        {
            return Err(format!("unknown binding {name}"));
        }
        self.defaults.insert(path, name.to_owned());
        Ok(())
    }

    pub fn set_raw_binding(&mut self, binding: RegisteredBinding) -> Result<(), String> {
        if binding.name.is_empty()
            || binding.version == 0
            || !binding
                .operation
                .accepts(&LogicalType::String, Operator::Contains, false)
        {
            return Err("invalid raw binding".into());
        }
        self.raw = Some(binding);
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
                element_nullable: Vec::new(),
            }),
        }
    }

    pub fn field_or_dynamic(&self, path: &FieldPath) -> Result<FieldDefinition, String> {
        match path.root {
            PathRoot::Data => {
                Ok(self
                    .fields
                    .get(&path.segments)
                    .cloned()
                    .unwrap_or_else(|| FieldDefinition {
                        path: path.segments.clone(),
                        kind: LogicalType::Unknown,
                        required: false,
                        nullable: true,
                        indexed: false,
                        multi: true,
                        element_nullable: Vec::new(),
                    }))
            }
            PathRoot::Metadata(_) => self.field(path),
        }
    }
}


fn temporal_kind(kind: &LogicalType) -> bool {
    match kind {
        LogicalType::Date | LogicalType::Datetime | LogicalType::Unixtime => true,
        LogicalType::Array(inner) => temporal_kind(inner),
        _ => false,
    }
}

fn validate_children(
    value: &mut Value,
    node: &ValidationNode,
    path: &mut Vec<String>,
    selected: Option<&HashSet<String>>,
) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            for (name, child) in &node.children {
                if selected.is_some_and(|selection| !selection.contains(name)) {
                    continue;
                }
                path.push(name.clone());
                if let Some(value) = fields.get_mut(name) {
                    if let Some(field) = &child.field {
                        validate_declared(
                            value,
                            &field.kind,
                            field.nullable,
                            &field.element_nullable,
                            0,
                            path,
                        )?;
                    }
                    if !matches!(value, Value::Null) {
                        validate_children(value, child, path, None)?;
                    }
                } else if child.requires_presence() {
                    return Err(format!("{}: required field is missing", path.join(".")));
                }
                path.pop();
            }
        }
        Value::Array(array) => {
            for value in &mut array.values {
                if !matches!(value, Value::Null) {
                    validate_children(value, node, path, None)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn validate_declared(
    value: &mut Value,
    kind: &LogicalType,
    nullable: bool,
    elements: &[bool],
    depth: usize,
    path: &[String],
) -> Result<(), String> {
    if matches!(value, Value::Null) {
        return if nullable {
            Ok(())
        } else {
            Err(format!("{}: null is not allowed", path.join(".")))
        };
    }
    if matches!(kind, LogicalType::Date | LogicalType::Datetime) {
        if let Value::String(text) = value {
            *value = Value::parse_typed_string(text, kind)
                .map_err(|e| format!("{}: {e}", path.join(".")))?;
        }
    } else if *kind == LogicalType::Unixtime {
        if let Value::Number(Number::Integer(seconds)) = value {
            *value = Value::Unixtime(
                i64::try_from(*seconds)
                    .map_err(|_| format!("{}: unixtime out of range", path.join(".")))?,
            );
        }
    }
    if let (Value::Array(array), LogicalType::Array(inner)) = (&mut *value, kind) {
        let item_nullable = elements.get(depth).copied().unwrap_or(true);
        for item in &mut array.values {
            validate_declared(item, inner, item_nullable, elements, depth + 1, path)?;
        }
        array.element_type = Some((**inner).clone());
        array.nullable_elements = item_nullable;
        return Ok(());
    }
    let actual = value.logical_type();
    if &actual != kind && *kind != LogicalType::Unknown {
        return Err(format!(
            "{}: expected {kind:?}, found {actual:?}",
            path.join(".")
        ));
    }
    Ok(())
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


/// Typed predicate with its resolved path, literal, and binding.
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
    pub operation: Option<Arc<dyn BindingOperation>>,
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
    let field = schema.field_or_dynamic(&term.path)?;
    let source_kind = field.kind.clone();
    let mut derived = Derived::None;
    let mut transform = Transform::Identity;
    let mut operation = None;
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
                let key = (term.path.segments.clone(), name.clone());
                if let Some(binding) = schema.operations.get(&key) {
                    operation = Some(binding.operation.clone());
                    binding_version = binding.version;
                } else {
                    let binding = schema
                        .custom
                        .get(&key)
                        .ok_or_else(|| format!("unknown binding {name}"))?;
                    transform = binding.transform;
                    binding_version = binding.version;
                }
                binding_name = name.clone();
            }
        }
    }
    if binding_name == "default" && derived == Derived::None {
        if let Some(name) = schema.defaults.get(&term.path.segments) {
            let key = (term.path.segments.clone(), name.clone());
            if name == "lower" {
                transform = Transform::CaseFold;
            } else if let Some(binding) = schema.operations.get(&key) {
                operation = Some(binding.operation.clone());
                binding_version = binding.version;
            } else if let Some(binding) = schema.custom.get(&key) {
                transform = binding.transform;
                binding_version = binding.version;
            }
            binding_name = name.clone();
        }
    }
    if derived != Derived::None
        && (transform != Transform::Identity
            || operation.is_some()
            || each
            || term.quantifier == Quantifier::Any)
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
            if !matches!(source_kind, LogicalType::Array(_) | LogicalType::Unknown) {
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
    if transform != Transform::Identity
        && !matches!(kind, LogicalType::String | LogicalType::Unknown)
    {
        return Err("text binding requires string values".into());
    }
    if transform != Transform::Identity
        && !matches!(term.literal, Literal::String(_) | Literal::Null)
    {
        return Err("text binding requires a string literal".into());
    }
    if let Some(op) = &operation {
        if !op.accepts(&kind, term.operator, multi) {
            return Err(format!("unsupported operation for binding {binding_name}"));
        }
    }
    if matches!(term.operator, Operator::Contains | Operator::NotContains)
        && !matches!(kind, LogicalType::String | LogicalType::Unknown)
    {
        return Err("contains requires string values".into());
    }
    if matches!(term.operator, Operator::Contains | Operator::NotContains)
        && !matches!(term.literal, Literal::String(_))
    {
        return Err("contains requires a string literal".into());
    }
    if matches!(
        term.operator,
        Operator::Gt | Operator::Ge | Operator::Lt | Operator::Le
    ) && matches!(kind, LogicalType::Object | LogicalType::Array(_))
    {
        return Err("ordered comparison requires scalar values".into());
    }
    let literal = if let Some(op) = &operation {
        op.prepare_literal(&term.literal, &kind)?
    } else {
        prepare_literal(&term.literal, &kind, transform)?
    };
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
        operation,
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
        (Literal::String(s), LogicalType::Unknown) => Value::String(s.clone()),
        (Literal::Number(n), LogicalType::Unknown) => Value::Number(*n),
        (Literal::Bool(b), LogicalType::Unknown) => Value::Bool(*b),
        _ => return Err(format!("literal is incompatible with {kind:?}")),
    };
    Ok(match value {
        Value::String(s) if transform == Transform::CaseFold => {
            Value::String(s.case_fold().collect())
        }
        other => other,
    })
}


/// Record data and inventory metadata available during evaluation.
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
            Value::String(crate::path_repr::query_path(&ctx.file.relative_path))
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
            Value::String(crate::path_repr::query_path(ctx.repository_root))
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

fn compare_bound(lhs: &Value, term: &BoundTerm) -> bool {
    if let Some(operation) = &term.operation {
        return operation.map_value(lhs).as_ref().is_some_and(|value| {
            compare_scalar(value, &term.literal, term.operator, Transform::Identity)
        });
    }
    compare_scalar(lhs, &term.literal, term.operator, term.transform)
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
    if term.multi {
        let mut flattened = Vec::new();
        for value in values {
            if let Value::Array(array) = value {
                flattened.extend(array.values.iter());
            } else {
                flattened.push(value);
            }
        }
        values = flattened;
    }
    if values.len() == 1 && matches!(values[0], Value::Null) && !matches!(term.literal, Value::Null)
    {
        return Ok(false);
    }
    if term.multi {
        if term.quantifier == Quantifier::Any {
            Ok(values.into_iter().any(|v| compare_bound(v, term)))
        } else {
            Ok(values.into_iter().all(|v| compare_bound(v, term)))
        }
    } else {
        Ok(values.first().is_some_and(|v| compare_bound(v, term)))
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
            let mut branch_evidence = HashSet::new();
            for p in parts {
                result &= eval_filter(p, ctx, &mut branch_evidence)?;
            }
            if result {
                evidence.extend(branch_evidence);
            }
            result
        }
        BoundExpr::Or(parts) => {
            let mut result = false;
            for p in parts {
                let mut branch_evidence = HashSet::new();
                if eval_filter(p, ctx, &mut branch_evidence)? {
                    result = true;
                    evidence.extend(branch_evidence);
                }
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


/// One selected FTS target with its literal prepared for the bound operation.
#[derive(Clone, Debug)]
pub struct BoundFtsField {
    pub path: FieldPath,
    pub binding_name: String,
    pub literal: String,
    pub transform: Transform,
    pub operation: Option<Arc<dyn BindingOperation>>,
}


/// Collection-specific FTS bindings, including the configured raw target.
#[derive(Clone, Debug)]
pub struct BoundFts {
    pub literal: String,
    pub raw_operation: Option<Arc<dyn BindingOperation>>,
    pub fields: Vec<BoundFtsField>,
}


pub fn bind_fts(fts: &FtsQuery, schema: &Schema) -> Result<BoundFts, String> {
    let raw_operation = schema.raw.as_ref().map(|binding| binding.operation.clone());
    if let Some(op) = &raw_operation {
        if !op.accepts(&LogicalType::String, Operator::Contains, false) {
            return Err("raw FTS binding does not support substring matching".into());
        }
    }
    let literal = raw_operation
        .as_ref()
        .map_or(Ok(fts.literal.clone()), |op| {
            op.prepare_fts_literal(&fts.literal)
        })?;
    let mut fields = Vec::new();
    for selected in &fts.fields {
        let definition = schema.field_or_dynamic(&selected.path)?;
        if !matches!(definition.kind, LogicalType::String)
            && !matches!(definition.kind, LogicalType::Array(ref inner) if **inner == LogicalType::String)
            && definition.kind != LogicalType::Unknown
        {
            return Err("FTS field requires string values".into());
        }
        let name = selected
            .binding
            .clone()
            .or_else(|| schema.defaults.get(&selected.path.segments).cloned())
            .unwrap_or_else(|| "default".into());
        let key = (selected.path.segments.clone(), name.clone());
        let (transform, operation) = if name == "default" {
            (Transform::Identity, None)
        } else if name == "lower" {
            (Transform::CaseFold, None)
        } else if let Some(binding) = schema.operations.get(&key) {
            if !binding.operation.accepts(
                &LogicalType::String,
                Operator::Contains,
                definition.multi || matches!(definition.kind, LogicalType::Array(_)),
            ) {
                return Err(format!("unsupported FTS operation for binding {name}"));
            }
            (Transform::Identity, Some(binding.operation.clone()))
        } else if let Some(binding) = schema.custom.get(&key) {
            (binding.transform, None)
        } else {
            return Err(format!("unknown FTS binding {name}"));
        };
        let needle = if let Some(op) = &operation {
            op.prepare_fts_literal(&fts.literal)?
        } else if transform == Transform::CaseFold {
            fts.literal.case_fold().collect()
        } else {
            fts.literal.clone()
        };
        fields.push(BoundFtsField {
            path: selected.path.clone(),
            binding_name: name,
            literal: needle,
            transform,
            operation,
        });
    }
    Ok(BoundFts {
        literal,
        raw_operation,
        fields,
    })
}

fn text_contains(
    text: &str,
    needle: &str,
    transform: Transform,
    operation: Option<&Arc<dyn BindingOperation>>,
) -> bool {
    if let Some(op) = operation {
        op.map_text(text).contains(needle)
    } else if transform == Transform::CaseFold {
        text.case_fold().collect::<String>().contains(needle)
    } else {
        text.contains(needle)
    }
}

pub fn eval_bound_fts(
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    evidence: &mut HashSet<String>,
) -> Result<bool, String> {
    let mut matched = false;
    if fts.fields.is_empty() {
        for segment in segments {
            if text_contains(
                &segment.text,
                &fts.literal,
                Transform::Identity,
                fts.raw_operation.as_ref(),
            ) {
                matched = true;
                evidence.insert(segment.path.join("."));
            }
        }
        return Ok(matched);
    }
    for field in &fts.fields {
        if let PathRoot::Metadata(metadata) = field.path.root {
            if let Value::String(text) = metadata_value(metadata, ctx)? {
                if text_contains(
                    &text,
                    &field.literal,
                    field.transform,
                    field.operation.as_ref(),
                ) {
                    matched = true;
                    evidence.insert(field.path.display());
                }
            }
        } else {
            for segment in segments
                .iter()
                .filter(|segment| segment.path == field.path.segments)
            {
                if text_contains(
                    &segment.text,
                    &field.literal,
                    field.transform,
                    field.operation.as_ref(),
                ) {
                    matched = true;
                    evidence.insert(field.path.display());
                }
            }
        }
    }
    Ok(matched)
}

pub fn eval_fts_query(
    fts: &FtsQuery,
    schema: &Schema,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    evidence: &mut HashSet<String>,
) -> Result<bool, String> {
    let bound = bind_fts(fts, schema)?;
    eval_bound_fts(&bound, segments, ctx, evidence)
}

#[cfg(test)]
#[path = "reference.test.rs"]
mod tests;
