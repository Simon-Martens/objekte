use napi::bindgen_prelude::Buffer;
use napi_derive::napi;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use structured_editor_core::{Editor, Error as CoreError, Result as CoreResult};

fn error(value: CoreError) -> napi::Error {
    let (code, issues) = match &value {
        CoreError::Validation(issues) => ("validation", json!(issues)),
        CoreError::Conflict(_) => ("conflict", json!([])),
        CoreError::NotFound(_) => ("not_found", json!([])),
        CoreError::InvalidPath(_) | CoreError::Data(_) => ("bad_request", json!([])),
        _ => ("internal", json!([])),
    };
    napi::Error::from_reason(
        json!({"code":code,"error":value.to_string(),"issues":issues}).to_string(),
    )
}
fn output<T: Serialize>(value: CoreResult<T>) -> napi::Result<String> {
    serde_json::to_string(&value.map_err(error)?)
        .map_err(|e| napi::Error::from_reason(e.to_string()))
}

#[napi]
pub struct Project {
    editor: Editor,
}

#[napi]
impl Project {
    #[napi(constructor)]
    pub fn new(root: String) -> napi::Result<Self> {
        Ok(Self {
            editor: Editor::open(root).map_err(error)?,
        })
    }

    #[napi]
    pub fn ui_config(&self) -> napi::Result<String> {
        output(Ok(&self.editor.config().ui))
    }
    #[napi]
    pub fn document_mappings(&self) -> napi::Result<String> {
        output(Ok(&self.editor.config().documents))
    }
    #[napi]
    pub fn document_schema(&self, name: String) -> napi::Result<String> {
        output(self.editor.document_schema(&name))
    }
    #[napi]
    pub fn document_files(&self, name: String) -> napi::Result<String> {
        output(self.editor.document_files(&name))
    }
    #[napi]
    pub fn json_document(&self, name: String, path: String) -> napi::Result<String> {
        output(self.editor.json_document(&name, &path))
    }
    #[napi]
    pub fn save_json_document(
        &self,
        name: String,
        path: String,
        value: String,
        revision: String,
    ) -> napi::Result<String> {
        let value = serde_json::from_str(&value)
            .map_err(|e| error(CoreError::Data(format!("document: {e}"))))?;
        output(
            self.editor
                .save_json_document(&name, &path, value, &revision),
        )
    }

    #[napi]
    pub fn settings_schema(&self) -> napi::Result<String> {
        output(self.editor.settings_schema())
    }
    #[napi]
    pub fn settings(&self) -> napi::Result<String> {
        output(self.editor.settings())
    }
    #[napi]
    pub fn save_settings(&self, value: String, revision: String) -> napi::Result<String> {
        let value = serde_json::from_str(&value)
            .map_err(|e| error(CoreError::Data(format!("settings: {e}"))))?;
        output(self.editor.save_settings(value, &revision))
    }

    #[napi]
    pub fn collections(&self) -> napi::Result<String> {
        let items: Vec<_> = self
            .editor
            .config()
            .collections
            .iter()
            .map(|(name, c)| {
                json!({
                    "name":name,"label":c.label,"format":c.format,"display":c.display,"displayFields":c.display_fields,"displayFallback":c.display_fallback,"categories":c.categories,"showInSidebar":c.show_in_sidebar
                })
            })
            .collect();
        output(Ok(items))
    }
    #[napi]
    pub fn project_schema(&self) -> napi::Result<String> {
        output(Ok(&self.editor.config().schema_value))
    }
    #[napi]
    pub fn schema(&self, collection: String) -> napi::Result<String> {
        output(
            self.editor
                .config()
                .collection(&collection)
                .map(|c| &c.editor_schema),
        )
    }
    #[napi]
    pub fn records(
        &self,
        collection: String,
        query: String,
        offset: u32,
        limit: u32,
    ) -> napi::Result<String> {
        output(
            self.editor
                .list(&collection, &query, offset as usize, limit as usize),
        )
    }
    #[napi]
    pub fn category_tree(&self, collection: String) -> napi::Result<String> {
        output(self.editor.categories(&collection))
    }
    #[napi]
    pub fn records_in_category(
        &self,
        collection: String,
        query: String,
        category: Vec<String>,
        offset: u32,
        limit: u32,
    ) -> napi::Result<String> {
        output(self.editor.list_in_category(
            &collection,
            &query,
            &category,
            offset as usize,
            limit as usize,
        ))
    }
    #[napi]
    pub fn record(&self, collection: String, id: String) -> napi::Result<String> {
        output(self.editor.get(&collection, &id))
    }

