//! Typed query binding and independent, index-free reference evaluation.

use memchr::memmem::Finder;
use smallvec::SmallVec;
use std::borrow::Cow;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use crate::bigram::BigramRequest;
use crate::bigram::{fold, fold_into};
use crate::inventory::{FileEntry, RecordEntry};
use crate::model::{FieldPath, LogicalType, MetadataField, Number, PathRoot, Value};
use crate::query::{
    CaseSensitivity, FilterExpr, FtsMode, FtsQuery, Literal, Modifier, Operand, Operator,
    Quantifier, Term,
};
use crate::query_path::{PathStep, QueryPath};
use crate::regex_query::CompiledRegex;
use crate::relation::RelationDefinition;
use crate::source::TextSegment;
use crate::temporal_query::TemporalBounds;


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


/// Input required by one prepared exact field predicate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExactInput {
    SelectedValue,
    OriginalRecordBytes,
}


/// Query-scoped exact evaluator prepared by a registered binding. A byte
/// evaluator owns missing-field and array semantics for its whole record;
/// value evaluators receive the ordinary field selection and quantifier rules.
pub trait PreparedFilterMatcher: Send + Sync + std::fmt::Debug {
    /// Estimated bytes owned by this prepared matcher, excluding its Arc.
    /// Extensions with heap-owned state should override this default.
    fn retained_bytes(&self) -> usize {
        std::mem::size_of_val(self)
    }

    fn input(&self) -> ExactInput {
        ExactInput::SelectedValue
    }

    fn matches(&self, value: Option<&Value>, bytes: Option<&[u8]>) -> Result<bool, String>;
    fn posting_compatible(&self) -> bool {
        false
    }

    fn bigram_compatible(&self) -> bool {
        false
    }
}


/// Query-scoped literal substring evaluator over one reader-owned segment.
pub trait PreparedTextMatcher: Send + Sync + std::fmt::Debug {
    /// Estimated bytes owned by this prepared matcher, excluding its Arc.
    /// Extensions with heap-owned state should override this default.
    fn retained_bytes(&self) -> usize {
        std::mem::size_of_val(self)
    }

    fn matches(&self, text: &str) -> Result<bool, String>;
    fn bigram_compatible(&self) -> bool {
        false
    }
}


/// One prepared named operation shared by FILTER, FTS, indexes, and scanners.
/// Implementations work at value or record boundaries, never per character.
pub trait BindingOperation: Send + Sync + std::fmt::Debug {
    fn accepts(&self, kind: &LogicalType, operator: Operator, multi: bool) -> bool;
    fn prepare_literal(&self, literal: &Literal, kind: &LogicalType) -> Result<Value, String>;
    fn map_value(&self, value: &Value) -> Option<Value>;
    fn map_text(&self, text: &str) -> String;

    /// Declare that `map_value` yields a stable scalar ordering representation.
    /// Opaque matchers and fallible record-dependent operations stay ineligible.
    fn sort_compatible(&self) -> bool {
        false
    }

    /// A dynamic field operand may be compared after both values are mapped.
    fn supports_dynamic_comparison(&self) -> bool {
        false
    }

    fn prepare_fts_literal(&self, text: &str) -> Result<String, String> {
        Ok(self.map_text(text))
    }

    /// Opt in only when `map_text` is the complete text representation on
    /// which a prepared regex may run without transforming regex syntax.
    fn regex_text_compatible(&self) -> bool {
        false
    }

    /// Return a prepared prefix only when mapping a source prefix preserves
    /// prefix matching in this operation's indexed string representation.
    fn prepare_posting_prefix(&self, _prefix: &str) -> Result<Option<String>, String> {
        Ok(None)
    }

    /// `None` selects the prepared mapped-value adapter. An override may
    /// inspect borrowed values or original record bytes and return errors.
    fn prepare_filter_matcher(
        &self,
        _literal: &Value,
        _operator: Operator,
    ) -> Result<Option<Arc<dyn PreparedFilterMatcher>>, String> {
        Ok(None)
    }

    /// `None` selects the prepared mapped-text substring adapter.
    fn prepare_text_matcher(
        &self,
        _literal: &str,
    ) -> Result<Option<Arc<dyn PreparedTextMatcher>>, String> {
        Ok(None)
    }

    /// `map_value` and `map_text` use identical string representations.
    fn value_text_equivalent(&self) -> bool {
        false
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
    /// Explicit resident orders assembled from already configured scalar sort columns.
    pub composite_sorts: Vec<Vec<crate::workload::SortField>>,
    pub(crate) fields: HashMap<Vec<String>, FieldDefinition>,
    /// Explicit `index: false` paths; absent annotations leave inference free.
    pub(crate) auto_index_excluded: HashSet<Vec<String>>,
    pub(crate) source_fingerprint: Option<[u8; 32]>,
    pub(crate) custom: HashMap<(Vec<String>, String), CustomBinding>,
    pub(crate) operations: HashMap<(Vec<String>, String), RegisteredBinding>,
    pub(crate) defaults: HashMap<Vec<String>, String>,
    pub(crate) raw: Option<RegisteredBinding>,
    pub(crate) relations: BTreeMap<Vec<String>, RelationDefinition>,
    converted_fields: bool,
    validation: ValidationNode,
    conversion_validation: ValidationNode,
}


impl Schema {
    /// Include the exact selected schema bytes in snapshot compatibility.
    pub fn set_source_fingerprint(&mut self, bytes: &[u8]) {
        use sha2::Digest;
        self.source_fingerprint = Some(sha2::Sha256::digest(bytes).into());
    }

    pub fn prohibit_auto_index(&mut self, path: Vec<String>) -> Result<(), String> {
        if self.fields.get(&path).is_some_and(|field| field.indexed) {
            return Err("automatic index prohibition conflicts with an explicit index".into());
        }
        self.auto_index_excluded.insert(path);
        Ok(())
    }

    pub fn is_auto_index_prohibited(&self, path: &[String]) -> bool {
        self.auto_index_excluded.contains(path)
    }

    /// Register an exact composite order over previously configured scalar sort fields.
    pub fn register_composite_sort(
        &mut self,
        fields: Vec<crate::workload::SortField>,
    ) -> Result<(), String> {
        if fields.len() < 2 {
            return Err("composite sort requires at least two fields".into());
        }
        let normalized = fields
            .iter()
            .map(crate::sorting::simple_composite_part)
            .collect::<Option<Vec<_>>>()
            .ok_or("composite sort requires resident scalar fields")?;
        if self.composite_sorts.iter().any(|existing| {
            existing
                .iter()
                .map(crate::sorting::simple_composite_part)
                .collect::<Option<Vec<_>>>()
                .as_ref()
                == Some(&normalized)
        }) {
            return Err("duplicate composite sort".into());
        }
        self.composite_sorts.push(fields);
        Ok(())
    }

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
        if let Some(root) = self.fields.get(&Vec::<String>::new()) {
            validate_declared(
                value,
                &root.kind,
                root.nullable,
                &root.element_nullable,
                0,
                &["@root"],
            )?;
        } else if !matches!(value, Value::Object(_)) {
            if self.fields.is_empty() {
                return Ok(());
            }
            return Err("record root must be an object".into());
        }
        validate_children(value, &self.validation, &mut Vec::new(), selected_top_level)
    }

    /// Apply conversions already approved by the published index. Search may
    /// need converted values again after reading bytes, but does not recheck
    /// the declared schema until an explicit index operation updates it.
    pub(crate) fn convert_indexed_record(
        &self,
        value: &mut Value,
        selected_top_level: Option<&HashSet<String>>,
    ) -> Result<(), String> {
        if !self.converted_fields {
            return Ok(());
        }
        if let Some(root) = self.fields.get(&Vec::<String>::new()) {
            convert_declared_indexed(value, &root.kind, &root.element_nullable, 0)?;
        }
        convert_children_indexed(value, &self.conversion_validation, selected_top_level)
    }

    pub fn has_converted_fields(&self) -> bool {
        self.converted_fields
    }

    pub fn register_field(&mut self, field: FieldDefinition) -> Result<(), String> {
        if self.fields.contains_key(&field.path) {
            return Err("duplicate field registration".into());
        }
        crate::relation::validate_field_against_relations(&field, &self.relations)?;
        let converted = converted_kind(&field.kind);
        self.converted_fields |= converted;
        let mut node = &mut self.validation;
        for part in &field.path {
            node = node.children.entry(part.clone()).or_default();
        }
        node.field = Some(field.clone());
        if self.converted_fields {
            self.update_conversion_validation(&field, converted);
        }
        self.fields.insert(field.path.clone(), field);
        Ok(())
    }

    fn update_conversion_validation(&mut self, field: &FieldDefinition, converted: bool) {
        if field.path.is_empty() {
            if converted {
                self.conversion_validation.field = Some(field.clone());
            }
            return;
        }
        if converted {
            let mut node = &mut self.conversion_validation;
            for depth in 0..field.path.len() {
                node = node.children.entry(field.path[depth].clone()).or_default();
                node.field = if depth + 1 == field.path.len() {
                    Some(field.clone())
                } else {
                    self.fields.get(&field.path[..=depth]).cloned()
                };
            }
        } else {
            let mut node = &mut self.conversion_validation;
            for part in &field.path {
                let Some(child) = node.children.get_mut(part) else {
                    return;
                };
                node = child;
            }
            node.field = Some(field.clone());
        }
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
            PathRoot::Backlinks => Err("backlink path requires relation binding".into()),
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
            PathRoot::Backlinks => Err("backlink path requires relation binding".into()),
        }
    }
}


fn converted_kind(kind: &LogicalType) -> bool {
    match kind {
        LogicalType::Id | LogicalType::Date | LogicalType::Datetime | LogicalType::Unixtime => true,
        LogicalType::Array(inner) => converted_kind(inner),
        _ => false,
    }
}

