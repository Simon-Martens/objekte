//! Test-only guard and registry for an alternate exact-term evaluator.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::query::binding::filter::BoundTerm;
use crate::query::semantics::record::RecordContext;
use crate::schema::operation::Transform;

struct AlternateMatcher {
    field: String,
    literal: String,
    invocations: AtomicU64,
}

pub(crate) struct AlternateMatcherGuard;

impl Drop for AlternateMatcherGuard {
    fn drop(&mut self) {
        *registry()
            .lock()
            .expect("alternate matcher registry poisoned") = None;
    }
}

pub(crate) fn install_alternate_matcher(field: &str, literal: &str) -> AlternateMatcherGuard {
    *registry()
        .lock()
        .expect("alternate matcher registry poisoned") = Some(AlternateMatcher {
        field: field.into(),
        literal: literal.into(),
        invocations: AtomicU64::new(0),
    });
    AlternateMatcherGuard
}

pub(crate) fn alternate_matcher_invocations() -> u64 {
    registry()
        .lock()
        .expect("alternate matcher registry poisoned")
        .as_ref()
        .map_or(0, |matcher| matcher.invocations.load(Ordering::Relaxed))
}

fn matcher_accepts(matcher: &AlternateMatcher, term: &BoundTerm) -> bool {
    term.path.root == crate::model::PathRoot::Data
        && term.path.segments.len() == 1
        && term.path.segments[0] == matcher.field
        && term.operator == crate::query::Operator::Contains
        && term.transform == Transform::Identity
        && matches!(
            &term.literal,
            crate::model::Value::String(literal) if literal == &matcher.literal
        )
}

pub(crate) fn alternate_term_matches(term: &BoundTerm) -> bool {
    registry()
        .lock()
        .expect("alternate matcher registry poisoned")
        .as_ref()
        .is_some_and(|matcher| matcher_accepts(matcher, term))
}

fn registry() -> &'static Mutex<Option<AlternateMatcher>> {
    static MATCHER: OnceLock<Mutex<Option<AlternateMatcher>>> = OnceLock::new();
    MATCHER.get_or_init(|| Mutex::new(None))
}

pub(crate) fn alternate_term_result(
    term: &BoundTerm,
    context: &RecordContext<'_>,
) -> Option<Result<bool, String>> {
    let mut registry = registry()
        .lock()
        .expect("alternate matcher registry poisoned");
    let matcher = registry.as_mut()?;
    if !matcher_accepts(matcher, term) {
        return None;
    }
    matcher.invocations.fetch_add(1, Ordering::Relaxed);
    let crate::model::Value::String(literal) = &term.literal else {
        unreachable!()
    };
    let selected = match context.value {
        crate::model::Value::Object(object) => object.get(&matcher.field),
        _ => None,
    };
    Some(Ok(match selected {
        Some(crate::model::Value::String(text)) => {
            literal.is_empty()
                || text
                    .as_bytes()
                    .windows(literal.len())
                    .any(|window| window == literal.as_bytes())
        }
        _ => false,
    }))
}
