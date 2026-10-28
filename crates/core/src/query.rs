use crate::{
    Collection, Direction, Error, Metrics, Result, Sort, ValueType, ViewHashes,
    config::complete_order, schema::check_number,
};
use num_cmp::NumCmp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Eq,
    Lt,
    Le,
    Gt,
    Ge,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Filter {
    pub field: String,
    pub operator: Operator,
    pub value: Value,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewRequest {
    #[serde(default)]
    pub filters: Vec<Filter>,
    pub sort: Option<Sort>,
    pub sub_sorts: Option<Vec<Sort>>,
    pub columns: Option<Vec<String>>,
    pub limit: Option<usize>,
    #[serde(default)]
    pub offset: usize,
    #[serde(default)]
    pub count: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ViewRow {
    pub collection: String,
    pub id: String,
    pub path: String,
    pub label: String,
    pub values: BTreeMap<String, Value>,
}
#[derive(Debug, Clone, Serialize)]
pub struct ViewResult {
    pub columns: Vec<String>,
    pub rows: Vec<ViewRow>,
    pub total: Option<usize>,
    pub metrics: Metrics,
    pub query_plan: Vec<String>,
    /// Resolved query definition, not a cache of its results.
    pub view_hashes: ViewHashes,
}
#[derive(Debug, Clone)]
pub(crate) enum Scalar {
    Missing,
    Null,
    Bool(bool),
    Int(i64),
    Real(f64),
    String(String),
}
impl Scalar {
    pub fn state(&self) -> i64 {
        match self {
            Self::Missing => 2,
            Self::Null => 1,
            _ => 0,
        }
    }
    pub fn sql(&self) -> rusqlite::types::Value {
        use rusqlite::types::Value as S;
        match self {
            Self::Missing | Self::Null => S::Null,
            Self::Bool(v) => S::Integer(i64::from(*v)),
            Self::Int(v) => S::Integer(*v),
            Self::Real(v) => S::Real(*v),
            Self::String(v) => S::Text(v.clone()),
        }
    }
    pub fn cmp(&self, other: &Self) -> Ordering {
        self.state()
            .cmp(&other.state())
            .then_with(|| match (self, other) {
                (Self::Int(a), Self::Int(b)) => a.cmp(b),
                (Self::Real(a), Self::Real(b)) => a.partial_cmp(b).unwrap(),
                (Self::Int(a), Self::Real(b)) => a.num_cmp(*b).unwrap(),
                (Self::Real(a), Self::Int(b)) => a.num_cmp(*b).unwrap(),
                (Self::Bool(a), Self::Bool(b)) => a.cmp(b),
                (Self::String(a), Self::String(b)) => a.cmp(b),
                _ => Ordering::Equal,
            })
    }
}
pub(crate) fn scalar(id: &str, data: &Value, field: &str) -> Scalar {
    if field == "@id" {
        return Scalar::String(id.into());
    }
    scalar_value(data.get(field))
}
fn scalar_value(value: Option<&Value>) -> Scalar {
    match value {
        None => Scalar::Missing,
        Some(Value::Null) => Scalar::Null,
        Some(Value::Bool(b)) => Scalar::Bool(*b),
        Some(Value::String(s)) => Scalar::String(s.clone()),
        Some(Value::Number(n)) => {
            if let Some(n) = n.as_i64() {
                Scalar::Int(n)
            } else {
                Scalar::Real(n.as_f64().unwrap())
            }
        }
        _ => unreachable!("view compilation rejects nonscalar fields"),
    }
}
pub(crate) struct Plan {
    pub sorts: Vec<Sort>,
    pub hashes: ViewHashes,
    pub columns: Vec<String>,
    pub filters: Vec<Filter>,
    pub limit: usize,
    pub offset: usize,
    pub count: bool,
}
impl Plan {
    pub fn compile(c: &Collection, req: &ViewRequest) -> Result<Self> {
        let primary = req
            .sort
            .clone()
            .unwrap_or_else(|| c.list.default_sort.clone());
        let secondary = req.sub_sorts.clone().unwrap_or_else(|| {
            if req.sort.is_some() {
                Vec::new()
            } else {
                c.list.sub_sorts.clone()
            }
        });
        let sorts = complete_order(std::iter::once(primary).chain(secondary).collect())?;
        for sort in &sorts {
            if sort.field != "@id" && !c.list.sortable.as_ref().unwrap().contains(&sort.field) {
                return Err(Error::Request(format!("{} is not sortable", sort.field)));
            }
        }
        let columns = req
            .columns
            .clone()
            .unwrap_or_else(|| c.list.columns.clone());
        if columns.is_empty() || columns.iter().collect::<BTreeSet<_>>().len() != columns.len() {
            return Err(Error::Request("columns must be nonempty and unique".into()));
        }
        for name in &columns {
            if name != "@id" && !c.fields.get(name).is_some_and(|f| f.kind.scalar()) {
                return Err(Error::Request(format!(
                    "unknown or nonscalar column {name}"
                )));
            }
        }
        for filter in &req.filters {
            if !c.list.filters.contains(&filter.field) {
                return Err(Error::Request(format!(
                    "{} is not a declared filter",
                    filter.field
                )));
            }
            let kind = if filter.field == "@id" {
                ValueType::String
            } else {
                c.fields[&filter.field].kind
            };
            validate_filter(filter, kind)?;
        }
        let limit = req.limit.unwrap_or(c.page_size);
        if limit == 0 || limit > i64::MAX as usize || req.offset > i64::MAX as usize {
            return Err(Error::Request("invalid limit/offset".into()));
        }
        // AND filter order does not change semantics. Preserve value types and operators.
        let mut filter_keys = req
            .filters
            .iter()
            .map(|f| serde_json::to_string(f).unwrap())
            .collect::<Vec<_>>();
        filter_keys.sort();
        filter_keys.dedup();
        let hashes = ViewHashes::new(
            &filter_keys,
            &sorts,
            &(&columns, &c.display, limit, req.offset, req.count),
            &(&c.name, &c.mapping_hash, &c.schema_hash),
        );
        Ok(Self {
            sorts,
            hashes,
            columns,
            filters: req.filters.clone(),
            limit,
            offset: req.offset,
            count: req.count,
        })
    }
    pub fn matches(&self, id: &str, data: &Value) -> bool {
        self.filters.iter().all(|f| {
            let lhs = scalar(id, data, &f.field);
            let rhs = scalar_value(Some(&f.value));
            if lhs.state() == 2 {
                return false;
            }
            if lhs.state() == 1 || rhs.state() == 1 {
                return matches!(f.operator, Operator::Eq) && lhs.state() == rhs.state();
            }
            let cmp = lhs.cmp(&rhs);
            match f.operator {
                Operator::Eq => cmp == Ordering::Equal,
                Operator::Lt => cmp == Ordering::Less,
                Operator::Le => cmp != Ordering::Greater,
                Operator::Gt => cmp == Ordering::Greater,
                Operator::Ge => cmp != Ordering::Less,
            }
        })
    }
    pub fn keys(&self, id: &str, data: &Value) -> Vec<Scalar> {
        self.sorts
            .iter()
            .map(|s| scalar(id, data, &s.field))
            .collect()
    }
    pub fn compare(&self, a: &(Vec<Scalar>, ViewRow), b: &(Vec<Scalar>, ViewRow)) -> Ordering {
        for ((left, right), sort) in a.0.iter().zip(&b.0).zip(&self.sorts) {
            let cmp = left.state().cmp(&right.state()).then_with(|| {
                let cmp = left.cmp(right);
                if sort.direction == Direction::Desc {
                    cmp.reverse()
                } else {
                    cmp
                }
            });
            if cmp != Ordering::Equal {
                return cmp;
            }
        }
        Ordering::Equal
    }
    pub fn row(&self, c: &Collection, id: &str, path: String, data: &Value) -> ViewRow {
        let label = if c.display == "@id" {
            id.to_owned()
        } else {
            match data.get(&c.display) {
                None | Some(Value::Null) => id.to_owned(),
                Some(Value::String(s)) => s.clone(),
                Some(v) => v.to_string(),
            }
        };
        let mut values = BTreeMap::new();
        for f in &self.columns {
            if f == "@id" {
                values.insert(f.clone(), Value::String(id.into()));
            } else if let Some(v) = data.get(f) {
                values.insert(f.clone(), v.clone());
            }
        }
        ViewRow {
            collection: c.name.clone(),
            id: id.into(),
            path,
            label,
            values,
        }
    }
    pub fn sql_where(&self, c: &Collection) -> (String, Vec<rusqlite::types::Value>) {
        let mut conditions = Vec::new();
        let mut params = Vec::new();
        for f in &self.filters {
            let op = match f.operator {
                Operator::Eq => "=",
                Operator::Lt => "<",
                Operator::Le => "<=",
                Operator::Gt => ">",
                Operator::Ge => ">=",
            };
            if f.field == "@id" {
                conditions.push(format!("v.id {op} ?"));
                params.push(scalar_value(Some(&f.value)).sql());
                continue;
            }
            let i = c.indexes.iter().position(|i| i.name == f.field).unwrap();
            if f.value.is_null() {
                conditions.push(format!("v.f{i}_s = 1"));
            } else {
                conditions.push(format!("v.f{i}_s = 0 AND v.f{i}_v {op} ?"));
                params.push(scalar_value(Some(&f.value)).sql());
            }
        }
        (
            if conditions.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", conditions.join(" AND "))
            },
            params,
        )
    }
    pub fn sql_order(&self, c: &Collection) -> String {
        self.sorts
            .iter()
            .map(|sort| {
                let direction = if sort.direction == Direction::Asc {
                    "ASC"
                } else {
                    "DESC"
                };
                if sort.field == "@id" {
                    format!("v.id {direction}")
                } else {
                    let i = c.indexes.iter().position(|i| i.name == sort.field).unwrap();
                    format!("v.f{i}_s ASC, v.f{i}_v {direction}")
                }
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}
fn validate_filter(f: &Filter, kind: ValueType) -> Result<()> {
    if f.value.is_null() {
        if matches!(f.operator, Operator::Eq) && f.field != "@id" {
            return Ok(());
        }
        return Err(Error::Request(
            "null only supports equality on data fields".into(),
        ));
    }
    let matches = match kind {
        ValueType::String => f.value.is_string(),
        ValueType::Boolean => f.value.is_boolean(),
        ValueType::Number => f.value.is_number(),
        ValueType::Integer => f
            .value
            .as_number()
            .is_some_and(|n| n.as_i64().is_some() || n.as_f64().is_some_and(|v| v.fract() == 0.0)),
        _ => false,
    };
    if !matches {
        return Err(Error::Request(format!(
            "wrong filter value type for {}",
            f.field
        )));
    }
    if !matches!(kind, ValueType::Integer | ValueType::Number)
        && !matches!(f.operator, Operator::Eq)
    {
        return Err(Error::Request(
            "ordered filters require numeric fields".into(),
        ));
    }
    check_number(&f.value).map_err(|e| Error::Request(e.into()))?;
    if kind == ValueType::Integer
        && f.value.as_i64().is_none()
        && !(-9223372036854775808.0..9223372036854775808.0).contains(&f.value.as_f64().unwrap())
    {
        return Err(Error::Request("integer exceeds signed 64-bit range".into()));
    }
    Ok(())
}
impl Collection {
    /// Parse one CLI filter. This deliberately is not a general expression language.
    pub fn parse_filter(&self, text: &str) -> Result<Filter> {
        let (field, op, value) = [
            (">=", Operator::Ge),
            ("<=", Operator::Le),
            ("=", Operator::Eq),
            (">", Operator::Gt),
            ("<", Operator::Lt),
        ]
        .into_iter()
        .find_map(|(s, op)| text.split_once(s).map(|(f, v)| (f.trim(), op, v.trim())))
        .ok_or_else(|| Error::Request("filter must be field=value or a numeric bound".into()))?;
        let kind = if field == "@id" {
            ValueType::String
        } else {
            self.fields
                .get(field)
                .ok_or_else(|| Error::Request(format!("unknown field {field}")))?
                .kind
        };
        let value = if kind == ValueType::String && value != "null" && !value.starts_with('"') {
            Value::String(value.into())
        } else {
            serde_json::from_str(value)
                .map_err(|e| Error::Request(format!("invalid filter value: {e}")))?
        };
        let filter = Filter {
            field: field.into(),
            operator: op,
            value,
        };
        Plan::compile(
            self,
            &ViewRequest {
                filters: vec![filter.clone()],
                ..Default::default()
            },
        )?;
        Ok(filter)
    }
}
