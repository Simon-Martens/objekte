//! Frontend-independent collection queries: values, matches, typed filters and ordering.
use super::*;
use structured_editor_search::{MatchRange, SearchMatcher, SearchOptions};
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct ListRequest {
    /// Restrict a child-list view to the parent's canonical membership.
    pub ids: Option<std::collections::BTreeSet<String>>,
    pub query: String,
    pub search_options: SearchOptions,
    /// Optional relation-picker field overrides, using the same normalized values.
    pub search_fields: Option<Vec<String>>,
    pub display_field: Option<String>,
    /// Optional table projection. Search, labels, filters and sort fields are
    /// retained automatically; omitted means return all visible fields.
    pub fields: Option<BTreeSet<String>>,
    pub label_view: String,
    pub category: Vec<String>,
    pub sort: String,
    pub descending: bool,
    pub filters: BTreeMap<String, String>,
    pub offset: usize,
    pub limit: usize,
}
#[derive(Clone, Debug, Serialize)]
pub struct ListMatch {
    pub field: String,
    pub text: String,
    pub ranges: Vec<MatchRange>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ListRow {
    pub id: String,
    pub label: String,
    pub value: Value,
    pub display: BTreeMap<String, String>,
    pub matches: Vec<ListMatch>,
    pub highlights: BTreeMap<String, Vec<MatchRange>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct ListFacet {
    pub field: String,
    pub label: String,
    pub kind: String,
    pub options: Vec<(String, String, usize)>,
}
#[derive(Clone, Serialize)]
pub struct ListPage {
    pub rows: Vec<ListRow>,
    pub total: usize,
    pub matched: usize,
    pub facets: Vec<ListFacet>,
}
pub(super) struct PreparedEntry {
    id: Option<String>,
    pub row: Option<Arc<ListRow>>,
    dependencies: BTreeSet<(String, String)>,
}
pub(super) struct PreparedList {
    key: String,
    pub rows: Vec<Arc<ListRow>>,
    pub by_path: HashMap<PathBuf, PreparedEntry>,
    pub path_ids: HashMap<PathBuf, String>,
    facets: Mutex<Vec<(String, Arc<Vec<ListFacet>>)>>,
    orders: Mutex<Vec<(String, Arc<Vec<usize>>)>>,
}
pub(super) struct CachedListPage {
    prepared: Arc<PreparedList>,
    rows: Vec<usize>,
    total: usize,
    matched: usize,
    facets: Arc<Vec<ListFacet>>,
    matcher: Arc<SearchMatcher>,
    search_fields: Vec<String>,
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
    fn list_row_dependencies(
        &self,
        collection: &Collection,
        raw: &Value,
        projected: &Value,
        properties: &serde_json::Map<String, Value>,
        visible: &impl Fn(&str) -> bool,
    ) -> Result<BTreeSet<(String, String)>> {
        let mut dependencies = derived_relation_ids(collection, raw);
        for (key, schema) in properties {
            if !visible(key) || schema["x-children"] == true {
                continue;
            }
            let (relation_schema, values): (&Value, Vec<&Value>) =
                if let Some(values) = projected[key].as_array() {
                    (&schema["items"], values.iter().collect())
                } else {
                    (schema, vec![&projected[key]])
                };
            let Some(target) = relation_schema
                .pointer("/x-relation/collection")
                .and_then(Value::as_str)
            else {
                continue;
            };
            for id in values.into_iter().filter_map(Value::as_str) {
                dependencies.insert((target.into(), id.into()));
                if let Ok(target_doc) = self.get(target, id) {
                    let target_collection = self.config.collection(target)?;
                    dependencies.extend(derived_relation_ids(target_collection, &target_doc.value));
                }
            }
        }
        Ok(dependencies)
    }

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
        let matcher = self.search_matcher(&request.query, request.search_options)?;
        let preparation = self
            .read_cache
            .preparation
            .lock()
            .unwrap()
            .entry((name.to_owned(), request.ids.is_some()))
            .or_default()
            .clone();
        let _preparing = preparation.lock().unwrap();
        let signature = self.list_signature(name, request.ids.as_ref())?;
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

        let raw_selected = request.fields.as_ref().map(|fields| {
            let mut fields = fields.clone();
            fields.extend(facet_fields.iter().cloned());
            fields.extend(c.categories.iter().map(|category| category.field.clone()));
            fields.insert(request.sort.clone());
            // Manual ordering and row actions need the metadata value.
            fields.insert("_editor".into());
            fields
                .into_iter()
                .map(|field| field.split('.').next().unwrap_or("").to_owned())
                .collect::<BTreeSet<_>>()
        });
        let selected = raw_selected.as_ref().map(|fields| {
            let mut fields = fields.clone();
            fields.extend(
                request
                    .search_fields
                    .as_ref()
                    .unwrap_or(&c.search)
                    .iter()
                    .map(|field| field.split('.').next().unwrap_or("").to_owned()),
            );
            fields
        });
        let prepared_key = serde_json::to_string(&(
            name,
            &request.ids,
            &request.search_fields,
            &request.display_field,
            &request.label_view,
            &selected,
            &raw_selected,
        ))
        .map_err(|e| Error::Data(e.to_string()))?;
        let previous = self
            .read_cache
            .prepared
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|(_, k, _)| k == &prepared_key)
            .map(|(old, _, rows)| (old.clone(), rows.clone()));
        let cached = previous
            .as_ref()
            .and_then(|(old, rows)| (old == &signature).then(|| rows.clone()));
        let prepared = if let Some(rows) = cached {
            rows
        } else {
            let mut prepared = PreparedList {
                key: prepared_key.clone(),
                rows: Vec::new(),
                by_path: HashMap::new(),
                path_ids: previous
                    .as_ref()
                    .map(|(_, rows)| rows.path_ids.clone())
                    .unwrap_or_default(),
                facets: Mutex::new(Vec::new()),
                orders: Mutex::new(Vec::new()),
            };
            let mut ids = HashSet::new();
            let mut relations = HashMap::<(String, String), String>::new();
            let changed_ids = if let Some((old, rows)) = &previous {
                changed_record_ids(self, name, old, &signature, &rows.path_ids)?
            } else {
                BTreeSet::new()
            };
            let source_changes: BTreeSet<_> = previous
                .as_ref()
                .map(|(old, _)| {
                    changed_paths(&old.source, &signature.source)
                        .into_iter()
                        .map(|(path, _)| path)
                        .collect()
                })
                .unwrap_or_default();
            for path in &source_changes {
                prepared.path_ids.remove(*path);
            }
            let has_derived = c.schema_value["properties"]
                .as_object()
                .is_some_and(|props| {
                    props
                        .values()
                        .any(|schema| schema.get("x-derived").is_some())
                });
            let has_relations = props.values().any(|schema| {
                schema.get("x-relation").is_some() || schema["items"].get("x-relation").is_some()
            });
            let display_fields: Vec<_> = props
                .iter()
                .filter(|(key, _)| {
                    visible(key) && selected.as_ref().is_none_or(|fields| fields.contains(*key))
                })
                .collect();
            let build = |path: &Path,
                         relations: &mut HashMap<(String, String), String>|
             -> Result<PreparedEntry> {
                let doc = self.cached_document(name, path)?;
                let id = doc.id.clone();
                if request.ids.as_ref().is_some_and(|ids| !ids.contains(&id)) {
                    return Ok(PreparedEntry {
                        id: None,
                        row: None,
                        dependencies: BTreeSet::new(),
                    });
                }
                let projected = if has_derived {
                    self.projected_value(name, &doc.value)?
                } else {
                    std::borrow::Cow::Borrowed(&doc.value)
                };
                let label = if let Some(display) = &request.display_field {
                    display_label(&projected, c, display, &id)
                } else {
                    self.label_projected(
                        name,
                        &projected,
                        &id,
                        if request.label_view.is_empty() {
                            "list"
                        } else {
                            &request.label_view
                        },
                    )?
                };
                let mut display = BTreeMap::new();
                for &(key, schema) in &display_fields {
                    if let Some(v) = projected.get(key) {
                        let mut text = if schema["x-children"] == true
                            || (schema["type"] == "array" && schema["items"]["type"] == "object")
                        {
                            v.as_array().map(|a| a.len()).unwrap_or(0).to_string()
                        } else {
                            field_display(v, schema)
                        };
                        let (relation_schema, values): (&Value, Vec<&Value>) =
                            if let Some(values) = v.as_array() {
                                (&schema["items"], values.iter().collect())
                            } else {
                                (schema, vec![v])
                            };
                        if schema["x-children"] != true {
                            if let Some(target) = relation_schema
                                .pointer("/x-relation/collection")
                                .and_then(Value::as_str)
                            {
                                text = values
                                    .into_iter()
                                    .filter_map(Value::as_str)
                                    .map(|id| {
                                        relations
                                            .entry((target.to_owned(), id.to_owned()))
                                            .or_insert_with(|| {
                                                self.get(target, id)
                                                    .ok()
                                                    .and_then(|doc| {
                                                        self.label(
                                                            target,
                                                            &doc.value,
                                                            id,
                                                            "reference",
                                                        )
                                                        .ok()
                                                    })
                                                    .unwrap_or_else(|| id.into())
                                            })
                                            .clone()
                                    })
                                    .collect::<Vec<_>>()
                                    .join(" · ");
                            }
                        }
                        display.insert(key.clone(), text);
                    }
                }
                for key in request.search_fields.as_ref().unwrap_or(&c.search) {
                    if !display.contains_key(key) {
                        if let (Some(v), Some(schema)) =
                            (field(&projected, key), field_schema(&c.editor_schema, key))
                        {
                            display.insert(key.clone(), field_display(v, schema));
                        }
                    }
                }
                let dependencies = if has_derived || has_relations {
                    self.list_row_dependencies(c, &doc.value, &projected, &props, &|key| {
                        visible(key) && selected.as_ref().is_none_or(|fields| fields.contains(key))
                    })?
                } else {
                    BTreeSet::new()
                };
                // Search-only fields live in their normalized display form, so
                // a large text is not retained a second time in the raw row.
                let value = if let (Some(selected), Some(object)) =
                    (&raw_selected, projected.as_object())
                {
                    Value::Object(
                        object
                            .iter()
                            .filter(|(key, _)| selected.contains(*key))
                            .map(|(key, value)| (key.clone(), value.clone()))
                            .collect(),
                    )
                } else {
                    projected.into_owned()
                };
                Ok(PreparedEntry {
                    id: Some(id.clone()),
                    row: Some(Arc::new(ListRow {
                        id,
                        label,
                        value,
                        display,
                        matches: Vec::new(),
                        highlights: BTreeMap::new(),
                    })),
                    dependencies,
                })
            };
            // Only cold, large indexes use workers; small reads and incremental
            // edits stay on this thread. Preserve source order and bounded concurrency.
            let mut parallel = if previous.is_none() && signature.source.len() >= 4096 {
                let paths: Vec<_> = signature
                    .source
                    .iter()
                    .filter(|(_, stamp)| stamp.is_some())
                    .map(|(path, _)| path)
                    .collect();
                let workers = std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(1)
                    .min(4);
                let entries =
                    std::thread::scope(|scope| -> Result<Vec<PreparedEntry>> {
                        let handles: Vec<_> = paths
                            .chunks(paths.len().div_ceil(workers).max(1))
                            .map(|chunk| {
                                let build = &build;
                                scope.spawn(move || {
                                    let mut relations = HashMap::new();
                                    chunk
                                        .iter()
                                        .map(|path| build(path, &mut relations))
                                        .collect::<Result<Vec<_>>>()
                                })
                            })
                            .collect();
                        let mut entries = Vec::with_capacity(paths.len());
                        for handle in handles {
                            entries.extend(handle.join().map_err(|_| {
                                Error::Data("list preparation worker failed".into())
                            })??);
                        }
                        Ok(entries)
                    })?;
                Some(entries.into_iter())
            } else {
                None
            };
            for (path, current_stamp) in &signature.source {
                if current_stamp.is_none() {
                    continue;
                }
                if let Some((_, previous_rows)) = &previous
                    && !source_changes.contains(path)
                    && let Some(entry) = previous_rows.by_path.get(path)
                    && entry.dependencies.is_disjoint(&changed_ids)
                {
                    if let Some(id) = &entry.id
                        && !ids.insert(id.clone())
                    {
                        return Err(Error::Data(format!("duplicate ID {id}")));
                    }
                    if let Some(row) = &entry.row {
                        prepared.rows.push(row.clone());
                    }
                    prepared.by_path.insert(
                        path.clone(),
                        PreparedEntry {
                            id: entry.id.clone(),
                            row: entry.row.clone(),
                            dependencies: entry.dependencies.clone(),
                        },
                    );
                    continue;
                }
                let entry = if let Some(entries) = &mut parallel {
                    entries.next().expect("one prepared entry per source file")
                } else {
                    build(path, &mut relations)?
                };
                if let Some(id) = &entry.id {
                    if !ids.insert(id.clone()) {
                        return Err(Error::Data(format!("duplicate ID {id}")));
                    }
                    prepared.path_ids.insert(path.clone(), id.clone());
                }
                for (target, id) in &entry.dependencies {
                    if let Ok(path) = self.find_path(target, id) {
                        prepared.path_ids.insert(path, id.clone());
                    }
                }
                if let Some(row) = &entry.row {
                    prepared.rows.push(row.clone());
                }
                prepared.by_path.insert(path.clone(), entry);
            }
            for (target, old_files) in previous
                .as_ref()
                .map(|(old, _)| &old.dependencies)
                .into_iter()
                .flatten()
            {
                if let Some(current) = signature.dependencies.get(target) {
                    for (path, stamp) in changed_paths(old_files, current) {
                        if stamp.is_none() {
                            prepared.path_ids.remove(path);
                        }
                    }
                }
            }
            let prepared = if let Some((_, previous)) = &previous
                && previous.rows.len() == prepared.rows.len()
                && previous
                    .rows
                    .iter()
                    .zip(&prepared.rows)
                    .all(|(a, b)| Arc::ptr_eq(a, b))
            {
                // Keep facet and order caches when a dependency changed but none
                // of these records displays it.
                previous.clone()
            } else {
                Arc::new(prepared)
            };
            // Old query pages must not pin an entire obsolete projection after
            // every edit in a large collection. Unchanged projections are shared.
            self.read_cache
                .lists
                .lock()
                .unwrap()
                .retain(|(_, _, page)| {
                    page.prepared.key != prepared_key || Arc::ptr_eq(&page.prepared, &prepared)
                });
            let mut cache = self.read_cache.prepared.lock().unwrap();
            cache.retain(|(_, k, _)| k != &prepared_key);
            if cache.len() >= 16 {
                cache.remove(0);
            }
            cache.push((signature.clone(), prepared_key, prepared.clone()));
            prepared
        };
        if let Some((old_signature, old_rows)) = &previous
            && old_rows.rows.len() == prepared.rows.len()
            && old_rows
                .rows
                .iter()
                .zip(&prepared.rows)
                .all(|(old, new)| Arc::ptr_eq(old, new))
        {
            let cached_page = self
                .read_cache
                .lists
                .lock()
                .unwrap()
                .iter()
                .find(|(old, cached_key, _)| old == old_signature && cached_key == &key)
                .map(|(_, _, page)| page.clone());
            if let Some(page) = cached_page {
                let mut cache = self.read_cache.lists.lock().unwrap();
                cache.retain(|(_, cached_key, _)| cached_key != &key);
                if cache.len() >= 16 {
                    cache.remove(0);
                }
                cache.push((signature, key, page.clone()));
                return Ok(page_slice(&page, request));
            }
        }
        let in_category = |row: &ListRow| {
            request
                .category
                .iter()
                .zip(&c.categories)
                .all(|(expected, level)| {
                    field(&row.value, &level.field).and_then(Value::as_str) == Some(expected)
                })
        };
        let total = if request.category.is_empty() {
            prepared.rows.len()
        } else {
            prepared.rows.iter().filter(|row| in_category(row)).count()
        };
        let facet_key = serde_json::to_string(&(&request.category, &facet_fields)).unwrap();
        let facets = {
            let mut cache = prepared.facets.lock().unwrap();
            if let Some((_, facets)) = cache.iter().find(|(key, _)| key == &facet_key) {
                facets.clone()
            } else {
                let mut values: BTreeMap<String, BTreeMap<String, (String, usize)>> =
                    BTreeMap::new();
                for row in prepared.rows.iter().filter(|row| in_category(row)) {
                    for key in &facet_fields {
                        if let Some(v) = row.value.get(key) {
                            let raw = display_text(v);
                            let label =
                                row.display.get(key).cloned().unwrap_or_else(|| raw.clone());
                            values
                                .entry(key.clone())
                                .or_default()
                                .entry(raw)
                                .or_insert((label, 0))
                                .1 += 1;
                        }
                    }
                }
                let facets = Arc::new(
                    facet_fields
                        .iter()
                        .map(|key| {
                            let options = values.remove(key).unwrap_or_default();
                            let schema = &props[key];
                            let kind =
                                if matches!(schema["type"].as_str(), Some("integer" | "number")) {
                                    "number"
                                } else if schema["type"] == "boolean" {
                                    "boolean"
                                } else if options.len() <= 40
                                    || schema.get("x-relation").is_some()
                                    || schema.get("enum").is_some()
                                {
                                    "select"
                                } else {
                                    "text"
                                };
                            let mut options: Vec<_> = options
                                .into_iter()
                                .map(|(v, (label, count))| (v, label, count))
                                .collect();
                            // Text filters only need their kind, not every distinct string.
                            if kind == "text" || kind == "number" {
                                options.clear();
                            } else {
                                options.sort_by(|a, b| {
                                    crate::alphabetic_cmp(&a.1, &b.1).then(a.0.cmp(&b.0))
                                });
                            }
                            ListFacet {
                                field: key.clone(),
                                label: schema["title"].as_str().unwrap_or(key).into(),
                                kind: kind.into(),
                                options,
                            }
                        })
                        .collect::<Vec<_>>(),
                );
                if cache.len() >= 16 {
                    cache.remove(0);
                }
                cache.push((facet_key, facets.clone()));
                facets
            }
        };
        let order_key = serde_json::to_string(&(
            &request.sort,
            request.descending,
            ui.pointer("/ordering/unordered"),
        ))
        .unwrap();
        let order = {
            let mut cache = prepared.orders.lock().unwrap();
            if let Some((_, order)) = cache.iter().find(|(key, _)| key == &order_key) {
                order.clone()
            } else {
                // ICU collation is comparatively expensive. Compute it once per
                // row instead of reinterpreting Unicode on every sort comparison.
                let text_keys: Vec<_> = prepared
                    .rows
                    .iter()
                    .map(|row| {
                        let text = match request.sort.as_str() {
                            "_id" => row.id.as_str(),
                            "" | "_label" | "_manual" => row.label.as_str(),
                            key => row.display.get(key).map(String::as_str).unwrap_or(""),
                        };
                        crate::collation::alphabetic_key(text)
                    })
                    .collect();
                let compare = |a: &usize, b: &usize| {
                    compare_rows(
                        &prepared.rows[*a],
                        &prepared.rows[*b],
                        request,
                        ui,
                        &props,
                        text_keys[*a].cmp(&text_keys[*b]),
                    )
                };
                let old_order = previous
                    .as_ref()
                    .filter(|(_, old)| !Arc::ptr_eq(old, &prepared))
                    .and_then(|(_, old)| {
                        old.orders
                            .lock()
                            .unwrap()
                            .iter()
                            .find(|(key, _)| key == &order_key)
                            .map(|(_, order)| (old.clone(), order.clone()))
                    });
                let mut order = if let Some((old, old_order)) = old_order {
                    let indexes: HashMap<_, _> = prepared
                        .rows
                        .iter()
                        .enumerate()
                        .map(|(i, row)| (row.id.as_str(), i))
                        .collect();
                    let mut retained = vec![false; prepared.rows.len()];
                    let mut order: Vec<_> = old_order
                        .iter()
                        .filter_map(|&i| {
                            let row = &old.rows[i];
                            let &index = indexes.get(row.id.as_str())?;
                            if Arc::ptr_eq(row, &prepared.rows[index]) {
                                retained[index] = true;
                                Some(index)
                            } else {
                                None
                            }
                        })
                        .collect();
                    let changed: Vec<_> = retained
                        .iter()
                        .enumerate()
                        .filter_map(|(i, keep)| (!keep).then_some(i))
                        .collect();
                    if changed.len() <= (prepared.rows.len() / 16).max(8) {
                        for index in changed {
                            let pos = order
                                .binary_search_by(|other| compare(other, &index))
                                .unwrap_or_else(|pos| pos);
                            order.insert(pos, index);
                        }
                    } else {
                        order.extend(changed);
                        order.sort_by(compare);
                    }
                    order
                } else {
                    let mut order: Vec<_> = (0..prepared.rows.len()).collect();
                    order.sort_by(compare);
                    order
                };
                order.shrink_to_fit();
                let order = Arc::new(order);
                if cache.len() >= 8 {
                    cache.remove(0);
                }
                cache.push((order_key, order.clone()));
                order
            }
        };
        let filters = request
            .filters
            .iter()
            .filter(|(_, v)| !v.is_empty())
            .map(|(key, expected)| {
                let filter = if let Some(key) = key.strip_suffix(".min") {
                    RowFilter::Min(key.into(), expected.parse().ok())
                } else if let Some(key) = key.strip_suffix(".max") {
                    RowFilter::Max(key.into(), expected.parse().ok())
                } else if expected == "__missing" {
                    RowFilter::Missing(key.clone())
                } else if facets.iter().any(|f| &f.field == key && f.kind == "text") {
                    RowFilter::Text(
                        key.clone(),
                        self.search_matcher(expected, SearchOptions::default())?,
                    )
                } else {
                    RowFilter::Equal(key.clone(), expected.clone())
                };
                Ok(filter)
            })
            .collect::<Result<Vec<_>>>()?;
        let search_fields = request.search_fields.as_ref().unwrap_or(&c.search).clone();
        let rows = order
            .iter()
            .copied()
            .filter(|&index| {
                let row = &prepared.rows[index];
                in_category(row)
                    && filters.iter().all(|filter| filter.matches(row))
                    && (!matcher.is_active()
                        || matcher.is_match(&row.id)
                        || matcher.is_match(&row.label)
                        || search_fields.iter().any(|key| {
                            row.display
                                .get(key)
                                .is_some_and(|text| matcher.is_match(text))
                        }))
            })
            .collect::<Vec<_>>();
        let matched = rows.len();
        let page = Arc::new(CachedListPage {
            prepared,
            rows,
            total,
            matched,
            facets,
            matcher,
            search_fields,
        });
        let mut cache = self.read_cache.lists.lock().unwrap();
        cache.retain(|(_, k, _)| k != &key);
        if cache.len() >= 16 {
            cache.remove(0);
        }
        cache.push((signature, key, page.clone()));
        Ok(page_slice(&page, request))
    }
}
pub(super) fn derived_relation_ids(
    collection: &Collection,
    value: &Value,
) -> BTreeSet<(String, String)> {
    let mut result = BTreeSet::new();
    if let Some(properties) = collection.schema_value["properties"].as_object() {
        for schema in properties.values() {
            let Some(via) = schema.pointer("/x-derived/via").and_then(Value::as_str) else {
                continue;
            };
            if let (Some(target), Some(id)) = (
                properties
                    .get(via)
                    .and_then(|s| s.pointer("/x-relation/collection"))
                    .and_then(Value::as_str),
                value
                    .get(via)
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty()),
            ) {
                result.insert((target.into(), id.into()));
            }
        }
    }
    result
}

