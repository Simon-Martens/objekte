//! Stored-JSON filters evaluated against Sonic-parsed source values.
mod fts;
mod metadata_plan;
mod parser;
mod pattern;
mod prefilter;
mod relation;
mod temporal;

use crate::{
    Error, Metrics, Record, RecordStatus, Result, TextFormat, config::fingerprint, source::Stamp,
    text::render_text,
};
pub use fts::FtsOptions;
pub(crate) use fts::{FtsPlan, declared_format};
pub(crate) use metadata_plan::{MetadataDecision, MetadataIdHint, MetadataPathHint};
pub(crate) use relation::RelationContext;
pub(crate) fn like_pattern(pattern: &str, ignore_case: bool) -> pattern::Like {
    pattern::Like::new(pattern, false, ignore_case)
}
use num_bigint::BigInt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use smallvec::SmallVec;
use sonic_rs::{Value as SonicValue, ValueRef};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};


#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum JsonPathStep {
    Key(String),
    Index(usize),
    Each,
}


pub type JsonPath = Vec<JsonPathStep>;

pub(crate) fn format_path(path: &JsonPath) -> String {
    let mut formatted = String::new();
    for step in path {
        match step {
            JsonPathStep::Key(key) => {
                let simple = key
                    .chars()
                    .next()
                    .is_some_and(|first| first == '_' || first.is_alphabetic())
                    && key.chars().all(|ch| ch == '_' || ch.is_alphanumeric());
                if simple {
                    if !formatted.is_empty() {
                        formatted.push('.');
                    }
                    formatted.push_str(key);
                } else {
                    formatted.push('[');
                    formatted.push_str(&serde_json::to_string(key).expect("string serializes"));
                    formatted.push(']');
                }
            }
            JsonPathStep::Index(index) => formatted.push_str(&format!("[{index}]")),
            JsonPathStep::Each => formatted.push_str("[*]"),
        }
    }
    formatted
}


#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Contains,
    NotContains,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
enum Modifier {
    Length,
    Each,
    Lower,
    Fts,
    /// `:fts` on a declared full-text field: its rendered, normalized text,
    /// with candidates from the field's rendered postings.
    Rendered(TextFormat),
    /// `:fts` elsewhere: the value as plain normalized text, verified per
    /// record. No full-text postings cover it reliably (undeclared typed
    /// fields and `exclude_fts` subtrees are not projected).
    Normalized,
}


// INFO: JSON path and optional modifier used as a filter operand.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct FieldOperand {
    path: JsonPath,
    modifier: Option<Modifier>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata: Option<MetadataField>,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) enum MetadataField {
    Id,
    Filename,
    Path,
    Size,
    Modified,
    Created,
    Status,
}


impl MetadataField {
    /// Resolve a system field name without its `@` prefix.
    pub(crate) fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "id" => Self::Id,
            "filename" => Self::Filename,
            "path" => Self::Path,
            "size" => Self::Size,
            "modified" => Self::Modified,
            "created" => Self::Created,
            "status" => Self::Status,
            _ => return None,
        })
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Id => "id",
            Self::Filename => "filename",
            Self::Path => "path",
            Self::Size => "size",
            Self::Modified => "modified",
            Self::Created => "created",
            Self::Status => "status",
        }
    }
}


#[derive(Clone, Copy)]
pub(crate) struct FileMetadata<'a> {
    pub id: &'a str,
    pub path: &'a str,
    pub size: u64,
    pub modified: i128,
    pub created: Option<i128>,
    pub status: RecordStatus,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
enum Operand {
    Field(FieldOperand),
    Literal(Value),
    SizeBytes(u64),
    Temporal(temporal::Period),
    /// A string compared with a `:fts` field, matched by full-text options.
    FullText(fts::FtsPattern),
}


/// A selected source value or a scalar computed by a field modifier.
enum SelectedValue<'a> {
    Borrowed(&'a Value),
    Owned(Value),
    Time(i128),
    Period(temporal::Period),
}


impl SelectedValue<'_> {
    fn value(&self) -> Option<&Value> {
        match self {
            Self::Borrowed(value) => Some(value),
            Self::Owned(value) => Some(value),
            Self::Time(_) | Self::Period(_) => None,
        }
    }
}


type SelectedValues<'a> = SmallVec<[SelectedValue<'a>; 1]>;


impl Operand {
    fn each(&self) -> bool {
        matches!(
            self,
            Self::Field(FieldOperand {
                modifier: Some(Modifier::Each),
                ..
            })
        )
    }

    fn select<'a>(
        &'a self,
        doc: &'a Value,
        metadata: Option<&FileMetadata<'_>>,
        relations: Relations<'a>,
    ) -> SelectedValues<'a> {
        match self {
            Self::Literal(value) => SmallVec::from_buf([SelectedValue::Borrowed(value)]),
            Self::SizeBytes(bytes) => {
                SmallVec::from_buf([SelectedValue::Owned(Value::from(*bytes))])
            }
            Self::Temporal(period) => SmallVec::from_buf([SelectedValue::Period(*period)]),
            Self::FullText(pattern) => SmallVec::from_buf([SelectedValue::Owned(Value::String(
                pattern.pattern().to_owned(),
            ))]),
            Self::Field(f) => f.select(doc, metadata, relations),
        }
    }
}


impl FieldOperand {
    /// Whether this is a `:fts` field, bound or not.
    fn full_text(&self) -> bool {
        matches!(
            self.modifier,
            Some(Modifier::Fts | Modifier::Rendered(_) | Modifier::Normalized)
        )
    }

    /// The format a `:fts` field's full-text version is read in.
    fn text_format(&self) -> TextFormat {
        match self.modifier {
            Some(Modifier::Rendered(format)) => format,
            _ => TextFormat::Plain,
        }
    }

