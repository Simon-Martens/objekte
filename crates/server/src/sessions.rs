use crate::{
    auth::{Actor, now},
    error::{Result, WebError},
};
use axum::http::StatusCode;
use std::collections::HashMap;
use structured_editor_core::{
    Editor, ValidationIssue,
    draft::{Draft, FieldInput},
};

#[derive(Clone)]
pub struct ParentContext {
    pub collection: String,
    pub id: String,
    pub field: String,
    pub revision: String,
}
#[derive(Clone)]
pub struct EditSession {
    pub baseline: Option<structured_editor_core::draft::DraftState>,
    pub close_requested: bool,
    pub view_path: String,
    pub document: Option<(String, String)>,
    pub parent: Option<ParentContext>,
    pub id: String,
    pub owner: String,
    pub collection: String,
    pub record_id: String,
    pub proposed_id: String,
    pub revision: Option<String>,
    pub schema: serde_json::Value,
    pub draft: Draft,
    pub version: u64,
    pub touched: i64,
    pub raw: Option<String>,
    pub picker: Option<Picker>,
    pub return_to: Option<(String, String)>,
    pub preview: std::collections::BTreeSet<String>,
    pub history: structured_editor_core::draft::DraftHistory,
    pub schema_warning: bool,
}
#[derive(Clone, Default)]
pub struct Picker {
    pub label: String,
    pub url: String,
    pub image: bool,
    pub field: String,
    pub kind: String,
    pub query: String,
    pub collection: String,
    pub path: String,
    pub offset: usize,
    pub from: usize,
    pub to: usize,
}
#[derive(Default)]
pub struct Sessions {
    entries: HashMap<String, EditSession>,
    gates: HashMap<String, std::sync::Weak<std::sync::Mutex<()>>>,
}
impl Sessions {
    /// Acquire the returned gate only after releasing the registry mutex.
    pub fn gate(
        &mut self,
        id: &str,
        actor: &Actor,
    ) -> Result<std::sync::Arc<std::sync::Mutex<()>>> {
        self.get(id, actor)?;
        self.gates.retain(|_, gate| gate.strong_count() > 0);
        if let Some(gate) = self.gates.get(id).and_then(|gate| gate.upgrade()) {
            return Ok(gate);
        }
        let gate = std::sync::Arc::new(std::sync::Mutex::new(()));
        self.gates
            .insert(id.into(), std::sync::Arc::downgrade(&gate));
        Ok(gate)
    }
    pub fn register(&mut self, actor: &Actor, session: EditSession) -> Result<()> {
        self.check_capacity(actor)?;
        self.insert(session);
        Ok(())
    }
    pub fn update(&mut self, actor: &Actor, session: EditSession) -> Result<()> {
        // Do not resurrect sessions removed by logout while a command was running.
        self.get(&session.id, actor)?;
        self.insert(session);
        Ok(())
    }
    fn check_capacity(&mut self, actor: &Actor) -> Result<()> {
        self.entries.retain(|_, s| now() - s.touched < 7200);
        if self.entries.len() >= 512
            || self
                .entries
                .values()
                .filter(|s| s.owner == actor.session)
                .count()
                >= 32
        {
            return Err(WebError(
                StatusCode::TOO_MANY_REQUESTS,
                "Too many open editing sessions. Sign out to clear them.".into(),
            ));
        }
        Ok(())
    }
    pub fn start_document(
        &mut self,
        editor: &Editor,
        actor: &Actor,
        name: &str,
        file: &str,
    ) -> Result<EditSession> {
        self.check_capacity(actor)?;
        if !actor.editable() {
            return Err(WebError(
                StatusCode::FORBIDDEN,
                "Editing permission required".into(),
            ));
        }
        let doc = editor.json_document(name, file)?;
        let schema = editor.document_editor_schema(name)?;
        let raw = (schema["x-presentation"]["editor"] == "raw" || schema["x-editor"] == "json")
            .then(|| serde_json::to_string_pretty(&doc.value).unwrap());
        let session = EditSession {
            baseline: None,
            close_requested: false,
            view_path: String::new(),
            document: Some((name.into(), file.into())),
            parent: None,
            id: uuid::Uuid::new_v4().simple().to_string(),
            owner: actor.session.clone(),
            collection: String::new(),
            record_id: file.into(),
            proposed_id: file.into(),
            revision: Some(doc.revision),
            draft: Draft::document(&schema, &doc.value),
            schema,
            version: 0,
            touched: now(),
            raw,
            picker: None,
            return_to: None,
            preview: Default::default(),
            history: Default::default(),
            schema_warning: false,
        };
        let mut session = session;
        session.mark_clean();
        self.insert(session.clone());
        Ok(session)
    }
    pub fn start(
        &mut self,
        editor: &Editor,
        actor: &Actor,
        collection: &str,
        record: Option<&str>,
    ) -> Result<EditSession> {
        self.check_capacity(actor)?;
        editor.require_operation(
            collection,
            if record.is_some() { "update" } else { "create" },
        )?;
        let schema = editor
            .config()
            .collection(collection)?
            .editor_schema
            .clone();
        let doc = record.map(|id| editor.get(collection, id)).transpose()?;
        let id = uuid::Uuid::new_v4().simple().to_string();
        let session = EditSession {
            baseline: None,
            close_requested: false,
            view_path: String::new(),
            document: None,
            parent: None,
            id: id.clone(),
            owner: actor.session.clone(),
            collection: collection.into(),
            record_id: record.unwrap_or("").into(),
            proposed_id: record.unwrap_or("").into(),
            revision: doc.as_ref().map(|d| d.revision.clone()),
            draft: if let Some(doc) = doc {
                editor.draft_for_id(collection, record.unwrap_or(""), &doc.value)?
            } else {
                editor.create_draft(collection, &serde_json::json!({}))?
            },
            schema,
            version: 0,
            touched: now(),
            raw: None,
            picker: None,
            return_to: None,
            preview: Default::default(),
            history: Default::default(),
            schema_warning: editor
                .config()
                .collection(collection)?
                .schema_path
                .is_some()
                && editor.schema_revision(collection)?.is_none(),
        };
        let mut session = session;
        if session.schema["x-presentation"]["editor"] == "raw" {
            session.raw = Some(
                serde_json::to_string_pretty(
                    &session
                        .value()
                        .map_err(|_| WebError::bad("Invalid draft"))?,
                )
                .unwrap(),
            );
        }
        if editor
            .settings()
            .ok()
            .is_some_and(|d| d.value["markdownPreview"] == true)
        {
            session.preview = session
                .draft
                .fields
                .iter()
                .filter(|f| {
                    matches!(
                        f.schema["x-editor"].as_str(),
                        Some("markdown" | "inline-markdown")
                    )
                })
                .map(|f| f.id.clone())
                .collect();
        }
        session.mark_clean();
        self.entries.insert(id, session.clone());
        Ok(session)
    }
    pub fn get(&mut self, id: &str, actor: &Actor) -> Result<&mut EditSession> {
        let session = self
            .entries
            .get_mut(id)
            .filter(|s| s.owner == actor.session && now() - s.touched < 7200)
            .ok_or_else(|| {
                WebError(
                    StatusCode::NOT_FOUND,
                    "Editing session missing or expired. Open the record again.".into(),
                )
            })?;
        session.touched = now();
        Ok(session)
    }
    pub fn insert(&mut self, session: EditSession) {
        self.entries.insert(session.id.clone(), session);
    }
    pub fn remove(&mut self, id: &str, actor: &Actor) -> Result<()> {
        self.get(id, actor)?;
        self.entries.remove(id);
        Ok(())
    }
    pub fn expire(&mut self) {
        self.entries
            .retain(|_, session| now() - session.touched < 7200);
        self.gates.retain(|_, gate| gate.strong_count() > 0);
    }
    pub fn logout(&mut self, actor: &Actor) {
        self.entries.retain(|_, s| s.owner != actor.session);
    }
}
impl EditSession {
    pub fn mark_clean(&mut self) {
        self.baseline = Some(self.snapshot());
    }
    pub fn dirty(&self) -> bool {
        let Some(baseline) = &self.baseline else {
            return true;
        };
        if self.proposed_id != baseline.id {
            return true;
        }
        let original = match &baseline.raw {
            Some(raw) => serde_json::from_str(raw).ok(),
            None => baseline.draft.value().ok(),
        };
        match (self.value().ok(), original) {
            (Some(current), Some(original)) => current != original,
            _ => self.snapshot() != *baseline,
        }
    }
    pub fn edit_key(&self) -> String {
        use crate::views::{encode, record_url};
        if let Some((name, file)) = &self.document {
            format!("/documents/{}/edit?file={}", encode(name), encode(file))
        } else if self.record_id.is_empty() {
            format!("/collections/{}/new", encode(&self.collection))
        } else {
            format!("{}/edit", record_url(&self.collection, &self.record_id))
        }
    }
    pub fn edit_url(&self) -> String {
        let mut url = self.edit_key();
        if !self.view_path.is_empty() {
            url += &format!(
                "{}node={}",
                if url.contains('?') { "&" } else { "?" },
                crate::views::encode(&self.view_path)
            );
        }
        url
    }
    pub fn close_url(&self) -> String {
        use crate::views::{encode, record_url};
        if let Some((name, file)) = &self.document {
            format!("/documents/{}?file={}", encode(name), encode(file))
        } else if !self.record_id.is_empty() {
            record_url(&self.collection, &self.record_id)
        } else if let Some(parent) = &self.parent {
            record_url(&parent.collection, &parent.id)
        } else {
            format!("/collections/{}", encode(&self.collection))
        }
    }

