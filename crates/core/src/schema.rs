//! JSON Schema loading and field-type metadata.
use crate::{Direction, JsonFilter, JsonPath, Result, Sort, error::config};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    sync::Arc,
};


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    Object,
    Array,
    String,
    Number,
    Integer,
    Boolean,
    Null,
    Any,
}


/// How an explicitly searchable string is rendered before full-text matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextFormat {
    Plain,
    Markdown,
    Html,
    Xml,
}


impl ValueType {
    pub fn scalar(self) -> bool {
        !matches!(self, Self::Object | Self::Array | Self::Any)
    }
}


#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", content = "name", rename_all = "snake_case")]
pub enum PathStep {
    Field(String),
    Each,
}


// INFO: Parsed schema field type, nullability, children, and relation target.
#[derive(Debug, Clone, Serialize)]
pub struct Field {
    pub kind: ValueType,
    pub nullable: bool,
    /// Opt in to ordered postings for this top-level scalar field.
    pub index: bool,
    /// Locale inherited from the schema root, or overridden on this field.
    pub language: Option<String>,
    /// Build an ordered index for this scalar path.
    pub sort_index: bool,
    /// Reduction used when a sort path reaches multiple values.
    pub sort_reduce: Option<SortReduce>,
    /// Rendering format for this full-text field; plain text is the default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_format: Option<TextFormat>,
    /// Omit this field and all descendants from full-text search.
    pub exclude_fts: bool,
    /// Declared JSON Schema alternatives, when this scalar is an enum.
    pub enum_values: Option<Vec<Value>>,
    pub properties: BTreeMap<String, Field>,
    pub items: Option<Box<Field>>,
    pub relation: Option<String>,
}


#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SortReduce {
    Min,
    Max,
    First,
    Last,
}


// INFO: Root field model paired with the compiled JSON Schema validator.
#[derive(Clone)]
pub(crate) struct Schema {
    pub field: Field,
    pub validator: Arc<jsonschema::Validator>,
}


pub(crate) fn escape(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}


impl Schema {
    pub fn compile(value: &Value, path: &Path) -> Result<Self> {
        index_paths(value, path)?;
        let field = parse(value, path, "", 0, None)?;
        sort_indexes(value, &field, path)?;
        if field.kind != ValueType::Object || field.nullable {
            return Err(config(
                path,
                "",
                "record schema must be a nonnullable object",
            ));
        }
        if field.properties.contains_key("@id") {
            return Err(config(
                path,
                "/properties/@id",
                "@id is reserved for source identity",
            ));
        }
        let validator = jsonschema::options()
            .with_draft(jsonschema::Draft::Draft202012)
            .build(value)
            .map_err(|e| config(path, e.schema_path().to_string(), e.to_string()))?;
        Ok(Self {
            field,
            validator: Arc::new(validator),
        })
    }

    pub fn issues(&self, value: &Value) -> Vec<crate::ValidationIssue> {
        let mut issues = if self.validator.is_valid(value) {
            Vec::new()
        } else {
            self.validator
                .iter_errors(value)
                .map(|e| crate::ValidationIssue {
                    location: e.instance_path().to_string(),
                    schema_path: Some(e.schema_path().to_string()),
                    code: "schema".into(),
                    message: e.to_string(),
                })
                .collect::<Vec<_>>()
        };
        domain_issues(&self.field, value, "", &mut issues);
        issues
    }
}


