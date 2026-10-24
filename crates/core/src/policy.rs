//! Shared collection policies, view values, ownership, defaults and system metadata.
use crate::{Editor, Error, Result, ValidationIssue};
use serde_json::{Value, json};
use std::{collections::BTreeMap, process::Command};

pub(crate) fn read_only(schema: &Value) -> bool {
    let p = &schema["x-ownership"];
    let kind = p.as_str().or_else(|| p["kind"].as_str()).unwrap_or("user");
    schema.get("const").is_some()
        || matches!(kind, "generated" | "immutable")
        || (kind == "imported" && p["editable"] != true)
}
pub(crate) fn ownership(
    root: &Value,
    schema: &Value,
    old: Option<&Value>,
    new: Option<&Value>,
    path: &str,
    issues: &mut Vec<ValidationIssue>,
) {
    let s = crate::draft::resolve(root, schema, 0);
    if read_only(&s) && old != new {
        issues.push(ValidationIssue {
            path: path.into(),
            message: "This field is not user-editable".into(),
        });
        return;
    }
    if let Some(props) = s["properties"].as_object() {
        for (k, child) in props {
            ownership(
                root,
                child,
                old.and_then(|v| v.get(k)),
                new.and_then(|v| v.get(k)),
                &format!("{path}/{}", k.replace('~', "~0").replace('/', "~1")),
                issues,
            );
        }
    }
    if let Some(items) = new.and_then(Value::as_array) {
        for (i, v) in items.iter().enumerate() {
            if let Some(s) = s.get("items") {
                ownership(
                    root,
                    s,
                    old.and_then(|v| v.get(i)),
                    Some(v),
                    &format!("{path}/{i}"),
                    issues,
                );
            }
        }
    }
}
impl Editor {
    pub fn allows(&self, collection: &str, action: &str) -> bool {
        let operations = if let Some(c) = self.config().collections.get(collection) {
            &c.operations
        } else if let Some(c) = self.config().media_collections.get(collection) {
            &c.operations
        } else {
            return false;
        };
        operations
            .as_ref()
            .is_none_or(|ops| ops.iter().any(|op| op == action))
    }
    pub fn require_operation(&self, collection: &str, action: &str) -> Result<()> {
        if self.allows(collection, action) {
            Ok(())
        } else {
            Err(Error::Data(format!(
                "{action} is disabled for {collection}"
            )))
        }
    }
    pub fn media_collection_name(&self) -> &str {
        self.config()
            .media_collections
            .iter()
            .find(|(_, c)| c.path == self.config().media.path)
            .map(|(n, _)| n.as_str())
            .unwrap_or("media")
    }
    pub fn schema_revision(&self, collection: &str) -> Result<Option<String>> {
        let Some(path) = self.config().collection(collection)?.schema_path.as_deref() else {
            return Ok(None);
        };
        let root = &self.config().root;
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "--show-toplevel"])
            .output();
        let Ok(top) = output else { return Ok(None) };
        if !top.status.success() {
            return Ok(None);
        };
        let top = std::path::PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
        let full = crate::contained(root, path)?;
        let bytes = std::fs::read(&full)?;
        // The revision must describe the schema actually loaded by this process.
        let mut loaded = self.config().collection(collection)?.schema_value.clone();
        loaded["properties"]
            .as_object_mut()
            .unwrap()
            .remove("_editor");
        let current: Value =
            serde_json::from_slice(&bytes).map_err(|e| Error::Config(e.to_string()))?;
        if current != loaded {
            return Ok(None);
        }
        let relative = full
            .strip_prefix(&top)
            .map_err(|e| Error::Data(e.to_string()))?
            .to_string_lossy();
        let log = Command::new("git")
            .arg("-C")
            .arg(&top)
            .args(["log", "-1", "--format=%H", "--", &relative])
            .output()?;
        let hash = String::from_utf8_lossy(&log.stdout).trim().to_owned();
        if hash.len() != 40 {
            return Ok(None);
        };
        let committed = Command::new("git")
            .arg("-C")
            .arg(&top)
            .args(["show", &format!("{hash}:{relative}")])
            .output()?;
        Ok((committed.status.success() && committed.stdout == bytes).then_some(hash))
    }
    /// Derived values are read projections. They are never duplicated into persisted records.
    pub fn view_value(&self, collection: &str, value: &Value) -> Result<Value> {
        Ok(self.projected_value(collection, value)?.into_owned())
    }
    pub(crate) fn projected_value<'a>(
        &self,
        collection: &str,
        value: &'a Value,
    ) -> Result<std::borrow::Cow<'a, Value>> {
        let c = self.config().collection(collection)?;
        if !c.schema_value["properties"]
            .as_object()
            .is_some_and(|props| {
                props
                    .values()
                    .any(|schema| schema.get("x-derived").is_some())
            })
        {
            return Ok(std::borrow::Cow::Borrowed(value));
        }
        let mut result = value.clone();
        if let Some(props) = c.schema_value["properties"].as_object() {
            for (key, s) in props {
                let Some(source) = s.pointer("/x-derived/via").and_then(Value::as_str) else {
                    continue;
                };
                let Some(id) = value[source].as_str().filter(|s| !s.is_empty()) else {
                    continue;
                };
                let target = c.schema_value["properties"][source]
                    .pointer("/x-relation/collection")
                    .and_then(Value::as_str)
                    .ok_or_else(|| Error::Config("derived field needs a relation source".into()))?;
                if let Some(object) = result.as_object_mut() {
                    object.remove(key);
                }
                if let Ok(doc) = self.get(target, id) {
                    if let Some(v) = s
                        .pointer("/x-derived/field")
                        .and_then(Value::as_str)
                        .and_then(|f| doc.value.get(f))
                    {
                        result[key] = v.clone();
                    }
                }
            }
        }
        Ok(std::borrow::Cow::Owned(result))
    }
    pub fn initial_value(&self, collection: &str, context: &Value) -> Result<Value> {
        let c = self.config().collection(collection)?;
        let mut value = json!({});
        if let Some(props) = c.schema_value["properties"].as_object() {
            for (k, s) in props {
                let s = crate::draft::resolve(&c.schema_value, s, 0);
                if let Some(v) = s.get("default").or_else(|| s.get("const")) {
                    value[k] = v.clone();
                }
                if !read_only(&s) {
                    if let Some(v) = context.get(k) {
                        value[k] = v.clone();
                    }
                }
            }
        }
        Ok(value)
    }
    /// A new draft exposes required editable fields without inventing saved values.
    /// Optional fields remain absent until explicitly included or defaulted.
    pub fn create_draft(&self, collection: &str, context: &Value) -> Result<crate::draft::Draft> {
        let mut draft =
            self.draft_for_id(collection, "", &self.initial_value(collection, context)?)?;
        for field in &mut draft.fields {
            if field.required && !field.hidden && !field.read_only {
                field.present = true;
            }
        }
        Ok(draft)
    }
    pub fn draft(&self, collection: &str, value: &Value) -> Result<crate::draft::Draft> {
        let c = self.config().collection(collection)?;
        let projected = self.view_value(collection, value)?;
        let mut draft = crate::draft::Draft::new(&c.editor_schema, &projected);
        for field in &mut draft.fields {
            if let Some(via) = field
                .schema
                .pointer("/x-derived/via")
                .and_then(Value::as_str)
            {
                if value[via].as_str().is_some_and(|s| !s.is_empty()) {
                    field.read_only = true;
                }
            }
        }
        Ok(draft)
    }
    /// Frontmatter identity is supplied by the explicit record-ID operation, not a second input.
    pub fn draft_for_id(
        &self,
        collection: &str,
        id: &str,
        value: &Value,
    ) -> Result<crate::draft::Draft> {
        let mut draft = self.draft(collection, value)?;
        let c = self.config().collection(collection)?;
        if c.schema_path.is_some() {
            if let crate::IdStrategy::Frontmatter(key) = &c.id {
                if let Some(f) = draft.fields.iter_mut().find(|f| &f.key == key) {
                    f.raw = if f.kind == crate::draft::InputKind::Text {
                        id.into()
                    } else {
                        json!(id).to_string()
                    };
                    f.present = !id.is_empty();
                    f.read_only = true;
                }
            }
        }
        Ok(draft)
    }
    pub fn validate_for_save(
        &self,
        collection: &str,
        id: &str,
        value: Value,
        previous: Option<&Value>,
    ) -> Result<Vec<ValidationIssue>> {
        match self.prepare_save(collection, id, value, previous) {
            Ok(value) => self.validate(collection, &value),
            Err(Error::Validation(issues)) => Ok(issues),
            Err(e) => Err(e),
        }
    }
    pub(crate) fn prepare_save(
        &self,
        collection: &str,
        id: &str,
        mut value: Value,
        previous: Option<&Value>,
    ) -> Result<Value> {
        let c = self.config().collection(collection)?;
        let original = previous
            .cloned()
            .unwrap_or(self.initial_value(collection, &json!({}))?);
        let mut issues = Vec::new();
        if let Some(props) = c.schema_value["properties"].as_object() {
            for (k, s) in props {
                if k == "_editor"
                    || (c.schema_path.is_some()
                        && matches!(&c.id, crate::IdStrategy::Frontmatter(field) if field == k))
                {
                    continue;
                }
                ownership(
                    &c.schema_value,
                    s,
                    original.get(k),
                    value.get(k),
                    &format!("/{k}"),
                    &mut issues,
                );
            }
        }
        if !issues.is_empty() {
            return Err(Error::Validation(issues));
        }
        if let Some(props) = c.schema_value["properties"].as_object() {
            for (k, s) in props {
                if let Some(via) = s.pointer("/x-derived/via").and_then(Value::as_str) {
                    if value[via].as_str().is_some_and(|s| !s.is_empty()) {
                        if let Some(o) = value.as_object_mut() {
                            o.remove(k);
                        }
                    }
                }
            }
        }
        if c.schema_path.is_some() {
            if let crate::IdStrategy::Frontmatter(field) = &c.id {
                value[field] = json!(id);
            }
        }
        if c.schema_path.is_some() {
            if value.get("_editor").is_some_and(|v| !v.is_object()) {
                return Err(Error::Data("_editor must be an object".into()));
            }
            let requested = value.pointer("/_editor/schema_revision");
            let old = previous.and_then(|p| p.pointer("/_editor/schema_revision"));
            if requested.is_some() && requested != old {
                return Err(Error::Data(
                    "Schema revision is managed by the editor".into(),
                ));
            }
            if let Some(revision) = self.schema_revision(collection)? {
                if value.get("_editor").is_none() {
                    value["_editor"] = json!({});
                }
                value["_editor"]["schema_revision"] = json!(revision);
            } else if let Some(metadata) = value.get_mut("_editor").and_then(Value::as_object_mut) {
                metadata.remove("schema_revision");
            }
        }
        Ok(value)
    }
    pub fn label(&self, collection: &str, value: &Value, id: &str, view: &str) -> Result<String> {
        let v = self.projected_value(collection, value)?;
        self.label_projected(collection, &v, id, view)
    }
    pub(crate) fn label_projected(
        &self,
        collection: &str,
        v: &Value,
        id: &str,
        view: &str,
    ) -> Result<String> {
        let c = self.config().collection(collection)?;
        if let Some(fields) =
            c.ui.pointer(&format!("/labels/{view}"))
                .and_then(Value::as_array)
        {
            let parts: Vec<String> = fields
                .iter()
                .filter_map(Value::as_str)
                .filter_map(|k| {
                    if k.starts_with('/') {
                        v.pointer(k)
                    } else {
                        k.split('.').try_fold(v, |value, part| value.get(part))
                    }
                })
                .map(|v| {
                    v.as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| v.to_string())
                })
                .map(|s| crate::documents::plain_markdown(&s))
                .filter(|s| !s.trim().is_empty())
                .collect();
            if !parts.is_empty() {
                return Ok(parts.join(" · ").chars().take(150).collect());
            }
        }
        Ok(crate::documents::display_label(&v, c, &c.display, id))
    }
    pub fn contextual_values(&self, collection: &str, path: &[String]) -> Result<Value> {
        let c = self.config().collection(collection)?;
        if path.len() > c.categories.len() {
            return Err(Error::Data("Invalid tree path".into()));
        }
        let mut values = json!({});
        for (level, value) in c.categories.iter().zip(path) {
            values[&level.field] = json!(value);
        }
        self.initial_value(collection, &values)
    }
    pub fn suggestions(
        &self,
        collection: &str,
        field: &str,
        context: &Value,
    ) -> Result<Vec<String>> {
        self.suggestions_query(collection, field, context, "")
    }
    pub fn suggestions_query(
        &self,
        collection: &str,
        field: &str,
        context: &Value,
        query: &str,
    ) -> Result<Vec<String>> {
        let c = self.config().collection(collection)?;
        let s = &c.editor_schema["properties"][field];
        let scope: BTreeMap<String, String> = s
            .pointer("/x-autocomplete/scope")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter_map(|k| {
                context
                    .get(k)
                    .and_then(Value::as_str)
                    .map(|v| (k.into(), v.to_owned()))
            })
            .collect();
        let mut values = if s["x-autocomplete"] == false {
            Vec::new()
        } else {
            self.field_values(collection, field, &scope, query, 100)
                .unwrap_or_default()
        };
        if let Some(options) = s["x-options"].as_array() {
            values.extend(options.iter().filter_map(Value::as_str).map(str::to_owned));
        }
        values.retain(|value| value.to_lowercase().contains(&query.to_lowercase()));
        values.sort_by(|a, b| crate::alphabetic_cmp(a, b).then(a.cmp(b)));
        values.dedup();
        Ok(values)
    }
}