fn changed_record_ids(
    editor: &Editor,
    source: &str,
    old: &read_cache::ListSignature,
    current: &read_cache::ListSignature,
    old_path_ids: &HashMap<PathBuf, String>,
) -> Result<BTreeSet<(String, String)>> {
    let mut changed = BTreeSet::new();
    let names: BTreeSet<_> = std::iter::once(source.to_owned())
        .chain(old.dependencies.keys().cloned())
        .chain(current.dependencies.keys().cloned())
        .collect();
    for name in names {
        let old_files = if name == source {
            Some(&old.source)
        } else {
            old.dependencies.get(&name)
        };
        let new_files = if name == source {
            Some(&current.source)
        } else {
            current.dependencies.get(&name)
        };
        let empty = read_cache::Signature::default();
        let old_files = old_files.unwrap_or(&empty);
        let new_files = new_files.unwrap_or(&empty);
        let collection = editor.config.collection(&name)?;
        for (path, new_stamp) in changed_paths(old_files, new_files) {
            let old_stamp = old_files
                .binary_search_by(|(p, _)| p.cmp(path))
                .ok()
                .and_then(|i| old_files[i].1.as_ref());
            if old_stamp.is_some() {
                let id = old_path_ids.get(path).cloned().or_else(|| {
                    matches!(collection.id, IdStrategy::Filename)
                        .then(|| path.file_stem()?.to_str().map(str::to_owned))
                        .flatten()
                });
                if let Some(id) = id {
                    changed.insert((name.clone(), id));
                }
            }
            if new_stamp.is_some() {
                let id = if matches!(collection.id, IdStrategy::Filename) {
                    path.file_stem()
                        .and_then(|stem| stem.to_str())
                        .map(str::to_owned)
                } else {
                    Some(editor.cached_document(&name, path)?.id.clone())
                };
                if let Some(id) = id {
                    changed.insert((name.clone(), id));
                }
            }
        }
    }
    Ok(changed)
}

