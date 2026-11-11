//! Side-effect-free validation and bounded field choices for desktop filters.

use std::collections::BTreeMap;

use objekte_search_core::model::{FieldPath, LogicalType, MetadataField, PathRoot};
use objekte_search_core::query_path::QueryPath;

use crate::backend::{OpenedSession, build_query};
use crate::ui::filter_builder::{FieldChoice, ValueType};
use crate::ui::model::{FilterValidationState, SearchDraft};

use super::query_spec;

/// Check the exact combined request against parser, options, and all selected
/// collection bindings without executing a query or scanning source records.
/// Call this on a worker; the controller attaches its own generation identity.
pub fn validate_request(session: &OpenedSession, draft: &SearchDraft) -> FilterValidationState {
    let mut effective = draft.clone();
    if !effective.filter_enabled {
        effective.structured.clear();
    }
    let result = query_spec(&effective)
        .and_then(|spec| build_query(&spec))
        .and_then(|query| session.repository.validate_query(&query));
    match result {
        Ok(()) => FilterValidationState::Valid,
        Err(error) => FilterValidationState::Invalid {
            byte_offset: byte_offset(&error),
            message: concise_error(&error, &effective.structured),
        },
    }
}

/// Suggestions include built-in metadata, loaded schemas, and available columns;
/// this function never discovers fields by walking records.
pub fn field_choices(
    session: &OpenedSession,
    scope: Option<&str>,
    available_columns: &[String],
) -> Vec<FieldChoice> {
    let mut fields: BTreeMap<String, Option<ValueType>> = BTreeMap::new();
    for collection in session.repository.collections() {
        if scope.is_some_and(|name| name != collection.definition.name) {
            continue;
        }
        for definition in collection.definition.schema.fields().values() {
            let path = FieldPath {
                root: PathRoot::Data,
                segments: definition.path.clone(),
            }
            .display();
            let kind = builder_type(&definition.kind);
            fields
                .entry(path)
                .and_modify(|existing| {
                    if *existing != kind {
                        *existing = None;
                    }
                })
                .or_insert(kind);
        }
        if let Some(report) = &collection.inference {
            for inferred in &report.report.fields {
                let path = FieldPath {
                    root: PathRoot::Data,
                    segments: inferred.path.clone(),
                }
                .display();
                let kind = if inferred.observed_kinds.len() == 1 {
                    builder_type(&inferred.observed_kinds[0])
                } else {
                    None
                };
                fields.entry(path).or_insert(kind);
            }
        }
    }
    for column in available_columns {
        if QueryPath::parse(column).is_ok() {
            fields.entry(column.clone()).or_insert(None);
        }
    }
    // Reserved metadata is available in every scope, even without a schema or
    // visible metadata column. Use the core names so insertion matches FILTER.
    for field in [
        MetadataField::RecordId,
        MetadataField::RecordByteStart,
        MetadataField::RecordByteEnd,
        MetadataField::RecordByteLength,
        MetadataField::RecordClass,
        MetadataField::FileId,
        MetadataField::FilePath,
        MetadataField::FileSize,
        MetadataField::FileCreated,
        MetadataField::FileModified,
        MetadataField::FileStatus,
        MetadataField::CollectionName,
        MetadataField::RepositoryRoot,
    ] {
        let kind = match field {
            MetadataField::RecordByteStart
            | MetadataField::RecordByteEnd
            | MetadataField::RecordByteLength
            | MetadataField::FileSize
            | MetadataField::FileId
            | MetadataField::FileStatus
            | MetadataField::RecordClass => ValueType::Number,
            _ => ValueType::String,
        };
        fields.insert(field.as_str().into(), Some(kind));
    }
    fields
        .into_iter()
        .map(|(path, value_type)| FieldChoice { path, value_type })
        .collect()
}

fn builder_type(kind: &LogicalType) -> Option<ValueType> {
    match kind {
        LogicalType::String | LogicalType::Id => Some(ValueType::String),
        LogicalType::Number => Some(ValueType::Number),
        LogicalType::Bool => Some(ValueType::Boolean),
        _ => None,
    }
}

fn byte_offset(message: &str) -> Option<usize> {
    message
        .rsplit_once(" at byte ")
        .and_then(|(_, tail)| tail.split_whitespace().next())
        .and_then(|digits| digits.parse().ok())
}

fn concise_error(message: &str, filter: &str) -> String {
    if message.starts_with("independent multivalued field operands")
        && has_unquoted_percent(filter)
    {
        return "Quote wildcard values, for example: name ~ 'A%'".into();
    }
    if message.starts_with("expected operand at byte ") {
        let ending = filter.trim_end();
        for operator in [
            "?>=", "?<=", "?!~", "?!=", ">=", "<=", "!=", "?=", "?>", "?<", "?~", "=", ">", "<",
            "~",
        ] {
            if ending.ends_with(operator) {
                return format!("Expected a value after {operator}");
            }
        }
    }
    message.to_owned()
}

fn has_unquoted_percent(filter: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;
    for ch in filter.chars() {
        if escaped {
            escaped = false;
        } else if quote.is_some() && ch == '\\' {
            escaped = true;
        } else if quote == Some(ch) {
            quote = None;
        } else if quote.is_none() {
            match ch {
                '\'' | '"' => quote = Some(ch),
                '%' => return true,
                _ => {}
            }
        }
    }
    false
}

#[cfg(test)]
#[path = "filter_validation.test.rs"]
mod tests;
