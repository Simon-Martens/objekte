//! Bounded filter suggestions from a pinned index publication.

use std::collections::BTreeSet;

use objekte_search_core::completion::{CompletionContext, CompletionTarget};
use objekte_search_core::lifecycle::IndexView;
use objekte_search_core::model::{FieldPath, MetadataField, Number, PathRoot};
use objekte_search_core::posting::ScalarKey;
use objekte_search_core::posting_values::PostingValuesRequest;

const MAX_CHOICES: usize = 20;

pub(super) fn choices(
    view: &IndexView,
    scope: Option<&str>,
    context: &CompletionContext,
) -> Vec<String> {
    let prefix = context.prefix.to_lowercase();
    match &context.target {
        CompletionTarget::Field { .. } => {
            let mut fields = BTreeSet::new();
            for collection in view.collection_definitions() {
                if scope.is_some_and(|name| name != collection.name) {
                    continue;
                }
                for definition in collection.schema.fields().values() {
                    fields.insert(
                        FieldPath {
                            root: PathRoot::Data,
                            segments: definition.path.clone(),
                        }
                        .display(),
                    );
                }
                if let Some(report) = view.inference_report(&collection.name) {
                    for inferred in &report.fields {
                        fields.insert(
                            FieldPath {
                                root: PathRoot::Data,
                                segments: inferred.path.clone(),
                            }
                            .display(),
                        );
                    }
                }
            }
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
                fields.insert(field.as_str().to_owned());
            }
            fields
                .into_iter()
                .filter(|field| field.to_lowercase().starts_with(&prefix))
                .take(MAX_CHOICES)
                .collect()
        }
        CompletionTarget::Modifier => [
            "?", "string", "number", "array", "null", "object", "exists", "lower", "length", "each",
        ]
        .into_iter()
        .filter(|name| name.starts_with(&prefix))
        .map(str::to_owned)
        .collect(),
        CompletionTarget::BooleanValue => ["true", "false"]
            .into_iter()
            .filter(|value| value.starts_with(&prefix))
            .map(str::to_owned)
            .collect(),
        CompletionTarget::Value { field } => value_choices(view, scope, field, &prefix),
    }
}

fn value_choices(view: &IndexView, scope: Option<&str>, field: &str, prefix: &str) -> Vec<String> {
    // The posting-values API requires a complete index. During construction,
    // field names still come from the current publication; value completion
    // begins once its dictionaries have finished building.
    if !view.initial_construction_complete() {
        return Vec::new();
    }
    let Ok(path) = FieldPath::parse(field) else {
        return Vec::new();
    };
    let mut values = BTreeSet::new();
    for collection in view
        .collection_definitions()
        .filter(|collection| scope.is_none_or(|name| name == collection.name))
        .take(8)
    {
        // Custom posting keys may be transformed values, not valid source
        // literals. Inserting one would silently change the user's query.
        if collection
            .schema
            .custom_bindings()
            .keys()
            .any(|(segments, _)| segments == &path.segments)
        {
            continue;
        }
        let mut request = PostingValuesRequest::new(&collection.name, field, MAX_CHOICES);
        if !prefix.is_empty() {
            request.prefix = Some(prefix.to_owned());
        }
        let page = view.posting_values(&request).or_else(|_| {
            request.prefix = None;
            view.posting_values(&request)
        });
        let Ok(page) = page else { continue };
        for value in page.values {
            let Some(literal) = scalar_literal(&value.value) else {
                continue;
            };
            let plain =
                serde_json::from_str::<String>(&literal).unwrap_or_else(|_| literal.clone());
            if plain.to_lowercase().contains(prefix) {
                values.insert(literal);
                if values.len() >= MAX_CHOICES {
                    return values.into_iter().collect();
                }
            }
        }
    }
    values.into_iter().collect()
}

fn scalar_literal(value: &ScalarKey) -> Option<String> {
    Some(match value {
        ScalarKey::String(value) | ScalarKey::Id(value) => serde_json::to_string(value).ok()?,
        ScalarKey::Number(Number::Integer(value)) => value.to_string(),
        ScalarKey::Number(Number::Float(value)) => value.to_string(),
        ScalarKey::Number(Number::ExactDecimal(value)) => value.to_string(),
        ScalarKey::Bool(value) => value.to_string(),
        ScalarKey::Date(value) => serde_json::to_string(&value.to_string()).ok()?,
        ScalarKey::Datetime(value) => serde_json::to_string(&value.to_string()).ok()?,
        ScalarKey::Unixtime(value) => value.to_string(),
    })
}