pub(crate) fn sort_indexes(value: &Value, root: &Field, file: &Path) -> Result<Vec<Vec<Sort>>> {
    fn validate(root: &Field, sort: &Sort, file: &Path) -> Result<()> {
        let path = JsonFilter::parse_path(&sort.field)
            .map_err(|error| config(file, "/x-sort-indexes", error))?;
        let mut field = root;
        let mut repeated = false;
        let mut reduction = None;
        for step in path {
            match step {
                crate::JsonPathStep::Key(name) => {
                    if field.kind == ValueType::Array {
                        repeated = true;
                        reduction = reduction.or(field.sort_reduce);
                        field = field.items.as_deref().expect("array items");
                    }
                    field = field.properties.get(&name).ok_or_else(|| {
                        config(
                            file,
                            "/x-sort-indexes",
                            format!("sort field {name} is not declared"),
                        )
                    })?;
                }
                crate::JsonPathStep::Each => {
                    if field.kind != ValueType::Array {
                        return Err(config(file, "/x-sort-indexes", "[*] requires an array"));
                    }
                    repeated = true;
                    reduction = reduction.or(field.sort_reduce);
                    field = field.items.as_deref().expect("array items");
                }
                crate::JsonPathStep::Index(_) => {
                    if field.kind != ValueType::Array {
                        return Err(config(
                            file,
                            "/x-sort-indexes",
                            "array index requires an array",
                        ));
                    }
                    field = field.items.as_deref().expect("array items");
                }
            }
        }
        if field.kind == ValueType::Array {
            repeated = true;
            reduction = reduction.or(field.sort_reduce);
            field = field.items.as_deref().expect("array items");
        }
        if !field.kind.scalar() {
            return Err(config(
                file,
                "/x-sort-indexes",
                "sort index needs scalar values",
            ));
        }
        if repeated && reduction.is_none() {
            return Err(config(
                file,
                "/x-sort-indexes",
                "array sort index needs x-sort-reduce",
            ));
        }
        Ok(())
    }

    fn walk(field: &Field, path: &str, out: &mut Vec<Vec<Sort>>) {
        if field.sort_index {
            out.push(vec![Sort {
                field: path.into(),
                direction: Direction::Asc,
            }]);
        }
        for (name, child) in &field.properties {
            let simple = name
                .chars()
                .next()
                .is_some_and(|c| c == '_' || c.is_alphabetic())
                && name.chars().all(|c| c == '_' || c.is_alphanumeric());
            let next = if simple && path.is_empty() {
                name.clone()
            } else if simple {
                format!("{path}.{name}")
            } else {
                format!(
                    "{path}[{}]",
                    serde_json::to_string(name).expect("schema key serializes")
                )
            };
            walk(child, &next, out);
        }
        if let Some(items) = &field.items {
            walk(items, &format!("{path}[*]"), out);
        }
    }
    let mut orders = Vec::new();
    walk(root, "", &mut orders);
    if let Some(composites) = value.get("x-sort-indexes") {
        let composites = composites
            .as_array()
            .filter(|items| items.len() <= 64)
            .ok_or_else(|| {
                config(
                    file,
                    "/x-sort-indexes",
                    "expected at most 64 composite indexes",
                )
            })?;
        for (index, item) in composites.iter().enumerate() {
            let terms = item
                .as_array()
                .filter(|terms| (2..=8).contains(&terms.len()))
                .ok_or_else(|| {
                    config(
                        file,
                        format!("/x-sort-indexes/{index}"),
                        "expected 2–8 sort terms",
                    )
                })?;
            let mut order = Vec::new();
            for (term, value) in terms.iter().enumerate() {
                let sort: Sort = serde_json::from_value(value.clone())
                    .map_err(|e| config(file, format!("/x-sort-indexes/{index}/{term}"), e))?;
                if sort.field == "@id" {
                    return Err(config(
                        file,
                        format!("/x-sort-indexes/{index}/{term}"),
                        "@id is implicit",
                    ));
                }
                JsonFilter::parse_path(&sort.field)
                    .map_err(|e| config(file, format!("/x-sort-indexes/{index}/{term}"), e))?;
                if order
                    .iter()
                    .any(|previous: &Sort| previous.field == sort.field)
                {
                    return Err(config(
                        file,
                        format!("/x-sort-indexes/{index}/{term}"),
                        "duplicate sort field",
                    ));
                }
                order.push(sort);
            }
            orders.push(order);
        }
    }
    orders.sort();
    orders.dedup();
    for order in &orders {
        for sort in order {
            validate(root, sort, file)?;
        }
    }
    Ok(orders)
}