fn page_slice(page: &CachedListPage, request: &ListRequest) -> ListPage {
    ListPage {
        rows: page
            .rows
            .iter()
            .skip(request.offset.min(page.matched))
            .take(request.limit.clamp(1, 200))
            .map(|&index| {
                let mut row = page.prepared.rows[index].as_ref().clone();
                if page.matcher.is_active() {
                    for (key, text) in std::iter::once(("_id", row.id.as_str()))
                        .chain(std::iter::once(("_label", row.label.as_str())))
                        .chain(
                            row.display
                                .iter()
                                .map(|(key, text)| (key.as_str(), text.as_str())),
                        )
                    {
                        let ranges = page.matcher.ranges(text);
                        if !ranges.is_empty() {
                            if key == "_id" || page.search_fields.iter().any(|field| field == key) {
                                row.matches.push(ListMatch {
                                    field: key.into(),
                                    text: text.into(),
                                    ranges: ranges.clone(),
                                });
                            }
                            row.highlights.insert(key.into(), ranges);
                        }
                    }
                }
                row
            })
            .collect(),
        total: page.total,
        matched: page.matched,
        facets: page.facets.as_ref().clone(),
    }
}

fn field_schema<'a>(schema: &'a Value, key: &str) -> Option<&'a Value> {
    key.trim_start_matches('/')
        .split(if key.starts_with('/') { '/' } else { '.' })
        .try_fold(schema, |schema, part| {
            let child = schema.get("properties")?.get(part)?;
            (part != "_editor" && child["x-editor"] != "hidden" && child["x-visible"] != false)
                .then_some(child)
        })
}

