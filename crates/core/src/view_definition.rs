//! Repository-level view definitions, independent of collection schemas.

use crate::{Error, JsonQuery, JsonQueryResult, Repository, Result, TextQuery, TextQueryResult};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::PathBuf};


/// Shape that a UI can use to render a named view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewType {
    List,
    Single,
}


/// Type and optional fallback for one shared view argument.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewArgument {
    #[serde(rename = "type")]
    pub kind: ViewArgumentType,
    #[serde(default)]
    pub default: Option<Value>,
}


/// JSON value type accepted by a view argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ViewArgumentType {
    String,
    Integer,
    Number,
    Boolean,
    Array,
    Object,
}


impl ViewArgumentType {
    fn accepts(self, value: &Value) -> bool {
        match self {
            Self::String => value.is_string(),
            Self::Integer => value.as_i64().is_some() || value.as_u64().is_some(),
            Self::Number => value.is_number(),
            Self::Boolean => value.is_boolean(),
            Self::Array => value.is_array(),
            Self::Object => value.is_object(),
        }
    }
}


/// A query template with the native arguments of its query API.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ViewQuery {
    Json { request: Value },
    Text { request: Value },
}


impl ViewQuery {
    fn request(&self) -> &Value {
        match self {
            Self::Json { request } | Self::Text { request } => request,
        }
    }

    fn validate_request(&self, value: Value) -> Result<()> {
        match self {
            Self::Json { .. } => {
                let request: JsonQuery = serde_json::from_value(value)
                    .map_err(|error| Error::Request(format!("invalid JSON view query: {error}")))?;
                if request.filter.trim().is_empty() {
                    return Err(Error::Request("JSON view query needs a filter".into()));
                }
            }
            Self::Text { .. } => {
                let request: TextQuery = serde_json::from_value(value)
                    .map_err(|error| Error::Request(format!("invalid text view query: {error}")))?;
                if request.pattern.trim().is_empty() {
                    return Err(Error::Request("text view query needs a pattern".into()));
                }
            }
        }
        Ok(())
    }
}


/// A named view and its shared arguments and independently scoped queries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ViewType,
    #[serde(default)]
    pub arguments: BTreeMap<String, ViewArgument>,
    pub queries: BTreeMap<String, ViewQuery>,
    #[serde(skip)]
    pub path: PathBuf,
}


/// One result from an independent view query.
#[derive(Debug, Serialize)]
#[serde(tag = "type", content = "result", rename_all = "snake_case")]
pub enum ViewQueryResult {
    Json(JsonQueryResult),
    Text(TextQueryResult),
}


/// Results keyed by query name, preserving the view's declared type.
#[derive(Debug, Serialize)]
pub struct NamedViewResult {
    pub name: String,
    #[serde(rename = "type")]
    pub kind: ViewType,
    pub queries: BTreeMap<String, ViewQueryResult>,
}


impl ViewDefinition {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.name.is_empty() || self.queries.is_empty() {
            return Err(Error::Request(
                "view needs a name and at least one query".into(),
            ));
        }
        for (name, argument) in &self.arguments {
            if name.is_empty() || name.contains('.') {
                return Err(Error::Request(format!("invalid view argument {name:?}")));
            }
            if let Some(default) = &argument.default
                && !argument.kind.accepts(default)
            {
                return Err(Error::Request(format!(
                    "wrong default type for view argument {name}"
                )));
            }
        }
        for (name, query) in &self.queries {
            if name.is_empty() || !query.request().is_object() {
                return Err(Error::Request(format!(
                    "view query {name:?} needs an object request"
                )));
            }
            let sample =
                self.arguments
                    .iter()
                    .map(|(name, definition)| {
                        let value = definition
                            .default
                            .clone()
                            .unwrap_or_else(|| match definition.kind {
                                ViewArgumentType::String => Value::String("sample".into()),
                                ViewArgumentType::Integer | ViewArgumentType::Number => {
                                    Value::from(1)
                                }
                                ViewArgumentType::Boolean => Value::Bool(false),
                                ViewArgumentType::Array => Value::Array(Vec::new()),
                                ViewArgumentType::Object => Value::Object(Default::default()),
                            });
                        (name.clone(), value)
                    })
                    .collect();
            let resolved = resolve(query.request(), &sample)?;
            query.validate_request(resolved)?;
        }
        Ok(())
    }

    fn bind(&self, supplied: &BTreeMap<String, Value>) -> Result<BTreeMap<String, Value>> {
        for name in supplied.keys() {
            if !self.arguments.contains_key(name) {
                return Err(Error::Request(format!(
                    "unknown argument {name} for view {}",
                    self.name
                )));
            }
        }
        let mut bound = BTreeMap::new();
        for (name, definition) in &self.arguments {
            let value = supplied
                .get(name)
                .or(definition.default.as_ref())
                .ok_or_else(|| {
                    Error::Request(format!("missing argument {name} for view {}", self.name))
                })?;
            if !definition.kind.accepts(value) {
                return Err(Error::Request(format!(
                    "wrong type for view argument {name}"
                )));
            }
            bound.insert(name.clone(), value.clone());
        }
        Ok(bound)
    }
}


