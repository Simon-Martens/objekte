//! Optional condition builder for the authoritative structured filter text.

use objekte_search_core::model::{Number, PathRoot};
use objekte_search_core::query::{
    FilterExpr, Literal, Operand, Operator, Quantifier, Term, parse_structured_filter,
};
use objekte_search_core::query_path::QueryPath;


/// Scalar literal type selected explicitly for a builder value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueType {
    String,
    Number,
    Boolean,
    Null,
}


/// Comparisons represented by the initial scalar builder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Comparison {
    Equal,
    NotEqual,
    Greater,
    AtLeast,
    Less,
    AtMost,
}


impl Comparison {
    pub fn symbol(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "!=",
            Self::Greater => ">",
            Self::AtLeast => ">=",
            Self::Less => "<",
            Self::AtMost => "<=",
        }
    }

    fn from_operator(operator: Operator) -> Option<Self> {
        Some(match operator {
            Operator::Eq => Self::Equal,
            Operator::Ne => Self::NotEqual,
            Operator::Gt => Self::Greater,
            Operator::Ge => Self::AtLeast,
            Operator::Lt => Self::Less,
            Operator::Le => Self::AtMost,
            _ => return None,
        })
    }

    fn accepts(self, kind: ValueType) -> bool {
        matches!(self, Self::Equal | Self::NotEqual) || kind == ValueType::Number
    }
}


/// One editable Field / Operator / Value row; empty fields remain local drafts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConditionDraft {
    pub field: String,
    pub comparison: Comparison,
    /// None means the value control has not been completed, even for strings.
    pub value: Option<String>,
    pub value_type: Option<ValueType>,
}


impl Default for ConditionDraft {
    fn default() -> Self {
        Self {
            field: String::new(),
            comparison: Comparison::Equal,
            value: None,
            value_type: None,
        }
    }
}


/// Metadata for a bounded field suggestion; unknown types require a choice.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FieldChoice {
    pub path: String,
    pub value_type: Option<ValueType>,
}


/// What the helper may truthfully show for the current free-text expression.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BuilderStatus {
    Empty,
    Synchronized,
    Incomplete,
    Unsupported,
    Invalid(String),
}


/// The builder's local rows; the text input remains the source of truth.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BuilderState {
    pub status: BuilderStatus,
    pub rows: Vec<ConditionDraft>,
}


impl BuilderState {
    /// Rebuild from a text edit, including undo/redo and query restoration.
    /// A text edit supersedes incomplete builder-only rows.
    pub fn from_text(text: &str) -> Self {
        if text.trim().is_empty() {
            return Self {
                status: BuilderStatus::Empty,
                rows: vec![ConditionDraft::default()],
            };
        }
        let parsed = match parse_structured_filter(text) {
            Ok(parsed) => parsed,
            Err(error) => {
                if text.trim_start().starts_with('"')
                    && serde_json::from_str::<String>(text).is_ok()
                {
                    return Self {
                        status: BuilderStatus::Unsupported,
                        rows: Vec::new(),
                    };
                }
                return Self {
                    status: BuilderStatus::Invalid(error),
                    rows: Vec::new(),
                };
            }
        };
        if parsed.sort.is_some() {
            return Self {
                status: BuilderStatus::Unsupported,
                rows: Vec::new(),
            };
        }
        let Some(filter) = parsed.filter else {
            return Self {
                status: BuilderStatus::Empty,
                rows: vec![ConditionDraft::default()],
            };
        };
        let mut rows = Vec::new();
        if collect_conditions(&filter, &mut rows) {
            Self {
                status: BuilderStatus::Synchronized,
                rows,
            }
        } else {
            Self {
                status: BuilderStatus::Unsupported,
                rows: Vec::new(),
            }
        }
    }

    /// Keep an unfinished builder change local; return text only when every row
    /// can be serialized as one valid expression.
    pub fn complete_change(&mut self) -> Result<Option<String>, String> {
        if matches!(
            self.status,
            BuilderStatus::Unsupported | BuilderStatus::Invalid(_)
        ) {
            return Err("Edit the filter text before using the builder".into());
        }
        match serialize_conditions(&self.rows)? {
            Some(text) => {
                self.status = if text.is_empty() {
                    BuilderStatus::Empty
                } else {
                    BuilderStatus::Synchronized
                };
                Ok(Some(text))
            }
            None => {
                self.status = BuilderStatus::Incomplete;
                Ok(None)
            }
        }
    }