    pub fn snapshot(&self) -> structured_editor_core::draft::DraftState {
        structured_editor_core::draft::DraftState {
            draft: self.draft.clone(),
            id: self.proposed_id.clone(),
            raw: self.raw.clone(),
        }
    }
    pub fn restore(&mut self, state: structured_editor_core::draft::DraftState) {
        self.draft = state.draft;
        self.proposed_id = state.id;
        self.raw = state.raw;
        self.picker = None;
    }
    pub fn apply(&mut self, form: &HashMap<String, String>) -> Result<()> {
        let version = form.get("version").and_then(|v| v.parse::<u64>().ok());
        if version != Some(self.version) {
            return Err(WebError(StatusCode::CONFLICT,"This editing session has newer changes. Your input has not been applied; reload the editing session to inspect its current draft.".into()));
        }
        fn collect(
            fields: &[structured_editor_core::draft::DraftField],
            form: &HashMap<String, String>,
            inputs: &mut HashMap<String, FieldInput>,
        ) {
            for field in fields {
                if ["present", "value", "boolean", "manual_order"]
                    .iter()
                    .any(|prefix| form.contains_key(&format!("{prefix}.{}", field.id)))
                {
                    inputs.insert(
                        field.id.clone(),
                        FieldInput {
                            present: form.contains_key(&format!("present.{}", field.id)),
                            raw: form
                                .get(&format!("value.{}", field.id))
                                .map(|value| {
                                    // The Markdown control hides redundant escapes. Submitting
                                    // that unchanged presentation must preserve the stored source.
                                    if matches!(field.schema["x-editor"].as_str(), Some("markdown" | "inline-markdown"))
                                        && value == crate::views::markdown_source::readable(&field.raw).as_ref()
                                    {
                                        field.raw.clone()
                                    } else {
                                        value.clone()
                                    }
                                })
                                .unwrap_or_else(|| {
                                    if field.kind
                                        == structured_editor_core::draft::InputKind::Boolean
                                        && form.contains_key(&format!("boolean.{}", field.id))
                                    {
                                        "false".into()
                                    } else {
                                        field.raw.clone()
                                    }
                                }),
                        },
                    );
                    if field.key == "_editor" {
                        if let Some(raw) = form.get(&format!("manual_order.{}", field.id)) {
                            let mut v = serde_json::from_str::<serde_json::Value>(&field.raw)
                                .unwrap_or(serde_json::json!({}));
                            if !v.is_object() {
                                v = serde_json::json!({});
                            }
                            if raw.trim().is_empty() {
                                v.as_object_mut().unwrap().remove("manual_order");
                            } else {
                                v["manual_order"] = raw
                                    .parse::<i64>()
                                    .map(serde_json::Value::from)
                                    .unwrap_or(serde_json::json!(raw));
                            }
                            let input = inputs.get_mut(&field.id).unwrap();
                            if !raw.trim().is_empty() {
                                input.present = true;
                            }
                            if !raw.trim().is_empty() || field.present {
                                input.raw = v.to_string();
                            }
                        }
                    }
                }
                collect(&field.items, form, inputs);
            }
        }
        let mut inputs = HashMap::new();
        collect(&self.draft.fields, form, &mut inputs);
        if let Some(baseline) = &self.baseline {
            for (id, input) in &mut inputs {
                if let Some(original) = baseline.draft.field(id) {
                    if matches!(original.schema["x-editor"].as_str(), Some("markdown" | "inline-markdown"))
                        && input.raw == crate::views::markdown_source::readable(&original.raw).as_ref()
                    {
                        input.raw = original.raw.clone();
                    }
                }
            }
        }
        if self.raw.is_some() {
            self.raw = Some(form.get("raw").cloned().unwrap_or_default());
        } else {
            self.draft.apply(&inputs);
        }
        if let Some(id) = form.get("record_id").filter(|_| self.document.is_none()) {
            self.proposed_id = id.clone();
        }
        self.version += 1;
        Ok(())
    }
    pub fn value(&self) -> std::result::Result<serde_json::Value, Vec<ValidationIssue>> {
        if let Some(raw) = &self.raw {
            serde_json::from_str(raw).map_err(|e| {
                vec![ValidationIssue {
                    path: "".into(),
                    message: e.to_string(),
                }]
            })
        } else {
            self.draft.value()
        }
    }
    pub fn refresh_derived(&mut self, editor: &Editor) -> Result<()> {
        if self.raw.is_some() || self.document.is_some() {
            return Ok(());
        }
        let Ok(value) = self.draft.value() else {
            return Ok(());
        };
        let projected = editor.draft_for_id(&self.collection, &self.proposed_id, &value)?;
        for f in &mut self.draft.fields {
            let identity = editor
                .config()
                .collection(&self.collection)?
                .schema_path
                .is_some()
                && matches!(&editor.config().collection(&self.collection)?.id, structured_editor_core::IdStrategy::Frontmatter(key) if key == &f.key);
            if f.schema.get("x-derived").is_some() || identity {
                if let Some(next) = projected.fields.iter().find(|p| p.key == f.key) {
                    if f.read_only && !next.read_only {
                        f.present = false;
                        f.raw.clear();
                        f.read_only = false;
                        continue;
                    }
                    f.raw = next.raw.clone();
                    f.present = next.present;
                    f.read_only = next.read_only;
                }
            }
        }
        Ok(())
    }
    pub fn validate(&self, editor: &Editor) -> Result<Vec<ValidationIssue>> {
        match self.value() {
            Ok(value) => {
                if let Some((name, _)) = &self.document {
                    return Ok(editor.validate_document(name, &value)?);
                }
                let previous = if self.revision.is_some() {
                    Some(editor.get(&self.collection, &self.record_id)?.value)
                } else {
                    None
                };
                Ok(editor.validate_for_save(
                    &self.collection,
                    &self.proposed_id,
                    value,
                    previous.as_ref(),
                )?)
            }
            Err(issues) => Ok(issues),
        }
    }
    pub fn save(
        &mut self,
        editor: &Editor,
    ) -> std::result::Result<Option<usize>, structured_editor_core::Error> {
        let value = self
            .value()
            .map_err(structured_editor_core::Error::Validation)?;
        if let Some((name, file)) = &self.document {
            let saved = editor.save_json_document(
                name,
                file,
                value,
                self.revision.as_deref().unwrap_or(""),
            )?;
            self.revision = Some(saved.revision);
            self.draft = if self.raw.is_some() {
                Draft::document(&self.schema, &saved.value)
            } else {
                Draft::document_form(&self.schema, &saved.value)
            };
            if self.raw.is_some() {
                self.raw = Some(serde_json::to_string_pretty(&saved.value).unwrap());
            }
            self.mark_clean();
            return Ok(None);
        }
        let mut references_renamed = None;
        let saved = if let Some(parent) = &self.parent {
            editor.create_child(
                &parent.collection,
                &parent.id,
                &parent.field,
                &parent.revision,
                &self.proposed_id,
                value,
            )?
        } else if self.revision.is_some() && self.proposed_id != self.record_id {
            let renamed = editor.rename_with_references(
                &self.collection,
                &self.record_id,
                &self.proposed_id,
                value,
                self.revision.as_deref().unwrap(),
            )?;
            references_renamed = Some(renamed.references_renamed);
            renamed.document
        } else {
            editor.save(
                &self.collection,
                &self.proposed_id,
                value,
                self.revision.as_deref(),
            )?
        };
        self.schema_warning = editor
            .config()
            .collection(&self.collection)?
            .schema_path
            .is_some()
            && editor.schema_revision(&self.collection)?.is_none();
        self.record_id = saved.id;
        self.proposed_id = self.record_id.clone();
        self.revision = Some(saved.revision);
        // Rename can normalize a frontmatter ID. Render the canonical saved value.
        self.draft = editor.draft_for_id(&self.collection, &self.record_id, &saved.value)?;
        if self.raw.is_some() {
            self.raw = Some(serde_json::to_string_pretty(&saved.value).unwrap());
        }
        self.mark_clean();
        Ok(references_renamed)
    }
}