    /// The values at this path as stored, without a modifier.
    fn select_stored<'a>(&self, doc: &'a Value, relations: Relations<'a>) -> SelectedValues<'a> {
        let mut selected = SelectedValues::new();
        self.visit(doc, relations, &mut |value| {
            selected.push(SelectedValue::Borrowed(value));
            true
        });
        selected
    }

    fn select<'a>(
        &self,
        doc: &'a Value,
        metadata: Option<&FileMetadata<'_>>,
        relations: Relations<'a>,
    ) -> SelectedValues<'a> {
        if let Some(field) = self.metadata {
            let Some(metadata) = metadata else {
                return SelectedValues::new();
            };
            let value = match field {
                MetadataField::Id => SelectedValue::Owned(Value::String(metadata.id.into())),
                MetadataField::Filename => SelectedValue::Owned(Value::String(
                    metadata.path.rsplit('/').next().unwrap_or("").into(),
                )),
                MetadataField::Path => SelectedValue::Owned(Value::String(metadata.path.into())),
                MetadataField::Size => SelectedValue::Owned(Value::from(metadata.size)),
                MetadataField::Modified => SelectedValue::Time(metadata.modified),
                MetadataField::Created => metadata
                    .created
                    .map_or(SelectedValue::Owned(Value::Null), SelectedValue::Time),
                MetadataField::Status => {
                    SelectedValue::Owned(Value::String(metadata.status.as_str().into()))
                }
            };
            return SmallVec::from_buf([value]);
        }
        let mut selected = SelectedValues::new();
        self.visit(doc, relations, &mut |value| {
            if let Some(value) = self.modified(value) {
                selected.push(value);
            }
            true
        });
        selected
    }

    /// Visit raw values at this path, following bound relations.
    fn visit<'a>(
        &self,
        doc: &'a Value,
        relations: Relations<'a>,
        emit: &mut impl FnMut(&'a Value) -> bool,
    ) {
        let flatten = self.modifier != Some(Modifier::Length);
        if let Some(found) = relations
            .and_then(|(collection, context)| context.values(collection, doc, &self.path, flatten))
        {
            for value in found {
                if !emit(value) {
                    return;
                }
            }
        } else {
            visit_values(doc, &self.path, flatten, emit);
        }
    }

    fn modified<'a>(&self, value: &'a Value) -> Option<SelectedValue<'a>> {
        match self.modifier {
            Some(Modifier::Length) => value
                .as_array()
                .map(|array| SelectedValue::Owned(Value::from(array.len()))),
            Some(Modifier::Lower) => value
                .as_str()
                .map(|text| SelectedValue::Owned(Value::String(text.to_ascii_lowercase()))),
            // `:fts` reads a string's full-text version; other values are
            // used as stored. An unbound `:fts` reads normalized plain text.
            Some(modifier @ (Modifier::Rendered(_) | Modifier::Normalized | Modifier::Fts)) => {
                let format = match modifier {
                    Modifier::Rendered(format) => format,
                    _ => TextFormat::Plain,
                };
                match value.as_str() {
                    Some(text) => render_text(text, format)
                        .map(|text| SelectedValue::Owned(Value::String(text))),
                    None => Some(SelectedValue::Borrowed(value)),
                }
            }
            _ => Some(SelectedValue::Borrowed(value)),
        }
    }

    fn more_than_one<'a>(&self, doc: &'a Value, relations: Relations<'a>) -> bool {
        if self.metadata.is_some() {
            return false;
        }
        let mut count = 0;
        self.visit(doc, relations, &mut |value| {
            let eligible = match self.modifier {
                Some(Modifier::Length) => value.is_array(),
                Some(Modifier::Lower) => value.is_string(),
                Some(Modifier::Rendered(format)) => value
                    .as_str()
                    .is_none_or(|text| render_text(text, format).is_some()),
                _ => true,
            };
            if eligible {
                count += 1;
            }
            count < 2
        });
        count > 1
    }
}


/// A bound relation context and the collection whose records it evaluates.
type Relations<'a> = Option<(&'a str, &'a RelationContext)>;


#[derive(Debug, Clone, Serialize)]
enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Exists(JsonPath),
    Missing(JsonPath),
    Compare {
        left: Operand,
        op: Op,
        any: bool,
        right: Operand,
        ignore_case: bool,
    },
    /// A comparison or presence test that follows a schema relation. Index
    /// planners treat it as unconstrained; evaluation reads linked records.
    Relation(Box<Expr>),
}


/// A necessary typed candidate expression. Unsupported AND children can be
/// omitted; an OR is useful only when every child has a safe candidate set.
pub(crate) enum TypedPlan {
    Term(JsonPath, IndexOp, Value),
    AnyTerm(JsonPath, IndexOp, Value),
    Present(JsonPath),
    Missing(JsonPath),
    Prefix(JsonPath, String),
    Like(JsonPath, String, bool),
    And(Vec<Self>),
    Or(Vec<Self>),
}


/// Predicates whose complete match set an in-memory scalar index can decide.
pub(crate) enum ExactPlan {
    Present(JsonPath),
    Missing(JsonPath),
    Term(JsonPath, IndexOp, Value),
    AnyTerm(JsonPath, IndexOp, Value),
    Prefix(JsonPath, String),
    And(Vec<Self>),
    Or(Vec<Self>),
}


/// Comparisons supported by JSON-query scalar postings.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}