/// Signatures are path-sorted. Shared snapshots compare in O(1); a changed
/// snapshot is diffed with a linear merge without allocating maps of every file.
pub(super) fn changed_paths<'a>(
    old: &'a read_cache::Signature,
    current: &'a read_cache::Signature,
) -> Vec<(&'a PathBuf, Option<&'a read_cache::Stamp>)> {
    if old == current {
        return Vec::new();
    }
    let mut changes = Vec::new();
    let (mut a, mut b) = (old.iter().peekable(), current.iter().peekable());
    while a.peek().is_some() || b.peek().is_some() {
        match (a.peek(), b.peek()) {
            (Some((ap, _)), Some((bp, _))) if ap < bp => {
                let (p, _) = a.next().unwrap();
                changes.push((p, None));
            }
            (Some((ap, _)), Some((bp, _))) if ap == bp => {
                let (_, old) = a.next().unwrap();
                let (p, new) = b.next().unwrap();
                if old != new {
                    changes.push((p, new.as_ref()));
                }
            }
            (_, Some(_)) => {
                let (p, new) = b.next().unwrap();
                changes.push((p, new.as_ref()));
            }
            (Some(_), None) => {
                let (p, _) = a.next().unwrap();
                changes.push((p, None));
            }
            _ => break,
        }
    }
    changes
}

