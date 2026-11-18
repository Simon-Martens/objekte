pub fn raw_fallback_contains(fts: &BoundFts, text: &str) -> bool {
    if let Some(regex) = &fts.fallback_regex {
        return regex.is_match(text);
    }
    match fts.case_sensitivity {
        CaseSensitivity::Sensitive => fts.fallback_finder.as_ref().map_or_else(
            || text.contains(&fts.source_literal),
            |finder| finder.find(text.as_bytes()).is_some(),
        ),
        CaseSensitivity::Insensitive => with_folded_candidate_str(text, |folded| {
            fts.fallback_finder.as_ref().map_or_else(
                || folded.contains(&fts.folded_source_literal),
                |finder| finder.find(folded.as_bytes()).is_some(),
            )
        }),
    }
}

pub fn eval_bound_fts(
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    evidence: &mut HashSet<String>,
) -> Result<bool, String> {
    let mut matched = false;
    if fts.fields.is_empty() {
        for segment in segments {
            if text_contains(
                &segment.text,
                &fts.literal,
                &fts.folded_literal,
                Transform::Identity,
                fts.raw_matcher.as_ref(),
                fts.case_sensitivity,
                fts.finder.as_ref(),
            )? {
                matched = true;
                evidence.insert(if segment.path.is_empty() {
                    "@root".into()
                } else {
                    segment.path.join(".")
                });
            }
        }
        return Ok(matched);
    }
    for field in &fts.fields {
        matched |=
            eval_bound_fts_field(field, fts.case_sensitivity, segments, ctx, Some(evidence))?;
    }
    Ok(matched)
}

/// Visit value-owned strings using only borrowed text and borrowed key names.
pub(crate) fn visit_borrowed_value_strings<'a>(
    value: &'a Value,
    path: &mut Vec<&'a str>,
    inspect: &mut impl FnMut(&[&str], &'a str) -> Result<bool, String>,
) -> Result<bool, String> {
    match value {
        Value::String(text) | Value::Id(text) => inspect(path, text),
        Value::Object(fields) => {
            let mut matched = false;
            for (name, child) in fields {
                path.push(name);
                matched |= visit_borrowed_value_strings(child, path, inspect)?;
                path.pop();
            }
            Ok(matched)
        }
        Value::Array(array) => {
            let mut matched = false;
            for child in &array.values {
                matched |= visit_borrowed_value_strings(child, path, inspect)?;
            }
            Ok(matched)
        }
        _ => Ok(false),
    }
}

/// Search strings in a logical value without constructing owned segments or paths.
/// This is valid for readers whose searchable corpus is the converted value.
pub(crate) fn eval_borrowed_value_fts(
    fts: &BoundFts,
    ctx: &RecordContext<'_>,
    mut evidence: Option<&mut HashSet<String>>,
) -> Result<bool, String> {
    let mut path = Vec::new();
    let mut matched = false;
    if fts.fields.is_empty() {
        let mut inspect = |path: &[&str], text: &str| {
            if !text_contains(
                text,
                &fts.literal,
                &fts.folded_literal,
                Transform::Identity,
                fts.raw_matcher.as_ref(),
                fts.case_sensitivity,
                fts.finder.as_ref(),
            )? {
                return Ok(false);
            }
            if let Some(evidence) = evidence.as_deref_mut() {
                evidence.insert(if path.is_empty() {
                    "@root".into()
                } else {
                    path.join(".")
                });
            }
            Ok(true)
        };
        return visit_borrowed_value_strings(ctx.value, &mut path, &mut inspect);
    }
    for field in &fts.fields {
        matched |= eval_borrowed_value_fts_field(
            field,
            fts.case_sensitivity,
            ctx,
            evidence.as_deref_mut(),
        )?;
    }
    Ok(matched)
}

pub(crate) fn eval_borrowed_value_fts_field(
    field: &BoundFtsField,
    case_sensitivity: CaseSensitivity,
    ctx: &RecordContext<'_>,
    mut evidence: Option<&mut HashSet<String>>,
) -> Result<bool, String> {
    if field.query_path.is_some() || !matches!(field.path.root, PathRoot::Data) {
        return eval_bound_fts_field(field, case_sensitivity, &[], ctx, evidence);
    }
    let mut path = Vec::new();
    let mut inspect = |path: &[&str], text: &str| {
        if !fts_field_selects_path(&field.path.segments, path) {
            return Ok(false);
        }
        if !text_contains(
            text,
            &field.literal,
            &field.folded_literal,
            field.transform,
            field.matcher.as_ref(),
            case_sensitivity,
            field.finder.as_ref(),
        )? {
            return Ok(false);
        }
        if let Some(evidence) = evidence.as_deref_mut() {
            evidence.insert(field.evidence_path.clone());
        }
        Ok(true)
    };
    visit_borrowed_value_strings(ctx.value, &mut path, &mut inspect)
}