    pub fn add_condition(&mut self) {
        if matches!(
            self.status,
            BuilderStatus::Unsupported | BuilderStatus::Invalid(_)
        ) {
            return;
        }
        self.rows.push(ConditionDraft::default());
        self.status = BuilderStatus::Incomplete;
    }
}


fn collect_conditions(expr: &FilterExpr, rows: &mut Vec<ConditionDraft>) -> bool {
    match expr {
        FilterExpr::And(parts) => parts.iter().all(|part| collect_conditions(part, rows)),
        FilterExpr::Term(term) => {
            let Some(row) = condition_from_term(term) else {
                return false;
            };
            rows.push(row);
            true
        }
        FilterExpr::Or(_) | FilterExpr::Not(_) => false,
    }
}

fn condition_from_term(term: &Term) -> Option<ConditionDraft> {
    if !term.modifiers.is_empty()
        || term.quantifier != Quantifier::Default
        || term.path.has_explicit_array_step
        || !matches!(term.path.field.root, PathRoot::Data | PathRoot::Metadata(_))
    {
        return None;
    }
    let comparison = Comparison::from_operator(term.operator)?;
    let (value, value_type) = match &term.operand {
        Operand::Literal(Literal::String(value)) => (value.clone(), ValueType::String),
        Operand::Literal(Literal::Number(number)) => (format_number(number), ValueType::Number),
        Operand::Literal(Literal::Bool(value)) => (value.to_string(), ValueType::Boolean),
        Operand::Literal(Literal::Null) => (String::new(), ValueType::Null),
        _ => return None,
    };
    comparison.accepts(value_type).then(|| ConditionDraft {
        field: term.path.field.display(),
        comparison,
        value: Some(value),
        value_type: Some(value_type),
    })
}

fn format_number(number: &Number) -> String {
    match number {
        Number::Integer(value) => value.to_string(),
        Number::Float(value) => value.to_string(),
        Number::ExactDecimal(value) => value.to_string(),
    }
}

/// Serialize only complete, scalar conditions. `None` means at least one row
/// is unfinished; callers leave the existing text unchanged in that case.
pub fn serialize_conditions(rows: &[ConditionDraft]) -> Result<Option<String>, String> {
    if rows.is_empty() {
        return Ok(Some(String::new()));
    }
    let mut parts = Vec::with_capacity(rows.len());
    for row in rows {
        if row.field.trim().is_empty() || row.value_type.is_none() {
            return Ok(None);
        }
        let path = QueryPath::parse(row.field.trim())?;
        if path.has_explicit_array_step
            || !matches!(path.field.root, PathRoot::Data | PathRoot::Metadata(_))
        {
            return Err("The builder supports scalar field paths only".into());
        }
        let kind = row.value_type.expect("checked above");
        if !row.comparison.accepts(kind) {
            return Err("This comparison requires a numeric value".into());
        }
        let literal = match kind {
            ValueType::String => {
                let Some(value) = &row.value else {
                    return Ok(None);
                };
                serde_json::to_string(value).map_err(|error| error.to_string())?
            }
            ValueType::Number => {
                let Some(value) = row
                    .value
                    .as_ref()
                    .map(|value| value.trim())
                    .filter(|value| !value.is_empty())
                else {
                    return Ok(None);
                };
                // Use the query grammar for both plain numbers and exact byte
                // sizes (e.g. 23MB or 02GiB). Require exactly one numeric literal
                // so the value control cannot introduce another condition.
                let parsed = parse_structured_filter(&format!("value = {value}"))?;
                if parsed.sort.is_some()
                    || !matches!(
                        parsed.filter,
                        Some(FilterExpr::Term(Term {
                            operand: Operand::Literal(Literal::Number(_)),
                            ..
                        }))
                    )
                {
                    return Err("Expected a number or byte size such as 23MB or 2GiB".into());
                }
                value.to_owned()
            }
            ValueType::Boolean => match row.value.as_deref() {
                Some("true") => "true".into(),
                Some("false") => "false".into(),
                None => return Ok(None),
                _ => return Err("Boolean value must be true or false".into()),
            },
            ValueType::Null => "null".into(),
        };
        parts.push(format!(
            "{} {} {literal}",
            path.field.display(),
            row.comparison.symbol()
        ));
    }
    let text = parts.join(" && ");
    // Reparse serialized output to keep the helper subordinate to the grammar.
    parse_structured_filter(&text)?;
    Ok(Some(text))
}

#[cfg(test)]
#[path = "filter_builder.test.rs"]
mod tests;