fn indexed_term(
    left: &Operand,
    op: Op,
    right: &Operand,
    _any: bool,
    ignore_case: bool,
) -> Option<(JsonPath, IndexOp, Value)> {
    if ignore_case {
        return None;
    }
    let (field, value, reversed) = match (left, right) {
        (Operand::Field(field), Operand::Literal(value)) => (field, value, false),
        (Operand::Literal(value), Operand::Field(field)) => (field, value, true),
        _ => return None,
    };
    if field.modifier.is_some()
        || field.metadata.is_some()
        || !(value.is_number() || value.is_boolean() || value.is_string() || value.is_null())
    {
        return None;
    }
    let operator = match (op, reversed) {
        (Op::Eq, _) => IndexOp::Eq,
        (Op::Ne, _) => IndexOp::Ne,
        (Op::Lt, false) | (Op::Gt, true) => IndexOp::Lt,
        (Op::Le, false) | (Op::Ge, true) => IndexOp::Le,
        (Op::Gt, false) | (Op::Lt, true) => IndexOp::Gt,
        (Op::Ge, false) | (Op::Le, true) => IndexOp::Ge,
        _ => return None,
    };
    Some((field.path.clone(), operator, value.clone()))
}


// INFO: Compiled filter expression, referenced paths, and stable hash.
/// A compiled, bound filter reusable across documents and collections.
#[derive(Debug, Clone)]
pub struct JsonFilter {
    expr: Expr,
    paths: Vec<JsonPath>,
    scalar_paths: Vec<FieldOperand>,
    metadata_fields: Vec<String>,
    hash: String,
    /// The collection and linked records bound for relation paths.
    relations: Option<(String, std::sync::Arc<RelationContext>)>,
}