fn resolve(value: &Value, arguments: &BTreeMap<String, Value>) -> Result<Value> {
    match value {
        Value::Object(object) if object.len() == 1 && object.contains_key("$page_offset") => {
            let operands = object["$page_offset"]
                .as_object()
                .ok_or_else(|| Error::Request("$page_offset needs page and size".into()))?;
            if operands.len() != 2 {
                return Err(Error::Request("$page_offset needs page and size".into()));
            }
            let page = resolve(
                operands
                    .get("page")
                    .ok_or_else(|| Error::Request("missing page".into()))?,
                arguments,
            )?;
            let size = resolve(
                operands
                    .get("size")
                    .ok_or_else(|| Error::Request("missing size".into()))?,
                arguments,
            )?;
            let (Some(page), Some(size)) = (page.as_u64(), size.as_u64()) else {
                return Err(Error::Request(
                    "page and size must be positive integers".into(),
                ));
            };
            let offset = page
                .checked_sub(1)
                .and_then(|page| page.checked_mul(size))
                .filter(|_| size > 0)
                .ok_or_else(|| Error::Request("invalid page offset".into()))?;
            Ok(Value::from(offset))
        }
        Value::Object(object) if object.len() == 1 && object.contains_key("$arg") => {
            let name = object["$arg"]
                .as_str()
                .ok_or_else(|| Error::Request("$arg must name a declared view argument".into()))?;
            arguments
                .get(name)
                .cloned()
                .ok_or_else(|| Error::Request(format!("unknown view argument reference {name}")))
        }
        Value::Object(object) => Ok(Value::Object(
            object
                .iter()
                .map(|(name, value)| {
                    resolve(value, arguments).map(|resolved| (name.clone(), resolved))
                })
                .collect::<Result<_>>()?,
        )),
        Value::Array(array) => Ok(Value::Array(
            array
                .iter()
                .map(|value| resolve(value, arguments))
                .collect::<Result<_>>()?,
        )),
        _ => Ok(value.clone()),
    }
}


impl Repository {
    /// Run a named, repository-level view with shared arguments. Query request
    /// fields are otherwise the native JSON or text query arguments.
    pub fn run_view(
        &mut self,
        name: &str,
        arguments: &BTreeMap<String, Value>,
    ) -> Result<NamedViewResult> {
        self.run_view_with_query_arguments(name, arguments, &BTreeMap::new())
    }

    /// Override native request fields for individual queries in a named view.
    /// Shared arguments are resolved after the overrides are applied.
    pub fn run_view_with_query_arguments(
        &mut self,
        name: &str,
        arguments: &BTreeMap<String, Value>,
        query_arguments: &BTreeMap<String, BTreeMap<String, Value>>,
    ) -> Result<NamedViewResult> {
        self.reload()?;
        let view = self
            .model()
            .views
            .get(name)
            .ok_or_else(|| Error::Request(format!("unknown view {name}")))?
            .clone();
        let bound = view.bind(arguments)?;
        for query_name in query_arguments.keys() {
            if !view.queries.contains_key(query_name) {
                return Err(Error::Request(format!(
                    "unknown query {query_name} in view {name}"
                )));
            }
        }
        let mut queries = BTreeMap::new();
        for (query_name, query) in &view.queries {
            let mut request = query.request().clone();
            if let Some(overrides) = query_arguments.get(query_name) {
                let object = request.as_object_mut().expect("validated request object");
                for (key, value) in overrides {
                    object.insert(key.clone(), value.clone());
                }
            }
            let request = resolve(&request, &bound)?;
            let result = match query {
                ViewQuery::Json { .. } => {
                    let request: JsonQuery = serde_json::from_value(request).map_err(|error| {
                        Error::Request(format!("invalid JSON query {query_name}: {error}"))
                    })?;
                    ViewQueryResult::Json(self.query_json(&request)?)
                }
                ViewQuery::Text { .. } => {
                    let request: TextQuery = serde_json::from_value(request).map_err(|error| {
                        Error::Request(format!("invalid text query {query_name}: {error}"))
                    })?;
                    ViewQueryResult::Text(self.search_text(&request)?)
                }
            };
            queries.insert(query_name.clone(), result);
        }
        Ok(NamedViewResult {
            name: view.name,
            kind: view.kind,
            queries,
        })
    }
}
