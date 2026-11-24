use crate::query::semantics::folding::{
    with_folded_candidate, with_folded_candidate_str, with_folded_value_str,
};
use crate::query::semantics::record::{RecordContext, metadata_value};
use crate::query::semantics::state::value_matches;

use memchr::memmem::Finder;

/// Substring pattern parsed once for a bound literal.
#[derive(Clone, Debug)]
pub(crate) enum PreparedContains {
    Literal(Finder<'static>),
    Unescaped(Finder<'static>),
    Wildcards(Vec<Finder<'static>>),
}

#[cfg(test)]
thread_local! {
    pub(crate) static PREPARED_CONTAINS_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

pub(crate) fn split_contains_wildcards(pattern: &str) -> (bool, Vec<String>) {
    let mut pieces = Vec::new();
    let mut current = String::new();
    let mut chars = pattern.chars().peekable();
    let mut wildcard = false;
    while let Some(ch) = chars.next() {
        if ch == '\\' && chars.peek() == Some(&'%') {
            current.push(chars.next().unwrap());
        } else if ch == '%' {
            pieces.push(std::mem::take(&mut current));
            wildcard = true;
        } else {
            current.push(ch);
        }
    }
    pieces.push(current);
    (wildcard, pieces)
}

impl PreparedContains {
    pub(crate) fn new(pattern: &str) -> Self {
        #[cfg(test)]
        PREPARED_CONTAINS_BUILDS.with(|count| count.set(count.get() + 1));
        if !pattern.as_bytes().contains(&b'%') {
            return Self::Literal(Finder::new(pattern).into_owned());
        }
        let (wildcard, mut pieces) = split_contains_wildcards(pattern);
        if wildcard {
            Self::Wildcards(
                pieces
                    .into_iter()
                    .filter(|piece| !piece.is_empty())
                    .map(|piece| Finder::new(&piece).into_owned())
                    .collect(),
            )
        } else {
            Self::Unescaped(Finder::new(&pieces.pop().unwrap()).into_owned())
        }
    }

    pub(crate) fn matches_bytes(&self, text: &[u8]) -> bool {
        match self {
            Self::Literal(finder) | Self::Unescaped(finder) => finder.find(text).is_some(),
            Self::Wildcards(pieces) => {
                let mut rest = text;
                for finder in pieces {
                    let Some(index) = finder.find(rest) else {
                        return false;
                    };
                    rest = &rest[index + finder.needle().len()..];
                }
                true
            }
        }
    }

    pub(crate) fn matches(&self, text: &str, _literal: &str) -> bool {
        self.matches_bytes(text.as_bytes())
    }
}

pub(crate) fn compare_scalar(lhs: &Value, rhs: &Value, op: Operator, transform: Transform) -> bool {
    if matches!(rhs, Value::Null) {
        return match op {
            Operator::Eq => matches!(lhs, Value::Null),
            Operator::Ne => !matches!(lhs, Value::Null),
            _ => false,
        };
    }
    if matches!(lhs, Value::Null) {
        return false;
    }
    if transform == Transform::CaseFold {
        let (Value::String(text), Value::String(pattern)) = (lhs, rhs) else {
            return false;
        };
        return with_folded_candidate_str(text, |folded| compare_folded_text(folded, pattern, op));
    }
    let order = match (lhs, rhs) {
        (Value::String(a), Value::String(b)) => Some(a.cmp(b)),
        (Value::Id(a), Value::Id(b)) => Some(a.cmp(b)),
        (Value::String(a), Value::Id(b)) | (Value::Id(a), Value::String(b)) => {
            Some(a.as_str().cmp(b.as_str()))
        }
        (Value::Number(a), Value::Number(b)) => Some(a.cmp(b)),
        (Value::Bool(a), Value::Bool(b)) => Some(a.cmp(b)),
        (Value::Date(a), Value::Date(b)) => Some(a.cmp(b)),
        (Value::Datetime(a), Value::Datetime(b)) => Some(a.cmp(b)),
        (Value::Unixtime(a), Value::Unixtime(b)) => Some(a.cmp(b)),
        _ => None,
    };
    match op {
        Operator::Eq => order == Some(Ordering::Equal),
        Operator::Ne => order.is_some_and(|o| o != Ordering::Equal),
        Operator::Gt => order == Some(Ordering::Greater),
        Operator::Ge => order.is_some_and(|o| o != Ordering::Less),
        Operator::Lt => order == Some(Ordering::Less),
        Operator::Le => order.is_some_and(|o| o != Ordering::Greater),
        Operator::Contains | Operator::NotContains => {
            let (Value::String(text), Value::String(pattern)) = (lhs, rhs) else {
                return false;
            };
            let found = contains_pattern(text, pattern);
            if op == Operator::Contains {
                found
            } else {
                !found
            }
        }
    }
}

/// `%` spans any sequence; `\%` means a literal percent. No other wildcard.
pub(crate) fn contains_pattern(text: &str, pattern: &str) -> bool {
    if !pattern.as_bytes().contains(&b'%') {
        return text.contains(pattern);
    }
    let (wildcard, pieces) = split_contains_wildcards(pattern);
    if !wildcard {
        return text.contains(&pieces[0]);
    }
    let mut rest = text;
    for piece in pieces.iter().filter(|piece| !piece.is_empty()) {
        let Some(index) = rest.find(piece) else {
            return false;
        };
        rest = &rest[index + piece.len()..];
    }
    true
}

pub(crate) fn comparable_logical_types(left: &LogicalType, right: &LogicalType) -> bool {
    left == right
        || matches!(
            (left, right),
            (LogicalType::String, LogicalType::Id) | (LogicalType::Id, LogicalType::String)
        )
}

fn compare_folded_text(folded: &str, pattern: &str, op: Operator) -> bool {
    let order = folded.cmp(pattern);
    match op {
        Operator::Eq => order.is_eq(),
        Operator::Ne => !order.is_eq(),
        Operator::Gt => order.is_gt(),
        Operator::Ge => !order.is_lt(),
        Operator::Lt => order.is_lt(),
        Operator::Le => !order.is_gt(),
        Operator::Contains => contains_pattern(folded, pattern),
        Operator::NotContains => !contains_pattern(folded, pattern),
    }
}

pub(crate) fn compare_bound(
    lhs: &Value,
    term: &BoundTerm,
    record_token: Option<(usize, usize)>,
) -> Result<bool, String> {
    if let Some(period) = &term.period {
        return Ok(period.matches(lhs, term.operator));
    }
    if let Some(matcher) = &term.matcher {
        return matcher.matches(Some(lhs), None);
    }
    if let (Some(prepared), Value::String(pattern)) = (&term.prepared_contains, &term.literal) {
        let Value::String(text) = lhs else {
            return Ok(false);
        };
        let found = if term.transform == Transform::CaseFold {
            if let Some(record_token) = record_token {
                with_folded_value_str(record_token, text, |folded| {
                    prepared.matches_bytes(folded.as_bytes())
                })
            } else {
                with_folded_candidate(text, |folded| prepared.matches_bytes(folded))
            }
        } else {
            prepared.matches(text, pattern)
        };
        return Ok(if term.operator == Operator::Contains {
            found
        } else {
            !found
        });
    }
    if let Some(record_token) = record_token
        && term.transform == Transform::CaseFold
        && let (Value::String(text), Value::String(pattern)) = (lhs, &term.literal)
    {
        return Ok(with_folded_value_str(record_token, text, |folded| {
            compare_folded_text(folded, &pattern, term.operator)
        }));
    }
    Ok(compare_scalar(
        lhs,
        &term.literal,
        term.operator,
        term.transform,
    ))
}

// None denotes a selected value that cannot satisfy a derived length
// comparison. It is distinct from a selected, explicit Value::Null.
type ScopedValues<'a> = Vec<(Vec<usize>, Option<Cow<'a, Value>>)>;

fn walk_scoped<'a>(
    value: &'a Value,
    steps: &[PathStep],
    positions: &mut Vec<usize>,
    output: &mut ScopedValues<'a>,
) -> bool {
    let Some(step) = steps.first() else {
        output.push((positions.clone(), Some(Cow::Borrowed(value))));
        return true;
    };
    match step {
        PathStep::Field(name) => match value {
            Value::Object(map) => map
                .get(name)
                .is_some_and(|value| walk_scoped(value, &steps[1..], positions, output)),
            Value::Array(array) => {
                let mut found = array.values.is_empty();
                for (index, item) in array.values.iter().enumerate() {
                    positions.push(index);
                    found |= walk_scoped(item, steps, positions, output);
                    positions.pop();
                }
                found
            }
            _ => false,
        },
        PathStep::ArrayIndex(index) => match value {
            Value::Array(array) => array
                .values
                .get(*index as usize)
                .is_some_and(|value| walk_scoped(value, &steps[1..], positions, output)),
            _ => false,
        },
        PathStep::EachElement => match value {
            Value::Array(array) => {
                let mut found = array.values.is_empty();
                for (index, item) in array.values.iter().enumerate() {
                    positions.push(index);
                    found |= walk_scoped(item, &steps[1..], positions, output);
                    positions.pop();
                }
                found
            }
            _ => false,
        },
    }
}

fn scoped_values<'a>(
    path: &FieldPath,
    query_path: &QueryPath,
    transform: Transform,
    derived: Derived,
    multi: bool,
    ctx: &'a RecordContext<'a>,
) -> Result<Option<ScopedValues<'a>>, String> {
    let mut values = Vec::new();
    let found = match path.root {
        PathRoot::Data => walk_scoped(ctx.value, &query_path.steps, &mut Vec::new(), &mut values),
        PathRoot::Metadata(field) => {
            let value = if field == MetadataField::RecordId && transform == Transform::CaseFold {
                Some(Value::String(ctx.record.record_id().0.to_string()))
            } else {
                metadata_value(field, ctx)?
            };
            let found = value.is_some();
            values.push((Vec::new(), value.map(Cow::Owned)));
            found
        }
        PathRoot::Backlinks => return Err("backlink path requires relation binding".into()),
    };
    if let Some(predicate) = derived.state_predicate() {
        let result = values.iter().any(|(_, value)| {
            value
                .as_deref()
                .is_some_and(|value| value_matches(value, predicate))
        });
        return Ok(Some(vec![(
            Vec::new(),
            Some(Cow::Owned(Value::Bool(result))),
        )]));
    }
    if !found {
        return Ok(None);
    }
    if derived == Derived::Length {
        return Ok(Some(
            values
                .into_iter()
                .map(|(scope, value)| {
                    let length = match value.as_deref() {
                        Some(Value::Array(array)) => Some(array.values.len()),
                        _ => None,
                    };
                    (
                        scope,
                        length.map(|length| {
                            Cow::Owned(Value::Number(Number::integer(length as i128)))
                        }),
                    )
                })
                .collect(),
        ));
    }
    if multi {
        let mut flat = Vec::new();
        for (scope, value) in values {
            match value {
                Some(Cow::Borrowed(Value::Array(array))) => {
                    for (index, item) in array.values.iter().enumerate() {
                        let mut scope = scope.clone();
                        scope.push(index);
                        flat.push((scope, Some(Cow::Borrowed(item))));
                    }
                }
                other => flat.push((scope, other)),
            }
        }
        return Ok(Some(flat));
    }
    Ok(Some(values))
}