pub(crate) fn index_paths(value: &Value, file: &Path) -> Result<Vec<JsonPath>> {
    let Some(paths) = value.get("x-index-paths") else {
        return Ok(Vec::new());
    };
    let paths = paths
        .as_array()
        .filter(|paths| paths.len() <= 4096)
        .ok_or_else(|| {
            config(
                file,
                "/x-index-paths",
                "expected an array of at most 4096 paths",
            )
        })?;
    let mut unique = BTreeSet::new();
    for (position, path) in paths.iter().enumerate() {
        let text = path.as_str().ok_or_else(|| {
            config(
                file,
                format!("/x-index-paths/{position}"),
                "expected a path string",
            )
        })?;
        let parsed = JsonFilter::parse_path(text)
            .map_err(|error| config(file, format!("/x-index-paths/{position}"), error))?;
        if !unique.insert(parsed) {
            return Err(config(
                file,
                format!("/x-index-paths/{position}"),
                "duplicate index path",
            ));
        }
    }
    Ok(unique.into_iter().collect())
}

fn parse(
    v: &Value,
    file: &Path,
    p: &str,
    depth: usize,
    inherited_language: Option<&str>,
) -> Result<Field> {
    if depth > 128 {
        return Err(config(file, p, "schema nesting exceeds 128"));
    }
    let obj = v
        .as_object()
        .ok_or_else(|| config(file, p, "schema must be an object"))?;
    for k in obj.keys() {
        if ![
            "$schema",
            "type",
            "properties",
            "items",
            "required",
            "additionalProperties",
            "enum",
            "index",
            "x-language",
            "x-sort-index",
            "x-sort-reduce",
            "x-sort-indexes",
            "x-full-text",
            "exclude_fts",
            "x-relation",
            "x-index-paths",
            "x-objekte",
            "default",
            "minLength",
            "minimum",
            "uniqueItems",
            "x-children",
            "x-content",
            "x-ownership",
        ]
        .contains(&k.as_str())
        {
            return Err(config(
                file,
                format!("{p}/{}", escape(k)),
                "unsupported schema keyword",
            ));
        }
    }
    if let Some(d) = obj.get("$schema")
        && (!p.is_empty() || d.as_str() != Some("https://json-schema.org/draft/2020-12/schema"))
    {
        return Err(config(
            file,
            p,
            "only the root Draft 2020-12 dialect declaration is supported",
        ));
    }
    if !p.is_empty()
        && (obj.contains_key("x-index-paths")
            || obj.contains_key("x-objekte")
            || obj.contains_key("x-sort-indexes"))
    {
        return Err(config(
            file,
            p,
            "root index and collection settings belong at the schema root",
        ));
    }
    let language = match obj.get("x-language") {
        Some(Value::String(tag)) if !tag.is_empty() => {
            tag.parse::<icu_locale_core::Locale>()
                .map_err(|_| config(file, p, "x-language needs a BCP 47 language tag"))?;
            Some(tag.clone())
        }
        Some(_) => return Err(config(file, p, "x-language needs a BCP 47 language tag")),
        None => inherited_language.map(str::to_owned),
    };
    let mut nullable = false;
    let type_name = match obj.get("type") {
        None if obj.is_empty() => "any",
        Some(Value::String(t)) => t,
        Some(Value::Array(ts))
            if ts.len() == 2 && ts.iter().filter(|t| t.as_str() == Some("null")).count() == 1 =>
        {
            nullable = true;
            ts.iter()
                .find(|t| t.as_str() != Some("null"))
                .and_then(Value::as_str)
                .ok_or_else(|| config(file, p, "invalid nullable type"))?
        }
        _ => {
            return Err(config(
                file,
                p,
                "declare a type, a concrete type plus null, or an empty schema",
            ));
        }
    };
    let kind = match type_name {
        "object" => ValueType::Object,
        "array" => ValueType::Array,
        "string" => ValueType::String,
        "number" => ValueType::Number,
        "integer" => ValueType::Integer,
        "boolean" => ValueType::Boolean,
        "null" => ValueType::Null,
        "any" if obj.is_empty() => ValueType::Any,
        _ => return Err(config(file, p, "unsupported type")),
    };
    let index = match obj.get("index") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => {
            if !p
                .strip_prefix("/properties/")
                .is_some_and(|name| !name.contains('/'))
            {
                return Err(config(file, p, "index requires a top-level field"));
            }
            if !kind.scalar() {
                return Err(config(file, p, "index requires a scalar type"));
            }
            true
        }
        Some(_) => return Err(config(file, p, "index must be a boolean")),
    };
    let sort_index = match obj.get("x-sort-index") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) if kind.scalar() && !p.is_empty() => true,
        Some(Value::Bool(true)) => {
            return Err(config(file, p, "x-sort-index requires a scalar field"));
        }
        Some(_) => return Err(config(file, p, "x-sort-index must be a boolean")),
    };
    let sort_reduce = match obj.get("x-sort-reduce") {
        None => None,
        Some(Value::String(value)) if kind == ValueType::Array => Some(match value.as_str() {
            "min" => SortReduce::Min,
            "max" => SortReduce::Max,
            "first" => SortReduce::First,
            "last" => SortReduce::Last,
            _ => return Err(config(file, p, "invalid x-sort-reduce")),
        }),
        Some(_) => return Err(config(file, p, "x-sort-reduce requires an array field")),
    };
    let text_format = match obj.get("x-full-text") {
        None => None,
        Some(Value::String(format)) if kind == ValueType::String => Some(match format.as_str() {
            "plain" => TextFormat::Plain,
            "markdown" => TextFormat::Markdown,
            "html" => TextFormat::Html,
            "xml" => TextFormat::Xml,
            _ => return Err(config(file, p, "unsupported x-full-text format")),
        }),
        Some(_) => {
            return Err(config(
                file,
                p,
                "x-full-text requires a string field and format",
            ));
        }
    };
    let exclude_fts = match obj.get("exclude_fts") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) if !p.is_empty() => true,
        Some(Value::Bool(true)) => return Err(config(file, p, "exclude_fts requires a field")),
        Some(_) => return Err(config(file, p, "exclude_fts must be a boolean")),
    };
    for k in ["properties", "required", "additionalProperties"] {
        if obj.contains_key(k) && kind != ValueType::Object {
            return Err(config(file, p, format!("{k} requires object type")));
        }
    }
    if obj.contains_key("items") && kind != ValueType::Array {
        return Err(config(file, p, "items requires array type"));
    }
    let enum_values = if let Some(values) = obj.get("enum") {
        if !kind.scalar() || kind == ValueType::Any {
            return Err(config(file, p, "enum requires a scalar type"));
        }
        let values = values
            .as_array()
            .filter(|values| !values.is_empty())
            .ok_or_else(|| config(file, p, "enum must be a nonempty array"))?;
        if values.iter().any(|value| {
            !value.is_null() && !indexable_kind(kind, value)
                || value.is_null() && !nullable && kind != ValueType::Null
        }) {
            return Err(config(file, p, "enum values must match the declared type"));
        }
        Some(values.clone())
    } else {
        None
    };
    if obj
        .get("additionalProperties")
        .is_some_and(|v| !v.is_boolean())
    {
        return Err(config(file, p, "additionalProperties must be a boolean"));
    }
    let mut properties = BTreeMap::new();
    if let Some(ps) = obj.get("properties") {
        for (name, child) in ps
            .as_object()
            .ok_or_else(|| config(file, p, "properties must be an object"))?
        {
            properties.insert(
                name.clone(),
                parse(
                    child,
                    file,
                    &format!("{p}/properties/{}", escape(name)),
                    depth + 1,
                    language.as_deref(),
                )?,
            );
        }
    }
    if let Some(required) = obj.get("required") {
        let a = required
            .as_array()
            .ok_or_else(|| config(file, p, "required must be an array"))?;
        for name in a {
            if !name.as_str().is_some_and(|s| properties.contains_key(s)) {
                return Err(config(
                    file,
                    p,
                    "required fields must have property declarations",
                ));
            }
        }
    }
    let items = if kind == ValueType::Array {
        Some(Box::new(parse(
            obj.get("items")
                .ok_or_else(|| config(file, p, "array requires items"))?,
            file,
            &format!("{p}/items"),
            depth + 1,
            language.as_deref(),
        )?))
    } else {
        None
    };
    let relation = if let Some(r) = obj.get("x-relation") {
        if kind != ValueType::String {
            return Err(config(file, p, "relations require string IDs"));
        }
        let r = r
            .as_object()
            .ok_or_else(|| config(file, p, "x-relation must be an object"))?;
        if r.len() != 1 {
            return Err(config(file, p, "x-relation supports only collection"));
        }
        Some(
            r.get("collection")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or_else(|| config(file, p, "relation needs a collection"))?
                .to_owned(),
        )
    } else {
        None
    };
    Ok(Field {
        kind,
        nullable,
        index,
        language,
        sort_index,
        sort_reduce,
        text_format,
        exclude_fts,
        enum_values,
        properties,
        items,
        relation,
    })
}

