//! Repository-aware binding and exact evaluation of finite relation paths.

use std::collections::HashMap;

use crate::inventory::RecordClass;
use crate::model::{CollectionId, FieldPath, LogicalType, PathRoot, RecordSlot, Value};
use crate::query::{FilterExpr, FtsField, FtsQuery, Literal, Operand, Quantifier, Query, Term};
use crate::query_path::{PathStep, QueryPath};
use crate::reference::{
    BoundExpr, BoundFts, BoundTerm, RecordContext, bind_field_probe, bind_filter_with_parameters,
    bind_fts, eval_bound_fts, eval_cross_record_field_term, eval_term,
};
use crate::relation::{RelationCardinality, RelationId, RelationTarget};
use crate::repository::Repository;
use crate::source::{ScanProjection, TextSegment, read_record_range};

const MAX_RELATION_HOPS: usize = 8;
const MAX_PATH_STEPS: usize = 64;


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


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum EvaluationMode {
    Indexed,
    Reference,
}


/// Physical work caused by relation traversal, separate from the source scan.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct RelationWork {
    pub source_reads: u64,
    pub target_reads: u64,
    pub edge_visits: u64,
    pub bytes_read: u64,
}


/// Immutable traversal choices shared by every hop of one predicate.
#[derive(Clone, Copy)]
struct TraverseConfig<'a> {
    repo: &'a Repository,
    hops: &'a [BoundRelationHop],
    quantifier: Quantifier,
    collect_text: bool,
    mode: EvaluationMode,
}


/// Bind a Boolean expression with collection-specific relation declarations.
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

