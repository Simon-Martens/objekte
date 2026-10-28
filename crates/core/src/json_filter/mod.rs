//! Schema-independent filters. SQL composition follows dbx's expression/bindings
//! separation; semantics belong here, not to SQLite's implicit type conversions.
mod parser;
mod sql;

use crate::{Error, Metrics, Record, Result, config::fingerprint};
use num_bigint::BigInt;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

pub(crate) use sql::register;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum JsonPathStep {
    Key(String),
    Index(usize),
    Each,
}
pub type JsonPath = Vec<JsonPathStep>;

#[derive(Debug, Clone, Copy, Serialize)]
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
#[derive(Debug, Clone, Serialize)]
enum Expr {
    And(Vec<Expr>),
    Or(Vec<Expr>),
    Exists(JsonPath),
    Compare {
        path: JsonPath,
        op: Op,
        any: bool,
        value: Value,
    },
}

/// A compiled, bound filter reusable across documents and collections.
#[derive(Debug, Clone)]
pub struct JsonFilter {
    expr: Expr,
    paths: Vec<JsonPath>,
    scalar_paths: Vec<JsonPath>,
    hash: String,
}
impl JsonFilter {
    pub fn compile(text: &str, parameters: &BTreeMap<String, Value>) -> Result<Self> {
        let expr = parser::parse(text, parameters)?;
        let mut paths = BTreeSet::new();
        let mut scalar_paths = BTreeSet::new();
        fn visit(e: &Expr, paths: &mut BTreeSet<JsonPath>, scalar: &mut BTreeSet<JsonPath>) {
            match e {
                Expr::And(es) | Expr::Or(es) => {
                    for e in es {
                        visit(e, paths, scalar);
                    }
                }
                Expr::Exists(p) => {
                    paths.insert(p.clone());
                }
                Expr::Compare { path, any, .. } => {
                    paths.insert(path.clone());
                    if !any {
                        scalar.insert(path.clone());
                    }
                }
            }
        }
        visit(&expr, &mut paths, &mut scalar_paths);
        let hash = fingerprint("json-filter/v1", &(&expr, caseless::UNICODE_VERSION));
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
    pub fn hash(&self) -> &str {
        &self.hash
    }
    /// Evaluate an already parsed JSON value with exactly the SQL execution semantics.
    pub fn matches(&self, value: &Value) -> Result<bool> {
        self.check_cardinality(value)?;
        fn evaluate(e: &Expr, doc: &Value) -> bool {
            match e {
                Expr::And(es) => es.iter().all(|e| evaluate(e, doc)),
                Expr::Or(es) => es.iter().any(|e| evaluate(e, doc)),
                Expr::Exists(p) => !values(doc, p, false).is_empty(),
                Expr::Compare {
                    path, op, value, ..
                } => values(doc, path, true)
                    .iter()
                    .any(|v| compare(v, *op, value)),
            }
        }
        Ok(evaluate(&self.expr, value))
    }
    fn check_cardinality(&self, value: &Value) -> Result<()> {
        check_paths(value, &self.scalar_paths)
    }
}

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
        }
    }
}
#[derive(Debug, Clone, Serialize)]
pub struct JsonQueryDependencies {
    /// All scoped collections, including nonmatching records and future additions.
    pub collections: Vec<String>,
    pub paths: Vec<JsonPath>,
    /// Persistent content generations observed by the SQL query; empty in direct mode.
    pub revisions: BTreeMap<String, i64>,
}
#[derive(Debug, Clone, Serialize)]
pub struct JsonQueryResult {
    pub records: Vec<Record>,
    pub total: Option<usize>,
    pub dependencies: JsonQueryDependencies,
    pub filter_hash: String,
    pub metrics: Metrics,
    pub sql: Option<String>,
    pub bindings: Vec<String>,
    pub query_plan: Vec<String>,
}

fn values<'a>(doc: &'a Value, path: &[JsonPathStep], flatten: bool) -> Vec<&'a Value> {
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
fn check_paths(doc: &Value, paths: &[JsonPath]) -> Result<()> {
    for path in paths {
        if values(doc, path, true).len() > 1 {
            return Err(Error::Request(format!(
                "path {} yields multiple values; use a '?' comparison",
                serde_json::to_string(path).unwrap()
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
fn compare(left: &Value, op: Op, right: &Value) -> bool {
    if kind(left) == "container" {
        return false;
    }
    if matches!(op, Op::Contains | Op::NotContains) {
        return match (left.as_str(), right.as_str()) {
            (Some(a), Some(b)) => contains(a, b) == matches!(op, Op::Contains),
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
fn contains(a: &str, b: &str) -> bool {
    caseless::default_case_fold_str(a).contains(&caseless::default_case_fold_str(b))
}

/// Compare decimal JSON number lexemes without conversion to floating point or
/// allocating exponent-sized zero strings (including exponents outside i64).
fn decimal_cmp(a: &str, b: &str) -> Ordering {
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
