/// One selected FTS target with its literal prepared for the bound operation.
#[derive(Clone, Debug)]
pub struct BoundFtsField {
    pub path: FieldPath,
    pub(crate) query_path: Option<QueryPath>,
    pub(crate) related: Option<Vec<crate::query::binding::relation::BoundRelationHop>>,
    pub(crate) evidence_path: String,
    pub binding_name: String,
    pub literal: String,
    pub folded_literal: String,
    pub(crate) finder: Option<Finder<'static>>,
    pub(crate) gram_request: BigramRequest,
    pub transform: Transform,
    pub operation: Option<Arc<dyn BindingOperation>>,
    pub matcher: Option<Arc<dyn PreparedTextMatcher>>,
}

/// Collection-specific FTS bindings, including the configured raw target.
#[derive(Clone, Debug)]
pub struct BoundFts {
    pub case_sensitivity: CaseSensitivity,
    pub mode: FtsMode,
    pub literal: String,
    pub source_literal: String,
    pub folded_literal: String,
    pub folded_source_literal: String,
    pub(crate) finder: Option<Finder<'static>>,
    pub(crate) fallback_finder: Option<Finder<'static>>,
    pub(crate) gram_request: BigramRequest,
    pub(crate) fallback_gram_request: BigramRequest,
    pub raw_operation: Option<Arc<dyn BindingOperation>>,
    pub raw_matcher: Option<Arc<dyn PreparedTextMatcher>>,
    pub(crate) fallback_regex: Option<CompiledRegex>,
    pub fields: Vec<BoundFtsField>,
}

fn logical_type_retained_heap_bytes(kind: &LogicalType) -> usize {
    match kind {
        LogicalType::Array(inner) => std::mem::size_of::<LogicalType>()
            .saturating_add(logical_type_retained_heap_bytes(inner)),
        _ => 0,
    }
}

pub(crate) fn value_retained_heap_bytes(value: &Value) -> usize {
    match value {
        Value::String(text) | Value::Id(text) => text.capacity(),
        Value::Array(array) => array
            .values
            .capacity()
            .saturating_mul(std::mem::size_of::<Value>())
            .saturating_add(
                array
                    .values
                    .iter()
                    .map(value_retained_heap_bytes)
                    .fold(0usize, usize::saturating_add),
            )
            .saturating_add(array.element_type.as_ref().map_or(0, |kind| {
                std::mem::size_of::<LogicalType>()
                    .saturating_add(logical_type_retained_heap_bytes(kind))
            })),
        Value::Object(fields) => fields
            .len()
            .saturating_mul(
                std::mem::size_of::<(String, Value)>() + 4 * std::mem::size_of::<usize>(),
            )
            .saturating_add(
                fields
                    .iter()
                    .map(|(name, value)| {
                        name.capacity()
                            .saturating_add(value_retained_heap_bytes(value))
                    })
                    .fold(0usize, usize::saturating_add),
            ),
        _ => 0,
    }
}