/// An empty selected path covers every text segment, including nested ones.
pub(crate) fn fts_field_selects_path<T: AsRef<str>>(selected: &[String], actual: &[T]) -> bool {
    selected.is_empty()
        || (selected.len() == actual.len()
            && selected
                .iter()
                .zip(actual)
                .all(|(expected, actual)| expected == actual.as_ref()))
}

/// Match one source-owned scalar occurrence before duplicate resolution.
/// The caller retains evidence only for occurrences that remain visible.
pub(crate) fn match_borrowed_source_segment<T: AsRef<str>>(
    fts: &BoundFts,
    path: &[T],
    text: &str,
    evidence: &mut Vec<String>,
    retain_evidence: bool,
) -> Result<bool, String> {
    match_borrowed_source_segment_folded(fts, path, text, None, evidence, retain_evidence)
}

fn match_borrowed_source_segment_folded<T: AsRef<str>>(
    fts: &BoundFts,
    path: &[T],
    text: &str,
    folded: Option<&str>,
    evidence: &mut Vec<String>,
    retain_evidence: bool,
) -> Result<bool, String> {
    if fts.fields.is_empty() {
        let found = text_contains_with_folded(
            text,
            folded,
            &fts.literal,
            &fts.folded_literal,
            Transform::Identity,
            fts.raw_matcher.as_ref(),
            fts.case_sensitivity,
            fts.finder.as_ref(),
        )?;
        if found && retain_evidence {
            evidence.push(if path.is_empty() {
                "@root".into()
            } else {
                path.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(".")
            });
        }
        return Ok(found);
    }
    let mut matched = false;
    for field in &fts.fields {
        if !fts_field_selects_path(&field.path.segments, path) {
            continue;
        }
        if text_contains_with_folded(
            text,
            folded,
            &field.literal,
            &field.folded_literal,
            field.transform,
            field.matcher.as_ref(),
            fts.case_sensitivity,
            field.finder.as_ref(),
        )? {
            matched = true;
            if retain_evidence {
                evidence.push(field.evidence_path.clone());
            }
        }
    }
    Ok(matched)
}

/// Match a string selected by an eligible local contains predicate.
pub(crate) fn match_borrowed_source_filter(term: &BoundTerm, text: &str) -> bool {
    match_borrowed_source_filter_folded(term, text, None)
}

fn match_borrowed_source_filter_folded(term: &BoundTerm, text: &str, folded: Option<&str>) -> bool {
    if let Some(matcher) = &term.matcher {
        let value = Value::String(text.to_owned());
        return matcher.matches(Some(&value), None).unwrap_or(false);
    }
    let Some(prepared) = &term.prepared_contains else {
        return false;
    };
    let found = if term.transform == Transform::CaseFold {
        if let Some(folded) = folded {
            prepared.matches_bytes(folded.as_bytes())
        } else {
            with_folded_candidate(text, |folded| prepared.matches_bytes(folded))
        }
    } else {
        prepared.matches_bytes(text.as_bytes())
    };
    if term.operator == Operator::Contains {
        found
    } else {
        !found
    }
}

