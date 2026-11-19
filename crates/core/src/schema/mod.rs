//! Schema declarations, field policy, extension operations, and validation.

use crate::model::{FieldPath, LogicalType, MetadataField, PathRoot};
use crate::query::Operator;
use crate::query::options::SortField;
use std::collections::{BTreeMap, HashMap, HashSet};

pub mod json;
pub mod operation;
pub mod relations;
pub mod validation;

pub use operation::{
    BindingOperation, CustomBinding, ExactInput, PreparedFilterMatcher, PreparedTextMatcher,
    RegisteredBinding, Transform,
};
use validation::{ValidationNode, converted_kind, metadata_type};

use relations::RelationDefinition;

/// Logical type, presence rules, and index policy for one field.
#[derive(Clone, Debug)]
pub struct FieldDefinition {
    pub path: Vec<String>,
    pub kind: LogicalType,
    pub required: bool,
    pub nullable: bool,
    pub indexed: bool,
    pub multi: bool,
    /// Nullability at each array level, from outermost to innermost.
    /// An empty list retains permissive legacy behavior for in-memory hints.
    pub element_nullable: Vec<bool>,
}

#[derive(Clone, Debug, Default)]
pub struct Schema {
    /// Explicit resident orders assembled from already configured scalar sort columns.
    pub composite_sorts: Vec<Vec<SortField>>,
    pub(crate) fields: HashMap<Vec<String>, FieldDefinition>,
    /// Explicit `index: false` paths; absent annotations leave inference free.
    pub(crate) auto_index_excluded: HashSet<Vec<String>>,
    pub(crate) source_fingerprint: Option<[u8; 32]>,
    pub(crate) custom: HashMap<(Vec<String>, String), CustomBinding>,
    pub(crate) operations: HashMap<(Vec<String>, String), RegisteredBinding>,
    pub(crate) defaults: HashMap<Vec<String>, String>,
    pub(crate) raw: Option<RegisteredBinding>,
    pub(crate) relations: BTreeMap<Vec<String>, RelationDefinition>,
    converted_fields: bool,
    indexed_value_fields: bool,
    validation: ValidationNode,
    pub(crate) indexed_value_paths: ValidationNode,
}

impl Schema {
    /// Include the exact selected schema bytes in snapshot compatibility.
    pub fn set_source_fingerprint(&mut self, bytes: &[u8]) {
        use sha2::Digest;
        self.source_fingerprint = Some(sha2::Sha256::digest(bytes).into());
    }

    pub fn prohibit_auto_index(&mut self, path: Vec<String>) -> Result<(), String> {
        if self.fields.get(&path).is_some_and(|field| field.indexed) {
            return Err("automatic index prohibition conflicts with an explicit index".into());
        }
        self.auto_index_excluded.insert(path);
        Ok(())
    }

    pub fn is_auto_index_prohibited(&self, path: &[String]) -> bool {
        self.auto_index_excluded.contains(path)
    }

    /// Register an exact composite order over previously configured scalar sort fields.
    pub fn register_composite_sort(&mut self, fields: Vec<SortField>) -> Result<(), String> {
        if fields.len() < 2 {
            return Err("composite sort requires at least two fields".into());
        }
        let normalized = fields
            .iter()
            .map(crate::index::sorting::simple_composite_part)
            .collect::<Option<Vec<_>>>()
            .ok_or("composite sort requires resident scalar fields")?;
        if self.composite_sorts.iter().any(|existing| {
            existing
                .iter()
                .map(crate::index::sorting::simple_composite_part)
                .collect::<Option<Vec<_>>>()
                .as_ref()
                == Some(&normalized)
        }) {
            return Err("duplicate composite sort".into());
        }
        self.composite_sorts.push(fields);
        Ok(())
    }

    pub fn fields(&self) -> &HashMap<Vec<String>, FieldDefinition> {
        &self.fields
    }

    pub fn custom_bindings(&self) -> &HashMap<(Vec<String>, String), CustomBinding> {
        &self.custom
    }

    pub fn has_converted_fields(&self) -> bool {
        self.converted_fields
    }

    pub fn register_field(&mut self, field: FieldDefinition) -> Result<(), String> {
        if self.fields.contains_key(&field.path) {
            return Err("duplicate field registration".into());
        }
        crate::schema::relations::validate_field_against_relations(&field, &self.relations)?;
        let converted = converted_kind(&field.kind);
        let indexed_value = converted || matches!(field.kind, LogicalType::Array(_));
        self.converted_fields |= converted;
        self.indexed_value_fields |= indexed_value;
        let mut node = &mut self.validation;
        for part in &field.path {
            node = node.children.entry(part.clone()).or_default();
        }
        node.field = Some(field.clone());
        if self.indexed_value_fields {
            self.update_indexed_value_paths(&field, indexed_value);
        }
        self.fields.insert(field.path.clone(), field);
        Ok(())
    }