fn validate_children<'a>(
    value: &mut Value,
    node: &'a ValidationNode,
    path: &mut Vec<&'a str>,
    selected: Option<&HashSet<String>>,
) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            for (name, child) in &node.children {
                if selected.is_some_and(|selection| !selection.contains(name)) {
                    continue;
                }
                path.push(name);
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

fn convert_children_indexed(
    value: &mut Value,
    node: &ValidationNode,
    selected: Option<&HashSet<String>>,
) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            for (name, child) in &node.children {
                if selected.is_some_and(|selection| !selection.contains(name)) {
                    continue;
                }
                if let Some(value) = fields.get_mut(name) {
                    if let Some(field) = &child.field {
                        convert_declared_indexed(value, &field.kind, &field.element_nullable, 0)?;
                    }
                    if !matches!(value, Value::Null) {
                        convert_children_indexed(value, child, None)?;
                    }
                }
            }
        }
        Value::Array(array) => {
            for value in &mut array.values {
                if !matches!(value, Value::Null) {
                    convert_children_indexed(value, node, None)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn convert_declared_indexed(
    value: &mut Value,
    kind: &LogicalType,
    elements: &[bool],
    depth: usize,
) -> Result<(), String> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    match kind {
        LogicalType::Id => {
            if let Value::String(text) = value {
                *value = Value::Id(text.clone());
            }
        }
        LogicalType::Date | LogicalType::Datetime => {
            if let Value::String(text) = value {
                *value = Value::parse_typed_string(text, kind)?;
            }
        }
        LogicalType::Unixtime => {
            if let Value::Number(Number::Integer(seconds)) = value {
                *value =
                    Value::Unixtime(i64::try_from(*seconds).map_err(|_| "unixtime out of range")?);
            }
        }
        _ => {}
    }
    if let (Value::Array(array), LogicalType::Array(inner)) = (&mut *value, kind) {
        for item in &mut array.values {
            convert_declared_indexed(item, inner, elements, depth + 1)?;
        }
        array.element_type = Some((**inner).clone());
        array.nullable_elements = elements.get(depth).copied().unwrap_or(true);
    }
    Ok(())
}

fn validate_declared(
    value: &mut Value,
    kind: &LogicalType,
    nullable: bool,
    elements: &[bool],
    depth: usize,
    path: &[&str],
) -> Result<(), String> {
    if matches!(value, Value::Null) {
        return if nullable {
            Ok(())
        } else {
            Err(format!("{}: null is not allowed", path.join(".")))
        };
    }
    if *kind == LogicalType::Id {
        match value {
            Value::String(text) | Value::Id(text) => {
                if text.is_empty() {
                    return Err(format!("{}: ID is empty", path.join(".")));
                }
                *value = Value::Id(text.clone());
            }
            _ => {}
        }
    } else if matches!(kind, LogicalType::Date | LogicalType::Datetime) {
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
        MetadataField::FilePath | MetadataField::CollectionName | MetadataField::RepositoryRoot => {
            LogicalType::String
        }
        MetadataField::RecordId => LogicalType::Id,
        MetadataField::RecordByteStart
        | MetadataField::RecordByteEnd
        | MetadataField::RecordByteLength
        | MetadataField::FileStatus
        | MetadataField::RecordClass
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


/// Substring pattern parsed once for a bound literal.
#[derive(Clone, Debug)]
pub(crate) enum PreparedContains {
    Literal(Finder<'static>),
    Unescaped(Finder<'static>),
    Wildcards(Vec<Finder<'static>>),
}


#[cfg(test)]
thread_local! {
    static PREPARED_CONTAINS_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn split_contains_wildcards(pattern: &str) -> (bool, Vec<String>) {
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
    (wildcard, pieces)
}


impl PreparedContains {
    fn retained_heap_bytes(&self) -> usize {
        match self {
            Self::Literal(finder) | Self::Unescaped(finder) => finder.needle().len(),
            Self::Wildcards(pieces) => pieces
                .capacity()
                .saturating_mul(std::mem::size_of::<Finder<'static>>())
                .saturating_add(
                    pieces
                        .iter()
                        .map(|finder| finder.needle().len())
                        .fold(0usize, usize::saturating_add),
                ),
        }
    }

    pub(crate) fn new(pattern: &str) -> Self {
        #[cfg(test)]
        PREPARED_CONTAINS_BUILDS.with(|count| count.set(count.get() + 1));
        if !pattern.as_bytes().contains(&b'%') {
            return Self::Literal(Finder::new(pattern).into_owned());
        }
        let (wildcard, mut pieces) = split_contains_wildcards(pattern);
        if wildcard {
            Self::Wildcards(
                pieces
                    .into_iter()
                    .filter(|piece| !piece.is_empty())
                    .map(|piece| Finder::new(&piece).into_owned())
                    .collect(),
            )
        } else {
            Self::Unescaped(Finder::new(&pieces.pop().unwrap()).into_owned())
        }
    }

    fn matches_bytes(&self, text: &[u8]) -> bool {
        match self {
            Self::Literal(finder) | Self::Unescaped(finder) => finder.find(text).is_some(),
            Self::Wildcards(pieces) => {
                let mut rest = text;
                for finder in pieces {
                    let Some(index) = finder.find(rest) else {
                        return false;
                    };
                    rest = &rest[index + finder.needle().len()..];
                }
                true
            }
        }
    }

    pub(crate) fn matches(&self, text: &str, _literal: &str) -> bool {
        self.matches_bytes(text.as_bytes())
    }
}


/// Typed predicate with its resolved path, literal, and binding.
#[derive(Clone, Debug)]
pub struct BoundTerm {
    pub path: FieldPath,
    pub query_path: QueryPath,
    pub rhs: Option<Box<BoundFieldOperand>>,
    pub related: Option<Box<crate::relation_query::BoundRelationPath>>,
    pub kind: LogicalType,
    pub source_kind: LogicalType,
    pub operator: Operator,
    pub quantifier: Quantifier,
    pub literal: Value,
    pub period: Option<TemporalBounds>,
    pub(crate) prepared_contains: Option<PreparedContains>,
    pub(crate) gram_request: Option<BigramRequest>,
    pub derived: Derived,
    pub transform: Transform,
    pub operation: Option<Arc<dyn BindingOperation>>,
    pub matcher: Option<Arc<dyn PreparedFilterMatcher>>,
    pub binding_name: String,
    pub binding_version: u32,
    pub indexed: bool,
    pub multi: bool,
    pub implicit_array: bool,
    pub span: (usize, usize),
}


/// Prepared right-hand field for one dynamic scalar comparison.
#[derive(Clone, Debug)]
pub struct BoundFieldOperand {
    pub path: FieldPath,
    pub query_path: QueryPath,
    pub kind: LogicalType,
    pub derived: Derived,
    pub transform: Transform,
    pub operation: Option<Arc<dyn BindingOperation>>,
    pub multi: bool,
}


/// Compare the exact semantics of two predicates within one collection plan.
/// Source spans and gram requests locate/accelerate a predicate; neither is
/// its identity. The enclosing plan supplies the collection-generation scope.
pub(crate) fn same_bound_term(a: &BoundTerm, b: &BoundTerm) -> bool {
    fn same_arc<T: ?Sized>(a: &Option<Arc<T>>, b: &Option<Arc<T>>) -> bool {
        match (a, b) {
            (None, None) => true,
            (Some(a), Some(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
    let same_rhs = match (&a.rhs, &b.rhs) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.path == b.path
                && a.query_path == b.query_path
                && a.kind == b.kind
                && a.derived == b.derived
                && a.transform == b.transform
                && a.multi == b.multi
                && same_arc(&a.operation, &b.operation)
        }
        _ => false,
    };
    let same_related = match (&a.related, &b.related) {
        (None, None) => true,
        (Some(a), Some(b)) => {
            a.hops == b.hops
                && a.target_is_lhs == b.target_is_lhs
                && a.evidence_path == b.evidence_path
                && same_bound_term(&a.terminal, &b.terminal)
                && match (&a.source_operand, &b.source_operand) {
                    (None, None) => true,
                    (Some(a), Some(b)) => same_bound_term(a, b),
                    _ => false,
                }
        }
        _ => false,
    };
    a.path == b.path
        && a.query_path == b.query_path
        && a.kind == b.kind
        && a.source_kind == b.source_kind
        && a.operator == b.operator
        && a.quantifier == b.quantifier
        && a.literal == b.literal
        && a.period.as_ref().map(|period| (&period.start, &period.end))
            == b.period.as_ref().map(|period| (&period.start, &period.end))
        && a.derived == b.derived
        && a.transform == b.transform
        && a.binding_name == b.binding_name
        && a.binding_version == b.binding_version
        && a.multi == b.multi
        && a.implicit_array == b.implicit_array
        && same_arc(&a.operation, &b.operation)
        && same_arc(&a.matcher, &b.matcher)
        && same_rhs
        && same_related
}


#[derive(Debug)]
struct MappedFilterMatcher {
    operation: Arc<dyn BindingOperation>,
    literal: Value,
    operator: Operator,
    text_equivalent: bool,
}


impl PreparedFilterMatcher for MappedFilterMatcher {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + value_retained_heap_bytes(&self.literal)
    }

    fn matches(&self, value: Option<&Value>, _: Option<&[u8]>) -> Result<bool, String> {
        Ok(value
            .and_then(|value| self.operation.map_value(value))
            .as_ref()
            .is_some_and(|mapped| {
                compare_scalar(mapped, &self.literal, self.operator, Transform::Identity)
            }))
    }

    fn posting_compatible(&self) -> bool {
        true
    }

    fn bigram_compatible(&self) -> bool {
        self.text_equivalent
    }
}


const MAX_FOLD_SCRATCH_CAPACITY: usize = 256 * 1024;

thread_local! {
    static FOLD_SCRATCH: RefCell<String> = const { RefCell::new(String::new()) };
    static FOLD_RECORD_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    #[cfg(test)]
    static FOLD_GROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}


/// Keep exceptional folding capacity until all segments of one record have
/// consumed it. Unwrapped callers retain the previous per-segment bound.
pub(crate) struct FoldRecordGuard;


impl FoldRecordGuard {
    pub(crate) fn new() -> Self {
        FOLD_RECORD_DEPTH.with(|depth| depth.set(depth.get() + 1));
        Self
    }
}


impl Drop for FoldRecordGuard {
    fn drop(&mut self) {
        FOLD_RECORD_DEPTH.with(|depth| {
            let remaining = depth.get() - 1;
            depth.set(remaining);
            if remaining == 0 {
                trim_fold_scratch();
            }
        });
    }
}


fn trim_fold_scratch() {
    FOLD_SCRATCH.with(|scratch| {
        let mut buffer = scratch.borrow_mut();
        if buffer.capacity() > MAX_FOLD_SCRATCH_CAPACITY {
            *buffer = String::new();
        }
    });
}

#[cfg(test)]
pub(crate) fn fold_scratch_stats() -> (usize, usize) {
    (
        FOLD_GROWS.with(std::cell::Cell::get),
        FOLD_SCRATCH.with(|scratch| scratch.borrow().capacity()),
    )
}

/// Reuse one worker thread's UTF-8 folding buffer without retaining large records.
fn with_folded_candidate_str<R>(text: &str, inspect: impl FnOnce(&str) -> R) -> R {
    FOLD_SCRATCH.with(|scratch| {
        let mut buffer = scratch.borrow_mut();
        #[cfg(test)]
        let old_capacity = buffer.capacity();
        fold_into(text, &mut buffer);
        #[cfg(test)]
        if buffer.capacity() > old_capacity {
            FOLD_GROWS.with(|count| count.set(count.get() + 1));
        }
        let result = inspect(&buffer);
        if FOLD_RECORD_DEPTH.with(|depth| depth.get() == 0)
            && buffer.capacity() > MAX_FOLD_SCRATCH_CAPACITY
        {
            *buffer = String::new();
        }
        result
    })
}

fn with_folded_candidate<R>(text: &str, inspect: impl FnOnce(&[u8]) -> R) -> R {
    with_folded_candidate_str(text, |folded| inspect(folded.as_bytes()))
}


#[derive(Debug)]
struct MappedTextMatcher {
    operation: Arc<dyn BindingOperation>,
    finder: Finder<'static>,
    case_sensitivity: CaseSensitivity,
}


impl MappedTextMatcher {
    fn new(
        operation: Arc<dyn BindingOperation>,
        literal: String,
        case_sensitivity: CaseSensitivity,
    ) -> Self {
        let finder = Finder::new(&literal).into_owned();
        Self {
            operation,
            finder,
            case_sensitivity,
        }
    }
}


impl PreparedTextMatcher for MappedTextMatcher {
    fn retained_bytes(&self) -> usize {
        std::mem::size_of::<Self>() + self.finder.needle().len()
    }

    fn matches(&self, text: &str) -> Result<bool, String> {
        let mapped = self.operation.map_text(text);
        Ok(match self.case_sensitivity {
            CaseSensitivity::Sensitive => self.finder.find(mapped.as_bytes()).is_some(),
            CaseSensitivity::Insensitive => {
                with_folded_candidate(&mapped, |folded| self.finder.find(folded).is_some())
            }
        })
    }

    fn bigram_compatible(&self) -> bool {
        true
    }
}


/// Regex over a selected registered text representation, one segment at a time.
#[derive(Debug)]
struct RegexTextMatcher {
    regex: CompiledRegex,
    transform: Transform,
    operation: Option<Arc<dyn BindingOperation>>,
}


impl PreparedTextMatcher for RegexTextMatcher {
    fn retained_bytes(&self) -> usize {
        // Every regex FTS binding also retains the original `fallback_regex`.
        // Its Arc<Regex> owns the compiled automaton once; these wrappers own
        // only their cloned candidate conditions.
        std::mem::size_of::<Self>() + self.regex.retained_local_heap_bytes()
    }

    fn matches(&self, text: &str) -> Result<bool, String> {
        if let Some(operation) = &self.operation {
            return Ok(self.regex.is_match(&operation.map_text(text)));
        }
        if self.transform == Transform::CaseFold {
            return Ok(with_folded_candidate_str(text, |folded| {
                self.regex.is_match(folded)
            }));
        }
        Ok(self.regex.is_match(text))
    }

    fn bigram_compatible(&self) -> bool {
        true
    }
}


#[derive(Clone, Debug)]
pub enum BoundExpr {
    Term(BoundTerm),
    And(Vec<BoundExpr>),
    Or(Vec<BoundExpr>),
    Not(Box<BoundExpr>),
}


pub fn bind_filter(expr: &FilterExpr, schema: &Schema) -> Result<BoundExpr, String> {
    bind_filter_with_parameters(expr, schema, &mut HashMap::new())
}

/// Bind one FILTER while sharing parameter type constraints across collections.
pub(crate) fn bind_filter_with_parameters(
    expr: &FilterExpr,
    schema: &Schema,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<BoundExpr, String> {
    Ok(match expr {
        FilterExpr::Term(term) => BoundExpr::Term(bind_term(term, schema, parameter_types)?),
        FilterExpr::And(parts) => BoundExpr::And(
            parts
                .iter()
                .map(|p| bind_filter_with_parameters(p, schema, parameter_types))
                .collect::<Result<_, _>>()?,
        ),
        FilterExpr::Or(parts) => BoundExpr::Or(
            parts
                .iter()
                .map(|p| bind_filter_with_parameters(p, schema, parameter_types))
                .collect::<Result<_, _>>()?,
        ),
        FilterExpr::Not(inner) => BoundExpr::Not(Box::new(bind_filter_with_parameters(
            inner,
            schema,
            parameter_types,
        )?)),
    })
}

fn query_path_type(
    schema: &Schema,
    path: &QueryPath,
    field: &FieldDefinition,
) -> Result<(LogicalType, bool, bool), String> {
    if path.field.root != PathRoot::Data {
        return Ok((field.kind.clone(), field.multi, false));
    }
    let mut prefix = Vec::new();
    let mut current = LogicalType::Unknown;
    let mut implicit_array = false;
    let mut each = false;
    for step in &path.steps {
        match step {
            PathStep::Field(name) => {
                if matches!(current, LogicalType::Array(_)) {
                    implicit_array = true;
                }
                prefix.push(name.clone());
                current = schema
                    .fields
                    .get(&prefix)
                    .map_or(LogicalType::Unknown, |definition| definition.kind.clone());
            }
            PathStep::ArrayIndex(_) | PathStep::EachElement => {
                current = match current {
                    LogicalType::Array(inner) => *inner,
                    LogicalType::Unknown => LogicalType::Unknown,
                    _ => return Err(format!("array selector on scalar field: {}", path.display)),
                };
                each |= matches!(step, PathStep::EachElement);
            }
        }
    }
    if current == LogicalType::Unknown {
        current = field.kind.clone();
    }
    let multi = if path.has_explicit_array_step {
        each || implicit_array || matches!(current, LogicalType::Array(_))
    } else {
        field.multi || matches!(current, LogicalType::Array(_))
    };
    Ok((current, multi, implicit_array))
}

fn bind_term(
    term: &Term,
    schema: &Schema,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<BoundTerm, String> {
    if let Operand::Field(field) = &term.operand {
        let mut left = term.clone();
        left.operand = Operand::Literal(Literal::Null);
        left.quantifier = Quantifier::Default;
        let mut lhs = bind_literal_term(&left, schema, parameter_types, true)?;
        let rhs_term = Term {
            path: field.path.clone(),
            modifiers: field.modifiers.clone(),
            quantifier: Quantifier::Default,
            operator: term.operator,
            operand: Operand::Literal(Literal::Null),
            span: field.span,
        };
        let rhs = bind_literal_term(&rhs_term, schema, parameter_types, true)?;
        if matches!(lhs.derived, Derived::Exists | Derived::Null)
            || matches!(rhs.derived, Derived::Exists | Derived::Null)
        {
            return Err("exists/null modifiers cannot compare to another field".into());
        }
        if lhs.kind != LogicalType::Unknown
            && rhs.kind != LogicalType::Unknown
            && lhs.kind != rhs.kind
        {
            return Err(format!(
                "incompatible field operand types: {:?} and {:?}",
                lhs.kind, rhs.kind
            ));
        }
        for operation in [lhs.operation.as_ref(), rhs.operation.as_ref()]
            .into_iter()
            .flatten()
        {
            if !operation.supports_dynamic_comparison() {
                return Err("custom binding does not support dynamic field comparison".into());
            }
        }
        let left_scope = explicit_scope(&lhs.query_path);
        let right_scope = explicit_scope(&rhs.query_path);
        if lhs.multi
            && rhs.multi
            && (left_scope.is_empty()
                || left_scope != right_scope
                || lhs.implicit_array
                || rhs.implicit_array
                || matches!(lhs.source_kind, LogicalType::Array(_))
                || matches!(rhs.source_kind, LogicalType::Array(_)))
        {
            return Err("independent multivalued field operands need a shared [*] scope or explicit indexes".into());
        }
        if term.quantifier == Quantifier::Any && term.modifiers.contains(&Modifier::Each) {
            return Err("each and existential operator conflict".into());
        }
        if term.quantifier == Quantifier::Any && !lhs.multi && !rhs.multi {
            return Err("existential comparison requires a multi-valued field".into());
        }
        lhs.quantifier = term.quantifier;
        lhs.rhs = Some(Box::new(BoundFieldOperand {
            path: rhs.path,
            query_path: rhs.query_path,
            kind: rhs.kind,
            derived: rhs.derived,
            transform: rhs.transform,
            operation: rhs.operation,
            multi: rhs.multi,
        }));
        return Ok(lhs);
    }
    bind_literal_term(term, schema, parameter_types, false)
}

fn explicit_scope(path: &QueryPath) -> Vec<Vec<PathStep>> {
    let mut prefix = Vec::new();
    let mut scopes = Vec::new();
    for step in &path.steps {
        prefix.push(step.clone());
        if matches!(step, PathStep::EachElement) {
            scopes.push(prefix.clone());
        }
    }
    scopes
}

fn bind_literal_term(
    term: &Term,
    schema: &Schema,
    parameter_types: &mut HashMap<String, LogicalType>,
    field_probe: bool,
) -> Result<BoundTerm, String> {
    let parameter_literal;
    let period_operand = match &term.operand {
        Operand::Period(period) => Some(period),
        _ => None,
    };
    let parsed_literal = match &term.operand {
        Operand::Literal(literal) => literal,
        Operand::Period(_) => {
            parameter_literal = Literal::Null;
            &parameter_literal
        }
        Operand::Macro { name, .. } => {
            return Err(format!("unresolved time macro @{name}"));
        }
        Operand::Field(field) => {
            return Err(format!(
                "field operand at byte {} is not yet supported by the binder",
                field.span.0
            ));
        }
        Operand::Parameter { name, bound } => {
            let value = bound
                .as_ref()
                .ok_or_else(|| format!("parameter ${name} requires execution bindings"))?;
            parameter_literal = match value {
                Value::Null => Literal::Null,
                Value::String(text) => Literal::String(text.clone()),
                Value::Number(number) => Literal::Number(number.clone()),
                Value::Bool(value) => Literal::Bool(*value),
                other => Literal::Typed(other.clone()),
            };
            &parameter_literal
        }
    };
    let parameter_name = if let Operand::Parameter { name, .. } = &term.operand {
        Some(name.as_str())
    } else {
        None
    };
    let field = schema.field_or_dynamic(&term.path.field)?;
    let (source_kind, path_multi, implicit_array) = query_path_type(schema, &term.path, &field)?;
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
                let key = (term.path.field.segments.clone(), name.clone());
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
        if let Some(name) = schema.defaults.get(&term.path.field.segments) {
            let key = (term.path.field.segments.clone(), name.clone());
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
            || (derived != Derived::Length && (each || term.quantifier == Quantifier::Any)))
    {
        return Err("derived modifier cannot be combined with quantifier or text binding".into());
    }
    if each && term.quantifier == Quantifier::Any {
        return Err("each and existential operator conflict".into());
    }
    let multi = path_multi;
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
    let kind = if term.path.field.root == PathRoot::Metadata(MetadataField::RecordId)
        && transform == Transform::CaseFold
        && derived == Derived::None
    {
        LogicalType::String
    } else {
        kind
    };
    if let Some(name) = parameter_name {
        let required = if kind == LogicalType::Unknown {
            match parsed_literal {
                Literal::Null => LogicalType::Unknown,
                Literal::String(_) => LogicalType::String,
                Literal::Number(_) => LogicalType::Number,
                Literal::Bool(_) => LogicalType::Bool,
                Literal::Typed(value) => value.logical_type(),
            }
        } else {
            kind.clone()
        };
        if let Some(previous) = parameter_types.get(name) {
            if *previous != LogicalType::Unknown
                && required != LogicalType::Unknown
                && *previous != required
            {
                return Err(format!(
                    "parameter ${name} has incompatible uses: {previous:?} and {required:?}"
                ));
            }
        }
        if required != LogicalType::Unknown {
            parameter_types.insert(name.to_owned(), required);
        }
    }
    if transform != Transform::Identity
        && !matches!(kind, LogicalType::String | LogicalType::Unknown)
    {
        return Err("text binding requires string values".into());
    }
    if let Some(op) = &operation {
        if !op.accepts(&kind, term.operator, multi) {
            return Err(format!("unsupported operation for binding {binding_name}"));
        }
    }
    if kind == LogicalType::Id && !matches!(term.operator, Operator::Eq | Operator::Ne) {
        return Err("ID permits only equality operators".into());
    }
    if matches!(term.operator, Operator::Contains | Operator::NotContains)
        && !matches!(kind, LogicalType::String | LogicalType::Unknown)
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
    if field_probe {
        return Ok(BoundTerm {
            path: term.path.field.clone(),
            query_path: term.path.clone(),
            rhs: None,
            related: None,
            kind,
            source_kind,
            operator: term.operator,
            quantifier: term.quantifier,
            literal: Value::Null,
            period: None,
            prepared_contains: None,
            gram_request: None,
            derived,
            transform,
            operation,
            matcher: None,
            binding_name,
            binding_version,
            indexed: field.indexed,
            multi,
            implicit_array,
            span: term.span,
        });
    }
    if transform != Transform::Identity
        && !matches!(parsed_literal, Literal::String(_) | Literal::Null)
    {
        return Err("text binding requires a string literal".into());
    }
    if matches!(term.operator, Operator::Contains | Operator::NotContains)
        && !matches!(parsed_literal, Literal::String(_))
    {
        return Err("contains requires a string literal".into());
    }
    let period = if let Some(source) = period_operand {
        if derived != Derived::None || transform != Transform::Identity || operation.is_some() {
            return Err("temporal period cannot use derived or text bindings".into());
        }
        Some(source.for_type(&kind)?)
    } else {
        None
    };
    let literal = if let Some(bounds) = &period {
        bounds.start.clone()
    } else if let Some(op) = &operation {
        op.prepare_literal(parsed_literal, &kind)?
    } else {
        prepare_literal(parsed_literal, &kind, transform)?
    };
    if matches!(literal, Value::Null) && !matches!(term.operator, Operator::Eq | Operator::Ne) {
        return Err("null permits only equality operators".into());
    }
    if derived != Derived::None && !matches!(literal, Value::Number(_) | Value::Bool(_)) {
        return Err("invalid derived literal".into());
    }
    let matcher = operation
        .as_ref()
        .map(|op| {
            op.prepare_filter_matcher(&literal, term.operator)
                .map(|prepared| {
                    prepared.unwrap_or_else(|| {
                        Arc::new(MappedFilterMatcher {
                            operation: op.clone(),
                            literal: literal.clone(),
                            operator: term.operator,
                            text_equivalent: op.value_text_equivalent(),
                        })
                    })
                })
        })
        .transpose()?;
    let gram_request = if derived == Derived::None
        && matches!(term.operator, Operator::Eq | Operator::Contains)
        && let Value::String(text) = &literal
        && !text.contains(['%', '\\'])
    {
        Some(BigramRequest::new(text))
    } else {
        None
    };
    let prepared_contains = if matcher.is_none()
        && matches!(term.operator, Operator::Contains | Operator::NotContains)
    {
        match &literal {
            Value::String(pattern) => Some(PreparedContains::new(pattern)),
            _ => None,
        }
    } else {
        None
    };
    Ok(BoundTerm {
        path: term.path.field.clone(),
        query_path: term.path.clone(),
        rhs: None,
        related: None,
        kind,
        source_kind,
        operator: term.operator,
        quantifier: term.quantifier,
        literal,
        period,
        prepared_contains,
        gram_request,
        derived,
        transform,
        operation,
        matcher,
        binding_name,
        binding_version,
        indexed: field.indexed,
        multi,
        implicit_array,
        span: term.span,
    })
}

pub(crate) fn bind_field_probe(
    term: &Term,
    schema: &Schema,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<BoundTerm, String> {
    bind_literal_term(term, schema, parameter_types, true)
}

fn prepare_literal(
    literal: &Literal,
    kind: &LogicalType,
    transform: Transform,
) -> Result<Value, String> {
    let value = match (literal, kind) {
        (Literal::Null, _) => Value::Null,
        (Literal::String(s), LogicalType::String) => Value::String(s.clone()),
        (Literal::String(s), LogicalType::Id) => Value::parse_typed_string(s, kind)?,
        (Literal::String(s), LogicalType::Date | LogicalType::Datetime) => {
            Value::parse_typed_string(s, kind)?
        }
        (Literal::Number(n), LogicalType::Number) => Value::Number(n.clone()),
        (Literal::Number(Number::Integer(n)), LogicalType::Unixtime) => {
            Value::Unixtime(i64::try_from(*n).map_err(|_| "unixtime out of range")?)
        }
        (Literal::Bool(b), LogicalType::Bool) => Value::Bool(*b),
        (Literal::String(s), LogicalType::Unknown) => Value::String(s.clone()),
        (Literal::Number(n), LogicalType::Unknown) => Value::Number(n.clone()),
        (Literal::Bool(b), LogicalType::Unknown) => Value::Bool(*b),
        (Literal::Typed(value), LogicalType::Unknown)
            if !matches!(value, Value::Object(_) | Value::Array(_)) =>
        {
            value.clone()
        }
        (Literal::Typed(value), expected)
            if !matches!(value, Value::Null | Value::Object(_) | Value::Array(_))
                && value.logical_type() == *expected =>
        {
            value.clone()
        }
        _ => return Err(format!("literal is incompatible with {kind:?}")),
    };
    Ok(match value {
        Value::String(s) if transform == Transform::CaseFold => Value::String(fold(&s)),
        other => other,
    })
}


/// Record data and inventory metadata available during evaluation.
pub struct RecordContext<'a> {
    pub value: &'a Value,
    pub source_bytes: Option<&'a [u8]>,
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
        MetadataField::RecordId => Value::Id(ctx.record.id.0.clone()),
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
        MetadataField::FileStatus => number(ctx.file.status.code() as u64),
        MetadataField::RecordClass => number(ctx.record.class.code() as u64),
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

fn walk_path<'a>(value: &'a Value, parts: &[String], output: &mut impl Extend<&'a Value>) -> bool {
    if parts.is_empty() {
        output.extend([value]);
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

fn walk_query_steps<'a>(
    value: &'a Value,
    steps: &[PathStep],
    output: &mut impl Extend<&'a Value>,
) -> bool {
    let Some(step) = steps.first() else {
        output.extend([value]);
        return true;
    };
    match step {
        PathStep::Field(name) => match value {
            Value::Object(map) => map
                .get(name)
                .is_some_and(|value| walk_query_steps(value, &steps[1..], output)),
            Value::Array(array) => {
                let mut found = array.values.is_empty();
                for item in &array.values {
                    found |= walk_query_steps(item, steps, output);
                }
                found
            }
            _ => false,
        },
        PathStep::ArrayIndex(index) => match value {
            Value::Array(array) => array
                .values
                .get(*index as usize)
                .is_some_and(|value| walk_query_steps(value, &steps[1..], output)),
            _ => false,
        },
        PathStep::EachElement => match value {
            Value::Array(array) => {
                let mut found = array.values.is_empty();
                for item in &array.values {
                    found |= walk_query_steps(item, &steps[1..], output);
                }
                found
            }
            _ => false,
        },
    }
}

pub(crate) fn data_values<'a>(value: &'a Value, parts: &[String]) -> Option<Vec<&'a Value>> {
    data_values_small(value, parts).map(SmallVec::into_vec)
}

pub(crate) fn data_values_small<'a>(
    value: &'a Value,
    parts: &[String],
) -> Option<SmallVec<[&'a Value; 1]>> {
    let mut selected = SmallVec::new();
    walk_path(value, parts, &mut selected).then_some(selected)
}

fn select_values<'a>(
    term: &BoundTerm,
    ctx: &'a RecordContext<'a>,
) -> Result<Option<Vec<&'a Value>>, String> {
    match term.path.root {
        PathRoot::Data => {
            let mut selected = Vec::new();
            let found = if term.query_path.has_explicit_array_step {
                walk_query_steps(ctx.value, &term.query_path.steps, &mut selected)
            } else {
                walk_path(ctx.value, &term.path.segments, &mut selected)
            };
            if found { Ok(Some(selected)) } else { Ok(None) }
        }
        PathRoot::Metadata(field) => {
            // Metadata is evaluated from inventory without source parsing.
            // The owned value is handled separately in `eval_term`.
            let _ = field;
            Ok(Some(Vec::new()))
        }
        PathRoot::Backlinks => Err("backlink path requires relation binding".into()),
    }
}

pub(crate) fn compare_scalar(lhs: &Value, rhs: &Value, op: Operator, transform: Transform) -> bool {
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
    if transform == Transform::CaseFold {
        let (Value::String(text), Value::String(pattern)) = (lhs, rhs) else {
            return false;
        };
        return with_folded_candidate_str(text, |folded| {
            let order = folded.cmp(pattern);
            match op {
                Operator::Eq => order.is_eq(),
                Operator::Ne => !order.is_eq(),
                Operator::Gt => order.is_gt(),
                Operator::Ge => !order.is_lt(),
                Operator::Lt => order.is_lt(),
                Operator::Le => !order.is_gt(),
                Operator::Contains => contains_pattern(folded, pattern),
                Operator::NotContains => !contains_pattern(folded, pattern),
            }
        });
    }
    let order = match (lhs, rhs) {
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Id(a), Value::Id(b)) => Some(a.cmp(b)),
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

fn compare_bound(lhs: &Value, term: &BoundTerm) -> Result<bool, String> {
    if let Some(period) = &term.period {
        return Ok(period.matches(lhs, term.operator));
    }
    if let Some(matcher) = &term.matcher {
        return matcher.matches(Some(lhs), None);
    }
    if let (Some(prepared), Value::String(pattern)) = (&term.prepared_contains, &term.literal) {
        let Value::String(text) = lhs else {
            return Ok(false);
        };
        let found = if term.transform == Transform::CaseFold {
            with_folded_candidate(text, |folded| prepared.matches_bytes(folded))
        } else {
            prepared.matches(text, pattern)
        };
        return Ok(if term.operator == Operator::Contains {
            found
        } else {
            !found
        });
    }
    Ok(compare_scalar(
        lhs,
        &term.literal,
        term.operator,
        term.transform,
    ))
}

// None denotes a selected value that cannot satisfy a derived length
// comparison. It is distinct from a selected, explicit Value::Null.
type ScopedValues<'a> = Vec<(Vec<usize>, Option<Cow<'a, Value>>)>;

fn walk_scoped<'a>(
    value: &'a Value,
    steps: &[PathStep],
    positions: &mut Vec<usize>,
    output: &mut ScopedValues<'a>,
) -> bool {
    let Some(step) = steps.first() else {
        output.push((positions.clone(), Some(Cow::Borrowed(value))));
        return true;
    };
    match step {
        PathStep::Field(name) => match value {
            Value::Object(map) => map
                .get(name)
                .is_some_and(|value| walk_scoped(value, &steps[1..], positions, output)),
            Value::Array(array) => {
                let mut found = array.values.is_empty();
                for (index, item) in array.values.iter().enumerate() {
                    positions.push(index);
                    found |= walk_scoped(item, steps, positions, output);
                    positions.pop();
                }
                found
            }
            _ => false,
        },
        PathStep::ArrayIndex(index) => match value {
            Value::Array(array) => array
                .values
                .get(*index as usize)
                .is_some_and(|value| walk_scoped(value, &steps[1..], positions, output)),
            _ => false,
        },
        PathStep::EachElement => match value {
            Value::Array(array) => {
                let mut found = array.values.is_empty();
                for (index, item) in array.values.iter().enumerate() {
                    positions.push(index);
                    found |= walk_scoped(item, &steps[1..], positions, output);
                    positions.pop();
                }
                found
            }
            _ => false,
        },
    }
}

fn scoped_values<'a>(
    path: &FieldPath,
    query_path: &QueryPath,
    transform: Transform,
    derived: Derived,
    multi: bool,
    ctx: &'a RecordContext<'a>,
) -> Result<Option<ScopedValues<'a>>, String> {
    let mut values = Vec::new();
    let found = match path.root {
        PathRoot::Data => walk_scoped(ctx.value, &query_path.steps, &mut Vec::new(), &mut values),
        PathRoot::Metadata(field) => {
            let value = if field == MetadataField::RecordId && transform == Transform::CaseFold {
                Value::String(ctx.record.id.0.clone())
            } else {
                metadata_value(field, ctx)?
            };
            values.push((Vec::new(), Some(Cow::Owned(value))));
            true
        }
        PathRoot::Backlinks => return Err("backlink path requires relation binding".into()),
    };
    if matches!(derived, Derived::Exists | Derived::Null) {
        let result = match derived {
            Derived::Exists => found,
            Derived::Null => {
                values.len() == 1 && matches!(values[0].1.as_deref(), Some(Value::Null))
            }
            _ => unreachable!(),
        };
        return Ok(Some(vec![(
            Vec::new(),
            Some(Cow::Owned(Value::Bool(result))),
        )]));
    }
    if !found {
        return Ok(None);
    }
    if derived == Derived::Length {
        return Ok(Some(
            values
                .into_iter()
                .map(|(scope, value)| {
                    let length = match value.as_deref() {
                        Some(Value::Array(array)) => Some(array.values.len()),
                        _ => None,
                    };
                    (
                        scope,
                        length.map(|length| {
                            Cow::Owned(Value::Number(Number::integer(length as i128)))
                        }),
                    )
                })
                .collect(),
        ));
    }
    if multi {
        let mut flat = Vec::new();
        for (scope, value) in values {
            match value {
                Some(Cow::Borrowed(Value::Array(array))) => {
                    for (index, item) in array.values.iter().enumerate() {
                        let mut scope = scope.clone();
                        scope.push(index);
                        flat.push((scope, Some(Cow::Borrowed(item))));
                    }
                }
                other => flat.push((scope, other)),
            }
        }
        return Ok(Some(flat));
    }
    Ok(Some(values))
}

fn mapped_field_value<'a>(
    value: &'a Value,
    transform: Transform,
    operation: Option<&Arc<dyn BindingOperation>>,
) -> Option<Cow<'a, Value>> {
    if let Some(operation) = operation {
        return operation.map_value(value).map(Cow::Owned);
    }
    match (value, transform) {
        (Value::String(text), Transform::CaseFold) => Some(Cow::Owned(Value::String(fold(text)))),
        (Value::Null, Transform::CaseFold) => Some(Cow::Borrowed(value)),
        (_, Transform::CaseFold) => None,
        _ => Some(Cow::Borrowed(value)),
    }
}

fn eval_field_term(
    term: &BoundTerm,
    rhs: &BoundFieldOperand,
    ctx: &RecordContext<'_>,
) -> Result<bool, String> {
    let paired = term.multi && rhs.multi;
    let Some(left) = scoped_values(
        &term.path,
        &term.query_path,
        term.transform,
        term.derived,
        term.multi && !paired,
        ctx,
    )?
    else {
        return Ok(false);
    };
    let Some(right) = scoped_values(
        &rhs.path,
        &rhs.query_path,
        rhs.transform,
        rhs.derived,
        rhs.multi && !paired,
        ctx,
    )?
    else {
        return Ok(false);
    };
    let compare = |a: Option<&Value>, b: Option<&Value>| {
        let (Some(a), Some(b)) = (a, b) else {
            return false;
        };
        let Some(a) = mapped_field_value(a, term.transform, term.operation.as_ref()) else {
            return false;
        };
        let Some(b) = mapped_field_value(b, rhs.transform, rhs.operation.as_ref()) else {
            return false;
        };
        if term.operator == Operator::Ne
            && (matches!(a.as_ref(), Value::Null) || matches!(b.as_ref(), Value::Null))
        {
            return matches!(a.as_ref(), Value::Null) != matches!(b.as_ref(), Value::Null);
        }
        compare_scalar(a.as_ref(), b.as_ref(), term.operator, Transform::Identity)
    };
    let mut results = Vec::new();
    if paired {
        let expected_scope_depth = explicit_scope(&term.query_path).len();
        if left.iter().chain(&right).any(|(scope, value)| {
            scope.len() != expected_scope_depth || matches!(value.as_deref(), Some(Value::Array(_)))
        }) {
            return Err(
                "ambiguous nested multivalued field comparison; use explicit array selectors"
                    .into(),
            );
        }
        let mut right_by_scope = HashMap::with_capacity(right.len());
        for (scope, value) in &right {
            if right_by_scope
                .insert(scope.as_slice(), value.as_deref())
                .is_some()
            {
                return Err("ambiguous right-hand field occurrence".into());
            }
        }
        for (scope, value) in &left {
            let matching = right_by_scope.remove(scope.as_slice());
            results.push(matching.is_some_and(|other| compare(value.as_deref(), other)));
        }
        results.extend(std::iter::repeat_n(false, right_by_scope.len()));
    } else if term.multi {
        let Some((_, scalar)) = right.first() else {
            return Ok(false);
        };
        results.extend(
            left.iter()
                .map(|(_, value)| compare(value.as_deref(), scalar.as_deref())),
        );
    } else if rhs.multi {
        let Some((_, scalar)) = left.first() else {
            return Ok(false);
        };
        results.extend(
            right
                .iter()
                .map(|(_, value)| compare(scalar.as_deref(), value.as_deref())),
        );
    } else {
        let (Some((_, a)), Some((_, b))) = (left.first(), right.first()) else {
            return Ok(false);
        };
        results.push(compare(a.as_deref(), b.as_deref()));
    }
    Ok(if term.quantifier == Quantifier::Any {
        results.into_iter().any(|result| result)
    } else {
        results.into_iter().all(|result| result)
    })
}

/// Compare a source field with a field on one explicitly reached target.
/// The caller retains source context for each occurrence and quantifies over
/// relation edges after this local comparison.
pub(crate) fn eval_cross_record_field_term(
    target: &BoundTerm,
    source: &BoundTerm,
    target_ctx: &RecordContext<'_>,
    source_ctx: &RecordContext<'_>,
    target_is_lhs: bool,
    source_positions: &[u32],
    source_selectors: &[Option<u32>],
) -> Result<bool, String> {
    let Some(target_values) = scoped_values(
        &target.path,
        &target.query_path,
        target.transform,
        target.derived,
        target.multi,
        target_ctx,
    )?
    else {
        return Ok(false);
    };
    let Some(source_values) = scoped_values(
        &source.path,
        &source.query_path,
        source.transform,
        source.derived,
        false,
        source_ctx,
    )?
    else {
        return Ok(false);
    };
    let source_value = if source.multi {
        let scope_depth = explicit_scope(&source.query_path).len();
        // Fixed selectors occupy edge positions but not source occurrence
        // scopes. Pair only the wildcard slots shared at binding.
        let positions = source_selectors
            .iter()
            .enumerate()
            .filter(|(_, selector)| selector.is_none())
            .take(scope_depth)
            .map(|(index, _)| {
                source_positions
                    .get(index)
                    .copied()
                    .ok_or("relation occurrence has fewer positions than the shared source scope")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if positions.len() != scope_depth {
            return Err(
                "relation occurrence has fewer positions than the shared source scope".into(),
            );
        }
        let mut matching = source_values.iter().filter(|(scope, _)| {
            scope.len() == scope_depth
                && scope
                    .iter()
                    .zip(&positions)
                    .all(|(source, relation)| u32::try_from(*source) == Ok(*relation))
        });
        let selected = matching.next();
        if matching.next().is_some() {
            return Err("ambiguous source field occurrence".into());
        }
        selected.and_then(|(_, value)| value.as_deref())
    } else {
        if source_values.len() != 1 {
            return Err("source-dependent comparison requires one source value".into());
        }
        source_values[0].1.as_deref()
    };
    let Some(source_value) = source_value else {
        return Ok(false);
    };
    let Some(source_value) =
        mapped_field_value(source_value, source.transform, source.operation.as_ref())
    else {
        return Ok(false);
    };
    let mut answers = Vec::with_capacity(target_values.len());
    for (_, value) in target_values {
        let Some(value) = value.as_deref() else {
            answers.push(false);
            continue;
        };
        let Some(value) = mapped_field_value(value, target.transform, target.operation.as_ref())
        else {
            answers.push(false);
            continue;
        };
        let (lhs, rhs) = if target_is_lhs {
            (value.as_ref(), source_value.as_ref())
        } else {
            (source_value.as_ref(), value.as_ref())
        };
        answers.push(compare_scalar(
            lhs,
            rhs,
            target.operator,
            Transform::Identity,
        ));
    }
    Ok(if target.quantifier == Quantifier::Any {
        answers.into_iter().any(|answer| answer)
    } else {
        answers.into_iter().all(|answer| answer)
    })
}

/// `%` spans any sequence; `\%` means a literal percent. No other wildcard.
pub(crate) fn contains_pattern(text: &str, pattern: &str) -> bool {
    if !pattern.as_bytes().contains(&b'%') {
        return text.contains(pattern);
    }
    let (wildcard, pieces) = split_contains_wildcards(pattern);
    if !wildcard {
        return text.contains(&pieces[0]);
    }
    let mut rest = text;
    for piece in pieces.iter().filter(|piece| !piece.is_empty()) {
        let Some(index) = rest.find(piece) else {
            return false;
        };
        rest = &rest[index + piece.len()..];
    }
    true
}

pub fn eval_term(term: &BoundTerm, ctx: &RecordContext<'_>) -> Result<bool, String> {
    if let Some(rhs) = &term.rhs {
        return eval_field_term(term, rhs, ctx);
    }
    if let Some(matcher) = &term.matcher {
        if matcher.input() == ExactInput::OriginalRecordBytes {
            return matcher.matches(None, ctx.source_bytes);
        }
    }
    if let PathRoot::Metadata(field) = term.path.root {
        if term.derived == Derived::None && !term.multi {
            let value = if field == MetadataField::RecordId && term.transform == Transform::CaseFold
            {
                Value::String(ctx.record.id.0.clone())
            } else {
                metadata_value(field, ctx)?
            };
            if matches!(value, Value::Null) && !matches!(term.literal, Value::Null) {
                return Ok(false);
            }
            return compare_bound(&value, term);
        }
    }
    let owned;
    let mut selected = if let PathRoot::Metadata(field) = term.path.root {
        owned = if field == MetadataField::RecordId && term.transform == Transform::CaseFold {
            Value::String(ctx.record.id.0.clone())
        } else {
            metadata_value(field, ctx)?
        };
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
        let compare_length = |value: &&Value| {
            let Value::Array(array) = value else {
                return false;
            };
            compare_scalar(
                &Value::Number(Number::integer(array.values.len() as i128)),
                &term.literal,
                term.operator,
                Transform::Identity,
            )
        };
        return Ok(if term.multi {
            if term.quantifier == Quantifier::Any {
                values.iter().any(compare_length)
            } else {
                values.iter().all(compare_length)
            }
        } else {
            values.first().is_some_and(|value| compare_length(value))
        });
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
            for value in values {
                if compare_bound(value, term)? {
                    return Ok(true);
                }
            }
            Ok(false)
        } else {
            for value in values {
                if !compare_bound(value, term)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    } else {
        values
            .first()
            .map_or(Ok(false), |value| compare_bound(value, term))
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
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
            }
            match_
        }
        BoundExpr::And(parts) => {
            let mut result = true;
            let mut branch_evidence = HashSet::new();
            for p in parts {
                if !result {
                    break;
                }
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

/// Three-valued filter evaluation when a record has no trusted logical data.
pub fn eval_unavailable_filter(
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    evidence: &mut HashSet<String>,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            if term.related.is_some()
                || term.path.root == PathRoot::Data
                || term.path.root == PathRoot::Backlinks
                || term
                    .rhs
                    .as_ref()
                    .is_some_and(|rhs| rhs.path.root == PathRoot::Data)
            {
                return Ok(None);
            }
            let matched = eval_term(term, ctx)?;
            if matched
                && term.derived == Derived::None
                && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
            }
            Ok(Some(matched))
        }
        BoundExpr::Not(inner) => {
            Ok(eval_unavailable_filter(inner, ctx, &mut HashSet::new())?.map(|value| !value))
        }
        BoundExpr::And(parts) => {
            let mut unknown = false;
            let mut branch = HashSet::new();
            for part in parts {
                match eval_unavailable_filter(part, ctx, &mut branch)? {
                    Some(false) => return Ok(Some(false)),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            if unknown {
                Ok(None)
            } else {
                evidence.extend(branch);
                Ok(Some(true))
            }
        }
        BoundExpr::Or(parts) => {
            let mut unknown = false;
            let mut matched = false;
            for part in parts {
                let mut branch = HashSet::new();
                match eval_unavailable_filter(part, ctx, &mut branch)? {
                    Some(true) => {
                        matched = true;
                        evidence.extend(branch);
                    }
                    None => unknown = true,
                    Some(false) => {}
                }
            }
            if matched {
                Ok(Some(true))
            } else if unknown {
                Ok(None)
            } else {
                Ok(Some(false))
            }
        }
    }
}

/// Three-valued membership on raw fallback records without evidence storage.
pub(crate) fn eval_unavailable_filter_membership(
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            if term.related.is_some()
                || term.path.root == PathRoot::Data
                || term.path.root == PathRoot::Backlinks
                || term
                    .rhs
                    .as_ref()
                    .is_some_and(|rhs| rhs.path.root == PathRoot::Data)
            {
                Ok(None)
            } else {
                eval_term(term, ctx).map(Some)
            }
        }
        BoundExpr::Not(inner) => {
            Ok(eval_unavailable_filter_membership(inner, ctx)?.map(|value| !value))
        }
        BoundExpr::And(parts) => {
            let mut unknown = false;
            for part in parts {
                match eval_unavailable_filter_membership(part, ctx)? {
                    Some(false) => return Ok(Some(false)),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            Ok(if unknown { None } else { Some(true) })
        }
        BoundExpr::Or(parts) => {
            let mut unknown = false;
            for part in parts {
                match eval_unavailable_filter_membership(part, ctx)? {
                    Some(true) => return Ok(Some(true)),
                    None => unknown = true,
                    Some(false) => {}
                }
            }
            Ok(if unknown { None } else { Some(false) })
        }
    }
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
            || selected.iter().any(|p| {
                p.root == PathRoot::Data && (p.segments.is_empty() || p.segments == segment.path)
            })
        {
            if segment.text.contains(literal) {
                matched = true;
                evidence.insert(if segment.path.is_empty() {
                    "@root".into()
                } else {
                    segment.path.join(".")
                });
            }
        }
    }
    matched
}


/// One selected FTS target with its literal prepared for the bound operation.
#[derive(Clone, Debug)]
pub struct BoundFtsField {
    pub path: FieldPath,
    pub(crate) query_path: Option<QueryPath>,
    pub(crate) related: Option<Vec<crate::relation_query::BoundRelationHop>>,
    pub(crate) evidence_path: String,
    pub binding_name: String,
    pub literal: String,
    pub folded_literal: String,
    finder: Option<Finder<'static>>,
    pub(crate) gram_request: BigramRequest,
    pub transform: Transform,
    pub operation: Option<Arc<dyn BindingOperation>>,
    pub matcher: Option<Arc<dyn PreparedTextMatcher>>,
}


/// Collection-specific FTS bindings, including the configured raw target.
#[derive(Clone, Debug)]
pub struct BoundFts {
    pub case_sensitivity: CaseSensitivity,
    pub mode: FtsMode,
    pub literal: String,
    pub source_literal: String,
    pub folded_literal: String,
    pub folded_source_literal: String,
    finder: Option<Finder<'static>>,
    fallback_finder: Option<Finder<'static>>,
    pub(crate) gram_request: BigramRequest,
    pub(crate) fallback_gram_request: BigramRequest,
    pub raw_operation: Option<Arc<dyn BindingOperation>>,
    pub raw_matcher: Option<Arc<dyn PreparedTextMatcher>>,
    pub(crate) fallback_regex: Option<CompiledRegex>,
    pub fields: Vec<BoundFtsField>,
}


fn field_path_retained_heap_bytes(path: &FieldPath) -> usize {
    path.segments
        .capacity()
        .saturating_mul(std::mem::size_of::<String>())
        .saturating_add(
            path.segments
                .iter()
                .map(String::capacity)
                .fold(0usize, usize::saturating_add),
        )
}

fn query_path_retained_heap_bytes(path: &QueryPath) -> usize {
    field_path_retained_heap_bytes(&path.field)
        .saturating_add(
            path.steps
                .capacity()
                .saturating_mul(std::mem::size_of::<PathStep>()),
        )
        .saturating_add(
            path.steps
                .iter()
                .map(|step| match step {
                    PathStep::Field(name) => name.capacity(),
                    PathStep::ArrayIndex(_) | PathStep::EachElement => 0,
                })
                .fold(0usize, usize::saturating_add),
        )
        .saturating_add(path.display.capacity())
}

fn logical_type_retained_heap_bytes(kind: &LogicalType) -> usize {
    match kind {
        LogicalType::Array(inner) => std::mem::size_of::<LogicalType>()
            .saturating_add(logical_type_retained_heap_bytes(inner)),
        _ => 0,
    }
}

fn value_retained_heap_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) | Value::Id(text) => text.capacity(),
        Value::Array(array) => array
            .values
            .capacity()
            .saturating_mul(std::mem::size_of::<Value>())
            .saturating_add(
                array
                    .values
                    .iter()
                    .map(value_retained_heap_bytes)
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(array.element_type.as_ref().map_or(0, |kind| {
                std::mem::size_of::<LogicalType>()
                    .saturating_add(logical_type_retained_heap_bytes(kind))
            })),
        Value::Object(fields) => fields
            .len()
            .saturating_mul(
                std::mem::size_of::<(String, Value)>() + 4 * std::mem::size_of::<usize>(),
            )
            .saturating_add(
                fields
                    .iter()
                    .map(|(name, value)| {
                        name.capacity()
                            .saturating_add(value_retained_heap_bytes(value))
                    })
                    .fold(0usize, usize::saturating_add),
            ),
        _ => 0,
    }
}

fn matcher_retained_heap_bytes<M: ?Sized>(
    matcher: &Option<Arc<M>>,
    size: impl Fn(&M) -> usize,
) -> usize {
    matcher.as_ref().map_or(0, |matcher| {
        (2 * std::mem::size_of::<usize>()).saturating_add(size(matcher))
    })
}

fn relation_hops_retained_heap_bytes(hops: &[crate::relation_query::BoundRelationHop]) -> usize {
    hops.iter()
        .map(|hop| {
            hop.selectors
                .capacity()
                .saturating_mul(std::mem::size_of::<Option<u32>>())
        })
        .fold(0usize, usize::saturating_add)
}

fn bound_field_operand_retained_heap_bytes(operand: &BoundFieldOperand) -> usize {
    field_path_retained_heap_bytes(&operand.path)
        .saturating_add(query_path_retained_heap_bytes(&operand.query_path))
        .saturating_add(logical_type_retained_heap_bytes(&operand.kind))
}

fn bound_term_retained_heap_bytes(term: &BoundTerm) -> usize {
    let rhs = term.rhs.as_ref().map_or(0, |operand| {
        std::mem::size_of::<BoundFieldOperand>()
            .saturating_add(bound_field_operand_retained_heap_bytes(operand))
    });
    let related = term.related.as_ref().map_or(0, |related| {
        std::mem::size_of::<crate::relation_query::BoundRelationPath>()
            .saturating_add(
                related
                    .hops
                    .capacity()
                    .saturating_mul(std::mem::size_of::<crate::relation_query::BoundRelationHop>()),
            )
            .saturating_add(relation_hops_retained_heap_bytes(&related.hops))
            .saturating_add(std::mem::size_of::<BoundTerm>())
            .saturating_add(bound_term_retained_heap_bytes(&related.terminal))
            .saturating_add(related.source_operand.as_ref().map_or(0, |operand| {
                std::mem::size_of::<BoundTerm>()
                    .saturating_add(bound_term_retained_heap_bytes(operand))
            }))
            .saturating_add(related.evidence_path.capacity())
    });
    field_path_retained_heap_bytes(&term.path)
        .saturating_add(query_path_retained_heap_bytes(&term.query_path))
        .saturating_add(rhs)
        .saturating_add(related)
        .saturating_add(logical_type_retained_heap_bytes(&term.kind))
        .saturating_add(logical_type_retained_heap_bytes(&term.source_kind))
        .saturating_add(value_retained_heap_bytes(&term.literal))
        .saturating_add(term.period.as_ref().map_or(0, |period| {
            value_retained_heap_bytes(&period.start)
                .saturating_add(value_retained_heap_bytes(&period.end))
        }))
        .saturating_add(
            term.prepared_contains
                .as_ref()
                .map_or(0, PreparedContains::retained_heap_bytes),
        )
        .saturating_add(
            term.gram_request
                .as_ref()
                .map_or(0, BigramRequest::retained_heap_bytes),
        )
        .saturating_add(term.binding_name.capacity())
        .saturating_add(matcher_retained_heap_bytes(&term.matcher, |matcher| {
            matcher.retained_bytes()
        }))
}

pub(crate) fn bound_expr_retained_heap_bytes(expr: &BoundExpr) -> usize {
    match expr {
        BoundExpr::Term(term) => bound_term_retained_heap_bytes(term),
        BoundExpr::And(parts) | BoundExpr::Or(parts) => parts
            .capacity()
            .saturating_mul(std::mem::size_of::<BoundExpr>())
            .saturating_add(
                parts
                    .iter()
                    .map(bound_expr_retained_heap_bytes)
                    .fold(0usize, usize::saturating_add),
            ),
        BoundExpr::Not(inner) => {
            std::mem::size_of::<BoundExpr>().saturating_add(bound_expr_retained_heap_bytes(inner))
        }
    }
}

pub(crate) fn bound_fts_retained_heap_bytes(fts: &BoundFts) -> usize {
    let strings = fts
        .literal
        .capacity()
        .saturating_add(fts.source_literal.capacity())
        .saturating_add(fts.folded_literal.capacity())
        .saturating_add(fts.folded_source_literal.capacity());
    strings
        .saturating_add(
            fts.finder
                .as_ref()
                .map_or(0, |finder| finder.needle().len()),
        )
        .saturating_add(
            fts.fallback_finder
                .as_ref()
                .map_or(0, |finder| finder.needle().len()),
        )
        .saturating_add(fts.gram_request.retained_heap_bytes())
        .saturating_add(fts.fallback_gram_request.retained_heap_bytes())
        .saturating_add(matcher_retained_heap_bytes(&fts.raw_matcher, |matcher| {
            matcher.retained_bytes()
        }))
        .saturating_add(
            fts.fallback_regex
                .as_ref()
                .map_or(0, CompiledRegex::retained_heap_bytes),
        )
        .saturating_add(
            fts.fields
                .capacity()
                .saturating_mul(std::mem::size_of::<BoundFtsField>()),
        )
        .saturating_add(
            fts.fields
                .iter()
                .map(|field| {
                    field_path_retained_heap_bytes(&field.path)
                        .saturating_add(
                            field
                                .query_path
                                .as_ref()
                                .map_or(0, query_path_retained_heap_bytes),
                        )
                        .saturating_add(field.related.as_ref().map_or(0, |hops| {
                            hops.capacity()
                                .saturating_mul(std::mem::size_of::<
                                    crate::relation_query::BoundRelationHop,
                                >())
                                .saturating_add(relation_hops_retained_heap_bytes(hops))
                        }))
                        .saturating_add(field.evidence_path.capacity())
                        .saturating_add(field.binding_name.capacity())
                        .saturating_add(field.literal.capacity())
                        .saturating_add(field.folded_literal.capacity())
                        .saturating_add(
                            field
                                .finder
                                .as_ref()
                                .map_or(0, |finder| finder.needle().len()),
                        )
                        .saturating_add(field.gram_request.retained_heap_bytes())
                        .saturating_add(matcher_retained_heap_bytes(&field.matcher, |matcher| {
                            matcher.retained_bytes()
                        }))
                })
                .fold(0usize, usize::saturating_add),
        )
}

pub fn bind_fts(fts: &FtsQuery, schema: &Schema) -> Result<BoundFts, String> {
    if let Some(name) = &fts.pattern_parameter {
        return Err(format!(
            "regex pattern parameter ${name} requires execution bindings"
        ));
    }
    if let FtsMode::Regex(options) = fts.mode {
        return bind_regex_fts(fts, schema, options);
    }
    let raw_operation = fts
        .fields
        .is_empty()
        .then(|| schema.raw.as_ref())
        .flatten()
        .map(|binding| binding.operation.clone());
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
    let folded_literal = fold(&literal);
    let folded_source_literal = fold(&fts.literal);
    let raw_matcher = raw_operation
        .as_ref()
        .map(|op| {
            op.prepare_text_matcher(&literal).and_then(|prepared| {
                if fts.case_sensitivity == CaseSensitivity::Insensitive && prepared.is_some() {
                    return Err("custom raw FTS matcher does not support insensitive mode".into());
                }
                Ok(prepared.unwrap_or_else(|| {
                    Arc::new(MappedTextMatcher::new(
                        op.clone(),
                        if fts.case_sensitivity == CaseSensitivity::Insensitive {
                            folded_literal.clone()
                        } else {
                            literal.clone()
                        },
                        fts.case_sensitivity,
                    ))
                }))
            })
        })
        .transpose()?;
    let finder = raw_matcher.is_none().then(|| {
        Finder::new(if fts.case_sensitivity == CaseSensitivity::Insensitive {
            &folded_literal
        } else {
            &literal
        })
        .into_owned()
    });
    let fallback_finder = Finder::new(if fts.case_sensitivity == CaseSensitivity::Insensitive {
        &folded_source_literal
    } else {
        &fts.literal
    })
    .into_owned();
    let mut fields = Vec::new();
    for selected in &fts.fields {
        if selected.query_path.is_some() {
            return Err("selected FTS array selectors require a declared relation hop".into());
        }
        let definition = schema.field_or_dynamic(&selected.path)?;
        if !matches!(definition.kind, LogicalType::String)
            && !matches!(definition.kind, LogicalType::Array(ref inner) if **inner == LogicalType::String)
            && definition.kind != LogicalType::Unknown
            && selected.path.root != PathRoot::Metadata(MetadataField::RecordId)
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
            fold(&fts.literal)
        } else {
            fts.literal.clone()
        };
        let folded_needle = fold(&needle);
        let matcher = operation
            .as_ref()
            .map(|op| {
                op.prepare_text_matcher(&needle).and_then(|prepared| {
                    if fts.case_sensitivity == CaseSensitivity::Insensitive && prepared.is_some() {
                        return Err(format!(
                            "custom FTS matcher for {name} does not support insensitive mode"
                        ));
                    }
                    Ok(prepared.unwrap_or_else(|| {
                        Arc::new(MappedTextMatcher::new(
                            op.clone(),
                            if fts.case_sensitivity == CaseSensitivity::Insensitive {
                                folded_needle.clone()
                            } else {
                                needle.clone()
                            },
                            fts.case_sensitivity,
                        ))
                    }))
                })
            })
            .transpose()?;
        let finder = matcher.is_none().then(|| {
            Finder::new(if fts.case_sensitivity == CaseSensitivity::Insensitive {
                &folded_needle
            } else {
                &needle
            })
            .into_owned()
        });
        fields.push(BoundFtsField {
            path: selected.path.clone(),
            query_path: None,
            related: None,
            evidence_path: selected.path.display(),
            binding_name: name,
            gram_request: BigramRequest::from_folded(&folded_needle),
            folded_literal: folded_needle,
            literal: needle,
            finder,
            transform,
            operation,
            matcher,
        });
    }
    Ok(BoundFts {
        case_sensitivity: fts.case_sensitivity,
        mode: FtsMode::Literal,
        gram_request: BigramRequest::from_folded(&folded_literal),
        fallback_gram_request: BigramRequest::from_folded(&folded_source_literal),
        folded_literal,
        folded_source_literal,
        finder,
        fallback_finder: Some(fallback_finder),
        literal,
        source_literal: fts.literal.clone(),
        raw_operation,
        raw_matcher,
        fallback_regex: None,
        fields,
    })
}

fn bind_regex_fts(
    fts: &FtsQuery,
    schema: &Schema,
    options: crate::query::RegexOptions,
) -> Result<BoundFts, String> {
    let compiled = CompiledRegex::compile(&fts.literal, options, fts.case_sensitivity)?;
    let raw_operation = fts
        .fields
        .is_empty()
        .then(|| schema.raw.as_ref())
        .flatten()
        .map(|binding| binding.operation.clone());
    if raw_operation
        .as_ref()
        .is_some_and(|operation| !operation.regex_text_compatible())
    {
        return Err("raw FTS binding does not support regex mode".into());
    }
    let raw_matcher = fts.fields.is_empty().then(|| {
        Arc::new(RegexTextMatcher {
            regex: compiled.clone(),
            transform: Transform::Identity,
            operation: raw_operation.clone(),
        }) as Arc<dyn PreparedTextMatcher>
    });
    let mut fields = Vec::with_capacity(fts.fields.len());
    for selected in &fts.fields {
        if selected.query_path.is_some() {
            return Err("selected FTS array selectors require a declared relation hop".into());
        }
        let definition = schema.field_or_dynamic(&selected.path)?;
        if !matches!(definition.kind, LogicalType::String)
            && !matches!(definition.kind, LogicalType::Array(ref inner) if **inner == LogicalType::String)
            && definition.kind != LogicalType::Unknown
            && selected.path.root != PathRoot::Metadata(MetadataField::RecordId)
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
            if !binding.operation.regex_text_compatible() {
                return Err(format!("FTS binding {name} does not support regex mode"));
            }
            (Transform::Identity, Some(binding.operation.clone()))
        } else if let Some(binding) = schema.custom.get(&key) {
            (binding.transform, None)
        } else {
            return Err(format!("unknown FTS binding {name}"));
        };
        fields.push(BoundFtsField {
            path: selected.path.clone(),
            query_path: None,
            related: None,
            evidence_path: selected.path.display(),
            binding_name: name,
            literal: fts.literal.clone(),
            folded_literal: String::new(),
            finder: None,
            gram_request: BigramRequest::new(""),
            transform,
            operation: operation.clone(),
            matcher: Some(Arc::new(RegexTextMatcher {
                regex: compiled.clone(),
                transform,
                operation,
            })),
        });
    }
    Ok(BoundFts {
        case_sensitivity: fts.case_sensitivity,
        mode: fts.mode,
        literal: fts.literal.clone(),
        source_literal: fts.literal.clone(),
        folded_literal: String::new(),
        folded_source_literal: String::new(),
        finder: None,
        fallback_finder: None,
        gram_request: BigramRequest::new(""),
        fallback_gram_request: BigramRequest::new(""),
        raw_operation,
        raw_matcher,
        fallback_regex: Some(compiled),
        fields,
    })
}

fn text_contains(
    text: &str,
    needle: &str,
    folded_needle: &str,
    transform: Transform,
    matcher: Option<&Arc<dyn PreparedTextMatcher>>,
    case_sensitivity: CaseSensitivity,
    finder: Option<&Finder<'static>>,
) -> Result<bool, String> {
    if let Some(matcher) = matcher {
        matcher.matches(text)
    } else if transform == Transform::CaseFold || case_sensitivity == CaseSensitivity::Insensitive {
        let needle = if case_sensitivity == CaseSensitivity::Insensitive {
            folded_needle
        } else {
            needle
        };
        Ok(with_folded_candidate_str(text, |folded| {
            finder.map_or_else(
                || folded.contains(needle),
                |finder| finder.find(folded.as_bytes()).is_some(),
            )
        }))
    } else {
        Ok(finder.map_or_else(
            || text.contains(needle),
            |finder| finder.find(text.as_bytes()).is_some(),
        ))
    }
}

pub fn raw_fallback_contains(fts: &BoundFts, text: &str) -> bool {
    if let Some(regex) = &fts.fallback_regex {
        return regex.is_match(text);
    }
    match fts.case_sensitivity {
        CaseSensitivity::Sensitive => fts.fallback_finder.as_ref().map_or_else(
            || text.contains(&fts.source_literal),
            |finder| finder.find(text.as_bytes()).is_some(),
        ),
        CaseSensitivity::Insensitive => with_folded_candidate_str(text, |folded| {
            fts.fallback_finder.as_ref().map_or_else(
                || folded.contains(&fts.folded_source_literal),
                |finder| finder.find(folded.as_bytes()).is_some(),
            )
        }),
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
                &fts.folded_literal,
                Transform::Identity,
                fts.raw_matcher.as_ref(),
                fts.case_sensitivity,
                fts.finder.as_ref(),
            )? {
                matched = true;
                evidence.insert(if segment.path.is_empty() {
                    "@root".into()
                } else {
                    segment.path.join(".")
                });
            }
        }
        return Ok(matched);
    }
    for field in &fts.fields {
        matched |=
            eval_bound_fts_field(field, fts.case_sensitivity, segments, ctx, Some(evidence))?;
    }
    Ok(matched)
}