impl JsonFilter {
    /// Stable, schema-bound expression bytes for verified-result identity.
    /// Only error-safe callers may use commutative Boolean normalization.
    pub(crate) fn cache_expression(&self) -> Vec<u8> {
        fn encode(expr: &Expr) -> Vec<u8> {
            fn children<'a>(expr: &'a Expr, and: bool, out: &mut Vec<&'a Expr>) {
                match (and, expr) {
                    (true, Expr::And(parts)) | (false, Expr::Or(parts)) => {
                        for part in parts {
                            children(part, and, out);
                        }
                    }
                    _ => out.push(expr),
                }
            }
            let kind = match expr {
                Expr::And(_) => Some(0_u8),
                Expr::Or(_) => Some(1_u8),
                _ => None,
            };
            let Some(kind) = kind else {
                return serde_json::to_vec(expr).expect("filter expression serializes");
            };
            let mut parts = Vec::new();
            children(expr, kind == 0, &mut parts);
            let mut encoded = parts.into_iter().map(encode).collect::<Vec<_>>();
            encoded.sort();
            let mut result = vec![kind];
            for part in encoded {
                result.extend_from_slice(&(part.len() as u64).to_le_bytes());
                result.extend_from_slice(&part);
            }
            result
        }
        encode(&self.expr)
    }

    /// Top-level conjuncts retain the already bound field semantics.
    pub(crate) fn cache_conjuncts(&self) -> Vec<Self> {
        fn append(expr: &Expr, out: &mut Vec<Expr>) {
            if let Expr::And(parts) = expr {
                for part in parts {
                    append(part, out);
                }
            } else {
                out.push(expr.clone());
            }
        }
        let mut parts = Vec::new();
        append(&self.expr, &mut parts);
        parts
            .into_iter()
            .map(|expr| Self {
                expr,
                paths: Vec::new(),
                scalar_paths: Vec::new(),
                metadata_fields: Vec::new(),
                hash: String::new(),
                relations: self.relations.clone(),
            })
            .collect()
    }

    pub(crate) fn cache_disjuncts(&self) -> Vec<Self> {
        fn append(expr: &Expr, out: &mut Vec<Expr>) {
            if let Expr::Or(parts) = expr {
                for part in parts {
                    append(part, out);
                }
            } else {
                out.push(expr.clone());
            }
        }
        let mut parts = Vec::new();
        append(&self.expr, &mut parts);
        parts
            .into_iter()
            .map(|expr| Self {
                expr,
                paths: Vec::new(),
                scalar_paths: Vec::new(),
                metadata_fields: Vec::new(),
                hash: String::new(),
                relations: self.relations.clone(),
            })
            .collect()
    }

    pub(crate) fn cache_subset(&self, indices: &[usize]) -> Self {
        let conjuncts = self.cache_conjuncts();
        let expr = Expr::And(
            indices
                .iter()
                .map(|&index| conjuncts[index].expr.clone())
                .collect(),
        );
        Self {
            expr,
            paths: Vec::new(),
            scalar_paths: Vec::new(),
            metadata_fields: Vec::new(),
            hash: String::new(),
            relations: self.relations.clone(),
        }
    }

    /// A necessary regex for finding candidates in raw JSON files.
    /// Matching files still require full JSON parsing and filter evaluation.
    pub fn source_candidate_pattern(&self) -> Result<Option<String>> {
        self.source_prefilter()
            .map(|pattern| pattern.map(|(_, text)| text))
    }

    pub fn compile(text: &str, parameters: &BTreeMap<String, Value>) -> Result<Self> {
        Self::compile_with_fts(text, parameters, FtsOptions::default())
    }

    /// Compile with the options `:fts` comparisons match by: the same as
    /// a full-text search's mode and case.
    pub fn compile_with_fts(
        text: &str,
        parameters: &BTreeMap<String, Value>,
        fts: FtsOptions,
    ) -> Result<Self> {
        let mut expr = parser::parse(text, parameters)?;
        fts::apply_options(&mut expr, fts)?;
        Self::compile_expr(expr)
    }

    /// Compile with `@now` macros evaluated at `now` (Unix nanoseconds).
    #[cfg(test)]
    pub(crate) fn compile_at(
        text: &str,
        parameters: &BTreeMap<String, Value>,
        now: i128,
    ) -> Result<Self> {
        let mut expr = parser::parse_at(text, parameters, now)?;
        fts::apply_options(&mut expr, FtsOptions::default())?;
        Self::compile_expr(expr)
    }

    fn compile_expr(expr: Expr) -> Result<Self> {
        let mut paths = BTreeSet::new();
        let mut scalar_paths = BTreeSet::new();
        let mut metadata_fields = BTreeSet::new();
        fn visit(
            e: &Expr,
            paths: &mut BTreeSet<JsonPath>,
            scalar: &mut BTreeSet<FieldOperand>,
            metadata: &mut BTreeSet<String>,
        ) {
            match e {
                Expr::And(es) | Expr::Or(es) => {
                    for e in es {
                        visit(e, paths, scalar, metadata);
                    }
                }
                Expr::Relation(e) => visit(e, paths, scalar, metadata),
                Expr::Exists(p) | Expr::Missing(p) => {
                    paths.insert(p.clone());
                }
                Expr::Compare {
                    left, right, any, ..
                } => {
                    for operand in [left, right] {
                        if let Operand::Field(field) = operand {
                            if let Some(name) = field.metadata {
                                metadata.insert(format!("@{}", name.as_str()));
                            } else {
                                paths.insert(field.path.clone());
                                if !any && !operand.each() {
                                    scalar.insert(field.clone());
                                }
                            }
                        }
                    }
                }
            }
        }
        visit(&expr, &mut paths, &mut scalar_paths, &mut metadata_fields);
        let hash = fingerprint("json-filter/v3", &expr);
        Ok(Self {
            expr,
            paths: paths.into_iter().collect(),
            scalar_paths: scalar_paths.into_iter().collect(),
            metadata_fields: metadata_fields.into_iter().collect(),
            hash,
            relations: None,
        })
    }

    pub fn paths(&self) -> &[JsonPath] {
        &self.paths
    }

    /// Top-level members that every JSON path starts from, or `None` when a
    /// path begins at the root value itself.
    pub(crate) fn top_level_keys(&self) -> Option<Vec<String>> {
        let mut keys = Vec::new();
        for path in &self.paths {
            let Some(JsonPathStep::Key(key)) = path.first() else {
                return None;
            };
            if !keys.contains(key) {
                keys.push(key.clone());
            }
        }
        Some(keys)
    }

    pub fn metadata_fields(&self) -> &[String] {
        &self.metadata_fields
    }

    pub(crate) fn uses_status(&self) -> bool {
        self.metadata_fields.iter().any(|field| field == "@status")
    }

    pub(crate) fn has_metadata(&self) -> bool {
        !self.metadata_fields.is_empty()
    }

    pub(crate) fn uses_source_path(&self) -> bool {
        self.metadata_fields
            .iter()
            .any(|field| field == "@path" || field == "@filename")
    }

    pub(crate) fn matches_source(
        &self,
        value: &Value,
        id: &str,
        path: &str,
        stamp: &Stamp,
        created: Option<i128>,
        status: RecordStatus,
    ) -> Result<bool> {
        if !self.has_metadata() {
            return if status == RecordStatus::IncorrectJson {
                Ok(false)
            } else {
                self.matches(value)
            };
        }
        let normalized_path = self.uses_source_path().then(|| path.replace('\\', "/"));
        let metadata = FileMetadata {
            id,
            path: normalized_path.as_deref().unwrap_or(path),
            size: stamp.size,
            modified: stamp.modified.parse().expect("validated source timestamp"),
            created,
            status,
        };
        self.matches_with_metadata(value, Some(&metadata))
    }

    /// Referenced paths in first-use order, including both comparison operands.
    pub fn paths_in_query_order(&self) -> Vec<JsonPath> {
        fn visit(expr: &Expr, seen: &mut BTreeSet<JsonPath>, ordered: &mut Vec<JsonPath>) {
            let mut push = |path: &JsonPath| {
                if seen.insert(path.clone()) {
                    ordered.push(path.clone());
                }
            };
            match expr {
                Expr::And(parts) | Expr::Or(parts) => {
                    for part in parts {
                        visit(part, seen, ordered);
                    }
                }
                Expr::Relation(part) => visit(part, seen, ordered),
                Expr::Exists(path) | Expr::Missing(path) => push(path),
                Expr::Compare { left, right, .. } => {
                    for operand in [left, right] {
                        if let Operand::Field(field) = operand
                            && field.metadata.is_none()
                        {
                            push(&field.path);
                        }
                    }
                }
            }
        }

        let mut seen = BTreeSet::new();
        let mut ordered = Vec::new();
        visit(&self.expr, &mut seen, &mut ordered);
        ordered
    }

    /// Parse a standalone path using the JSON filter path syntax.
    pub fn parse_path(text: &str) -> Result<JsonPath> {
        parser::parse_path(text)
    }

    pub fn hash(&self) -> &str {
        &self.hash
    }

    pub(crate) fn needs_cardinality_check_in(
        &self,
        indexed_safe: impl Fn(&JsonPath) -> bool,
    ) -> bool {
        self.scalar_paths.iter().any(|path| {
            // Relation paths are checked against their linked records.
            if path.modifier == Some(Modifier::Length)
                || indexed_safe(&path.path)
                || self.is_relation_path(&path.path)
            {
                return false;
            }
            // The picker separately records every top-level array field, so
            // a direct key is scalar-safe when that array-risk set is empty.
            !matches!(path.path.as_slice(), [JsonPathStep::Key(_)])
        })
    }

    pub(crate) fn top_level_scalar_fields(&self) -> Vec<String> {
        self.scalar_paths
            .iter()
            .filter(|path| path.modifier != Some(Modifier::Length))
            .filter_map(|path| match path.path.as_slice() {
                [JsonPathStep::Key(name)] => Some(name.clone()),
                _ => None,
            })
            .collect()
    }

    pub(crate) fn references_indexed_field(&self, indexed: &[String]) -> bool {
        fn operand_indexed(operand: &Operand, indexed: &[String]) -> bool {
            matches!(operand, Operand::Field(FieldOperand {
                path,
                metadata: None,
                ..
            }) if matches!(path.first(), Some(JsonPathStep::Key(name)) if indexed.binary_search(name).is_ok()))
        }

        fn references(expr: &Expr, indexed: &[String]) -> bool {
            match expr {
                Expr::And(parts) | Expr::Or(parts) => {
                    parts.iter().any(|part| references(part, indexed))
                }
                // Relation operands use the linked collection, not this index.
                Expr::Relation(_) => false,
                Expr::Exists(path) | Expr::Missing(path) => {
                    matches!(path.first(), Some(JsonPathStep::Key(name)) if indexed.binary_search(name).is_ok())
                }
                Expr::Compare { left, right, .. } => {
                    operand_indexed(left, indexed) || operand_indexed(right, indexed)
                }
            }
        }
        references(&self.expr, indexed)
    }

    /// Top-level keys required by every possible match, including OR branches.
    pub(crate) fn required_top_level_fields(&self) -> Vec<String> {
        fn first_key(path: &JsonPath) -> Option<String> {
            match path.first() {
                Some(JsonPathStep::Key(key)) => Some(key.clone()),
                _ => None,
            }
        }

        fn required(expr: &Expr) -> BTreeSet<String> {
            match expr {
                Expr::And(parts) => parts.iter().flat_map(required).collect(),
                Expr::Relation(_) => BTreeSet::new(),
                Expr::Or(parts) => {
                    let mut parts = parts.iter().map(required);
                    let Some(mut common) = parts.next() else {
                        return BTreeSet::new();
                    };
                    for part in parts {
                        common.retain(|field| part.contains(field));
                    }
                    common
                }
                Expr::Exists(path) => first_key(path).into_iter().collect(),
                Expr::Missing(_) => BTreeSet::new(),
                Expr::Compare { left, right, .. } => [left, right]
                    .into_iter()
                    .filter_map(|operand| match operand {
                        Operand::Field(field) if field.metadata.is_none() => first_key(&field.path),
                        _ => None,
                    })
                    .collect(),
            }
        }

        required(&self.expr).into_iter().collect()
    }

    /// Necessary typed candidates for AND/OR filters. Exact JSON evaluation
    /// still decides whether a candidate matches.
    pub(crate) fn typed_plan(&self) -> Option<TypedPlan> {
        fn plan(expr: &Expr) -> Option<TypedPlan> {
            match expr {
                Expr::And(parts) => {
                    let children = parts.iter().filter_map(plan).collect::<Vec<_>>();
                    (!children.is_empty()).then_some(TypedPlan::And(children))
                }
                Expr::Relation(_) => None,
                Expr::Or(parts) => {
                    let children = parts.iter().map(plan).collect::<Option<Vec<_>>>()?;
                    (!children.is_empty()).then_some(TypedPlan::Or(children))
                }
                Expr::Exists(path) => Some(TypedPlan::Present(path.clone())),
                Expr::Missing(path) => Some(TypedPlan::Missing(path.clone())),
                Expr::Compare {
                    left: Operand::Field(field),
                    op: Op::Contains,
                    right: Operand::Literal(Value::String(pattern)),
                    any: false,
                    ignore_case,
                } if field.modifier.is_none() && field.metadata.is_none() => {
                    if !ignore_case && let Some(prefix) = pattern::Like::anchored_prefix(pattern) {
                        return Some(TypedPlan::Prefix(field.path.clone(), prefix));
                    }
                    Some(TypedPlan::Like(
                        field.path.clone(),
                        pattern.clone(),
                        *ignore_case,
                    ))
                }
                Expr::Compare {
                    left,
                    op,
                    right,
                    any,
                    ignore_case,
                } => indexed_term(left, *op, right, *any, *ignore_case).map(
                    |(path, operator, value)| {
                        if *any {
                            TypedPlan::AnyTerm(path, operator, value)
                        } else {
                            TypedPlan::Term(path, operator, value)
                        }
                    },
                ),
            }
        }
        plan(&self.expr)
    }

    /// Use only predicates with exact posting semantics.
    pub(crate) fn exact_index_plan(&self) -> Option<ExactPlan> {
        fn plan(expr: &Expr) -> Option<ExactPlan> {
            match expr {
                Expr::And(parts) => Some(ExactPlan::And(
                    parts.iter().map(plan).collect::<Option<Vec<_>>>()?,
                )),
                Expr::Relation(_) => None,
                Expr::Or(parts) => Some(ExactPlan::Or(
                    parts.iter().map(plan).collect::<Option<Vec<_>>>()?,
                )),
                Expr::Exists(path) => Some(ExactPlan::Present(path.clone())),
                Expr::Missing(path) => Some(ExactPlan::Missing(path.clone())),
                Expr::Compare {
                    left: Operand::Field(field),
                    op: Op::Contains,
                    any: false,
                    right: Operand::Literal(Value::String(pattern)),
                    ignore_case: false,
                } if field.modifier.is_none() && field.metadata.is_none() => {
                    pattern::Like::anchored_prefix(pattern)
                        .map(|prefix| ExactPlan::Prefix(field.path.clone(), prefix))
                }
                Expr::Compare {
                    left,
                    op,
                    right,
                    any,
                    ignore_case,
                } => indexed_term(left, *op, right, *any, *ignore_case).map(
                    |(path, operator, value)| {
                        if *any {
                            ExactPlan::AnyTerm(path, operator, value)
                        } else {
                            ExactPlan::Term(path, operator, value)
                        }
                    },
                ),
            }
        }

        plan(&self.expr)
    }

    /// Evaluate an already parsed JSON value with the same semantics as cached queries.
    pub fn matches(&self, value: &Value) -> Result<bool> {
        if !self.metadata_fields.is_empty() {
            return Err(Error::Request(
                "metadata operands require a source record".into(),
            ));
        }
        self.matches_with_metadata(value, None)
    }

    pub(crate) fn matches_with_metadata(
        &self,
        value: &Value,
        metadata: Option<&FileMetadata<'_>>,
    ) -> Result<bool> {
        self.check_cardinality(value)?;
        let relations = self
            .relations
            .as_ref()
            .map(|(collection, context)| (collection.as_str(), context.as_ref()));
        Ok(evaluate(&self.expr, value, metadata, relations))
    }

    pub(crate) fn check_cardinality(&self, value: &Value) -> Result<()> {
        let relations = self
            .relations
            .as_ref()
            .map(|(collection, context)| (collection.as_str(), context.as_ref()));
        check_paths(value, &self.scalar_paths, relations)
    }
}


