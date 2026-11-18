//! Relation declarations and compatibility re-exports of physical storage.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::model::{CollectionId, LogicalType};
use crate::reference::{FieldDefinition, Schema};
use crate::repository::CollectionDefinition;

pub(crate) use crate::index::relation::{RelationContribution, validate_indexes};
pub use crate::index::relation::{RelationEdge, RelationIndex, RelationTarget};

/// Declared cardinality of a source relation field.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum RelationCardinality {
    One,
    Many,
}

/// Format-independent declaration of one source field's target namespace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct RelationDefinition {
    pub path: Vec<String>,
    pub target_collection: String,
    pub cardinality: RelationCardinality,
    pub reverse_alias: Option<String>,
}

/// Stable relation identity within a repository configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
pub struct RelationId {
    pub source: CollectionId,
    pub ordinal: u32,
}

impl Schema {
    pub fn relations(&self) -> &BTreeMap<Vec<String>, RelationDefinition> {
        &self.relations
    }

    pub fn register_relation(&mut self, declaration: RelationDefinition) -> Result<(), String> {
        if declaration.path.is_empty() || declaration.target_collection.is_empty() {
            return Err("relation path or target collection is empty".into());
        }
        if let Some(alias) = &declaration.reverse_alias
            && !valid_alias(alias)
        {
            return Err("invalid relation reverse alias".into());
        }
        for depth in 1..declaration.path.len() {
            if let Some(ancestor) = self.fields.get(&declaration.path[..depth])
                && !traversable_kind(&ancestor.kind)
            {
                return Err(format!(
                    "relation path crosses non-object field: {}",
                    ancestor.path.join(".")
                ));
            }
        }
        if self
            .fields
            .keys()
            .any(|path| path.len() > declaration.path.len() && path.starts_with(&declaration.path))
        {
            return Err("relation field is also registered as an embedded object".into());
        }
        let field = self
            .fields
            .get_mut(&declaration.path)
            .ok_or("relation field is not registered")?;
        let expected = if field.multi || matches!(field.kind, LogicalType::Array(_)) {
            RelationCardinality::Many
        } else {
            RelationCardinality::One
        };
        let valid_type = field.kind == LogicalType::Id
            || field.kind == LogicalType::Array(Box::new(LogicalType::Id));
        if !valid_type || declaration.cardinality != expected {
            return Err("relation requires matching ID field cardinality".into());
        }
        if self.relations.contains_key(&declaration.path) {
            return Err("duplicate relation declaration".into());
        }
        // Typed reverse membership is required even without an ordinary index request.
        field.indexed = true;
        self.relations.insert(declaration.path.clone(), declaration);
        Ok(())
    }
}

pub(crate) fn validate_field_against_relations(
    field: &FieldDefinition,
    relations: &BTreeMap<Vec<String>, RelationDefinition>,
) -> Result<(), String> {
    for relation in relations.values() {
        if relation.path.len() > field.path.len()
            && relation.path.starts_with(&field.path)
            && !traversable_kind(&field.kind)
        {
            return Err("relation path crosses non-object field".into());
        }
        if field.path.len() > relation.path.len() && field.path.starts_with(&relation.path) {
            return Err("relation field is also registered as an embedded object".into());
        }
    }
    Ok(())
}

fn traversable_kind(kind: &LogicalType) -> bool {
    match kind {
        LogicalType::Object => true,
        LogicalType::Array(inner) => traversable_kind(inner),
        _ => false,
    }
}

fn valid_alias(alias: &str) -> bool {
    let mut bytes = alias.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphabetic() || first == b'_')
        && bytes.all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

pub(crate) fn validate_definitions(definitions: &[CollectionDefinition]) -> Result<(), String> {
    let mut names = HashMap::new();
    for (index, definition) in definitions.iter().enumerate() {
        if names.insert(&definition.name, index).is_some() {
            return Err(format!("duplicate collection: {}", definition.name));
        }
    }
    let mut aliases = HashMap::<(String, String), (String, Vec<String>)>::new();
    for definition in definitions {
        for relation in definition.schema.relations().values() {
            if !names.contains_key(&relation.target_collection) {
                return Err(format!(
                    "relation target collection is unknown: {}",
                    relation.target_collection
                ));
            }
            if let Some(alias) = &relation.reverse_alias {
                let key = (relation.target_collection.clone(), alias.clone());
                if aliases
                    .insert(key, (definition.name.clone(), relation.path.clone()))
                    .is_some()
                {
                    return Err(format!("duplicate reverse relation alias: {alias}"));
                }
            }
        }
    }
    Ok(())
}