/// Visit value-owned strings using only borrowed text and borrowed key names.
pub(crate) fn visit_borrowed_value_strings<'a>(
    value: &'a Value,
    path: &mut Vec<&'a str>,
    inspect: &mut impl FnMut(&[&str], &'a str) -> Result<bool, String>,
) -> Result<bool, String> {
    match value {
        Value::String(text) | Value::Id(text) => inspect(path, text),
        Value::Object(fields) => {
            let mut matched = false;
            for (name, child) in fields {
                path.push(name);
                matched |= visit_borrowed_value_strings(child, path, inspect)?;
                path.pop();
            }
            Ok(matched)
        }
        Value::Array(array) => {
            let mut matched = false;
            for child in &array.values {
                matched |= visit_borrowed_value_strings(child, path, inspect)?;
            }
            Ok(matched)
        }
        _ => Ok(false),
    }
}

/// Search strings in a logical value without constructing owned segments or paths.
/// This is valid for readers whose searchable corpus is the converted value.
pub(crate) fn eval_borrowed_value_fts(
    fts: &BoundFts,
    ctx: &RecordContext<'_>,
    mut evidence: Option<&mut HashSet<String>>,
) -> Result<bool, String> {
    let mut path = Vec::new();
    let mut matched = false;
    if fts.fields.is_empty() {
        let mut inspect = |path: &[&str], text: &str| {
            if !text_contains(
                text,
                &fts.literal,
                &fts.folded_literal,
                Transform::Identity,
                fts.raw_matcher.as_ref(),
                fts.case_sensitivity,
                fts.finder.as_ref(),
            )? {
                return Ok(false);
            }
            if let Some(evidence) = evidence.as_deref_mut() {
                evidence.insert(if path.is_empty() {
                    "@root".into()
                } else {
                    path.join(".")
                });
            }
            Ok(true)
        };
        return visit_borrowed_value_strings(ctx.value, &mut path, &mut inspect);
    }
    for field in &fts.fields {
        matched |= eval_borrowed_value_fts_field(
            field,
            fts.case_sensitivity,
            ctx,
            evidence.as_deref_mut(),
        )?;
    }
    Ok(matched)
}