// INFO: Filter text, bindings, scope, and pagination requested by a caller.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct JsonQuery {
    pub filter: String,
    pub parameters: BTreeMap<String, Value>,
    /// None means every currently registered collection; Some([]) is an empty scope.
    pub collections: Option<Vec<String>>,
    pub limit: usize,
    pub offset: usize,
    pub count: bool,
    /// Sort terms applied within each collection. Empty means ID order.
    pub sort: Vec<crate::Sort>,
    /// Select a query backend. `Auto` uses the in-memory bigram index.
    pub backend: JsonQueryBackend,
    /// How `:fts` comparisons in the filter match.
    pub fts: FtsOptions,
}


#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonQueryBackend {
    #[default]
    Auto,
    Direct,
    Grep,
    Bigram,
}


impl Default for JsonQuery {
    fn default() -> Self {
        Self {
            filter: String::new(),
            parameters: BTreeMap::new(),
            collections: None,
            limit: 50,
            offset: 0,
            count: false,
            sort: Vec::new(),
            backend: JsonQueryBackend::Auto,
            fts: FtsOptions::default(),
        }
    }
}


// INFO: Collections and paths observed by a query.
#[derive(Debug, Clone, Serialize)]
pub struct JsonQueryDependencies {
    /// All scoped collections, including nonmatching records and future additions.
    pub collections: Vec<String>,
    pub paths: Vec<JsonPath>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub metadata_fields: Vec<String>,
}


