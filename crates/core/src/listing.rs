//! Frontend-independent collection queries: values, matches, typed filters and ordering.
use super::*;
#[derive(Clone, Debug, Default, Serialize)]
pub struct ListRequest {
    /// Restrict a child-list view to the parent's canonical membership.
    pub ids: Option<std::collections::BTreeSet<String>>,
    pub query: String,
    pub category: Vec<String>,
    pub sort: String,
    pub descending: bool,
    pub filters: BTreeMap<String, String>,
    pub offset: usize,
    pub limit: usize,
}
#[derive(Clone, Debug)]
pub struct ListMatch {
    pub field: String,
    pub text: String,
}
#[derive(Clone, Debug)]
pub struct ListRow {
    pub id: String,
    pub label: String,
    pub value: Value,
    pub display: BTreeMap<String, String>,
    pub matches: Vec<ListMatch>,
}
#[derive(Clone, Debug)]
pub struct ListFacet {
    pub field: String,
    pub label: String,
    pub kind: String,
    pub options: Vec<(String, String, usize)>,
}
#[derive(Clone)]
pub struct ListPage {
    pub rows: Vec<ListRow>,
    pub total: usize,
    pub matched: usize,
    pub facets: Vec<ListFacet>,
}
pub fn display_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => "null".into(),
        Value::Array(v) => v.iter().map(display_text).collect::<Vec<_>>().join(" · "),
        _ => value.to_string(),
    }
}

fn field_display(value: &Value, schema: &Value) -> String {
    if let Some(items) = value.as_array() {
        return items
            .iter()
            .map(|item| field_display(item, &schema["items"]))
            .collect::<Vec<_>>()
            .join(" · ");
    }
    let text = display_text(value);
    if value.is_string()
        && ["x-content", "x-editor"]
            .iter()
            .any(|key| matches!(schema[key].as_str(), Some("markdown" | "inline-markdown")))
    {
        plain_markdown(&text)
    } else {
        text
    }
}
impl Editor {
    pub fn query_collection(&self, name: &str, request: &ListRequest) -> Result<ListPage> {
        self.query_collection_with_ui(name, request, &self.config.collection(name)?.ui)
    }