pub(crate) fn eval_borrowed_value_fts_field(
    field: &BoundFtsField,
    case_sensitivity: CaseSensitivity,
    ctx: &RecordContext<'_>,
    mut evidence: Option<&mut HashSet<String>>,
) -> Result<bool, String> {
    if field.query_path.is_some() || !matches!(field.path.root, PathRoot::Data) {
        return eval_bound_fts_field(field, case_sensitivity, &[], ctx, evidence);
    }
    let mut path = Vec::new();
    let mut inspect = |path: &[&str], text: &str| {
        if !fts_field_selects_path(&field.path.segments, path) {
            return Ok(false);
        }
        if !text_contains(
            text,
            &field.literal,
            &field.folded_literal,
            field.transform,
            field.matcher.as_ref(),
            case_sensitivity,
            field.finder.as_ref(),
        )? {
            return Ok(false);
        }
        if let Some(evidence) = evidence.as_deref_mut() {
            evidence.insert(field.evidence_path.clone());
        }
        Ok(true)
    };
    visit_borrowed_value_strings(ctx.value, &mut path, &mut inspect)
}

/// An empty selected path covers every text segment, including nested ones.
pub(crate) fn fts_field_selects_path<T: AsRef<str>>(selected: &[String], actual: &[T]) -> bool {
    selected.is_empty()
        || (selected.len() == actual.len()
            && selected
                .iter()
                .zip(actual)
                .all(|(expected, actual)| expected == actual.as_ref()))
}