// INFO: Matched records, count, dependencies, metrics, and query plan.
#[derive(Debug, Clone, Serialize)]
pub struct JsonQueryResult {
    pub records: Vec<Record>,
    pub total: Option<usize>,
    pub truncated: bool,
    /// Wall-clock duration of this search in milliseconds.
    pub elapsed_ms: f64,
    pub dependencies: JsonQueryDependencies,
    pub filter_hash: String,
    pub metrics: Metrics,
    pub query_plan: Vec<String>,
}


// INFO: Final metadata returned after all streamed records are verified.
#[derive(Debug, Clone, Serialize)]
pub struct JsonQueryStreamSummary {
    pub returned: usize,
    pub total: Option<usize>,
    pub truncated: bool,
    /// Wall-clock duration through the final streamed record in milliseconds.
    pub elapsed_ms: f64,
    pub dependencies: JsonQueryDependencies,
    pub filter_hash: String,
    pub metrics: Metrics,
    pub query_plan: Vec<String>,
}


pub(crate) fn values<'a>(doc: &'a Value, path: &[JsonPathStep], flatten: bool) -> Vec<&'a Value> {
    let mut out = Vec::new();
    visit_values(doc, path, flatten, &mut |value| {
        out.push(value);
        true
    });
    out
}

/// Sonic traversal with implicit array descent and last-key-wins behavior.
pub(crate) fn sonic_values<'a>(
    document: &'a SonicValue,
    path: &[JsonPathStep],
    flatten: bool,
) -> Vec<&'a SonicValue> {
    fn visit<'a>(
        document: &'a SonicValue,
        path: &[JsonPathStep],
        flatten: bool,
        out: &mut Vec<&'a SonicValue>,
    ) {
        match path.split_first() {
            None => {
                if flatten && let ValueRef::Array(items) = document.as_ref() {
                    for item in items {
                        visit(item, path, flatten, out);
                    }
                } else {
                    out.push(document);
                }
            }
            Some((JsonPathStep::Key(key), rest)) => match document.as_ref() {
                ValueRef::Object(fields) => {
                    let mut last = None;
                    for (name, value) in fields {
                        if name == key {
                            last = Some(value);
                        }
                    }
                    if let Some(value) = last {
                        visit(value, rest, flatten, out);
                    }
                }
                ValueRef::Array(items) => {
                    for item in items {
                        visit(item, path, flatten, out);
                    }
                }
                _ => {}
            },
            Some((JsonPathStep::Index(index), rest)) => {
                if let ValueRef::Array(items) = document.as_ref()
                    && let Some(value) = items.get(*index)
                {
                    visit(value, rest, flatten, out);
                }
            }
            Some((JsonPathStep::Each, rest)) => {
                if let ValueRef::Array(items) = document.as_ref() {
                    for item in items {
                        visit(item, rest, flatten, out);
                    }
                }
            }
        }
    }

    let mut selected = Vec::new();
    visit(document, path, flatten, &mut selected);
    selected
}

