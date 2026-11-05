//! Per-collection binding and safe candidates for rendered filter operands.

use super::*;
use crate::{
    Field, TextMode, ValueType,
    extensions::{BoundTextQuery, ScalarValue, TextComparison, TextCompileRequest, TextSyntax},
    text::TextPattern,
};
use std::sync::Arc;


/// How `:fts` comparisons match: the same options as full-text search.
/// `mode` picks normalized text (markup removed, punctuation ignored,
/// whitespace collapsed), the stored text, or a regex over the stored text.
/// `case_sensitive` applies to the two word modes; a regex sets its own
/// case flags.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FtsOptions {
    pub mode: TextMode,
    pub case_sensitive: bool,
    /// Explicit options for registered definitions. Legacy mode and case
    /// remain the fallback when these are absent.
    #[serde(skip_serializing_if = "crate::TextSearchOptions::is_default")]
    pub search_options: crate::TextSearchOptions,
}


/// The string a `:fts` field is compared with, and how it matches. The
/// options are part of the serialized filter, so cached results are kept
/// apart per option. Matching itself is the shared [`TextPattern`], the
/// same rule full-text search uses.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct FtsPattern {
    pattern: String,
    options: FtsOptions,
    #[serde(skip)]
    compiled: Option<Arc<TextPattern>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    bound_identity: Option<String>,
    #[serde(skip)]
    bound: Option<Arc<BoundTextQuery>>,
}


impl FtsPattern {
    pub(crate) fn new(pattern: String, options: FtsOptions, op: Op) -> Result<Self> {
        let compiled = Self::compile(&pattern, options, op)?;
        Ok(Self {
            pattern,
            options,
            compiled: Some(Arc::new(compiled)),
            bound_identity: None,
            bound: None,
        })
    }

    pub(crate) fn pattern(&self) -> &str {
        &self.pattern
    }

    pub(crate) fn options(&self) -> FtsOptions {
        self.options
    }

    pub(crate) fn bind_registered(&mut self, binding: BoundTextQuery) {
        self.bound_identity = Some(binding.identity.clone());
        self.bound = Some(Arc::new(binding));
    }

    pub(crate) fn registered(&self) -> Option<&BoundTextQuery> {
        self.bound.as_deref()
    }

    fn compile(pattern: &str, options: FtsOptions, op: Op) -> Result<TextPattern> {
        // `=` under regex compares the stored text exactly, not as a regex.
        let (mode, case_sensitive) = match op {
            Op::Eq | Op::Ne if options.mode == TextMode::Regex => (TextMode::WordRaw, true),
            _ => (options.mode, options.case_sensitive),
        };
        TextPattern::compile(pattern, mode, case_sensitive).map_err(|error| match error {
            Error::Request(message) => Error::Request(format!(":fts {message}")),
            other => other,
        })
    }

    /// Whether `value`, read as the full-text version in `format`, matches.
    pub(crate) fn matches(&self, value: &str, format: TextFormat, op: Op) -> Result<bool> {
        if let Some(bound) = &self.bound {
            let mut prepared_buffer = String::new();
            let mut scanner_buffer = String::new();
            let Some(prepared) = bound.representation.render(value, &mut prepared_buffer)? else {
                return Ok(false);
            };
            let matched =
                if bound.preparation == crate::extensions::TextPreparation::VisibleNormalized {
                    bound
                        .operator
                        .matches_prepared(ScalarValue::Text(prepared), &mut scanner_buffer)?
                } else {
                    bound
                        .operator
                        .matches(ScalarValue::Text(prepared), &mut scanner_buffer)?
                };
            return Ok(if matches!(op, Op::Ne | Op::NotContains) {
                !matched
            } else {
                matched
            });
        }
        let compiled = match &self.compiled {
            Some(compiled) => Arc::clone(compiled),
            None => match Self::compile(&self.pattern, self.options, op) {
                Ok(compiled) => Arc::new(compiled),
                Err(_) => return Ok(false),
            },
        };
        Ok(match op {
            Op::Eq => compiled.equals(value, format),
            Op::Ne => !compiled.equals(value, format),
            Op::NotContains => !compiled.matches(value, format),
            _ => compiled.matches(value, format),
        })
    }

    /// A literal every candidate's indexed text must contain, and whether
    /// it is a regex, for narrowing by a declared field's postings.
    fn candidate(&self) -> Option<(String, bool)> {
        let compiled = self.compiled.as_deref()?;
        match compiled.mode() {
            TextMode::Regex => Some((compiled.source().to_owned(), false)),
            // Each word of the normalized needle appears in the indexed
            // normalized text; the longest narrows best.
            TextMode::Word => compiled
                .needle()?
                .split(' ')
                .max_by_key(|word| word.len())
                .filter(|word| !word.is_empty())
                .map(|word| (word.to_owned(), true)),
            // The raw postings fold ASCII case only.
            TextMode::WordRaw => (!compiled.folds() || compiled.source().is_ascii())
                .then(|| (compiled.source().to_ascii_lowercase(), true)),
        }
    }
}