/// Match one source-owned scalar occurrence before duplicate resolution.
/// The caller retains evidence only for occurrences that remain visible.
pub(crate) fn match_borrowed_source_segment<T: AsRef<str>>(
    fts: &BoundFts,
    path: &[T],
    text: &str,
    evidence: &mut Vec<String>,
    retain_evidence: bool,
) -> Result<bool, String> {
    if fts.fields.is_empty() {
        let found = text_contains(
            text,
            &fts.literal,
            &fts.folded_literal,
            Transform::Identity,
            fts.raw_matcher.as_ref(),
            fts.case_sensitivity,
            fts.finder.as_ref(),
        )?;
        if found && retain_evidence {
            evidence.push(if path.is_empty() {
                "@root".into()
            } else {
                path.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(".")
            });
        }
        return Ok(found);
    }
    let mut matched = false;
    for field in &fts.fields {
        if !fts_field_selects_path(&field.path.segments, path) {
            continue;
        }
        if text_contains(
            text,
            &field.literal,
            &field.folded_literal,
            field.transform,
            field.matcher.as_ref(),
            fts.case_sensitivity,
            field.finder.as_ref(),
        )? {
            matched = true;
            if retain_evidence {
                evidence.push(field.evidence_path.clone());
            }
        }
    }
    Ok(matched)
}