    /// Query using the resolved presentation of this particular list occurrence.
    pub fn query_collection_with_ui(
        &self,
        name: &str,
        request: &ListRequest,
        ui: &Value,
    ) -> Result<ListPage> {
        self.require_operation(name, "read")?;
        let signature = self.read_signature()?;
        let mut query = request.clone();
        query.offset = 0;
        query.limit = 0;
        let key =
            serde_json::to_string(&(name, &query, ui)).map_err(|e| Error::Data(e.to_string()))?;
        let cached = self
            .read_cache
            .lists
            .lock()
            .unwrap()
            .iter()
            .find(|(old, k, _)| old == &signature && k == &key)
            .map(|(_, _, page)| page.clone());
        if let Some(page) = cached {
            return Ok(page_slice(&page, request));
        }
        let c = self.config.collection(name)?;
        if request.category.len() > c.categories.len() {
            return Err(Error::Data("category path is too deep".into()));
        }
        let props = c.editor_schema["properties"]
            .as_object()
            .cloned()
            .unwrap_or_default();
        let visible = |key: &str| {
            props.get(key).is_some_and(|s| {
                key != "_editor" && s["x-editor"] != "hidden" && s["x-visible"] != false
            })
        };
        if !request.sort.is_empty()
            && !["_label", "_id", "_manual"].contains(&request.sort.as_str())
            && !visible(&request.sort)
        {
            return Err(Error::Data("Unknown sort column".into()));
        }
        let mut facet_fields: Vec<String> = ui
            .pointer("/list/filters")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .filter(|s| visible(s))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        if ui.pointer("/list/autoFilters") == Some(&Value::Bool(true)) {
            for (key, s) in &props {
                if visible(key)
                    && matches!(
                        s["type"].as_str(),
                        Some("string" | "integer" | "number" | "boolean")
                    )
                    && !matches!(s["x-editor"].as_str(), Some("markdown" | "html"))
                    && !facet_fields.contains(key)
                {
                    facet_fields.push(key.clone());
                }
            }
        }
        for key in request.filters.keys() {
            let key = key
                .strip_suffix(".min")
                .or_else(|| key.strip_suffix(".max"))
                .unwrap_or(key);
            if !facet_fields.iter().any(|f| f == key) {
                return Err(Error::Data(format!("Unknown filter {key}")));
            }
        }
        let q = request.query.trim().to_lowercase();
        let mut rows = Vec::new();
        let prepared_key = serde_json::to_string(&(name, &request.ids, &request.category))
            .map_err(|e| Error::Data(e.to_string()))?;
        let cached = self
            .read_cache
            .prepared
            .lock()
            .unwrap()
            .iter()
            .find(|(old, k, _)| old == &signature && k == &prepared_key)
            .map(|(_, _, rows)| rows.clone());
        let prepared = if let Some(rows) = cached {
            rows
        } else {
            let mut prepared = Vec::new();
            let mut ids = HashSet::new();
            let mut relations = HashMap::<(String, String), String>::new();
            for path in self.files(name)? {
                // Filename IDs allow a child list to avoid loading unrelated records.
                if matches!(c.id, IdStrategy::Filename)
                    && request.ids.as_ref().is_some_and(|ids| {
                        !path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .is_some_and(|id| ids.contains(id))
                    })
                {
                    continue;
                }
                let doc = self.cached_document(name, &path)?;
                let id = doc.id.clone();
                if request.ids.as_ref().is_some_and(|ids| !ids.contains(&id)) {
                    continue;
                }
                let value = self.view_value(name, &doc.value)?;
                if !ids.insert(id.clone()) {
                    return Err(Error::Data(format!("duplicate ID {id}")));
                }
                if !request
                    .category
                    .iter()
                    .zip(&c.categories)
                    .all(|(expected, level)| {
                        field(&value, &level.field).and_then(Value::as_str) == Some(expected)
                    })
                {
                    continue;
                }
                let mut display = BTreeMap::new();
                for (key, schema) in &props {
                    if !visible(key) {
                        continue;
                    }
                    if let Some(v) = value.get(key) {
                        let mut text = field_display(v, schema);
                        if schema["x-children"] == true {
                            text = v.as_array().map(|a| a.len()).unwrap_or(0).to_string();
                        }
                        if let (Some(target), Some(id)) = (
                            schema
                                .pointer("/x-relation/collection")
                                .and_then(Value::as_str),
                            v.as_str(),
                        ) {
                            let k = (target.to_owned(), id.to_owned());
                            text = relations
                                .entry(k)
                                .or_insert_with(|| {
                                    self.get(target, id)
                                        .ok()
                                        .and_then(|d| {
                                            self.label(target, &d.value, id, "reference").ok()
                                        })
                                        .unwrap_or_else(|| id.into())
                                })
                                .clone();
                        }
                        display.insert(key.clone(), text);
                    }
                }
                let label = self.label(name, &value, &id, "list")?;
                prepared.push(ListRow {
                    id,
                    label,
                    value,
                    display,
                    matches: Vec::new(),
                });
            }
            let prepared = Arc::new(prepared);
            let mut cache = self.read_cache.prepared.lock().unwrap();
            cache.retain(|(old, k, _)| old == &signature && k != &prepared_key);
            if cache.len() >= 16 {
                cache.remove(0);
            }
            cache.push((signature.clone(), prepared_key, prepared.clone()));
            prepared
        };
        let total = prepared.len();
        let mut facet_values: BTreeMap<String, BTreeMap<String, (String, usize)>> = BTreeMap::new();
        for row in prepared.iter() {
            let ListRow {
                id,
                label,
                value,
                display,
                ..
            } = row;
            for key in &facet_fields {
                if let Some(v) = value.get(key) {
                    let raw = display_text(v);
                    let label = display.get(key).cloned().unwrap_or_else(|| raw.clone());
                    let entry = facet_values
                        .entry(key.clone())
                        .or_default()
                        .entry(raw)
                        .or_insert((label, 0));
                    entry.1 += 1;
                }
            }
            let mut matches = Vec::new();
            if !q.is_empty() {
                if id.to_lowercase().contains(&q) {
                    matches.push(ListMatch {
                        field: "_id".into(),
                        text: id.clone(),
                    });
                }
                for key in &c.search {
                    if !visible(key) {
                        continue;
                    }
                    if let Some(v) = display
                        .get(key)
                        .cloned()
                        .or_else(|| field(&value, key).map(display_text))
                    {
                        if v.to_lowercase().contains(&q) {
                            matches.push(ListMatch {
                                field: key.clone(),
                                text: v,
                            });
                        }
                    }
                }
                if matches.is_empty() && !label.to_lowercase().contains(&q) {
                    continue;
                }
            }
            let mut row = row.clone();
            row.matches = matches;
            rows.push(row);
        }
        rows.retain(|row| {
            request.filters.iter().all(|(key, expected)| {
                let value = &row.value;
                if expected.is_empty() {
                    return true;
                }
                if let Some(key) = key.strip_suffix(".min") {
                    return field(value, key)
                        .and_then(Value::as_f64)
                        .zip(expected.parse::<f64>().ok())
                        .is_some_and(|(v, n)| v >= n);
                }
                if let Some(key) = key.strip_suffix(".max") {
                    return field(value, key)
                        .and_then(Value::as_f64)
                        .zip(expected.parse::<f64>().ok())
                        .is_some_and(|(v, n)| v <= n);
                }
                let Some(v) = field(value, key) else {
                    return expected == "__missing";
                };
                if expected == "__missing" {
                    return false;
                }
                let is_text = props[key]["type"] == "string"
                    && facet_values.get(key).is_some_and(|v| v.len() > 40)
                    && props[key].get("x-relation").is_none()
                    && props[key].get("enum").is_none();
                if is_text {
                    row.display
                        .get(key)
                        .is_some_and(|s| s.to_lowercase().contains(&expected.to_lowercase()))
                } else {
                    display_text(v) == *expected
                }
            })
        });
        let facets = facet_fields
            .into_iter()
            .map(|key| {
                let options = facet_values.remove(&key).unwrap_or_default();
                let s = &props[&key];
                let kind = if matches!(s["type"].as_str(), Some("integer" | "number")) {
                    "number"
                } else if s["type"] == "boolean" {
                    "boolean"
                } else if options.len() <= 40
                    || s.get("x-relation").is_some()
                    || s.get("enum").is_some()
                {
                    "select"
                } else {
                    "text"
                };
                let mut options: Vec<_> =
                    options.into_iter().map(|(v, (l, n))| (v, l, n)).collect();
                options.sort_by(|a, b| crate::alphabetic_cmp(&a.1, &b.1).then(a.0.cmp(&b.0)));
                ListFacet {
                    label: s["title"].as_str().unwrap_or(&key).into(),
                    field: key,
                    kind: kind.into(),
                    options,
                }
            })
            .collect();
        let sort = request.sort.as_str();
        rows.sort_by(|a, b| {
            let order = match sort {
                "_id" => crate::alphabetic_cmp(&a.id, &b.id),
                "_manual" => a
                    .value
                    .pointer("/_editor/manual_order")
                    .and_then(Value::as_i64)
                    .unwrap_or(i64::MAX)
                    .cmp(
                        &b.value
                            .pointer("/_editor/manual_order")
                            .and_then(Value::as_i64)
                            .unwrap_or(i64::MAX),
                    ),
                "" | "_label" => crate::alphabetic_cmp(&a.label, &b.label),
                key => match (field(&a.value, key), field(&b.value, key)) {
                    (None, None) => std::cmp::Ordering::Equal,
                    (None, _) => return std::cmp::Ordering::Greater,
                    (_, None) => return std::cmp::Ordering::Less,
                    (Some(Value::Array(a)), Some(Value::Array(b)))
                        if props[key]["x-children"] == true =>
                    {
                        a.len().cmp(&b.len())
                    }
                    (Some(Value::Number(a)), Some(Value::Number(b))) => a
                        .as_f64()
                        .partial_cmp(&b.as_f64())
                        .unwrap_or(std::cmp::Ordering::Equal),
                    _ => crate::alphabetic_cmp(
                        a.display.get(key).map(String::as_str).unwrap_or(""),
                        b.display.get(key).map(String::as_str).unwrap_or(""),
                    ),
                },
            };
            (if request.descending {
                order.reverse()
            } else {
                order
            })
            .then(a.id.cmp(&b.id))
        });
        let matched = rows.len();
        let page = Arc::new(ListPage {
            rows,
            total,
            matched,
            facets,
        });
        let mut cache = self.read_cache.lists.lock().unwrap();
        cache.retain(|(old, k, _)| old == &signature && k != &key);
        if cache.len() >= 16 {
            cache.remove(0);
        }
        cache.push((signature, key, page.clone()));
        Ok(page_slice(&page, request))
    }
}
fn page_slice(page: &ListPage, request: &ListRequest) -> ListPage {
    ListPage {
        rows: page
            .rows
            .iter()
            .skip(request.offset.min(page.matched))
            .take(request.limit.clamp(1, 200))
            .cloned()
            .collect(),
        total: page.total,
        matched: page.matched,
        facets: page.facets.clone(),
    }
}
