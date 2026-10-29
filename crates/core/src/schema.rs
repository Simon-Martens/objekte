use crate::{Result, error::config};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::BTreeMap, path::Path, sync::Arc};


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
    pub properties: BTreeMap<String, Field>,
    pub items: Option<Box<Field>>,
    pub relation: Option<String>,
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
        let field = parse(value, path, "", 0)?;
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


fn parse(v: &Value, file: &Path, p: &str, depth: usize) -> Result<Field> {
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
            "x-relation",
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
    for k in ["properties", "required", "additionalProperties"] {
        if obj.contains_key(k) && kind != ValueType::Object {
            return Err(config(file, p, format!("{k} requires object type")));
        }
    }
    if obj.contains_key("items") && kind != ValueType::Array {
        return Err(config(file, p, "items requires array type"));
    }
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
    match f.kind {
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