pub fn bind_fts(fts: &FtsQuery, schema: &Schema) -> Result<BoundFts, String> {
    if let Some(name) = &fts.pattern_parameter {
        return Err(format!(
            "regex pattern parameter ${name} requires execution bindings"
        ));
    }
    if let FtsMode::Regex(options) = fts.mode {
        return bind_regex_fts(fts, schema, options);
    }
    let raw_operation = fts
        .fields
        .is_empty()
        .then(|| schema.raw.as_ref())
        .flatten()
        .map(|binding| binding.operation.clone());
    if let Some(op) = &raw_operation {
        if !op.accepts(&LogicalType::String, Operator::Contains, false) {
            return Err("raw FTS binding does not support substring matching".into());
        }
    }
    let literal = raw_operation
        .as_ref()
        .map_or(Ok(fts.literal.clone()), |op| {
            op.prepare_fts_literal(&fts.literal)
        })?;
    let folded_literal = fold(&literal);
    let folded_source_literal = fold(&fts.literal);
    let raw_matcher = raw_operation
        .as_ref()
        .map(|op| {
            op.prepare_text_matcher(&literal).and_then(|prepared| {
                if fts.case_sensitivity == CaseSensitivity::Insensitive && prepared.is_some() {
                    return Err("custom raw FTS matcher does not support insensitive mode".into());
                }
                Ok(prepared.unwrap_or_else(|| {
                    Arc::new(MappedTextMatcher::new(
                        op.clone(),
                        if fts.case_sensitivity == CaseSensitivity::Insensitive {
                            folded_literal.clone()
                        } else {
                            literal.clone()
                        },
                        fts.case_sensitivity,
                    ))
                }))
            })
        })
        .transpose()?;
    let finder = raw_matcher.is_none().then(|| {
        Finder::new(if fts.case_sensitivity == CaseSensitivity::Insensitive {
            &folded_literal
        } else {
            &literal
        })
        .into_owned()
    });
    let fallback_finder = Finder::new(if fts.case_sensitivity == CaseSensitivity::Insensitive {
        &folded_source_literal
    } else {
        &fts.literal
    })
    .into_owned();
    let mut fields = Vec::new();
    for selected in &fts.fields {
        if selected.query_path.is_some() {
            return Err("selected FTS array selectors require a declared relation hop".into());
        }
        let definition = schema.field_or_dynamic(&selected.path)?;
        if !matches!(definition.kind, LogicalType::String)
            && !matches!(definition.kind, LogicalType::Array(ref inner) if **inner == LogicalType::String)
            && definition.kind != LogicalType::Unknown
            && selected.path.root != PathRoot::Metadata(MetadataField::RecordId)
        {
            return Err("FTS field requires string values".into());
        }
        let name = selected
            .binding
            .clone()
            .or_else(|| schema.defaults.get(&selected.path.segments).cloned())
            .unwrap_or_else(|| "default".into());
        let key = (selected.path.segments.clone(), name.clone());
        let (transform, operation) = if name == "default" {
            (Transform::Identity, None)
        } else if name == "lower" {
            (Transform::CaseFold, None)
        } else if let Some(binding) = schema.operations.get(&key) {
            if !binding.operation.accepts(
                &LogicalType::String,
                Operator::Contains,
                definition.multi || matches!(definition.kind, LogicalType::Array(_)),
            ) {
                return Err(format!("unsupported FTS operation for binding {name}"));
            }
            (Transform::Identity, Some(binding.operation.clone()))
        } else if let Some(binding) = schema.custom.get(&key) {
            (binding.transform, None)
        } else {
            return Err(format!("unknown FTS binding {name}"));
        };
        let needle = if let Some(op) = &operation {
            op.prepare_fts_literal(&fts.literal)?
        } else if transform == Transform::CaseFold {
            fold(&fts.literal)
        } else {
            fts.literal.clone()
        };
        let folded_needle = fold(&needle);
        let matcher = operation
            .as_ref()
            .map(|op| {
                op.prepare_text_matcher(&needle).and_then(|prepared| {
                    if fts.case_sensitivity == CaseSensitivity::Insensitive && prepared.is_some() {
                        return Err(format!(
                            "custom FTS matcher for {name} does not support insensitive mode"
                        ));
                    }
                    Ok(prepared.unwrap_or_else(|| {
                        Arc::new(MappedTextMatcher::new(
                            op.clone(),
                            if fts.case_sensitivity == CaseSensitivity::Insensitive {
                                folded_needle.clone()
                            } else {
                                needle.clone()
                            },
                            fts.case_sensitivity,
                        ))
                    }))
                })
            })
            .transpose()?;
        let finder = matcher.is_none().then(|| {
            Finder::new(if fts.case_sensitivity == CaseSensitivity::Insensitive {
                &folded_needle
            } else {
                &needle
            })
            .into_owned()
        });
        fields.push(BoundFtsField {
            path: selected.path.clone(),
            query_path: None,
            related: None,
            evidence_path: selected.path.display(),
            binding_name: name,
            gram_request: BigramRequest::from_folded(&folded_needle),
            folded_literal: folded_needle,
            literal: needle,
            finder,
            transform,
            operation,
            matcher,
        });
    }
    Ok(BoundFts {
        case_sensitivity: fts.case_sensitivity,
        mode: FtsMode::Literal,
        gram_request: BigramRequest::from_folded(&folded_literal),
        fallback_gram_request: BigramRequest::from_folded(&folded_source_literal),
        folded_literal,
        folded_source_literal,
        finder,
        fallback_finder: Some(fallback_finder),
        literal,
        source_literal: fts.literal.clone(),
        raw_operation,
        raw_matcher,
        fallback_regex: None,
        fields,
    })
}