/// Visit selected values in path order. Returning false stops traversal early.
fn visit_values<'a>(
    doc: &'a Value,
    path: &[JsonPathStep],
    flatten: bool,
    emit: &mut impl FnMut(&'a Value) -> bool,
) -> bool {
    fn visit<'a>(
        doc: &'a Value,
        path: &[JsonPathStep],
        flatten: bool,
        emit: &mut impl FnMut(&'a Value) -> bool,
    ) -> bool {
        match path.split_first() {
            None => {
                if flatten && let Value::Array(items) = doc {
                    for item in items {
                        if !visit(item, path, flatten, emit) {
                            return false;
                        }
                    }
                } else {
                    return emit(doc);
                }
            }
            Some((JsonPathStep::Key(key), rest)) => match doc {
                Value::Object(map) => {
                    if let Some(v) = map.get(key) {
                        return visit(v, rest, flatten, emit);
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        if !visit(item, path, flatten, emit) {
                            return false;
                        }
                    }
                }
                _ => {}
            },
            Some((JsonPathStep::Index(i), rest)) => {
                if let Some(v) = doc.as_array().and_then(|a| a.get(*i)) {
                    return visit(v, rest, flatten, emit);
                }
            }
            Some((JsonPathStep::Each, rest)) => {
                if let Value::Array(items) = doc {
                    for item in items {
                        if !visit(item, rest, flatten, emit) {
                            return false;
                        }
                    }
                }
            }
        }
        true
    }
    visit(doc, path, flatten, emit)
}

fn evaluate<'a>(
    e: &'a Expr,
    doc: &'a Value,
    metadata: Option<&FileMetadata<'_>>,
    relations: Relations<'a>,
) -> bool {
    match e {
        Expr::And(es) => es.iter().all(|e| evaluate(e, doc, metadata, relations)),
        Expr::Or(es) => es.iter().any(|e| evaluate(e, doc, metadata, relations)),
        Expr::Relation(e) => evaluate(e, doc, metadata, relations),
        Expr::Exists(p) => !reached(doc, p, relations).is_empty(),
        Expr::Missing(p) => reached(doc, p, relations).is_empty(),
        // `:fts` reads string values through the full-text options; other
        // values compare as without `:fts`.
        Expr::Compare {
            left: Operand::Field(field),
            op,
            any,
            right: Operand::FullText(pattern),
            ..
        } => {
            let values = field.select_stored(doc, relations);
            if values.is_empty() {
                return false;
            }
            let format = field.text_format();
            let literal = Value::String(pattern.pattern().to_owned());
            let mut tests = values.iter().map(|value| match value.value() {
                Some(Value::String(text)) => pattern.matches(text, format, *op),
                Some(other) => compare(other, *op, &literal, false, false),
                None => false,
            });
            // A `:fts` field is never `:each`; several values match like `?`.
            let _ = any;
            tests.any(|matched| matched)
        }
        Expr::Compare {
            left,
            op,
            any,
            right,
            ignore_case,
        } => {
            let a = left.select(doc, metadata, relations);
            let b = right.select(doc, metadata, relations);
            if a.is_empty() || b.is_empty() {
                return false;
            }
            let mut pairs = a.iter().flat_map(|a| b.iter().map(move |b| (a, b)));
            let test = |(a, b): (&SelectedValue<'_>, &SelectedValue<'_>)| {
                compare_selected(a, *op, b, matches!(right, Operand::Field(_)), *ignore_case)
            };
            if !any && (left.each() || right.each()) {
                pairs.all(test)
            } else {
                pairs.any(test)
            }
        }
    }
}

/// Values at a presence-test path, following bound relations.
fn reached<'a>(doc: &'a Value, path: &JsonPath, relations: Relations<'a>) -> Vec<&'a Value> {
    relations
        .and_then(|(collection, context)| context.values(collection, doc, path, false))
        .unwrap_or_else(|| values(doc, path, false))
}

fn check_paths<'a>(doc: &'a Value, paths: &[FieldOperand], relations: Relations<'a>) -> Result<()> {
    for path in paths {
        if path.more_than_one(doc, relations) {
            return Err(Error::Request(format!(
                "path {} yields multiple values; use a '?' comparison or :each",
                serde_json::to_string(&path.path).unwrap()
            )));
        }
    }
    Ok(())
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        _ => "container",
    }
}

fn compare_selected(
    left: &SelectedValue<'_>,
    op: Op,
    right: &SelectedValue<'_>,
    right_field: bool,
    ignore_case: bool,
) -> bool {
    match (left, right) {
        (SelectedValue::Time(instant), SelectedValue::Period(period)) => {
            temporal::compare(*instant, op, *period)
        }
        (SelectedValue::Period(period), SelectedValue::Time(instant)) => {
            let reversed = match op {
                Op::Lt => Op::Gt,
                Op::Le => Op::Ge,
                Op::Gt => Op::Lt,
                Op::Ge => Op::Le,
                other => other,
            };
            temporal::compare(*instant, reversed, *period)
        }
        // A JSON string in date/time literal syntax is the start instant of
        // its own period; other values keep ordinary type-mismatch semantics.
        (value, SelectedValue::Period(period)) if let Some(instant) = value_instant(value) => {
            temporal::compare(instant, op, *period)
        }
        (SelectedValue::Period(period), value) if let Some(instant) = value_instant(value) => {
            let reversed = match op {
                Op::Lt => Op::Gt,
                Op::Le => Op::Ge,
                Op::Gt => Op::Lt,
                Op::Ge => Op::Le,
                other => other,
            };
            temporal::compare(instant, reversed, *period)
        }
        (SelectedValue::Time(a), SelectedValue::Time(b)) => match op {
            Op::Eq => a == b,
            Op::Ne => a != b,
            Op::Lt => a < b,
            Op::Le => a <= b,
            Op::Gt => a > b,
            Op::Ge => a >= b,
            Op::Contains | Op::NotContains => false,
        },
        _ => match (left.value(), right.value()) {
            (Some(a), Some(b)) => compare(a, op, b, right_field, ignore_case),
            (Some(_), None) | (None, Some(_)) => matches!(op, Op::Ne),
            _ => false,
        },
    }
}

