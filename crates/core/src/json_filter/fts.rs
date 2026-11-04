//! Per-collection binding and safe candidates for rendered filter operands.

use super::*;
use crate::{Field, TextMode, ValueType, text::TextPattern};
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
}


impl FtsPattern {
    pub(crate) fn new(pattern: String, options: FtsOptions, op: Op) -> Result<Self> {
        let compiled = Self::compile(&pattern, options, op)?;
        Ok(Self {
            pattern,
            options,
            compiled: Some(Arc::new(compiled)),
        })
    }

    pub(crate) fn pattern(&self) -> &str {
        &self.pattern
    }

    pub(crate) fn options(&self) -> FtsOptions {
        self.options
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
    pub(crate) fn matches(&self, value: &str, format: TextFormat, op: Op) -> bool {
        let compiled = match &self.compiled {
            Some(compiled) => Arc::clone(compiled),
            None => match Self::compile(&self.pattern, self.options, op) {
                Ok(compiled) => Arc::new(compiled),
                Err(_) => return false,
            },
        };
        match op {
            Op::Eq => compiled.equals(value, format),
            Op::Ne => !compiled.equals(value, format),
            Op::NotContains => !compiled.matches(value, format),
            _ => compiled.matches(value, format),
        }
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