/// Apply built-in FTS and FILTER matchers while one decoded source segment
/// and, when needed, one case-folded representation are alive.
pub(crate) fn match_borrowed_source_pair<T: AsRef<str>>(
    fts: &BoundFts,
    path: &[T],
    text: &str,
    filter: &BoundTerm,
    evidence: &mut Vec<String>,
    retain_evidence: bool,
) -> Result<(bool, bool), String> {
    let fts_needs_fold = if fts.fields.is_empty() {
        fts.raw_matcher.is_none() && fts.case_sensitivity == CaseSensitivity::Insensitive
    } else {
        fts.fields.iter().any(|field| {
            fts_field_selects_path(&field.path.segments, path)
                && field.matcher.is_none()
                && (field.transform == Transform::CaseFold
                    || fts.case_sensitivity == CaseSensitivity::Insensitive)
        })
    };
    let need_fold = filter.transform == Transform::CaseFold || fts_needs_fold;
    let apply = |folded: Option<&str>, evidence: &mut Vec<String>| {
        let fts_hit = match_borrowed_source_segment_folded(
            fts,
            path,
            text,
            folded,
            evidence,
            retain_evidence,
        )?;
        let filter_hit = match_borrowed_source_filter_folded(filter, text, folded);
        Ok((fts_hit, filter_hit))
    };
    if need_fold {
        with_folded_candidate_str(text, |folded| apply(Some(folded), evidence))
    } else {
        apply(None, evidence)
    }
}

/// Evaluate one borrowed selected target, retaining every matched path when requested.
pub(crate) fn eval_bound_fts_field(
    field: &BoundFtsField,
    case_sensitivity: CaseSensitivity,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    mut evidence: Option<&mut HashSet<String>>,
) -> Result<bool, String> {
    let mut matched = false;
    if let PathRoot::Metadata(metadata) = field.path.root {
        if let Value::String(text) | Value::Id(text) = metadata_value(metadata, ctx)?
            && text_contains(
                &text,
                &field.literal,
                &field.folded_literal,
                field.transform,
                field.matcher.as_ref(),
                case_sensitivity,
                field.finder.as_ref(),
            )?
        {
            matched = true;
            if let Some(evidence) = evidence.as_deref_mut() {
                evidence.insert(field.evidence_path.clone());
            }
        }
    } else if let Some(path) = &field.query_path {
        let mut selected = Vec::new();
        walk_query_steps(ctx.value, &path.steps, &mut selected);
        for value in selected {
            if let Value::String(text) = value
                && text_contains(
                    text,
                    &field.literal,
                    &field.folded_literal,
                    field.transform,
                    field.matcher.as_ref(),
                    case_sensitivity,
                    field.finder.as_ref(),
                )?
            {
                matched = true;
                if let Some(evidence) = evidence.as_deref_mut() {
                    evidence.insert(field.evidence_path.clone());
                } else {
                    return Ok(true);
                }
            }
        }
    } else {
        for segment in segments
            .iter()
            .filter(|segment| fts_field_selects_path(&field.path.segments, &segment.path))
        {
            if text_contains(
                &segment.text,
                &field.literal,
                &field.folded_literal,
                field.transform,
                field.matcher.as_ref(),
                case_sensitivity,
                field.finder.as_ref(),
            )? {
                matched = true;
                if let Some(evidence) = evidence.as_deref_mut() {
                    evidence.insert(field.evidence_path.clone());
                } else {
                    return Ok(true);
                }
            }
        }
    }
    Ok(matched)
}

/// Membership-only FTS skips path materialization and stops at the first match.
pub(crate) fn eval_bound_fts_membership(
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
) -> Result<bool, String> {
    if fts.fields.is_empty() {
        for segment in segments {
            if text_contains(
                &segment.text,
                &fts.literal,
                &fts.folded_literal,
                Transform::Identity,
                fts.raw_matcher.as_ref(),
                fts.case_sensitivity,
                fts.finder.as_ref(),
            )? {
                return Ok(true);
            }
        }
        return Ok(false);
    }
    for field in &fts.fields {
        if eval_bound_fts_field(field, fts.case_sensitivity, segments, ctx, None)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn eval_fts_query(
    fts: &FtsQuery,
    schema: &Schema,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    evidence: &mut HashSet<String>,
) -> Result<bool, String> {
    let bound = bind_fts(fts, schema)?;
    eval_bound_fts(&bound, segments, ctx, evidence)
}

use super::*;

pub fn eval_fts(
    literal: &str,
    selected: &[FieldPath],
    segments: &[TextSegment],
    evidence: &mut HashSet<String>,
) -> bool {
    let mut matched = false;
    for segment in segments {
        if selected.is_empty()
            || selected.iter().any(|p| {
                p.root == PathRoot::Data && (p.segments.is_empty() || p.segments == segment.path)
            })
        {
            if segment.text.contains(literal) {
                matched = true;
                evidence.insert(if segment.path.is_empty() {
                    "@root".into()
                } else {
                    segment.path.join(".")
                });
            }
        }
    }
    matched
}
