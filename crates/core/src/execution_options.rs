//! Runtime result, ordering, and index alphabet options.

use serde::{Deserialize, Serialize};

use crate::model::FieldPath;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReturnMode {
    Locator,
    Bytes,
    Logical,
}

/// Field and direction requested by a query.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SortField {
    pub field: String,
    pub descending: bool,
    /// Typed expression for reductions and positional or transformed fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expression: Option<SortExpression>,
}

/// Fixed binary identity for one resident composite constituent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompositeSortPart {
    pub path: FieldPath,
    pub descending: bool,
}

pub(crate) fn simple_composite_part(field: &SortField) -> Option<CompositeSortPart> {
    let expression = field.expression.as_ref();
    if expression.is_some_and(|expr| expr.binding.is_some() || expr.reducer.is_some()) {
        return None;
    }
    Some(CompositeSortPart {
        path: FieldPath::parse(expression.map_or(field.field.as_str(), |expr| expr.path.as_str()))
            .ok()?,
        descending: field.descending,
    })
}

/// Reduction within one source record's reached sort values.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SortReducer {
    Min,
    Max,
    First,
    Last,
}

/// Programmatic sort expression, independent of its display spelling.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SortExpression {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reducer: Option<SortReducer>,
}

impl SortField {
    pub fn simple(field: impl Into<String>, descending: bool) -> Self {
        Self {
            field: field.into(),
            descending,
            expression: None,
        }
    }

    pub fn computed(expression: SortExpression, descending: bool) -> Self {
        Self {
            field: expression.path.clone(),
            descending,
            expression: Some(expression),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BigramAlphabet {
    Ascii,
    Unicode,
}

#[cfg(test)]
#[path = "execution_options.test.rs"]
mod tests;