fn value_instant(value: &SelectedValue<'_>) -> Option<i128> {
    let text = value.value()?.as_str()?;
    temporal::parse(text).ok().map(|period| period.start)
}

fn compare(left: &Value, op: Op, right: &Value, right_field: bool, ignore_case: bool) -> bool {
    if kind(left) == "container" || kind(right) == "container" {
        return false;
    }
    if matches!(op, Op::Contains | Op::NotContains) {
        return match (left.as_str(), right.as_str()) {
            (Some(a), Some(b)) => {
                pattern::Like::matches_operand(b, right_field, ignore_case, a)
                    == matches!(op, Op::Contains)
            }
            _ => false,
        };
    }
    let ordering = match (left, right) {
        (Value::Number(a), Value::Number(b)) => Some(decimal_cmp(a.as_str(), b.as_str())),
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) if matches!(op, Op::Eq | Op::Ne) => Some(a.cmp(b)),
        (Value::Null, Value::Null) if matches!(op, Op::Eq | Op::Ne) => Some(Ordering::Equal),
        _ => None,
    };
    match op {
        Op::Eq => ordering == Some(Ordering::Equal),
        Op::Ne => ordering != Some(Ordering::Equal),
        Op::Lt => ordering == Some(Ordering::Less),
        Op::Le => matches!(ordering, Some(Ordering::Less | Ordering::Equal)),
        Op::Gt => ordering == Some(Ordering::Greater),
        Op::Ge => matches!(ordering, Some(Ordering::Greater | Ordering::Equal)),
        _ => unreachable!(),
    }
}

#[cfg(test)]
#[path = "mod.test.rs"]
mod tests;


/// A decimal lexeme split without materializing its normalized digits.
struct DecimalParts<'a> {
    negative: bool,
    mantissa: &'a str,
    exponent: &'a str,
    fraction_len: usize,
    leading_zeros: usize,
    significant_len: usize,
}


impl<'a> DecimalParts<'a> {
    fn parse(lexeme: &'a str) -> Self {
        let negative = lexeme.starts_with('-');
        let unsigned = lexeme.strip_prefix('-').unwrap_or(lexeme);
        let (mantissa, exponent) = unsigned.split_once(['e', 'E']).unwrap_or((unsigned, "0"));
        let fraction_len = mantissa
            .split_once('.')
            .map_or(0, |(_, fraction)| fraction.len());
        let leading_zeros = mantissa
            .bytes()
            .filter(|&byte| byte != b'.')
            .take_while(|&byte| byte == b'0')
            .count();
        let significant_len = mantissa.len() - usize::from(mantissa.contains('.')) - leading_zeros;
        Self {
            negative: negative && significant_len != 0,
            mantissa,
            exponent,
            fraction_len,
            leading_zeros,
            significant_len,
        }
    }

    fn digits(&self) -> impl Iterator<Item = u8> + '_ {
        self.mantissa
            .bytes()
            .filter(|&byte| byte != b'.')
            .skip(self.leading_zeros)
    }

    fn magnitude(&self) -> DecimalMagnitude {
        if let (Ok(exponent), Ok(fraction), Ok(digits)) = (
            self.exponent.parse::<i128>(),
            i128::try_from(self.fraction_len),
            i128::try_from(self.significant_len),
        ) && let Some(magnitude) = exponent
            .checked_sub(fraction)
            .and_then(|value| value.checked_add(digits))
        {
            return DecimalMagnitude::Small(magnitude);
        }
        let exponent: BigInt = self.exponent.parse().expect("validated JSON exponent");
        DecimalMagnitude::Big(
            exponent - BigInt::from(self.fraction_len) + BigInt::from(self.significant_len),
        )
    }
}


enum DecimalMagnitude {
    Small(i128),
    Big(BigInt),
}


impl DecimalMagnitude {
    fn cmp(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Small(a), Self::Small(b)) => a.cmp(b),
            (Self::Big(a), Self::Big(b)) => a.cmp(b),
            (Self::Small(a), Self::Big(b)) => BigInt::from(*a).cmp(b),
            (Self::Big(a), Self::Small(b)) => a.cmp(&BigInt::from(*b)),
        }
    }
}


/// Compare validated decimal JSON number lexemes exactly. Ordinary fractional
/// and exponential values use borrowed digits and fixed-width magnitudes.
pub(crate) fn decimal_cmp(a: &str, b: &str) -> Ordering {
    if let (Ok(a), Ok(b)) = (a.parse::<i64>(), b.parse::<i64>()) {
        return a.cmp(&b);
    }
    let a = DecimalParts::parse(a);
    let b = DecimalParts::parse(b);
    if a.significant_len == 0 || b.significant_len == 0 {
        return match (a.significant_len == 0, b.significant_len == 0) {
            (true, true) => Ordering::Equal,
            (true, false) if b.negative => Ordering::Greater,
            (true, false) => Ordering::Less,
            (false, true) if a.negative => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => unreachable!(),
        };
    }
    if a.negative != b.negative {
        return if a.negative {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let order = a.magnitude().cmp(&b.magnitude()).then_with(|| {
        a.digits()
            .chain(std::iter::repeat(b'0'))
            .zip(b.digits().chain(std::iter::repeat(b'0')))
            .take(a.significant_len.max(b.significant_len))
            .find_map(|(a, b)| (a != b).then(|| a.cmp(&b)))
            .unwrap_or(Ordering::Equal)
    });
    if a.negative { order.reverse() } else { order }
}
