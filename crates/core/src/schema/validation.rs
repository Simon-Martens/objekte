//! Declared-shape validation and indexed conversion.

use std::collections::{HashMap, HashSet};

use crate::model::{LogicalType, MetadataField, Number, Value};

use super::FieldDefinition;
use super::Schema;

impl Schema {
    pub fn coerce_record(&self, value: &mut Value) -> Result<(), String> {
        self.coerce_projected_record(value, None)
    }

    pub fn coerce_projected_record(
        &self,
        value: &mut Value,
        selected_top_level: Option<&HashSet<String>>,
    ) -> Result<(), String> {
        if let Some(root) = self.fields.get(&Vec::<String>::new()) {
            validate_declared(
                value,
                &root.kind,
                root.nullable,
                &root.element_nullable,
                0,
                &["@root"],
            )?;
        } else if !matches!(value, Value::Object(_)) {
            if self.fields.is_empty() {
                return Ok(());
            }
            return Err("record root must be an object".into());
        }
        validate_children(value, &self.validation, &mut Vec::new(), selected_top_level)
    }

    /// Apply conversions and declared array annotations already approved by
    /// the published index, without rechecking the declared schema.
    pub(crate) fn convert_indexed_record(
        &self,
        value: &mut Value,
        selected_top_level: Option<&HashSet<String>>,
    ) -> Result<(), String> {
        if !self.indexed_value_fields {
            return Ok(());
        }
        if let Some(root) = self.fields.get(&Vec::<String>::new()) {
            convert_declared_indexed(value, &root.kind, &root.element_nullable, 0)?;
        }
        convert_children_indexed(value, &self.indexed_value_paths, selected_top_level)
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ValidationNode {
    pub(crate) field: Option<FieldDefinition>,
    pub(crate) children: HashMap<String, ValidationNode>,
}

impl ValidationNode {
    fn requires_presence(&self) -> bool {
        self.field.as_ref().map_or_else(
            || self.children.values().any(Self::requires_presence),
            |field| field.required,
        )
    }
}

pub(crate) fn converted_kind(kind: &LogicalType) -> bool {
    match kind {
        LogicalType::Id | LogicalType::Date | LogicalType::Datetime | LogicalType::Unixtime => true,
        LogicalType::Array(inner) => converted_kind(inner),
        _ => false,
    }
}

pub(super) fn validate_children<'a>(
    value: &mut Value,
    node: &'a ValidationNode,
    path: &mut Vec<&'a str>,
    selected: Option<&HashSet<String>>,
) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            for (name, child) in &node.children {
                if selected.is_some_and(|selection| !selection.contains(name)) {
                    continue;
                }
                path.push(name);
                if let Some(value) = fields.get_mut(name) {
                    if let Some(field) = &child.field {
                        validate_declared(
                            value,
                            &field.kind,
                            field.nullable,
                            &field.element_nullable,
                            0,
                            path,
                        )?;
                    }
                    if !matches!(value, Value::Null) {
                        validate_children(value, child, path, None)?;
                    }
                } else if child.requires_presence() {
                    return Err(format!("{}: required field is missing", path.join(".")));
                }
                path.pop();
            }
        }
        Value::Array(array) => {
            for value in &mut array.values {
                if !matches!(value, Value::Null) {
                    validate_children(value, node, path, None)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn convert_children_indexed(
    value: &mut Value,
    node: &ValidationNode,
    selected: Option<&HashSet<String>>,
) -> Result<(), String> {
    match value {
        Value::Object(fields) => {
            for (name, child) in &node.children {
                if selected.is_some_and(|selection| !selection.contains(name)) {
                    continue;
                }
                if let Some(value) = fields.get_mut(name) {
                    if let Some(field) = &child.field {
                        convert_declared_indexed(value, &field.kind, &field.element_nullable, 0)?;
                    }
                    if !matches!(value, Value::Null) {
                        convert_children_indexed(value, child, None)?;
                    }
                }
            }
        }
        Value::Array(array) => {
            for value in &mut array.values {
                if !matches!(value, Value::Null) {
                    convert_children_indexed(value, node, None)?;
                }
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn convert_declared_indexed(
    value: &mut Value,
    kind: &LogicalType,
    elements: &[bool],
    depth: usize,
) -> Result<(), String> {
    if matches!(value, Value::Null) {
        return Ok(());
    }
    match kind {
        LogicalType::Id => {
            if let Value::String(text) = value {
                *value = Value::Id(text.clone());
            }
        }
        LogicalType::Date | LogicalType::Datetime => {
            if let Value::String(text) = value {
                *value = Value::parse_typed_string(text, kind)?;
            }
        }
        LogicalType::Unixtime => {
            if let Value::Number(Number::Integer(seconds)) = value {
                *value =
                    Value::Unixtime(i64::try_from(*seconds).map_err(|_| "unixtime out of range")?);
            }
        }
        _ => {}
    }
    if let (Value::Array(array), LogicalType::Array(inner)) = (&mut *value, kind) {
        for item in &mut array.values {
            convert_declared_indexed(item, inner, elements, depth + 1)?;
        }
        array.element_type = Some((**inner).clone());
        array.nullable_elements = elements.get(depth).copied().unwrap_or(true);
    }
    Ok(())
}

pub(super) fn validate_declared(
    value: &mut Value,
    kind: &LogicalType,
    nullable: bool,
    elements: &[bool],
    depth: usize,
    path: &[&str],
) -> Result<(), String> {
    if matches!(value, Value::Null) {
        return if nullable {
            Ok(())
        } else {
            Err(format!("{}: null is not allowed", path.join(".")))
        };
    }
    if *kind == LogicalType::Id {
        match value {
            Value::String(text) | Value::Id(text) => {
                if text.is_empty() {
                    return Err(format!("{}: ID is empty", path.join(".")));
                }
                *value = Value::Id(text.clone());
            }
            _ => {}
        }
    } else if matches!(kind, LogicalType::Date | LogicalType::Datetime) {
        if let Value::String(text) = value {
            *value = Value::parse_typed_string(text, kind)
                .map_err(|e| format!("{}: {e}", path.join(".")))?;
        }
    } else if *kind == LogicalType::Unixtime {
        if let Value::Number(Number::Integer(seconds)) = value {
            *value = Value::Unixtime(
                i64::try_from(*seconds)
                    .map_err(|_| format!("{}: unixtime out of range", path.join(".")))?,
            );
        }
    }
    if let (Value::Array(array), LogicalType::Array(inner)) = (&mut *value, kind) {
        let item_nullable = elements.get(depth).copied().unwrap_or(true);
        for item in &mut array.values {
            validate_declared(item, inner, item_nullable, elements, depth + 1, path)?;
        }
        array.element_type = Some((**inner).clone());
        array.nullable_elements = item_nullable;
        return Ok(());
    }
    let actual = value.logical_type();
    if &actual != kind && *kind != LogicalType::Unknown {
        return Err(format!(
            "{}: expected {kind:?}, found {actual:?}",
            path.join(".")
        ));
    }
    Ok(())
}

pub(super) fn metadata_type(field: MetadataField) -> LogicalType {
    match field {
        MetadataField::FilePath | MetadataField::CollectionName | MetadataField::RepositoryRoot => {
            LogicalType::String
        }
        MetadataField::RecordId => LogicalType::Id,
        MetadataField::RecordByteStart
        | MetadataField::RecordByteEnd
        | MetadataField::RecordByteLength
        | MetadataField::FileStatus
        | MetadataField::RecordClass
        | MetadataField::FileId
        | MetadataField::FileSize => LogicalType::Number,
        MetadataField::FileCreated | MetadataField::FileModified => LogicalType::Datetime,
    }
}