    #[napi]
    pub fn validate_new_id(&self, collection: String, id: String) -> napi::Result<String> {
        output(self.editor.validate_new_id(&collection, &id))
    }

    #[napi]
    pub fn generate_id(&self, collection: String, value: String) -> napi::Result<String> {
        let value: Value =
            serde_json::from_str(&value).map_err(|e| napi::Error::from_reason(e.to_string()))?;
        output(self.editor.generate_id(&collection, &value))
    }
    #[napi]
    pub fn field_values(
        &self,
        collection: String,
        field: String,
        query: String,
        scope: String,
        limit: u32,
    ) -> napi::Result<String> {
        let scope: BTreeMap<String, String> = serde_json::from_str(&scope)
            .map_err(|e| error(CoreError::Data(format!("invalid field scope: {e}"))))?;
        output(
            self.editor
                .field_values(&collection, &field, &scope, &query, limit as usize),
        )
    }
    #[napi]
    pub fn validate(&self, collection: String, value: String) -> napi::Result<String> {
        let value: Value =
            serde_json::from_str(&value).map_err(|e| napi::Error::from_reason(e.to_string()))?;
        output(self.editor.validate(&collection, &value))
    }
    #[napi]
    pub fn save(
        &self,
        collection: String,
        id: String,
        value: String,
        revision: Option<String>,
    ) -> napi::Result<String> {
        let value: Value =
            serde_json::from_str(&value).map_err(|e| napi::Error::from_reason(e.to_string()))?;
        output(
            self.editor
                .save(&collection, &id, value, revision.as_deref()),
        )
    }
    #[napi]
    pub fn rename(
        &self,
        collection: String,
        old_id: String,
        new_id: String,
        value: String,
        revision: String,
    ) -> napi::Result<String> {
        let value: Value =
            serde_json::from_str(&value).map_err(|e| napi::Error::from_reason(e.to_string()))?;
        output(
            self.editor
                .rename(&collection, &old_id, &new_id, value, &revision),
        )
    }
    #[napi]
    pub fn delete(&self, collection: String, id: String, revision: String) -> napi::Result<String> {
        self.editor
            .delete(&collection, &id, &revision)
            .map_err(error)?;
        output(Ok(json!({"ok":true})))
    }
    #[napi]
    pub fn relation_search(
        &self,
        collection: String,
        query: String,
        offset: u32,
        limit: u32,
        display: Option<String>,
        search: Option<Vec<String>>,
    ) -> napi::Result<String> {
        output(self.editor.search_relation(
            &collection,
            &query,
            display.as_deref(),
            search.as_deref(),
            offset as usize,
            limit as usize,
        ))
    }
    #[napi]
    pub fn relation_resolve(
        &self,
        collection: String,
        id: String,
        display: Option<String>,
    ) -> napi::Result<String> {
        output(
            self.editor
                .resolve_relation(&collection, &id, display.as_deref()),
        )
    }
    #[napi]
    pub fn references_to(&self, collection: String, id: String) -> napi::Result<String> {
        output(self.editor.references_to(&collection, &id))
    }
    #[napi]
    pub fn media_list(&self, path: String, offset: u32, limit: u32) -> napi::Result<String> {
        output(
            self.editor
                .media_list(&path, offset as usize, limit as usize),
        )
    }
    #[napi]
    pub fn media_search(&self, query: String, offset: u32, limit: u32) -> napi::Result<String> {
        output(
            self.editor
                .media_search(&query, offset as usize, limit as usize),
        )
    }
    #[napi]
    pub fn media_references(&self, path: String) -> napi::Result<String> {
        output(self.editor.media_references(&path))
    }
    #[napi]
    pub fn media_read(&self, path: String) -> napi::Result<Buffer> {
        self.editor
            .media_read(&path)
            .map(Buffer::from)
            .map_err(error)
    }
    #[napi]
    pub fn media_upload(&self, path: String, bytes: Buffer, max_bytes: u32) -> napi::Result<()> {
        self.editor
            .media_upload(&path, &bytes, max_bytes as usize)
            .map_err(error)
    }
    #[napi]
    pub fn media_mkdir(&self, path: String) -> napi::Result<()> {
        self.editor.media_mkdir(&path).map_err(error)
    }
    #[napi]
    pub fn media_move(&self, source: String, target: String) -> napi::Result<()> {
        self.editor.media_move(&source, &target).map_err(error)
    }
    #[napi]
    pub fn media_delete(&self, path: String) -> napi::Result<()> {
        self.editor.media_delete(&path).map_err(error)
    }
}