pub(crate) fn reject_related_sort(repo: &Repository, query: &Query) -> Result<(), String> {
    for field in &query.options.sort {
        let path = QueryPath::parse(&field.field)?;
        for (index, collection) in repo.collections.iter().enumerate() {
            if !query.collections.is_empty()
                && !query.collections.contains(&collection.definition.name)
            {
                continue;
            }
            if resolve_path(repo, index, &path)?.is_some() {
                return Err(format!(
                    "related sort field is not supported: {}",
                    field.field
                ));
            }
        }
    }
    Ok(())
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
    for selected in &request.fields {
        let path = QueryPath::parse(&selected.path.display())?;
        if let Some((hops, terminal)) = resolve_path(repo, collection_index, &path)? {
            let target = hops.last().ok_or("relation FTS has no hop")?.to.0 as usize;
            let target_request = FtsQuery {
                fields: vec![FtsField {
                    path: terminal.field,
                    binding: selected.binding.clone(),
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
            field.related = Some(hops);
            field.evidence_path = selected.path.display();
            related.push(field);
        } else {
            local.fields.push(selected.clone());
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
pub(crate) fn eval_filter_truth(
    repo: &Repository,
    expr: &BoundExpr,
    ctx: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    match expr {
        BoundExpr::Term(term) => {
            let result = if term.related.is_some() {
                eval_related_term_mode(repo, term, ctx, mode, work)?
            } else {
                Some(eval_term(term, ctx)?)
            };
            if result == Some(true)
                && term.derived == crate::reference::Derived::None
                && !matches!(
                    term.operator,
                    crate::query::Operator::Ne | crate::query::Operator::NotContains
                )
            {
                evidence.insert(term.query_path.display.clone());
                if let Some(rhs) = &term.rhs {
                    evidence.insert(rhs.query_path.display.clone());
                }
                if let Some(related) = &term.related {
                    evidence.insert(related.evidence_path.clone());
                }
            }
            Ok(result)
        }
        BoundExpr::And(parts) => {
            let mut answer = Some(true);
            let mut fields = std::collections::HashSet::new();
            for part in parts {
                let next = eval_filter_truth(repo, part, ctx, &mut fields, mode, work)?;
                answer = match (answer, next) {
                    (Some(false), _) | (_, Some(false)) => Some(false),
                    (None, _) | (_, None) => None,
                    _ => Some(true),
                };
            }
            if answer == Some(true) {
                evidence.extend(fields);
            }
            Ok(answer)
        }
        BoundExpr::Or(parts) => {
            let mut answer = Some(false);
            for part in parts {
                let mut fields = std::collections::HashSet::new();
                let next = eval_filter_truth(repo, part, ctx, &mut fields, mode, work)?;
                answer = match (answer, next) {
                    (Some(true), _) | (_, Some(true)) => Some(true),
                    (None, _) | (_, None) => None,
                    _ => Some(false),
                };
                if next == Some(true) {
                    evidence.extend(fields);
                }
            }
            Ok(answer)
        }
        BoundExpr::Not(inner) => {
            Ok(
                eval_filter_truth(repo, inner, ctx, &mut Default::default(), mode, work)?
                    .map(|value| !value),
            )
        }
    }
}

/// Evaluate selected local and relation FTS fields with source identity intact.
pub(crate) fn eval_fts_with_relations(
    repo: &Repository,
    fts: &BoundFts,
    segments: &[TextSegment],
    ctx: &RecordContext<'_>,
    evidence: &mut std::collections::HashSet<String>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<bool, String> {
    if fts.fields.iter().all(|field| field.related.is_none()) {
        return eval_bound_fts(fts, segments, ctx, evidence);
    }
    let mut matched = false;
    for field in &fts.fields {
        let mut one = fts.clone();
        one.fields = vec![field.clone()];
        if let Some(hops) = &field.related {
            let source_index = repo
                .collections
                .iter()
                .position(|collection| collection.definition.name == ctx.collection_name)
                .ok_or("relation FTS source collection is unknown")?;
            let mut truth = Some(false);
            let config = TraverseConfig {
                repo,
                hops,
                quantifier: Quantifier::Any,
                collect_text: true,
                mode,
            };
            visit_hop(
                &config,
                0,
                source_index,
                ctx.record.slot,
                &mut truth,
                work,
                &mut |target, target_segments| {
                    eval_bound_fts(&one, target_segments, target, &mut Default::default())
                },
            )?;
            if truth == Some(true) {
                matched = true;
                evidence.insert(field.evidence_path.clone());
            }
        } else if eval_bound_fts(&one, segments, ctx, evidence)? {
            matched = true;
        }
    }
    Ok(matched)
}

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
            && target.kind != source.kind
        {
            return Err(format!(
                "incompatible field operand types: {:?} and {:?}",
                target.kind, source.kind
            ));
        }
        if source.multi {
            return Err(
                "source-dependent comparison requires a scalar source field or explicit position"
                    .into(),
            );
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
            || repo.collections[hop.relation.source.0 as usize].relations
                [hop.relation.ordinal as usize]
                .declaration
                .cardinality
                == RelationCardinality::Many
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
            .backlink_relation(&repo.collections[current].definition.name, alias)
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
            from: repo.collections[current].generation.collection,
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
                    let relation = repo.collections[current]
                        .relations
                        .iter()
                        .find(|item| item.declaration.path == declaration.path)
                        .ok_or("declared relation index is missing")?;
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
            from: repo.collections[current].generation.collection,
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

/// Exact three-valued result for a relation term. Unknown means a dangling or
/// logically unavailable target, so Boolean negation must keep it unknown.
pub(crate) fn eval_related_term(
    repo: &Repository,
    term: &BoundTerm,
    source: &RecordContext<'_>,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    eval_related_term_mode(repo, term, source, EvaluationMode::Indexed, work)
}

fn eval_related_term_mode(
    repo: &Repository,
    term: &BoundTerm,
    source: &RecordContext<'_>,
    mode: EvaluationMode,
    work: &mut RelationWork,
) -> Result<Option<bool>, String> {
    let related = term.related.as_ref().ok_or("relation term is not bound")?;
    let start = repo
        .collections
        .iter()
        .position(|collection| collection.definition.name == source.collection_name)
        .ok_or("relation source collection is unknown")?;
    let mut truth = if term.quantifier == Quantifier::Any {
        Some(false)
    } else {
        Some(true)
    };
    let config = TraverseConfig {
        repo,
        hops: &related.hops,
        quantifier: term.quantifier,
        collect_text: false,
        mode,
    };
    visit_hop(
        &config,
        0,
        start,
        source.record.slot,
        &mut truth,
        work,
        &mut |target, _| {
            if let Some(source_operand) = &related.source_operand {
                eval_cross_record_field_term(
                    &related.terminal,
                    source_operand,
                    target,
                    source,
                    related.target_is_lhs,
                )
            } else {
                eval_term(&related.terminal, target)
            }
        },
    )?;
    Ok(truth)
}

fn merge_truth(current: &mut Option<bool>, next: Option<bool>, quantifier: Quantifier) {
    *current = if quantifier == Quantifier::Any {
        match (*current, next) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (None, _) | (_, None) => None,
            _ => Some(false),
        }
    } else {
        match (*current, next) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (None, _) | (_, None) => None,
            _ => Some(true),
        }
    };
}

fn decisive(truth: Option<bool>, quantifier: Quantifier) -> bool {
    truth == Some(quantifier == Quantifier::Any)
}

fn visit_hop(
    config: &TraverseConfig<'_>,
    depth: usize,
    collection_index: usize,
    source_slot: RecordSlot,
    truth: &mut Option<bool>,
    work: &mut RelationWork,
    terminal: &mut impl FnMut(&RecordContext<'_>, &[TextSegment]) -> Result<bool, String>,
) -> Result<(), String> {
    let TraverseConfig {
        repo,
        hops,
        quantifier,
        collect_text,
        mode,
    } = *config;
    if decisive(*truth, quantifier) {
        return Ok(());
    }
    if depth == hops.len() {
        let collection = &repo.collections[collection_index];
        let record = &collection.generation.records[source_slot.0 as usize];
        if record.class == RecordClass::RawFallback {
            merge_truth(truth, None, quantifier);
            return Ok(());
        }
        let file = &repo.files.files[record.file.0 as usize];
        let path = repo
            .files
            .native_path(record.file)
            .ok_or("missing target source path")?;
        let bytes = read_record_range(&path, record.revision, record.range)?;
        work.target_reads += 1;
        work.bytes_read += bytes.len() as u64;
        let extracted = crate::record_preparation::prepare_query_record(
            &collection.source,
            &collection.definition.schema,
            &bytes,
            &ScanProjection::full(collect_text),
            false,
        )?;
        let ctx = RecordContext {
            value: &extracted.value,
            source_bytes: Some(&bytes),
            record,
            file,
            collection_name: &collection.definition.name,
            repository_root: &repo.files.root,
        };
        merge_truth(truth, Some(terminal(&ctx, &extracted.text)?), quantifier);
        return Ok(());
    }
    let hop = &hops[depth];
    if hop.from.0 as usize != collection_index {
        return Err("relation hop collection mismatch".into());
    }
    let source_collection = &repo.collections[hop.relation.source.0 as usize];
    let relation = source_collection
        .relations
        .get(hop.relation.ordinal as usize)
        .ok_or("relation hop index is missing")?;
    if mode == EvaluationMode::Reference {
        match hop.direction {
            HopDirection::Forward => {
                let Some(value) = read_reference_value(repo, collection_index, source_slot, work)?
                else {
                    merge_truth(truth, None, quantifier);
                    return Ok(());
                };
                let references = logical_references(&value, &relation.declaration.path)?;
                let present = references.present;
                let null = references.null;
                let targets = &repo.collections[hop.to.0 as usize].generation.records;
                let mut selected = 0usize;
                for (id, positions) in references.values {
                    if !positions_match(&hop.selectors, &positions) {
                        continue;
                    }
                    work.edge_visits += 1;
                    selected += 1;
                    if let Some(target) = targets.iter().find(|record| record.id.0 == id) {
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            target.slot,
                            truth,
                            work,
                            terminal,
                        )?;
                    } else {
                        merge_truth(truth, None, quantifier);
                    }
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0 && (!present || null || hop.selectors.iter().any(Option::is_some))
                {
                    merge_truth(truth, Some(false), quantifier);
                }
            }
            HopDirection::Backlink => {
                let target =
                    &repo.collections[collection_index].generation.records[source_slot.0 as usize];
                let mut seen = 0usize;
                let mut selected = 0usize;
                for candidate in &source_collection.identity_order {
                    let Some(value) =
                        read_reference_value(repo, hop.to.0 as usize, *candidate, work)?
                    else {
                        continue;
                    };
                    let references = logical_references(&value, &relation.declaration.path)?;
                    if !references.values.iter().any(|(id, _)| id == &target.id.0) {
                        continue;
                    }
                    work.edge_visits += 1;
                    if hop
                        .selectors
                        .first()
                        .is_none_or(|selector| selector.is_none_or(|index| index as usize == seen))
                    {
                        selected += 1;
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            *candidate,
                            truth,
                            work,
                            terminal,
                        )?;
                    }
                    seen += 1;
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
                if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                    merge_truth(truth, Some(false), quantifier);
                }
            }
        }
        return Ok(());
    }
    match hop.direction {
        HopDirection::Forward => {
            if relation.unavailable().contains(source_slot.0) {
                merge_truth(truth, None, quantifier);
                return Ok(());
            }
            let edges = relation
                .edges(source_slot)
                .ok_or("relation source slot is missing")?;
            let mut selected = 0;
            for edge in edges {
                if !positions_match(&hop.selectors, &edge.positions) {
                    continue;
                }
                work.edge_visits += 1;
                selected += 1;
                match edge.target {
                    RelationTarget::Resolved(target) => visit_hop(
                        config,
                        depth + 1,
                        hop.to.0 as usize,
                        target,
                        truth,
                        work,
                        terminal,
                    )?,
                    RelationTarget::Unresolved(_) => merge_truth(truth, None, quantifier),
                }
                if decisive(*truth, quantifier) {
                    break;
                }
            }
            if selected == 0
                && (!relation.present().contains(source_slot.0)
                    || relation.nulls().contains(source_slot.0)
                    || hop.selectors.iter().any(Option::is_some))
            {
                merge_truth(truth, Some(false), quantifier);
            }
        }
        HopDirection::Backlink => {
            let target =
                &repo.collections[collection_index].generation.records[source_slot.0 as usize];
            let sources = repo.relation_reverse_sources(relation, &target.id.0);
            let mut seen = 0usize;
            let mut selected = 0usize;
            if let Some(sources) = sources {
                for source in &source_collection.identity_order {
                    if !sources.contains(source.0) {
                        continue;
                    }
                    work.edge_visits += 1;
                    if hop
                        .selectors
                        .first()
                        .is_none_or(|selector| selector.is_none_or(|index| index as usize == seen))
                    {
                        selected += 1;
                        visit_hop(
                            config,
                            depth + 1,
                            hop.to.0 as usize,
                            *source,
                            truth,
                            work,
                            terminal,
                        )?;
                    }
                    seen += 1;
                    if decisive(*truth, quantifier) {
                        break;
                    }
                }
            }
            if selected == 0 && hop.selectors.iter().any(Option::is_some) {
                merge_truth(truth, Some(false), quantifier);
            }
        }
    }
    Ok(())
}

fn read_reference_value(
    repo: &Repository,
    collection_index: usize,
    slot: RecordSlot,
    work: &mut RelationWork,
) -> Result<Option<crate::model::Value>, String> {
    let collection = &repo.collections[collection_index];
    let record = collection
        .generation
        .records
        .get(slot.0 as usize)
        .ok_or("reference relation slot is missing")?;
    if record.class == RecordClass::RawFallback {
        return Ok(None);
    }
    let path = repo
        .files
        .native_path(record.file)
        .ok_or("reference relation path is missing")?;
    let bytes = read_record_range(&path, record.revision, record.range)?;
    work.source_reads += 1;
    work.bytes_read += bytes.len() as u64;
    let extracted = crate::record_preparation::prepare_query_record(
        &collection.source,
        &collection.definition.schema,
        &bytes,
        &ScanProjection::full(false),
        false,
    )?;
    Ok(Some(extracted.value))
}


/// Independent logical ID occurrences and presence/null state.
struct LogicalReferences {
    present: bool,
    null: bool,
    values: Vec<(String, Vec<u32>)>,
}


/// An intentionally separate, iterative oracle for declared ID occurrences.
/// It walks logical values directly and never consults build-time edge state.
fn logical_references(value: &Value, path: &[String]) -> Result<LogicalReferences, String> {
    let mut found = false;
    let mut null = false;
    let mut references = Vec::new();
    let mut pending = vec![(value, 0usize, Vec::<u32>::new())];
    while let Some((value, offset, positions)) = pending.pop() {
        match value {
            Value::Array(array) => {
                if array.values.is_empty() {
                    found = true;
                }
                for (index, item) in array.values.iter().enumerate().rev() {
                    let mut child_positions = positions.clone();
                    child_positions
                        .push(u32::try_from(index).map_err(|_| "relation array index overflow")?);
                    pending.push((item, offset, child_positions));
                }
            }
            Value::Object(fields) if offset < path.len() => {
                if let Some(next) = fields.get(&path[offset]) {
                    pending.push((next, offset + 1, positions));
                }
            }
            Value::Id(id) if offset == path.len() => {
                found = true;
                references.push((id.clone(), positions));
            }
            Value::Null if offset == path.len() => {
                found = true;
                null = true;
            }
            _ if offset == path.len() => return Err("relation field has non-ID value".into()),
            _ => {}
        }
    }
    Ok(LogicalReferences {
        present: found,
        null,
        values: references,
    })
}

fn positions_match(selectors: &[Option<u32>], positions: &[u32]) -> bool {
    selectors.len() <= positions.len()
        && selectors
            .iter()
            .zip(positions)
            .all(|(selector, position)| selector.is_none_or(|index| index == *position))
}

#[cfg(test)]
#[path = "relation_query.test.rs"]
mod tests;