pub(crate) fn check_number(v: &Value) -> std::result::Result<(), &'static str> {
    let Some(n) = v.as_number() else {
        return Ok(());
    };
    if !n.to_string().contains(['.', 'e', 'E']) && n.as_i64().is_none() {
        return Err("integer exceeds signed 64-bit range");
    }
    if n.as_f64().is_none_or(|n| !n.is_finite()) {
        return Err("number must be finite");
    }
    Ok(())
}

pub(crate) fn indexable(f: &Field, v: &Value) -> bool {
    indexable_kind(f.kind, v)
}

fn indexable_kind(kind: ValueType, v: &Value) -> bool {
    match kind {
        ValueType::String => v.is_string(),
        ValueType::Boolean => v.is_boolean(),
        ValueType::Number => v.is_number() && check_number(v).is_ok(),
        ValueType::Integer => {
            v.is_number()
                && check_number(v).is_ok()
                && (v.as_i64().is_some()
                    || v.as_f64().is_some_and(|n| {
                        n.fract() == 0.0
                            && (-9223372036854775808.0..9223372036854775808.0).contains(&n)
                    }))
        }
        ValueType::Null => v.is_null(),
        _ => false,
    }
}

fn domain_issues(f: &Field, v: &Value, p: &str, out: &mut Vec<crate::ValidationIssue>) {
    if v.is_null() {
        return;
    }
    if v.is_number() && matches!(f.kind, ValueType::Number | ValueType::Integer) {
        let error = check_number(v).err().or_else(|| {
            (f.kind == ValueType::Integer
                && v.as_i64().is_none()
                && v.as_f64()
                    .is_some_and(|n| !(-9223372036854775808.0..9223372036854775808.0).contains(&n)))
            .then_some("integer exceeds signed 64-bit range")
        });
        if let Some(message) = error {
            out.push(crate::ValidationIssue::new(p, "numeric_domain", message));
        }
    }
    if f.relation.is_some()
        && let Some(id) = v.as_str()
        && !id.is_empty()
        && let Err(e) = crate::repository::validate_id(id)
    {
        out.push(crate::ValidationIssue::new(
            p,
            "reference_id",
            &e.to_string(),
        ));
    }
    if let Some(obj) = v.as_object() {
        for (k, child) in &f.properties {
            if let Some(v) = obj.get(k) {
                domain_issues(child, v, &format!("{p}/{}", escape(k)), out);
            }
        }
    }
    if let (Some(item), Some(vs)) = (&f.items, v.as_array()) {
        for (i, v) in vs.iter().enumerate() {
            domain_issues(item, v, &format!("{p}/{i}"), out);
        }
    }
}
