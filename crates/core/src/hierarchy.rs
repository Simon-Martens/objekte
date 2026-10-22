//! Parent-owned child lists. Identity and record storage remain independent of placement.
use super::*;
#[derive(Clone, Debug, Serialize)]
pub struct ChildCollection {
    pub field: String,
    pub collection: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct ParentLink {
    pub collection: String,
    pub id: String,
    pub field: String,
    pub label: String,
}
#[derive(Clone, Debug, Serialize)]
pub struct HierarchyNode {
    pub collection: String,
    pub id: String,
    pub label: String,
    pub children: Vec<HierarchyNode>,
    pub branch: bool,
}
impl Collection {
    pub fn child_collections(&self) -> Vec<ChildCollection> {
        let mut children: Vec<_> = self.editor_schema["properties"]
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(_, s)| s["x-children"] == true)
            .filter_map(|(key, s)| {
                Some(ChildCollection {
                    field: key.clone(),
                    collection: s.pointer("/items/x-relation/collection")?.as_str()?.into(),
                    label: s["title"].as_str().unwrap_or(key).into(),
                })
            })
            .collect();
        let order = self.ui["fieldOrder"].as_array();
        children.sort_by_key(|c| {
            order
                .and_then(|o| o.iter().position(|v| v.as_str() == Some(&c.field)))
                .unwrap_or(usize::MAX)
        });
        children
    }
}
pub(crate) fn validate_child_collections(collections: &BTreeMap<String, Collection>) -> Result<()> {
    fn visit(
        name: &str,
        collections: &BTreeMap<String, Collection>,
        path: &mut Vec<String>,
    ) -> Result<()> {
        if path.iter().any(|v| v == name) {
            return Err(Error::Config(format!(
                "child collection cycle: {} → {name}",
                path.join(" → ")
            )));
        }
        if path.len() > 16 {
            return Err(Error::Config(
                "child collection hierarchy is too deep".into(),
            ));
        }
        path.push(name.into());
        let c = &collections[name];
        for (field, s) in c.schema_value["properties"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if s["x-children"] != true {
                continue;
            }
            let target = s
                .pointer("/items/x-relation/collection")
                .and_then(Value::as_str)
                .filter(|n| collections.contains_key(*n));
            if s["type"] != "array"
                || s["items"]["type"] != "string"
                || s["uniqueItems"] != true
                || target.is_none()
                || s["items"]["x-relation"]["allowUnresolved"] == true
            {
                return Err(Error::Config(format!(
                    "{name}.{field}: child lists require unique arrays of existing record references"
                )));
            }
            visit(target.unwrap(), collections, path)?;
        }
        path.pop();
        Ok(())
    }
    for name in collections.keys() {
        visit(name, collections, &mut Vec::new())?;
    }
    Ok(())
}
impl Editor {
    pub fn child_collection(&self, collection: &str, field: &str) -> Result<ChildCollection> {
        self.config
            .collection(collection)?
            .child_collections()
            .into_iter()
            .find(|c| c.field == field)
            .ok_or_else(|| Error::Data(format!("unknown child list {collection}.{field}")))
    }
    pub fn child_ids(&self, collection: &str, id: &str, field: &str) -> Result<BTreeSet<String>> {
        self.require_operation(collection, "read")?;
        let child = self.child_collection(collection, field)?;
        self.require_operation(&child.collection, "read")?;
        let d = self.get(collection, id)?;
        Ok(d.value[field]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect())
    }
    pub fn parent_links(&self, collection: &str, id: &str) -> Result<Vec<ParentLink>> {
        let mut links = Vec::new();
        for (parent, c) in &self.config.collections {
            if !self.allows(parent, "read") {
                continue;
            }
            let fields: Vec<_> = c
                .child_collections()
                .into_iter()
                .filter(|f| f.collection == collection)
                .collect();
            if fields.is_empty() {
                continue;
            }
            for file in self.files(parent)? {
                let value = parse(c, &fs::read(&file)?)?;
                for f in &fields {
                    if value[&f.field]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|v| v.as_str() == Some(id)))
                    {
                        let parent_id = record_id(c, &file, &value)?;
                        links.push(ParentLink {
                            collection: parent.clone(),
                            label: self.label(parent, &value, &parent_id, "reference")?,
                            id: parent_id,
                            field: f.field.clone(),
                        });
                    }
                }
            }
        }
        links.sort_by(|a, b| crate::alphabetic_cmp(&a.label, &b.label).then(a.id.cmp(&b.id)));
        Ok(links)
    }
    pub fn hierarchy(&self, root: &str) -> Result<Vec<HierarchyNode>> {
        fn node(e: &Editor, name: &str, id: &str, depth: usize) -> Result<HierarchyNode> {
            if depth > 16 {
                return Err(Error::Data("hierarchy is too deep".into()));
            }
            let c = e.config.collection(name)?;
            let doc = e.get(name, id)?;
            let defs = c.child_collections();
            let mut children = Vec::new();
            for f in &defs {
                if !e.allows(&f.collection, "read") {
                    continue;
                }
                for child in doc.value[&f.field]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                {
                    match node(e, &f.collection, child, depth + 1) {
                        Ok(child) => children.push(child),
                        // A file removed outside the editor must not make all navigation fail.
                        Err(Error::NotFound(_)) => {}
                        Err(error) => return Err(error),
                    }
                }
            }
            children.sort_by(|a, b| {
                b.branch
                    .cmp(&a.branch)
                    .then(crate::alphabetic_cmp(&a.label, &b.label))
                    .then(a.id.cmp(&b.id))
            });
            Ok(HierarchyNode {
                collection: name.into(),
                id: id.into(),
                label: e.label(name, &doc.value, id, "reference")?,
                children,
                branch: !defs.is_empty(),
            })
        }
        self.require_operation(root, "read")?;
        let c = self.config.collection(root)?;
        let mut result = Vec::new();
        for file in self.files(root)? {
            let v = parse(c, &fs::read(&file)?)?;
            result.push(node(self, root, &record_id(c, &file, &v)?, 0)?);
        }
        result.sort_by(|a, b| crate::alphabetic_cmp(&a.label, &b.label).then(a.id.cmp(&b.id)));
        Ok(result)
    }
    pub fn set_child_membership(
        &self,
        parent: &str,
        id: &str,
        field: &str,
        child_id: &str,
        present: bool,
        expected: &str,
    ) -> Result<Document> {
        let _guard = self.write_guard.lock().unwrap();
        let target = self.child_collection(parent, field)?;
        self.require_operation(&target.collection, "read")?;
        if present {
            self.get(&target.collection, child_id)?;
        }
        let mut document = self.get(parent, id)?;
        if document.revision != expected {
            return Err(Error::Conflict(
                "parent changed on disk; reload before changing its child list".into(),
            ));
        }
        let mut ids: Vec<Value> = document.value[field]
            .as_array()
            .cloned()
            .unwrap_or_default();
        ids.retain(|v| v.as_str() != Some(child_id));
        if present {
            ids.push(Value::String(child_id.into()));
        }
        document.value[field] = Value::Array(ids);
        self.save_unlocked(parent, id, document.value, Some(expected))
    }
    /// Create a record and attach it under the same process write lock. Failed parent
    /// writes roll back the new file; a process crash can leave an unattached record,
    /// never silently remove existing content. Existing IDs are never overwritten.
    pub fn create_child(
        &self,
        parent: &str,
        id: &str,
        field: &str,
        expected: &str,
        child_id: &str,
        value: Value,
    ) -> Result<Document> {
        let _guard = self.write_guard.lock().unwrap();
        self.require_operation(parent, "update")?;
        let target = self.child_collection(parent, field)?;
        let mut document = self.get(parent, id)?;
        if document.revision != expected {
            return Err(Error::Conflict(
                "parent changed on disk; reopen the child creation form".into(),
            ));
        }
        let child = self.save_unlocked(&target.collection, child_id, value, None)?;
        let mut ids = document.value[field]
            .as_array()
            .cloned()
            .unwrap_or_default();
        ids.push(Value::String(child_id.into()));
        document.value[field] = Value::Array(ids);
        if let Err(error) = self.save_unlocked(parent, id, document.value, Some(expected)) {
            let path = self.find_path(&target.collection, child_id)?;
            if let Err(rollback) = fs::remove_file(path) {
                return Err(Error::Data(format!(
                    "{error}; new record {}/{child_id} remains unattached: {rollback}",
                    target.collection
                )));
            }
            self.clear_field_values(&target.collection);
            return Err(error);
        }
        Ok(child)
    }
}