/// Positions use browser UTF-16 offsets. Only the selected range changes.
pub fn insert_markdown_link(
    source: &str,
    from: usize,
    to: usize,
    url: &str,
    label: &str,
    image: bool,
) -> Result<String> {
    if url.trim().is_empty()
        || !(url.starts_with('/') && !url.starts_with("//")
            || url.starts_with("https://")
            || url.starts_with("http://")
            || url.starts_with("mailto:"))
    {
        return Err(Error::Data(
            "Choose a relative, HTTP(S), or mailto URL".into(),
        ));
    }
    fn byte(s: &str, offset: usize) -> Option<usize> {
        let mut n = 0;
        for (i, c) in s.char_indices() {
            if n == offset {
                return Some(i);
            }
            n += c.len_utf16();
        }
        if n == offset { Some(s.len()) } else { None }
    }
    let from = byte(source, from).ok_or_else(|| Error::Data("Invalid text selection".into()))?;
    let to = byte(source, to)
        .filter(|t| *t >= from)
        .ok_or_else(|| Error::Data("Invalid text selection".into()))?;
    let label = if label.is_empty() {
        &source[from..to]
    } else {
        label
    };
    let label = label
        .replace('\\', "\\\\")
        .replace('[', "\\[")
        .replace(']', "\\]")
        .replace('\n', " ");
    let url = url
        .replace(' ', "%20")
        .replace('(', "%28")
        .replace(')', "%29")
        .replace('\n', "%0A")
        .replace('\r', "%0D");
    Ok(format!(
        "{}{}[{}]({}){}",
        &source[..from],
        if image { "!" } else { "" },
        label,
        url,
        &source[to..]
    ))
}