/// Turn every string compared with a `:fts` field by `~`, `!~`, `=`, or
/// `!=` into a full-text pattern with `options`.
pub(super) fn apply_options(expr: &mut Expr, options: FtsOptions) -> Result<()> {
    match expr {
        Expr::And(parts) | Expr::Or(parts) => {
            for part in parts {
                apply_options(part, options)?;
            }
        }
        Expr::Relation(inner) => apply_options(inner, options)?,
        Expr::Compare {
            left: Operand::Field(field),
            op,
            right,
            ..
        } if field.full_text()
            && matches!(op, Op::Eq | Op::Ne | Op::Contains | Op::NotContains) =>
        {
            let pattern = match right {
                Operand::Literal(Value::String(text)) => text.clone(),
                Operand::FullText(existing) => existing.pattern.clone(),
                _ if matches!(field.modifier, Some(Modifier::Text(_))) => {
                    return Err(Error::Request(
                        "text definition requires a string literal or parameter".into(),
                    ));
                }
                _ => return Ok(()),
            };
            *right = Operand::FullText(FtsPattern::new(pattern, options, *op)?);
        }
        _ => {}
    }
    Ok(())
}


/// Necessary full-text candidates for a Boolean filter subtree.
pub(crate) enum FtsPlan {
    /// A literal or regex, the declared field path, whether it is a
    /// literal, and whether it narrows the normalized (else the raw) postings.
    Literal(String, JsonPath, bool, bool),
    And(Vec<Self>),
    Or(Vec<Self>),
}


/// Safe registered-text terms. An OR has coverage only when every branch
/// contributes a compatible necessary condition.
pub(crate) enum RegisteredFtsPlan<'a> {
    Term(&'a JsonPath, &'a BoundTextQuery),
    And(Vec<Self>),
    Or(Vec<Self>),
}


pub(crate) fn declared_format(
    fields: &BTreeMap<String, Field>,
    path: &JsonPath,
) -> Option<TextFormat> {
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
    field.text_format.or_else(|| {
        (field.kind == ValueType::Array)
            .then(|| {
                field
                    .items
                    .as_deref()
                    .filter(|item| !item.exclude_fts)
                    .and_then(|item| item.text_format)
            })
            .flatten()
    })
}

fn bind_field(field: &mut FieldOperand, fields: &BTreeMap<String, Field>) {
    // `:fts` always reads the full-text version of a field: rendered when
    // a format is declared, otherwise normalized plain text.
    if field.modifier == Some(Modifier::Fts) {
        field.modifier = Some(
            declared_format(fields, &field.path).map_or(Modifier::Normalized, Modifier::Rendered),
        );
    }
}

fn bind_registered_expr(
    expr: &mut Expr,
    collection: &crate::config::Collection,
    registry: &crate::extensions::Extensions,
) -> Result<()> {
    match expr {
        Expr::And(parts) | Expr::Or(parts) => {
            for part in parts {
                bind_registered_expr(part, collection, registry)?;
            }
        }
        Expr::Relation(inner) => bind_registered_expr(inner, collection, registry)?,
        Expr::Compare {
            left: Operand::Field(field),
            op,
            right,
            ignore_case,
            ..
        } => {
            let explicit = match &field.modifier {
                Some(Modifier::Text(name)) => Some(name.as_str()),
                Some(Modifier::Fts)
                    if crate::schema::field_at_path(&collection.fields, &field.path)
                        .is_some_and(|field| field.text_search.is_some()) =>
                {
                    None
                }
                _ => return Ok(()),
            };
            let comparison = match op {
                Op::Contains | Op::NotContains => TextComparison::Contains,
                Op::Eq | Op::Ne => TextComparison::Equal,
                _ => {
                    return Err(Error::Request(
                        "text definitions support only ~, !~, = and !=".into(),
                    ));
                }
            };
            let Operand::FullText(pattern) = right else {
                return Err(Error::Request(
                    "text definition requires a string literal or parameter".into(),
                ));
            };
            let options = pattern.options();
            let syntax =
                options
                    .search_options
                    .syntax
                    .unwrap_or(if options.mode == TextMode::Regex {
                        TextSyntax::Regex
                    } else {
                        TextSyntax::Literal
                    });
            if options.mode == TextMode::WordRaw {
                return Err(Error::Request(
                    "raw-word mode conflicts with a selected text definition".into(),
                ));
            }
            let request = TextCompileRequest {
                pattern: pattern.pattern().into(),
                comparison,
                syntax,
                case_sensitive: options
                    .search_options
                    .case_sensitive
                    .or_else(|| options.case_sensitive.then_some(true))
                    .or_else(|| (*ignore_case).then_some(false)),
            };
            let path = crate::json_filter::format_path(&field.path);
            let binding = registry.bind_text_definition(collection, &path, explicit, &request)?;
            pattern.bind_registered(binding);
        }
        _ => {}
    }
    Ok(())
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
        // Relation operands were bound against their linked schema.
        Expr::Exists(_) | Expr::Missing(_) | Expr::Relation(_) => {}
    }
}


