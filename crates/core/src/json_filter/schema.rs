//! Match filter operands to declared collection fields.

use super::*;
use crate::schema::{Field, ValueType};


impl JsonFilter {
    pub(crate) fn follows_schema(&self, c: &crate::Collection) -> bool {
        follows(&self.expr, &c.schema.field)
    }
}


fn follows(expr: &Expr, root: &Field) -> bool {
    match expr {
        Expr::And(parts) | Expr::Or(parts) => parts.iter().all(|part| follows(part, root)),
        Expr::Exists(path) => resolve(root, path).is_some(),
        Expr::Compare {
            left,
            op,
            any,
            right,
            ..
        } => {
            if matches!((left, right), (Operand::Literal(_), Operand::Literal(_))) {
                return true;
            }
            let (Some(a), Some(b)) = (
                operand_type(root, left, *any),
                operand_type(root, right, *any),
            ) else {
                return false;
            };
            compatible(a, b, *op)
        }
    }
}

fn resolve<'a>(root: &'a Field, path: &[JsonPathStep]) -> Option<(&'a Field, bool)> {
    let mut field = root;
    let mut multiple = false;
    for step in path {
        if field.kind == ValueType::Any {
            continue;
        }
        match step {
            JsonPathStep::Key(name) => {
                while field.kind == ValueType::Array {
                    field = field.items.as_deref()?;
                    multiple = true;
                }
                field = field.properties.get(name)?;
            }
            JsonPathStep::Index(_) | JsonPathStep::Each => {
                if field.kind != ValueType::Array {
                    return None;
                }
                field = field.items.as_deref()?;
                multiple |= matches!(step, JsonPathStep::Each);
            }
        }
    }
    Some((field, multiple))
}

fn operand_type(root: &Field, operand: &Operand, any: bool) -> Option<ValueType> {
    match operand {
        Operand::Literal(value) => Some(match value {
            Value::Null => ValueType::Null,
            Value::Bool(_) => ValueType::Boolean,
            Value::Number(_) => ValueType::Number,
            Value::String(_) => ValueType::String,
            Value::Array(_) | Value::Object(_) => return None,
        }),
        Operand::Field(field) => {
            let (resolved, multiple) = resolve(root, &field.path)?;
            let kind = resolved.kind;
            let allowed_multiple = any || field.modifier == Some(Modifier::Each);
            if multiple && !allowed_multiple {
                return None;
            }
            match field.modifier {
                Some(Modifier::Length) if matches!(kind, ValueType::Array | ValueType::Any) => {
                    Some(ValueType::Number)
                }
                Some(Modifier::Lower) if matches!(kind, ValueType::String | ValueType::Any) => {
                    Some(ValueType::String)
                }
                Some(Modifier::Length | Modifier::Lower) => None,
                _ if kind == ValueType::Array && allowed_multiple => {
                    Some(resolved.items.as_deref()?.kind)
                }
                _ if kind == ValueType::Array => None,
                _ => Some(kind),
            }
        }
    }
}

fn compatible(a: ValueType, b: ValueType, op: Op) -> bool {
    if a == ValueType::Any || b == ValueType::Any {
        return true;
    }
    let numeric = |kind| matches!(kind, ValueType::Number | ValueType::Integer);
    match op {
        Op::Contains | Op::NotContains => a == ValueType::String && b == ValueType::String,
        Op::Lt | Op::Le | Op::Gt | Op::Ge => {
            (numeric(a) && numeric(b)) || (a == ValueType::String && b == ValueType::String)
        }
        Op::Eq | Op::Ne => {
            a == b || (numeric(a) && numeric(b)) || a == ValueType::Null || b == ValueType::Null
        }
    }
}