fn mapped_field_value<'a>(
    value: &'a Value,
    transform: Transform,
    operation: Option<&Arc<dyn BindingOperation>>,
) -> Option<Cow<'a, Value>> {
    if let Some(operation) = operation {
        return operation.map_value(value).map(Cow::Owned);
    }
    match (value, transform) {
        (Value::String(text), Transform::CaseFold) => Some(Cow::Owned(Value::String(fold(text)))),
        (Value::Null, Transform::CaseFold) => Some(Cow::Borrowed(value)),
        (_, Transform::CaseFold) => None,
        _ => Some(Cow::Borrowed(value)),
    }
}

pub(crate) fn eval_field_term(
    term: &BoundTerm,
    rhs: &BoundFieldOperand,
    ctx: &RecordContext<'_>,
) -> Result<bool, String> {
    let paired = term.multi && rhs.multi;
    let Some(left) = scoped_values(
        &term.path,
        &term.query_path,
        term.transform,
        term.derived,
        term.multi && !paired,
        ctx,
    )?
    else {
        return Ok(false);
    };
    let Some(right) = scoped_values(
        &rhs.path,
        &rhs.query_path,
        rhs.transform,
        rhs.derived,
        rhs.multi && !paired,
        ctx,
    )?
    else {
        return Ok(false);
    };
    let compare = |a: Option<&Value>, b: Option<&Value>| {
        let (Some(a), Some(b)) = (a, b) else {
            return false;
        };
        let Some(a) = mapped_field_value(a, term.transform, term.operation.as_ref()) else {
            return false;
        };
        let Some(b) = mapped_field_value(b, rhs.transform, rhs.operation.as_ref()) else {
            return false;
        };
        if term.operator == Operator::Ne
            && (matches!(a.as_ref(), Value::Null) || matches!(b.as_ref(), Value::Null))
        {
            return matches!(a.as_ref(), Value::Null) != matches!(b.as_ref(), Value::Null);
        }
        compare_scalar(a.as_ref(), b.as_ref(), term.operator, Transform::Identity)
    };
    let mut results = Vec::new();
    if paired {
        let expected_scope_depth = explicit_scope(&term.query_path).len();
        if left.iter().chain(&right).any(|(scope, value)| {
            scope.len() != expected_scope_depth || matches!(value.as_deref(), Some(Value::Array(_)))
        }) {
            return Err(
                "ambiguous nested multivalued field comparison; use explicit array selectors"
                    .into(),
            );
        }
        let mut right_by_scope = HashMap::with_capacity(right.len());
        for (scope, value) in &right {
            if right_by_scope
                .insert(scope.as_slice(), value.as_deref())
                .is_some()
            {
                return Err("ambiguous right-hand field occurrence".into());
            }
        }
        for (scope, value) in &left {
            let matching = right_by_scope.remove(scope.as_slice());
            results.push(matching.is_some_and(|other| compare(value.as_deref(), other)));
        }
        results.extend(std::iter::repeat_n(false, right_by_scope.len()));
    } else if term.multi {
        let Some((_, scalar)) = right.first() else {
            return Ok(false);
        };
        results.extend(
            left.iter()
                .map(|(_, value)| compare(value.as_deref(), scalar.as_deref())),
        );
    } else if rhs.multi {
        let Some((_, scalar)) = left.first() else {
            return Ok(false);
        };
        results.extend(
            right
                .iter()
                .map(|(_, value)| compare(scalar.as_deref(), value.as_deref())),
        );
    } else {
        let (Some((_, a)), Some((_, b))) = (left.first(), right.first()) else {
            return Ok(false);
        };
        results.push(compare(a.as_deref(), b.as_deref()));
    }
    Ok(if term.quantifier == Quantifier::Any {
        results.into_iter().any(|result| result)
    } else {
        results.into_iter().all(|result| result)
    })
}

