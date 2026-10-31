//! Stored-JSON filters evaluated against Sonic-parsed source values.
mod fts;
mod parser;
mod pattern;
mod prefilter;

use crate::{Error, Metrics, Record, Result, TextFormat, config::fingerprint, text::render_text};
pub(crate) use fts::FtsPlan;
use num_bigint::BigInt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
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
    Rendered(TextFormat),
}


// INFO: JSON path and optional modifier used as a filter operand.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub(crate) struct FieldOperand {
    path: JsonPath,
    modifier: Option<Modifier>,
}


#[derive(Debug, Clone, Serialize, Deserialize)]
enum Operand {
    Field(FieldOperand),
    Literal(Value),
}


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

    fn select(&self, doc: &Value) -> Vec<Value> {
        match self {
            Self::Literal(v) => vec![v.clone()],
            Self::Field(f) => f.select(doc),
        }
    }
}


impl FieldOperand {
    fn select(&self, doc: &Value) -> Vec<Value> {
        let selected = values(doc, &self.path, self.modifier != Some(Modifier::Length));
        selected
            .into_iter()
            .filter_map(|v| match self.modifier {
                Some(Modifier::Length) => v.as_array().map(|a| Value::from(a.len())),
                Some(Modifier::Lower) => v.as_str().map(|s| Value::String(s.to_ascii_lowercase())),
                Some(Modifier::Rendered(format)) => v
                    .as_str()
                    .and_then(|text| render_text(text, format))
                    .map(Value::String),
                _ => Some(v.clone()),
            })
            .collect()
    }
}


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
}


/// A necessary typed candidate expression. Unsupported AND children can be
/// omitted; an OR is useful only when every child has a safe candidate set.
pub(crate) enum TypedPlan {
    Term(JsonPath, IndexOp, Value),
    Present(JsonPath),
    Missing(JsonPath),
    Prefix(JsonPath, String),
    And(Vec<Self>),
    Or(Vec<Self>),
}


/// Predicates whose complete match set an in-memory scalar index can decide.
pub(crate) enum ExactPlan {
    Present(JsonPath),
    Missing(JsonPath),
    Term(JsonPath, IndexOp, Value),
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
    any: bool,
    ignore_case: bool,
) -> Option<(JsonPath, IndexOp, Value)> {
    if any || ignore_case {
        return None;
    }
    let (field, value, reversed) = match (left, right) {
        (Operand::Field(field), Operand::Literal(value)) => (field, value, false),
        (Operand::Literal(value), Operand::Field(field)) => (field, value, true),
        _ => return None,
    };
    if field.modifier.is_some()
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
    hash: String,
}


impl JsonFilter {
    /// A necessary regex for finding candidates in raw JSON files.
    /// Matching files still require full JSON parsing and filter evaluation.
    pub fn source_candidate_pattern(&self) -> Result<Option<String>> {
        self.source_prefilter()
            .map(|pattern| pattern.map(|(_, text)| text))
    }

    pub fn compile(text: &str, parameters: &BTreeMap<String, Value>) -> Result<Self> {
        let expr = parser::parse(text, parameters)?;
        let mut paths = BTreeSet::new();
        let mut scalar_paths = BTreeSet::new();
        fn visit(e: &Expr, paths: &mut BTreeSet<JsonPath>, scalar: &mut BTreeSet<FieldOperand>) {
            match e {
                Expr::And(es) | Expr::Or(es) => {
                    for e in es {
                        visit(e, paths, scalar);
                    }
                }
                Expr::Exists(p) | Expr::Missing(p) => {
                    paths.insert(p.clone());
                }
                Expr::Compare {
                    left, right, any, ..
                } => {
                    for operand in [left, right] {
                        if let Operand::Field(field) = operand {
                            paths.insert(field.path.clone());
                            if !any && !operand.each() {
                                scalar.insert(field.clone());
                            }
                        }
                    }
                }
            }
        }
        visit(&expr, &mut paths, &mut scalar_paths);
        let hash = fingerprint("json-filter/v3", &expr);
        Ok(Self {
            expr,
            paths: paths.into_iter().collect(),
            scalar_paths: scalar_paths.into_iter().collect(),
            hash,
        })
    }

    pub fn paths(&self) -> &[JsonPath] {
        &self.paths
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
            if path.modifier == Some(Modifier::Length) || indexed_safe(&path.path) {
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
                        Operand::Field(field) => first_key(&field.path),
                        Operand::Literal(_) => None,
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
                    ignore_case: false,
                } if field.modifier.is_none() => pattern::Like::anchored_prefix(pattern)
                    .map(|prefix| TypedPlan::Prefix(field.path.clone(), prefix)),
                Expr::Compare {
                    left,
                    op,
                    right,
                    any,
                    ignore_case,
                } => indexed_term(left, *op, right, *any, *ignore_case)
                    .map(|(path, operator, value)| TypedPlan::Term(path, operator, value)),
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
                } if field.modifier.is_none() => pattern::Like::anchored_prefix(pattern)
                    .map(|prefix| ExactPlan::Prefix(field.path.clone(), prefix)),
                Expr::Compare {
                    left,
                    op,
                    right,
                    any,
                    ignore_case,
                } => indexed_term(left, *op, right, *any, *ignore_case)
                    .map(|(path, operator, value)| ExactPlan::Term(path, operator, value)),
            }
        }

        plan(&self.expr)
    }

    /// Evaluate an already parsed JSON value with the same semantics as cached queries.
    pub fn matches(&self, value: &Value) -> Result<bool> {
        self.check_cardinality(value)?;
        fn evaluate(e: &Expr, doc: &Value) -> bool {
            match e {
                Expr::And(es) => es.iter().all(|e| evaluate(e, doc)),
                Expr::Or(es) => es.iter().any(|e| evaluate(e, doc)),
                Expr::Exists(p) => !values(doc, p, false).is_empty(),
                Expr::Missing(p) => values(doc, p, false).is_empty(),
                Expr::Compare {
                    left,
                    op,
                    any,
                    right,
                    ignore_case,
                } => {
                    let a = left.select(doc);
                    let b = right.select(doc);
                    if a.is_empty() || b.is_empty() {
                        return false;
                    }
                    let mut pairs = a.iter().flat_map(|a| b.iter().map(move |b| (a, b)));
                    let test = |(a, b)| {
                        compare(a, *op, b, matches!(right, Operand::Field(_)), *ignore_case)
                    };
                    if !any && (left.each() || right.each()) {
                        pairs.all(test)
                    } else {
                        pairs.any(test)
                    }
                }
            }
        }
        Ok(evaluate(&self.expr, value))
    }

    fn check_cardinality(&self, value: &Value) -> Result<()> {
        check_paths(value, &self.scalar_paths)
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
    /// Select a query backend. `Auto` uses the in-memory bigram index.
    pub backend: JsonQueryBackend,
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
            backend: JsonQueryBackend::Auto,
        }
    }
}


