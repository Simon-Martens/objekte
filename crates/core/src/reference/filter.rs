pub fn eval_term(term: &BoundTerm, ctx: &RecordContext<'_>) -> Result<bool, String> {
    #[cfg(test)]
    if let Some(result) = crate::query::semantics::test_matcher::alternate_term_result(term, ctx) {
        return result;
    }
    if let Some(rhs) = &term.rhs {
        return eval_field_term(term, rhs, ctx);
    }
    if let Some(matcher) = &term.matcher {
        if matcher.input() == ExactInput::OriginalRecordBytes {
            return matcher.matches(None, ctx.source_bytes);
        }
    }
    if let PathRoot::Metadata(field) = term.path.root {
        if term.derived == Derived::None && !term.multi {
            let value = if field == MetadataField::RecordId && term.transform == Transform::CaseFold
            {
                Value::String(ctx.record.id.0.to_string())
            } else {
                metadata_value(field, ctx)?
            };
            if matches!(value, Value::Null) && !matches!(term.literal, Value::Null) {
                return Ok(false);
            }
            return compare_bound(&value, term, None);
        }
    }
    let owned;
    let mut selected = if let PathRoot::Metadata(field) = term.path.root {
        owned = if field == MetadataField::RecordId && term.transform == Transform::CaseFold {
            Value::String(ctx.record.id.0.to_string())
        } else {
            metadata_value(field, ctx)?
        };
        Some(vec![&owned])
    } else {
        select_values(term, ctx)?
    };
    if let Some(predicate) = term.derived.state_predicate() {
        let result = selected_matches(selected.as_deref(), predicate);
        return Ok(compare_scalar(
            &Value::Bool(result),
            &term.literal,
            term.operator,
            Transform::Identity,
        ));
    }
    let Some(mut values) = selected.take() else {
        return Ok(false);
    };
    if term.derived == Derived::Length {
        let compare_length = |value: &&Value| {
            let Value::Array(array) = value else {
                return false;
            };
            compare_scalar(
                &Value::Number(Number::integer(array.values.len() as i128)),
                &term.literal,
                term.operator,
                Transform::Identity,
            )
        };
        return Ok(if term.multi {
            if term.quantifier == Quantifier::Any {
                values.iter().any(compare_length)
            } else {
                values.iter().all(compare_length)
            }
        } else {
            values.first().is_some_and(|value| compare_length(value))
        });
    }
    if term.multi {
        let mut flattened = Vec::new();
        for value in values {
            if let Value::Array(array) = value {
                flattened.extend(array.values.iter());
            } else {
                flattened.push(&value);
            }
        }
        values = flattened;
    }
    if values.len() == 1 && matches!(values[0], Value::Null) && !matches!(term.literal, Value::Null)
    {
        return Ok(false);
    }
    let record_token = (term.path.root == PathRoot::Data).then_some((
        ctx.record as *const RecordEntry as usize,
        ctx.value as *const Value as usize,
    ));
    if term.multi {
        if term.quantifier == Quantifier::Any {
            for value in values {
                if compare_bound(value, term, record_token)? {
                    return Ok(true);
                }
            }
            Ok(false)
        } else {
            for value in values {
                if !compare_bound(value, term, record_token)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
    } else {
        values
            .first()
            .map_or(Ok(false), |value| compare_bound(value, term, record_token))
    }
}

pub fn eval_filter(
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    evidence: &mut HashSet<String>,
) -> Result<bool, String> {
    Ok(match expr {
        BoundExpr::Term(term) => {
            let match_ = eval_term(term, ctx)?;
            if match_
                && term.derived == Derived::None
                && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
            }
            match_
        }
        BoundExpr::And(parts) => {
            let mut result = true;
            let mut branch_evidence = HashSet::new();
            for p in parts {
                if !result {
                    break;
                }
                result &= eval_filter(p, ctx, &mut branch_evidence)?;
            }
            if result {
                evidence.extend(branch_evidence);
            }
            result
        }
        BoundExpr::Or(parts) => {
            let mut result = false;
            for p in parts {
                let mut branch_evidence = HashSet::new();
                if eval_filter(p, ctx, &mut branch_evidence)? {
                    result = true;
                    evidence.extend(branch_evidence);
                }
            }
            result
        }
        BoundExpr::Not(inner) => !eval_filter(inner, ctx, &mut HashSet::new())?,
    })
}

/// Three-valued filter evaluation when a record has no trusted logical data.
pub fn eval_unavailable_filter(
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    evidence: &mut HashSet<String>,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            if term.related.is_some()
                || term.path.root == PathRoot::Data
                || term.path.root == PathRoot::Backlinks
                || term
                    .rhs
                    .as_ref()
                    .is_some_and(|rhs| rhs.path.root == PathRoot::Data)
            {
                return Ok(None);
            }
            let matched = eval_term(term, ctx)?;
            if matched
                && term.derived == Derived::None
                && !matches!(term.operator, Operator::Ne | Operator::NotContains)
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
            }
            Ok(Some(matched))
        }
        BoundExpr::Not(inner) => {
            Ok(eval_unavailable_filter(inner, ctx, &mut HashSet::new())?.map(|value| !value))
        }
        BoundExpr::And(parts) => {
            let mut unknown = false;
            let mut branch = HashSet::new();
            for part in parts {
                match eval_unavailable_filter(part, ctx, &mut branch)? {
                    Some(false) => return Ok(Some(false)),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            if unknown {
                Ok(None)
            } else {
                evidence.extend(branch);
                Ok(Some(true))
            }
        }
        BoundExpr::Or(parts) => {
            let mut unknown = false;
            let mut matched = false;
            for part in parts {
                let mut branch = HashSet::new();
                match eval_unavailable_filter(part, ctx, &mut branch)? {
                    Some(true) => {
                        matched = true;
                        evidence.extend(branch);
                    }
                    None => unknown = true,
                    Some(false) => {}
                }
            }
            if matched {
                Ok(Some(true))
            } else if unknown {
                Ok(None)
            } else {
                Ok(Some(false))
            }
        }
    }
}

/// Three-valued membership on raw fallback records without evidence storage.
pub(crate) fn eval_unavailable_filter_membership(
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            if term.related.is_some()
                || term.path.root == PathRoot::Data
                || term.path.root == PathRoot::Backlinks
                || term
                    .rhs
                    .as_ref()
                    .is_some_and(|rhs| rhs.path.root == PathRoot::Data)
            {
                Ok(None)
            } else {
                eval_term(term, ctx).map(Some)
            }
        }
        BoundExpr::Not(inner) => {
            Ok(eval_unavailable_filter_membership(inner, ctx)?.map(|value| !value))
        }
        BoundExpr::And(parts) => {
            let mut unknown = false;
            for part in parts {
                match eval_unavailable_filter_membership(part, ctx)? {
                    Some(false) => return Ok(Some(false)),
                    None => unknown = true,
                    Some(true) => {}
                }
            }
            Ok(if unknown { None } else { Some(true) })
        }
        BoundExpr::Or(parts) => {
            let mut unknown = false;
            for part in parts {
                match eval_unavailable_filter_membership(part, ctx)? {
                    Some(true) => return Ok(Some(true)),
                    None => unknown = true,
                    Some(false) => {}
                }
            }
            Ok(if unknown { None } else { Some(false) })
        }
    }
}

use super::*;
use crate::schema::ExactInput;
