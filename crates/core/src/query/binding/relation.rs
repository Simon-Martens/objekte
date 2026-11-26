/// Direction of a declared, collection-scoped relation hop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HopDirection {
    Forward,
    Backlink,
}

/// One resolved hop and its explicit array selectors in source order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BoundRelationHop {
    pub relation: RelationId,
    pub direction: HopDirection,
    pub selectors: Vec<Option<u32>>,
    pub from: CollectionId,
    pub to: CollectionId,
}

/// A finite path that begins at the matching source and ends at a local term.
#[derive(Clone, Debug)]
pub struct BoundRelationPath {
    pub hops: Vec<BoundRelationHop>,
    pub terminal: Box<BoundTerm>,
    pub source_operand: Option<Box<BoundTerm>>,
    pub target_is_lhs: bool,
    pub evidence_path: String,
}

pub(crate) fn bind_filter_with_relations(
    repo: &Repository,
    collection_index: usize,
    expr: &FilterExpr,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<BoundExpr, String> {
    match expr {
        FilterExpr::Term(term) => {
            if let Some(related) = bind_related_term(repo, collection_index, term, parameter_types)?
            {
                Ok(BoundExpr::Term(related))
            } else {
                bind_filter_with_parameters(
                    expr,
                    &repo.collections[collection_index].definition.schema,
                    parameter_types,
                )
            }
        }
        FilterExpr::And(parts) => Ok(BoundExpr::And(
            parts
                .iter()
                .map(|part| {
                    bind_filter_with_relations(repo, collection_index, part, parameter_types)
                })
                .collect::<Result<_, _>>()?,
        )),
        FilterExpr::Or(parts) => Ok(BoundExpr::Or(
            parts
                .iter()
                .map(|part| {
                    bind_filter_with_relations(repo, collection_index, part, parameter_types)
                })
                .collect::<Result<_, _>>()?,
        )),
        FilterExpr::Not(inner) => Ok(BoundExpr::Not(Box::new(bind_filter_with_relations(
            repo,
            collection_index,
            inner,
            parameter_types,
        )?))),
    }
}

/// Bind selected relation fields against the target collection's schema.
/// Unscoped FTS remains local to the source collection.
pub(crate) fn bind_fts_with_relations(
    repo: &Repository,
    collection_index: usize,
    request: &FtsQuery,
) -> Result<BoundFts, String> {
    let mut local = request.clone();
    local.fields.clear();
    let mut related = Vec::new();
    let mut related_base = None;
    let mut local_positions = Vec::new();
    for selected in &request.fields {
        let syntax = selected
            .query_path
            .clone()
            .unwrap_or_else(|| selected.path.display());
        let path = QueryPath::parse(&syntax)?;
        if path.field != selected.path {
            return Err("selected FTS query path disagrees with its field".into());
        }
        if let Some((hops, terminal)) = resolve_path(repo, collection_index, &path)? {
            let target = hops.last().ok_or("relation FTS has no hop")?.to.0 as usize;
            if terminal.has_explicit_array_step
                && repo.collections[target]
                    .source
                    .as_ref()
                    .is_some_and(|source| {
                        source.reader().text_derivation()
                            != TextDerivation::LogicalStringsAfterConversion
                    })
            {
                return Err(
                    "selected FTS array positions require logical-string text derivation".into(),
                );
            }
            let target_request = FtsQuery {
                fields: vec![FtsField {
                    path: terminal.field.clone(),
                    binding: selected.binding.clone(),
                    query_path: None,
                }],
                ..request.clone()
            };
            let mut bound = bind_fts(&target_request, &repo.collections[target].definition.schema)?;
            let mut field = bound
                .fields
                .pop()
                .ok_or("relation FTS field was not bound")?;
            if related_base.is_none() {
                related_base = Some(bound);
            }
            if terminal.has_explicit_array_step {
                field.query_path = Some(terminal);
            }
            field.related = Some(hops);
            field.evidence_path = syntax;
            related.push(field);
        } else {
            let mut direct = selected.clone();
            direct.query_path = None;
            local.fields.push(direct);
            if path.has_explicit_array_step {
                if repo.collections[collection_index]
                    .source
                    .as_ref()
                    .is_some_and(|source| {
                        source.reader().text_derivation()
                            != TextDerivation::LogicalStringsAfterConversion
                    })
                {
                    return Err(
                        "selected FTS array positions require logical-string text derivation"
                            .into(),
                    );
                }
                local_positions.push((local.fields.len() - 1, path, syntax));
            }
        }
    }
    if local.fields.is_empty() && !related.is_empty() {
        let mut bound = related_base.ok_or("relation FTS base is missing")?;
        bound.fields = related;
        return Ok(bound);
    }
    let mut bound = bind_fts(
        &local,
        &repo.collections[collection_index].definition.schema,
    )?;
    for (index, path, syntax) in local_positions {
        let field = bound
            .fields
            .get_mut(index)
            .ok_or("selected FTS field is missing")?;
        field.query_path = Some(path);
        field.evidence_path = syntax;
    }
    if !related.is_empty() {
        if local.fields.is_empty() {
            bound.fields.clear();
        }
        bound.fields.extend(related);
    }
    Ok(bound)
}

/// Keep unavailable relation truth through Boolean negation. The final query
/// boundary accepts only a definite true result.
fn bind_related_term(
    repo: &Repository,
    collection_index: usize,
    term: &Term,
    parameter_types: &mut HashMap<String, LogicalType>,
) -> Result<Option<BoundTerm>, String> {
    let left = resolve_path(repo, collection_index, &term.path)?;
    let right = if let Operand::Field(field) = &term.operand {
        resolve_path(repo, collection_index, &field.path)?
    } else {
        None
    };
    if let (Some((left_hops, left_terminal)), Some((right_hops, right_terminal))) = (&left, &right)
    {
        if left_hops != right_hops {
            return Err("independent relation field operands need a shared occurrence path".into());
        }
        let target_index = left_hops.last().ok_or("relation path has no hop")?.to.0 as usize;
        let mut local = term.clone();
        local.path = left_terminal.clone();
        if let Operand::Field(field) = &mut local.operand {
            field.path = right_terminal.clone();
        }
        local.quantifier = Quantifier::Default;
        let BoundExpr::Term(mut terminal) = bind_filter_with_parameters(
            &FilterExpr::Term(local),
            &repo.collections[target_index].definition.schema,
            parameter_types,
        )?
        else {
            return Err("related field comparison did not bind".into());
        };
        if term.quantifier == Quantifier::Any && terminal.multi {
            terminal.quantifier = Quantifier::Any;
        }
        let mut outer = terminal.clone();
        outer.rhs = None;
        outer.path = term.path.field.clone();
        outer.query_path = term.path.clone();
        outer.quantifier = term.quantifier;
        outer.multi = true;
        outer.related = Some(Box::new(BoundRelationPath {
            hops: left_hops.clone(),
            terminal: Box::new(terminal),
            source_operand: None,
            target_is_lhs: true,
            evidence_path: if let Operand::Field(field) = &term.operand {
                field.path.display.clone()
            } else {
                term.path.display.clone()
            },
        }));
        return Ok(Some(outer));
    }
    let target_is_lhs = left.is_some();
    let Some((hops, terminal_path)) = left.or(right) else {
        return Ok(None);
    };
    let target_index = hops.last().ok_or("relation path has no hop")?.to.0 as usize;
    let mut terminal_term = term.clone();
    terminal_term.path = terminal_path;
    terminal_term.quantifier = Quantifier::Default;
    let target_schema = &repo.collections[target_index].definition.schema;
    let (mut terminal, source_operand) = if let Operand::Field(field) = &term.operand {
        if !target_is_lhs {
            terminal_term.modifiers = field.modifiers.clone();
        }
        terminal_term.operand = Operand::Literal(Literal::Null);
        let target = bind_field_probe(&terminal_term, target_schema, parameter_types)?;
        let mut source_term = term.clone();
        if target_is_lhs {
            source_term.path = field.path.clone();
            source_term.modifiers = field.modifiers.clone();
        }
        source_term.operand = Operand::Literal(Literal::Null);
        source_term.quantifier = Quantifier::Default;
        let source = bind_field_probe(
            &source_term,
            &repo.collections[collection_index].definition.schema,
            parameter_types,
        )?;
        if target.kind != LogicalType::Unknown
            && source.kind != LogicalType::Unknown
            && !crate::query::semantics::comparison::comparable_logical_types(
                &target.kind,
                &source.kind,
            )
        {
            return Err(format!(
                "incompatible field operand types: {:?} and {:?}",
                target.kind, source.kind
            ));
        }
        if source.multi {
            let source_scopes = explicit_scopes(&source.query_path);
            let first_hop = hops.first().ok_or("relation path has no hop")?;
            if first_hop.direction != HopDirection::Forward {
                return Err("source-dependent multivalued field needs a forward shared [*] occurrence scope".into());
            }
            let declaration = repo.declared_relation(first_hop.relation)?.declaration;
            let relation_scopes = relation_source_scopes(
                if target_is_lhs {
                    &term.path
                } else {
                    &field.path
                },
                &declaration.path,
            );
            if source_scopes.is_empty()
                || !relation_scopes.starts_with(&source_scopes)
                || source.implicit_array
                || matches!(source.source_kind, LogicalType::Array(_))
            {
                return Err(
                    "source-dependent multivalued field needs a shared [*] relation occurrence scope or explicit indexes"
                        .into(),
                );
            }
        }
        for operation in [target.operation.as_ref(), source.operation.as_ref()]
            .into_iter()
            .flatten()
        {
            if !operation.supports_dynamic_comparison() {
                return Err("custom binding does not support dynamic field comparison".into());
            }
        }
        (target, Some(Box::new(source)))
    } else {
        let BoundExpr::Term(target) = bind_filter_with_parameters(
            &FilterExpr::Term(terminal_term),
            target_schema,
            parameter_types,
        )?
        else {
            return Err("relation terminal did not bind to a term".into());
        };
        (target, None)
    };
    if term.quantifier == Quantifier::Any && terminal.multi {
        terminal.quantifier = Quantifier::Any;
    }
    let mut outer = terminal.clone();
    outer.rhs = None;
    outer.path = term.path.field.clone();
    outer.query_path = term.path.clone();
    outer.quantifier = term.quantifier;
    outer.multi |= hops.iter().any(|hop| {
        hop.direction == HopDirection::Backlink
            || repo.collections[hop.relation.source.0 as usize]
                .relation_declaration(hop.relation.ordinal)
                .is_some_and(|declaration| declaration.cardinality == RelationCardinality::Many)
    });
    outer.related = Some(Box::new(BoundRelationPath {
        hops,
        terminal: Box::new(terminal),
        source_operand,
        target_is_lhs,
        evidence_path: if target_is_lhs {
            term.path.display.clone()
        } else if let Operand::Field(field) = &term.operand {
            field.path.display.clone()
        } else {
            term.path.display.clone()
        },
    }));
    Ok(Some(outer))
}

fn explicit_scopes(path: &QueryPath) -> Vec<Vec<PathStep>> {
    let mut prefix = Vec::new();
    let mut scopes = Vec::new();
    for step in &path.steps {
        prefix.push(step.clone());
        if matches!(step, PathStep::EachElement) {
            scopes.push(prefix.clone());
        }
    }
    scopes
}

// Only source-side array scopes before the first relation field can pair with
// source values. Later selectors belong to target traversal, not source data.
fn relation_source_scopes(path: &QueryPath, relation_fields: &[String]) -> Vec<Vec<PathStep>> {
    let mut prefix = Vec::new();
    let mut scopes = Vec::new();
    let mut field_count = 0;
    for step in &path.steps {
        prefix.push(step.clone());
        if matches!(step, PathStep::EachElement) {
            scopes.push(prefix.clone());
        }
        if matches!(step, PathStep::Field(_)) {
            field_count += 1;
            if field_count == relation_fields.len() {
                break;
            }
        }
    }
    scopes
}

/// Resolve the first explicit relation at each collection boundary.
pub(crate) fn resolve_path(
    repo: &Repository,
    collection_index: usize,
    path: &QueryPath,
) -> Result<Option<(Vec<BoundRelationHop>, QueryPath)>, String> {
    if path.steps.len() > MAX_PATH_STEPS {
        return Err("relation path exceeds the 64-step limit".into());
    }
    let mut current = collection_index;
    let mut remaining = path.steps.as_slice();
    let mut hops = Vec::new();
    if path.field.root == PathRoot::Backlinks {
        let Some(PathStep::Field(alias)) = remaining.first() else {
            return Err("backlink path requires an alias".into());
        };
        let relation = repo
            .declared_backlink(repo.collections[current].collection_id(), alias)?
            .ok_or_else(|| format!("unknown backlink alias {alias}"))?;
        let mut consumed = 1;
        let mut selectors = Vec::new();
        while let Some(step) = remaining.get(consumed) {
            match step {
                PathStep::ArrayIndex(index) => selectors.push(Some(*index)),
                PathStep::EachElement => selectors.push(None),
                PathStep::Field(_) => break,
            }
            consumed += 1;
        }
        if !remaining[consumed..]
            .iter()
            .any(|step| matches!(step, PathStep::Field(_)))
        {
            return Err("backlink path requires a terminal field".into());
        }
        let to = relation.id.source;
        hops.push(BoundRelationHop {
            relation: relation.id,
            direction: HopDirection::Backlink,
            selectors,
            from: repo.collections[current].collection_id(),
            to,
        });
        current = to.0 as usize;
        remaining = &remaining[consumed..];
    } else if path.field.root != PathRoot::Data {
        return Ok(None);
    }
    loop {
        let schema = &repo.collections[current].definition.schema;
        let mut prefix = Vec::new();
        let mut relation_at = None;
        for (index, step) in remaining.iter().enumerate() {
            let PathStep::Field(name) = step else {
                continue;
            };
            prefix.push(name.clone());
            if let Some(declaration) = schema.relations().get(&prefix) {
                let mut consumed = index + 1;
                let mut selectors = remaining[..consumed]
                    .iter()
                    .filter_map(selector)
                    .collect::<Vec<_>>();
                while let Some(step) = remaining.get(consumed) {
                    if matches!(step, PathStep::Field(_)) {
                        break;
                    }
                    selectors.extend(selector(step));
                    consumed += 1;
                }
                if remaining[consumed..]
                    .iter()
                    .any(|step| matches!(step, PathStep::Field(_)))
                {
                    let relation = repo
                        .declared_relation_by_path(
                            repo.collections[current].collection_id(),
                            &declaration.path,
                        )?
                        .ok_or("declared relation has no target")?;
                    relation_at = Some((relation, consumed, selectors));
                    break;
                }
            }
        }
        let Some((relation, consumed, selectors)) = relation_at else {
            break;
        };
        if hops.len() >= MAX_RELATION_HOPS {
            return Err("relation path exceeds the 8-hop limit".into());
        }
        let target = relation.target_collection;
        hops.push(BoundRelationHop {
            relation: relation.id,
            direction: HopDirection::Forward,
            selectors,
            from: repo.collections[current].collection_id(),
            to: target,
        });
        current = target.0 as usize;
        remaining = &remaining[consumed..];
    }
    if hops.is_empty() {
        return Ok(None);
    }
    if remaining.is_empty() || !matches!(remaining.first(), Some(PathStep::Field(_))) {
        return Err("relation path requires a terminal field".into());
    }
    let segments = remaining
        .iter()
        .filter_map(|step| match step {
            PathStep::Field(name) => Some(name.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let field = FieldPath {
        root: PathRoot::Data,
        segments,
    };
    let terminal = QueryPath {
        display: field.display(),
        field,
        steps: remaining.to_vec(),
        has_explicit_array_step: remaining
            .iter()
            .any(|step| !matches!(step, PathStep::Field(_))),
        has_position: remaining
            .iter()
            .any(|step| matches!(step, PathStep::ArrayIndex(_))),
    };
    Ok(Some((hops, terminal)))
}

fn selector(step: &PathStep) -> Option<Option<u32>> {
    match step {
        PathStep::Field(_) => None,
        PathStep::ArrayIndex(index) => Some(Some(*index)),
        PathStep::EachElement => Some(None),
    }
}

use crate::query::binding::filter::{
    BoundExpr, BoundTerm, bind_field_probe, bind_filter_with_parameters,
};
use crate::query::binding::fts::{BoundFts, bind_fts};

use super::*;