// INFO: Collections and paths observed by a query.
#[derive(Debug, Clone, Serialize)]
pub struct JsonQueryDependencies {
    /// All scoped collections, including nonmatching records and future additions.
    pub collections: Vec<String>,
    pub paths: Vec<JsonPath>,
}


// INFO: Matched records, count, dependencies, metrics, and query plan.
#[derive(Debug, Clone, Serialize)]
pub struct JsonQueryResult {
    pub records: Vec<Record>,
    pub total: Option<usize>,
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
    pub dependencies: JsonQueryDependencies,
    pub filter_hash: String,
    pub metrics: Metrics,
    pub query_plan: Vec<String>,
}


pub(crate) fn values<'a>(doc: &'a Value, path: &[JsonPathStep], flatten: bool) -> Vec<&'a Value> {
    fn visit<'a>(doc: &'a Value, path: &[JsonPathStep], flatten: bool, out: &mut Vec<&'a Value>) {
        match path.split_first() {
            None => {
                if flatten && let Value::Array(items) = doc {
                    for item in items {
                        visit(item, path, flatten, out);
                    }
                } else {
                    out.push(doc);
                }
            }
            Some((JsonPathStep::Key(key), rest)) => match doc {
                Value::Object(map) => {
                    if let Some(v) = map.get(key) {
                        visit(v, rest, flatten, out);
                    }
                }
                Value::Array(items) => {
                    for item in items {
                        visit(item, path, flatten, out);
                    }
                }
                _ => {}
            },
            Some((JsonPathStep::Index(i), rest)) => {
                if let Some(v) = doc.as_array().and_then(|a| a.get(*i)) {
                    visit(v, rest, flatten, out);
                }
            }
            Some((JsonPathStep::Each, rest)) => {
                if let Value::Array(items) = doc {
                    for item in items {
                        visit(item, rest, flatten, out);
                    }
                }
            }
        }
    }
    let mut out = Vec::new();
    visit(doc, path, flatten, &mut out);
    out
}

fn check_paths(doc: &Value, paths: &[FieldOperand]) -> Result<()> {
    for path in paths {
        if path.select(doc).len() > 1 {
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

fn compare(left: &Value, op: Op, right: &Value, right_field: bool, ignore_case: bool) -> bool {
    if kind(left) == "container" || kind(right) == "container" {
        return false;
    }
    if matches!(op, Op::Contains | Op::NotContains) {
        return match (left.as_str(), right.as_str()) {
            (Some(a), Some(b)) => {
                pattern::Like::new(b, right_field, ignore_case).matches(a)
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

/// Compare decimal JSON number lexemes without conversion to floating point or
/// allocating exponent-sized zero strings (including exponents outside i64).
pub(crate) fn decimal_cmp(a: &str, b: &str) -> Ordering {
    // The common integer case needs no allocation. Keep the exact decimal path
    // for fractional/exponential spellings and values outside signed 64 bits.
    if let (Ok(a), Ok(b)) = (a.parse::<i64>(), b.parse::<i64>()) {
        return a.cmp(&b);
    }

    fn parts(s: &str) -> (bool, String, BigInt) {
        let negative = s.starts_with('-');
        let s = s.strip_prefix('-').unwrap_or(s);
        let (mantissa, exp) = s.split_once(['e', 'E']).unwrap_or((s, "0"));
        let fraction = mantissa.split_once('.').map_or(0, |(_, f)| f.len());
        let digits = mantissa.replace('.', "").trim_start_matches('0').to_owned();
        let exp: BigInt = exp.parse().expect("validated JSON exponent");
        let magnitude = exp - BigInt::from(fraction) + BigInt::from(digits.len());
        (negative && !digits.is_empty(), digits, magnitude)
    }
    let (an, a, am) = parts(a);
    let (bn, b, bm) = parts(b);
    if an != bn {
        return if an {
            Ordering::Less
        } else {
            Ordering::Greater
        };
    }
    let order = if a.is_empty() || b.is_empty() {
        (!a.is_empty()).cmp(&(!b.is_empty()))
    } else {
        am.cmp(&bm).then_with(|| {
            a.bytes()
                .chain(std::iter::repeat(b'0'))
                .zip(b.bytes().chain(std::iter::repeat(b'0')))
                .take(a.len().max(b.len()))
                .find_map(|(a, b)| (a != b).then(|| a.cmp(&b)))
                .unwrap_or(Ordering::Equal)
        })
    };
    if an { order.reverse() } else { order }
}