/// Compare a source field with a field on one explicitly reached target.
/// The caller retains source context for each occurrence and quantifies over
/// relation edges after this local comparison.
pub(crate) fn eval_cross_record_field_term(
    target: &BoundTerm,
    source: &BoundTerm,
    target_ctx: &RecordContext<'_>,
    source_ctx: &RecordContext<'_>,
    target_is_lhs: bool,
    source_positions: &[u32],
    source_selectors: &[Option<u32>],
) -> Result<bool, String> {
    let Some(target_values) = scoped_values(
        &target.path,
        &target.query_path,
        target.transform,
        target.derived,
        target.multi,
        target_ctx,
    )?
    else {
        return Ok(false);
    };
    let Some(source_values) = scoped_values(
        &source.path,
        &source.query_path,
        source.transform,
        source.derived,
        false,
        source_ctx,
    )?
    else {
        return Ok(false);
    };
    let source_value = if source.multi {
        let scope_depth = explicit_scope(&source.query_path).len();
        // Fixed selectors occupy edge positions but not source occurrence
        // scopes. Pair only the wildcard slots shared at binding.
        let positions = source_selectors
            .iter()
            .enumerate()
            .filter(|(_, selector)| selector.is_none())
            .take(scope_depth)
            .map(|(index, _)| {
                source_positions
                    .get(index)
                    .copied()
                    .ok_or("relation occurrence has fewer positions than the shared source scope")
            })
            .collect::<Result<Vec<_>, _>>()?;
        if positions.len() != scope_depth {
            return Err(
                "relation occurrence has fewer positions than the shared source scope".into(),
            );
        }
        let mut matching = source_values.iter().filter(|(scope, _)| {
            scope.len() == scope_depth
                && scope
                    .iter()
                    .zip(&positions)
                    .all(|(source, relation)| u32::try_from(*source) == Ok(*relation))
        });
        let selected = matching.next();
        if matching.next().is_some() {
            return Err("ambiguous source field occurrence".into());
        }
        selected.and_then(|(_, value)| value.as_deref())
    } else {
        if source_values.len() != 1 {
            return Err("source-dependent comparison requires one source value".into());
        }
        source_values[0].1.as_deref()
    };
    let Some(source_value) = source_value else {
        return Ok(false);
    };
    let Some(source_value) =
        mapped_field_value(source_value, source.transform, source.operation.as_ref())
    else {
        return Ok(false);
    };
    let mut answers = Vec::with_capacity(target_values.len());
    for (_, value) in target_values {
        let Some(value) = value.as_deref() else {
            answers.push(false);
            continue;
        };
        let Some(value) = mapped_field_value(value, target.transform, target.operation.as_ref())
        else {
            answers.push(false);
            continue;
        };
        let (lhs, rhs) = if target_is_lhs {
            (value.as_ref(), source_value.as_ref())
        } else {
            (source_value.as_ref(), value.as_ref())
        };
        answers.push(compare_scalar(
            lhs,
            rhs,
            target.operator,
            Transform::Identity,
        ));
    }
    Ok(if target.quantifier == Quantifier::Any {
        answers.into_iter().any(|answer| answer)
    } else {
        answers.into_iter().all(|answer| answer)
    })
}

/// `%` spans any sequence; `\%` means a literal percent. No other wildcard.
use super::*;