    fn update_indexed_value_paths(&mut self, field: &FieldDefinition, indexed_value: bool) {
        if field.path.is_empty() {
            if indexed_value {
                self.indexed_value_paths.field = Some(field.clone());
            }
            return;
        }
        if indexed_value {
            let mut node = &mut self.indexed_value_paths;
            for depth in 0..field.path.len() {
                node = node.children.entry(field.path[depth].clone()).or_default();
                node.field = if depth + 1 == field.path.len() {
                    Some(field.clone())
                } else {
                    self.fields.get(&field.path[..=depth]).cloned()
                };
            }
        } else {
            let mut node = &mut self.indexed_value_paths;
            for part in &field.path {
                let Some(child) = node.children.get_mut(part) else {
                    return;
                };
                node = child;
            }
            node.field = Some(field.clone());
        }
    }

    pub fn register_custom(
        &mut self,
        path: Vec<String>,
        binding: CustomBinding,
    ) -> Result<(), String> {
        if matches!(
            binding.name.as_str(),
            "lower" | "length" | "each" | "null" | "exists"
        ) {
            return Err("custom name collides with built-in modifier".into());
        }
        if !self.fields.contains_key(&path) {
            return Err("custom binding field is unknown".into());
        }
        if self
            .operations
            .contains_key(&(path.clone(), binding.name.clone()))
        {
            return Err("duplicate custom binding".into());
        }
        if self
            .custom
            .contains_key(&(path.clone(), binding.name.clone()))
        {
            return Err("duplicate custom binding".into());
        }
        self.custom.insert((path, binding.name.clone()), binding);
        Ok(())
    }

    pub fn register_operation(
        &mut self,
        path: Vec<String>,
        binding: RegisteredBinding,
    ) -> Result<(), String> {
        if !self.fields.contains_key(&path) {
            return Err("binding field is unknown".into());
        }
        if binding.name.is_empty()
            || binding.version == 0
            || matches!(
                binding.name.as_str(),
                "default" | "lower" | "length" | "each" | "null" | "exists"
            )
        {
            return Err("invalid binding registration".into());
        }
        let key = (path, binding.name.clone());
        if self.custom.contains_key(&key) || self.operations.contains_key(&key) {
            return Err("duplicate binding registration".into());
        }
        self.operations.insert(key, binding);
        Ok(())
    }

    pub fn set_default_binding(&mut self, path: Vec<String>, name: &str) -> Result<(), String> {
        if !self.fields.contains_key(&path)
            || (name != "lower"
                && !self
                    .operations
                    .contains_key(&(path.clone(), name.to_owned()))
                && !self.custom.contains_key(&(path.clone(), name.to_owned())))
        {
            return Err(format!("unknown binding {name}"));
        }
        self.defaults.insert(path, name.to_owned());
        Ok(())
    }

    pub fn set_raw_binding(&mut self, binding: RegisteredBinding) -> Result<(), String> {
        if binding.name.is_empty()
            || binding.version == 0
            || !binding
                .operation
                .accepts(&LogicalType::String, Operator::Contains, false)
        {
            return Err("invalid raw binding".into());
        }
        self.raw = Some(binding);
        Ok(())
    }

    pub fn field(&self, path: &FieldPath) -> Result<FieldDefinition, String> {
        match path.root {
            PathRoot::Data => self
                .fields
                .get(&path.segments)
                .cloned()
                .ok_or_else(|| format!("unknown field: {}", path.segments.join("."))),
            PathRoot::Metadata(field) => Ok(FieldDefinition {
                path: Vec::new(),
                kind: metadata_type(field),
                required: true,
                nullable: field == MetadataField::FileCreated,
                indexed: true,
                multi: false,
                element_nullable: Vec::new(),
            }),
            PathRoot::Backlinks => Err("backlink path requires relation binding".into()),
        }
    }

    pub fn field_or_dynamic(&self, path: &FieldPath) -> Result<FieldDefinition, String> {
        match path.root {
            PathRoot::Data => {
                Ok(self
                    .fields
                    .get(&path.segments)
                    .cloned()
                    .unwrap_or_else(|| FieldDefinition {
                        path: path.segments.clone(),
                        kind: LogicalType::Unknown,
                        required: false,
                        nullable: true,
                        indexed: false,
                        multi: true,
                        element_nullable: Vec::new(),
                    }))
            }
            PathRoot::Metadata(_) => self.field(path),
            PathRoot::Backlinks => Err("backlink path requires relation binding".into()),
        }
    }
}