enum RowFilter {
    Min(String, Option<f64>),
    Max(String, Option<f64>),
    Missing(String),
    Text(String, Arc<SearchMatcher>),
    Equal(String, String),
}
impl RowFilter {
    fn matches(&self, row: &ListRow) -> bool {
        match self {
            Self::Min(key, n) => field(&row.value, key)
                .and_then(Value::as_f64)
                .zip(*n)
                .is_some_and(|(v, n)| v >= n),
            Self::Max(key, n) => field(&row.value, key)
                .and_then(Value::as_f64)
                .zip(*n)
                .is_some_and(|(v, n)| v <= n),
            Self::Missing(key) => field(&row.value, key).is_none(),
            Self::Text(key, matcher) => row
                .display
                .get(key)
                .is_some_and(|text| matcher.is_match(text)),
            Self::Equal(key, expected) => {
                field(&row.value, key).is_some_and(|v| display_text(v) == *expected)
            }
        }
    }
}
fn compare_rows(
    a: &ListRow,
    b: &ListRow,
    request: &ListRequest,
    ui: &Value,
    props: &serde_json::Map<String, Value>,
    text_order: std::cmp::Ordering,
) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let order = match request.sort.as_str() {
        "_id" => text_order,
        "_manual" => {
            let missing =
                if ui.pointer("/ordering/unordered").and_then(Value::as_str) == Some("first") {
                    i64::MIN
                } else {
                    i64::MAX
                };
            a.value
                .pointer("/_editor/manual_order")
                .and_then(Value::as_i64)
                .unwrap_or(missing)
                .cmp(
                    &b.value
                        .pointer("/_editor/manual_order")
                        .and_then(Value::as_i64)
                        .unwrap_or(missing),
                )
                .then(text_order)
        }
        "" | "_label" => text_order,
        key => match (field(&a.value, key), field(&b.value, key)) {
            (None, None) => Ordering::Equal,
            (None, _) => return Ordering::Greater,
            (_, None) => return Ordering::Less,
            (Some(Value::Array(a)), Some(Value::Array(b)))
                if props[key]["x-children"] == true || props[key]["items"]["type"] == "object" =>
            {
                a.len().cmp(&b.len())
            }
            (Some(Value::Number(a)), Some(Value::Number(b))) => a
                .as_f64()
                .partial_cmp(&b.as_f64())
                .unwrap_or(Ordering::Equal),
            _ => text_order,
        },
    };
    (if request.descending {
        order.reverse()
    } else {
        order
    })
    .then(a.id.cmp(&b.id))
}
impl Editor {
    fn search_matcher(&self, query: &str, options: SearchOptions) -> Result<Arc<SearchMatcher>> {
        let mut cache = self.read_cache.matchers.lock().unwrap();
        if let Some((_, _, matcher)) = cache.iter().find(|(q, o, _)| q == query && *o == options) {
            return Ok(matcher.clone());
        }
        let matcher = Arc::new(SearchMatcher::new(query, options).map_err(Error::SearchPattern)?);
        if cache.len() >= 16 {
            cache.remove(0);
        }
        cache.push((query.into(), options, matcher.clone()));
        Ok(matcher)
    }
}
