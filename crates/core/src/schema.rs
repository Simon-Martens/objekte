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


/// Named text interpretations declared for one logical string field.
#[derive(Debug, Clone, Serialize)]
pub struct TextSearchField {
    pub default: Option<String>,
    pub definitions: BTreeMap<String, TextSearchFieldOptions>,
}


/// Whether a declared interpretation participates in ordinary text warmup.
#[derive(Debug, Clone, Serialize)]
pub struct TextSearchFieldOptions {
    pub index: bool,
}


/// Follow a logical field path, including implicit array traversal.
pub(crate) fn field_at_path<'a>(
    fields: &'a BTreeMap<String, Field>,
    path: &[crate::JsonPathStep],
) -> Option<&'a Field> {
    let (crate::JsonPathStep::Key(first), rest) = path.split_first()? else {
        return None;
    };
    let mut field = fields.get(first)?;
    for step in rest {
        while field.kind == ValueType::Array && matches!(step, crate::JsonPathStep::Key(_)) {
            field = field.items.as_deref()?;
        }
        field = match step {
            crate::JsonPathStep::Key(key) => field.properties.get(key)?,
            crate::JsonPathStep::Each | crate::JsonPathStep::Index(_) => field.items.as_deref()?,
        };
    }
    Some(field)
}

/// Configured text fields in canonical traversal spelling, including `[*]`.
pub(crate) fn text_search_paths(fields: &BTreeMap<String, Field>) -> Vec<String> {
    fn visit(field: &Field, path: &mut crate::JsonPath, paths: &mut Vec<String>) {
        if field.text_search.is_some() {
            paths.push(crate::json_filter::format_path(path));
        }
        for (name, child) in &field.properties {
            path.push(crate::JsonPathStep::Key(name.clone()));
            visit(child, path, paths);
            path.pop();
        }
        if let Some(item) = &field.items {
            path.push(crate::JsonPathStep::Each);
            visit(item, path, paths);
            path.pop();
        }
    }
    let mut paths = Vec::new();
    for (name, field) in fields {
        visit(
            field,
            &mut vec![crate::JsonPathStep::Key(name.clone())],
            &mut paths,
        );
    }
    paths
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
    /// Additional named definitions and the implicit field default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text_search: Option<TextSearchField>,
    /// Omit this field and all descendants from full-text search.
    pub exclude_fts: bool,
    /// Declared JSON Schema alternatives, when this scalar is an enum.
    pub enum_values: Option<Vec<Value>>,
    /// Values seen by inference, offered as suggestions but not enforced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub suggested_values: Option<Vec<String>>,
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
        Self::compile_with_annotations(value, path, &BTreeSet::new())
    }

    pub(crate) fn compile_with_annotations(
        value: &Value,
        path: &Path,
        annotations: &BTreeSet<String>,
    ) -> Result<Self> {
        index_paths(value, path)?;
        let field = parse(value, path, "", 0, None, annotations)?;
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
                    source: None,
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
    annotations: &BTreeSet<String>,
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
            "x-enum-suggested",
            "index",
            "x-language",
            "x-sort-index",
            "x-sort-reduce",
            "x-sort-indexes",
            "x-full-text",
            "x-text-search",
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
            && !annotations.contains(k)
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
    let text_search = match obj.get("x-text-search") {
        None => None,
        Some(raw) if kind == ValueType::String => {
            if text_format.is_some() {
                return Err(config(file, p, "x-text-search conflicts with x-full-text"));
            }
            let text = raw
                .as_object()
                .ok_or_else(|| config(file, p, "x-text-search must be an object"))?;
            if text
                .keys()
                .any(|key| key != "default" && key != "definitions")
            {
                return Err(config(
                    file,
                    p,
                    "x-text-search accepts only default and definitions",
                ));
            }
            let default = text
                .get("default")
                .map(|value| {
                    value
                        .as_str()
                        .filter(|name| !name.is_empty())
                        .map(str::to_owned)
                        .ok_or_else(|| config(file, p, "x-text-search default must be a name"))
                })
                .transpose()?;
            let definitions = text
                .get("definitions")
                .and_then(Value::as_object)
                .ok_or_else(|| config(file, p, "x-text-search needs definitions"))?;
            if definitions.is_empty() {
                return Err(config(
                    file,
                    p,
                    "x-text-search needs at least one definition",
                ));
            }
            let mut parsed = BTreeMap::new();
            for (name, options) in definitions {
                let options = options
                    .as_object()
                    .ok_or_else(|| config(file, p, "text definition options must be an object"))?;
                if options.keys().any(|key| key != "index") {
                    return Err(config(
                        file,
                        p,
                        "text definition options currently accept only index",
                    ));
                }
                let index = match options.get("index") {
                    None => false,
                    Some(Value::Bool(index)) => *index,
                    Some(_) => return Err(config(file, p, "text index must be a boolean")),
                };
                parsed.insert(name.clone(), TextSearchFieldOptions { index });
            }
            if default
                .as_ref()
                .is_some_and(|name| !parsed.contains_key(name))
            {
                return Err(config(
                    file,
                    p,
                    "x-text-search default must appear in definitions",
                ));
            }
            Some(TextSearchField {
                default,
                definitions: parsed,
            })
        }
        Some(_) => return Err(config(file, p, "x-text-search requires a string field")),
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
    let suggested_values = match obj.get("x-enum-suggested") {
        None => None,
        Some(Value::Array(values)) if kind == ValueType::String => Some(
            values
                .iter()
                .map(|value| value.as_str().map(str::to_owned))
                .collect::<Option<Vec<_>>>()
                .ok_or_else(|| config(file, p, "x-enum-suggested must list strings"))?,
        ),
        Some(_) => {
            return Err(config(
                file,
                p,
                "x-enum-suggested needs a string field and an array of strings",
            ));
        }
    };
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
                    annotations,
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
            annotations,
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
        text_search,
        exclude_fts,
        enum_values,
        suggested_values,
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