fn bind_regex_fts(
    fts: &FtsQuery,
    schema: &Schema,
    options: crate::query::RegexOptions,
) -> Result<BoundFts, String> {
    let compiled = CompiledRegex::compile(&fts.literal, options, fts.case_sensitivity)?;
    let raw_operation = fts
        .fields
        .is_empty()
        .then(|| schema.raw.as_ref())
        .flatten()
        .map(|binding| binding.operation.clone());
    if raw_operation
        .as_ref()
        .is_some_and(|operation| !operation.regex_text_compatible())
    {
        return Err("raw FTS binding does not support regex mode".into());
    }
    let raw_matcher = fts.fields.is_empty().then(|| {
        Arc::new(RegexTextMatcher {
            regex: compiled.clone(),
            transform: Transform::Identity,
            operation: raw_operation.clone(),
        }) as Arc<dyn PreparedTextMatcher>
    });
    let mut fields = Vec::with_capacity(fts.fields.len());
    for selected in &fts.fields {
        if selected.query_path.is_some() {
            return Err("selected FTS array selectors require a declared relation hop".into());
        }
        let definition = schema.field_or_dynamic(&selected.path)?;
        if !matches!(definition.kind, LogicalType::String)
            && !matches!(definition.kind, LogicalType::Array(ref inner) if **inner == LogicalType::String)
            && definition.kind != LogicalType::Unknown
            && selected.path.root != PathRoot::Metadata(MetadataField::RecordId)
        {
            return Err("FTS field requires string values".into());
        }
        let name = selected
            .binding
            .clone()
            .or_else(|| schema.defaults.get(&selected.path.segments).cloned())
            .unwrap_or_else(|| "default".into());
        let key = (selected.path.segments.clone(), name.clone());
        let (transform, operation) = if name == "default" {
            (Transform::Identity, None)
        } else if name == "lower" {
            (Transform::CaseFold, None)
        } else if let Some(binding) = schema.operations.get(&key) {
            if !binding.operation.regex_text_compatible() {
                return Err(format!("FTS binding {name} does not support regex mode"));
            }
            (Transform::Identity, Some(binding.operation.clone()))
        } else if let Some(binding) = schema.custom.get(&key) {
            (binding.transform, None)
        } else {
            return Err(format!("unknown FTS binding {name}"));
        };
        fields.push(BoundFtsField {
            path: selected.path.clone(),
            query_path: None,
            related: None,
            evidence_path: selected.path.display(),
            binding_name: name,
            literal: fts.literal.clone(),
            folded_literal: String::new(),
            finder: None,
            gram_request: BigramRequest::new(""),
            transform,
            operation: operation.clone(),
            matcher: Some(Arc::new(RegexTextMatcher {
                regex: compiled.clone(),
                transform,
                operation,
            })),
        });
    }
    Ok(BoundFts {
        case_sensitivity: fts.case_sensitivity,
        mode: fts.mode,
        literal: fts.literal.clone(),
        source_literal: fts.literal.clone(),
        folded_literal: String::new(),
        folded_source_literal: String::new(),
        finder: None,
        fallback_finder: None,
        gram_request: BigramRequest::new(""),
        fallback_gram_request: BigramRequest::new(""),
        raw_operation,
        raw_matcher,
        fallback_regex: Some(compiled),
        fields,
    })
}

pub(crate) fn text_contains(
    text: &str,
    needle: &str,
    folded_needle: &str,
    transform: Transform,
    matcher: Option<&Arc<dyn PreparedTextMatcher>>,
    case_sensitivity: CaseSensitivity,
    finder: Option<&Finder<'static>>,
) -> Result<bool, String> {
    text_contains_with_folded(
        text,
        None,
        needle,
        folded_needle,
        transform,
        matcher,
        case_sensitivity,
        finder,
    )
}

pub(crate) fn text_contains_with_folded(
    text: &str,
    folded: Option<&str>,
    needle: &str,
    folded_needle: &str,
    transform: Transform,
    matcher: Option<&Arc<dyn PreparedTextMatcher>>,
    case_sensitivity: CaseSensitivity,
    finder: Option<&Finder<'static>>,
) -> Result<bool, String> {
    if let Some(matcher) = matcher {
        matcher.matches(text)
    } else if transform == Transform::CaseFold || case_sensitivity == CaseSensitivity::Insensitive {
        let needle = if case_sensitivity == CaseSensitivity::Insensitive {
            folded_needle
        } else {
            needle
        };
        let matches = |folded: &str| {
            finder.map_or_else(
                || folded.contains(needle),
                |finder| finder.find(folded.as_bytes()).is_some(),
            )
        };
        Ok(if let Some(folded) = folded {
            matches(folded)
        } else {
            with_folded_candidate_str(text, matches)
        })
    } else {
        Ok(finder.map_or_else(
            || text.contains(needle),
            |finder| finder.find(text.as_bytes()).is_some(),
        ))
    }
}

use crate::query::binding::matcher::{MappedTextMatcher, RegexTextMatcher};

use super::*;