impl JsonFilter {
    /// A fallible registered term can raise an error before or after another
    /// Boolean child. Candidate pruning must not skip its source record.
    pub(crate) fn registered_text_pruning_safe(&self) -> bool {
        fn safe(expr: &Expr) -> bool {
            match expr {
                Expr::And(parts) | Expr::Or(parts) => parts.iter().all(safe),
                Expr::Relation(inner) => safe(inner),
                Expr::Compare {
                    right: Operand::FullText(pattern),
                    ..
                } => pattern
                    .registered()
                    .is_none_or(BoundTextQuery::allows_candidate_pruning),
                _ => true,
            }
        }
        safe(&self.expr)
    }

    pub(crate) fn registered_text_cacheable(&self) -> bool {
        fn cacheable(expr: &Expr) -> bool {
            match expr {
                Expr::And(parts) | Expr::Or(parts) => parts.iter().all(cacheable),
                Expr::Relation(inner) => cacheable(inner),
                Expr::Compare {
                    right: Operand::FullText(pattern),
                    ..
                } => pattern.registered().is_none_or(|binding| binding.cacheable),
                _ => true,
            }
        }
        cacheable(&self.expr)
    }

    pub(crate) fn registered_fts_plan(&self) -> Option<RegisteredFtsPlan<'_>> {
        fn plan(expr: &Expr) -> Option<RegisteredFtsPlan<'_>> {
            match expr {
                Expr::And(parts) => {
                    let children = parts.iter().filter_map(plan).collect::<Vec<_>>();
                    (!children.is_empty()).then_some(RegisteredFtsPlan::And(children))
                }
                Expr::Or(parts) => {
                    let children = parts.iter().map(plan).collect::<Option<Vec<_>>>()?;
                    (!children.is_empty()).then_some(RegisteredFtsPlan::Or(children))
                }
                Expr::Compare {
                    left: Operand::Field(field),
                    op: Op::Contains | Op::Eq,
                    right: Operand::FullText(pattern),
                    ..
                } => Some(RegisteredFtsPlan::Term(&field.path, pattern.registered()?)),
                _ => None,
            }
        }
        plan(&self.expr)
    }

    pub(crate) fn has_registered_text_terms(&self, fields: &BTreeMap<String, Field>) -> bool {
        fn visit(expr: &Expr, fields: &BTreeMap<String, Field>) -> bool {
            match expr {
                Expr::And(parts) | Expr::Or(parts) => parts.iter().any(|part| visit(part, fields)),
                Expr::Relation(inner) => visit(inner, fields),
                Expr::Compare {
                    left: Operand::Field(field),
                    ..
                } => match &field.modifier {
                    Some(Modifier::Text(_)) => true,
                    Some(Modifier::Fts) => crate::schema::field_at_path(fields, &field.path)
                        .is_some_and(|field| field.text_search.is_some()),
                    _ => false,
                },
                _ => false,
            }
        }
        visit(&self.expr, fields)
    }

    pub(crate) fn for_fields_with_text(
        &self,
        collection: &crate::config::Collection,
        registry: &crate::extensions::Extensions,
    ) -> Result<Self> {
        let mut bound = self.clone();
        bind_registered_expr(&mut bound.expr, collection, registry)?;
        bind_expr(&mut bound.expr, &collection.fields);
        for field in &mut bound.scalar_paths {
            bind_field(field, &collection.fields);
        }
        bound.hash = crate::config::fingerprint("json-filter-bound-text/v1", &bound.expr);
        Ok(bound)
    }

    /// Resolve `:fts` independently for each collection. An undeclared path
    /// becomes the ordinary stored-value comparison for that collection.
    pub(crate) fn for_fields(&self, fields: &BTreeMap<String, Field>) -> Self {
        let mut bound = self.clone();
        bind_expr(&mut bound.expr, fields);
        for field in &mut bound.scalar_paths {
            if !self.is_relation_path(&field.path) {
                bind_field(field, fields);
            }
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
                            modifier: Some(Modifier::Rendered(_)),
                            path,
                            ..
                        }),
                    op: Op::Contains | Op::Eq,
                    right: Operand::FullText(pattern),
                    ..
                } => pattern.candidate().map(|(literal, is_literal)| {
                    let clean = pattern.options().mode == TextMode::Word;
                    FtsPlan::Literal(literal, path.clone(), is_literal, clean)
                }),
                _ => None,
            }
        }
        plan(&self.expr)
    }
}