/// Match a string selected by an eligible local contains predicate.
pub(crate) fn match_borrowed_source_filter(term: &BoundTerm, text: &str) -> bool {
    let Some(prepared) = &term.prepared_contains else {
        return false;
    };
    let found = if term.transform == Transform::CaseFold {
        with_folded_candidate(text, |folded| prepared.matches_bytes(folded))
    } else {
        prepared.matches_bytes(text.as_bytes())
    };
    if term.operator == Operator::Contains {
        found
    } else {
        !found
    }
}

/// Evaluate one borrowed selected target, retaining every matched path when requested.
pub(crate) fn eval_bound_fts_field(
    field: &BoundFtsField,
    case_sensitivity: CaseSensitivity,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    mut evidence: Option<&mut HashSet<String>>,
) -> Result<bool, String> {
    let mut matched = false;
    if let PathRoot::Metadata(metadata) = field.path.root {
        if let Value::String(text) | Value::Id(text) = metadata_value(metadata, ctx)?
            && text_contains(
                &text,
                &field.literal,
                &field.folded_literal,
                field.transform,
                field.matcher.as_ref(),
                case_sensitivity,
                field.finder.as_ref(),
            )?
        {
            matched = true;
            if let Some(evidence) = evidence.as_deref_mut() {
                evidence.insert(field.evidence_path.clone());
            }
        }
    } else if let Some(path) = &field.query_path {
        let mut selected = Vec::new();
        walk_query_steps(ctx.value, &path.steps, &mut selected);
        for value in selected {
            if let Value::String(text) = value
                && text_contains(
                    text,
                    &field.literal,
                    &field.folded_literal,
                    field.transform,
                    field.matcher.as_ref(),
                    case_sensitivity,
                    field.finder.as_ref(),
                )?
            {
                matched = true;
                if let Some(evidence) = evidence.as_deref_mut() {
                    evidence.insert(field.evidence_path.clone());
                } else {
                    return Ok(true);
                }
            }
        }
    } else {
        for segment in segments
            .iter()
            .filter(|segment| fts_field_selects_path(&field.path.segments, &segment.path))
        {
            if text_contains(
                &segment.text,
                &field.literal,
                &field.folded_literal,
                field.transform,
                field.matcher.as_ref(),
                case_sensitivity,
                field.finder.as_ref(),
            )? {
                matched = true;
                if let Some(evidence) = evidence.as_deref_mut() {
                    evidence.insert(field.evidence_path.clone());
                } else {
                    return Ok(true);
                }
            }
        }
    }
    Ok(matched)
}

/// Membership-only FTS skips path materialization and stops at the first match.
pub(crate) fn eval_bound_fts_membership(
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
) -> Result<bool, String> {
    if fts.fields.is_empty() {
        for segment in segments {
            if text_contains(
                &segment.text,
                &fts.literal,
                &fts.folded_literal,
                Transform::Identity,
                fts.raw_matcher.as_ref(),
                fts.case_sensitivity,
                fts.finder.as_ref(),
            )? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    for field in &fts.fields {
        if eval_bound_fts_field(field, fts.case_sensitivity, segments, ctx, None)? {
            return Ok(true);
        }
    }
    Ok(false)
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
