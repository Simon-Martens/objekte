//! Per-collection binding and safe candidates for rendered filter operands.

use super::*;
use crate::{Field, ValueType};


/// Necessary full-text candidates for a Boolean filter subtree.
pub(crate) enum FtsPlan {
    Literal(String),
    And(Vec<Self>),
    Or(Vec<Self>),
}


fn declared_format(fields: &BTreeMap<String, Field>, path: &JsonPath) -> Option<TextFormat> {
    let (JsonPathStep::Key(first), rest) = path.split_first()? else {
        return None;
    };
    let mut field = fields.get(first)?;
    if field.exclude_fts {
        return None;
    }
    for step in rest {
        // JSON paths can traverse arrays implicitly before the next key.
        while field.kind == ValueType::Array && matches!(step, JsonPathStep::Key(_)) {
            field = field.items.as_deref()?;
            if field.exclude_fts {
                return None;
            }
        }
        field = match step {
            JsonPathStep::Key(key) => field.properties.get(key)?,
            JsonPathStep::Index(_) | JsonPathStep::Each => field.items.as_deref()?,
        };
        if field.exclude_fts {
            return None;
        }
    }
    field.text_format
}

fn bind_field(field: &mut FieldOperand, fields: &BTreeMap<String, Field>) {
    if field.modifier == Some(Modifier::Fts) {
        field.modifier = declared_format(fields, &field.path).map(Modifier::Rendered);
    }
}

fn bind_operand(operand: &mut Operand, fields: &BTreeMap<String, Field>) {
    if let Operand::Field(field) = operand {
        bind_field(field, fields);
    }
}

fn bind_expr(expr: &mut Expr, fields: &BTreeMap<String, Field>) {
    match expr {
        Expr::And(parts) | Expr::Or(parts) => {
            for part in parts {
                bind_expr(part, fields);
            }
        }
        Expr::Compare { left, right, .. } => {
            bind_operand(left, fields);
            bind_operand(right, fields);
        }
        Expr::Exists(_) | Expr::Missing(_) => {}
    }
}


impl JsonFilter {
    /// Resolve `:fts` independently for each collection. An undeclared path
    /// becomes the ordinary stored-value comparison for that collection.
    pub(crate) fn for_fields(&self, fields: &BTreeMap<String, Field>) -> Self {
        let mut bound = self.clone();
        bind_expr(&mut bound.expr, fields);
        for field in &mut bound.scalar_paths {
            bind_field(field, fields);
        }
        bound
    }

    pub(crate) fn fts_plan(&self) -> Option<FtsPlan> {
        fn plan(expr: &Expr) -> Option<FtsPlan> {
            match expr {
                Expr::And(parts) => {
                    let children = parts.iter().filter_map(plan).collect::<Vec<_>>();
                    (!children.is_empty()).then_some(FtsPlan::And(children))
                }
                Expr::Or(parts) => {
                    let children = parts.iter().map(plan).collect::<Option<Vec<_>>>()?;
                    (!children.is_empty()).then_some(FtsPlan::Or(children))
                }
                Expr::Compare {
                    left:
                        Operand::Field(FieldOperand {
                            modifier: Some(Modifier::Rendered(format)),
                            ..
                        }),
                    op: Op::Contains,
                    right: Operand::Literal(Value::String(pattern)),
                    ignore_case,
                    ..
                } if *format != TextFormat::Plain => {
                    pattern::Like::new(pattern, false, *ignore_case)
                        .required_ascii_runs()
                        .into_iter()
                        .max_by_key(String::len)
                        .map(FtsPlan::Literal)
                }
                _ => None,
            }
        }
        plan(&self.expr)
    }
}
