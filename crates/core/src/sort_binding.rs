//! Query-scoped sorting bindings and scalar-key preparation.

use std::cmp::Ordering;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use icu_collator::CollatorBorrowed;
use unicode_casefold::UnicodeCaseFold;

use crate::inventory::RecordClass;
use crate::model::{CollectionId, PathRoot, RecordSlot, Value};
use crate::query::Query;
use crate::query_path::{PathStep, QueryPath, split_path_binding};
use crate::reference::{BindingOperation, RecordContext, Transform, metadata_value};
use crate::relation::RelationTarget;
use crate::relation_query::{
    BoundRelationHop, HopDirection, MergedRecordSlot, MergedRelationView, resolve_path,
};
use crate::repository::Repository;
use crate::sorting::{SortColumn, SortConfig, SortKey, SortOptions, make_collator};
use crate::workload::{SortExpression, SortField, SortReducer};

const MAX_SORT_TARGET_BYTES: u64 = 64 * 1024 * 1024;
const MAX_SORT_EDGE_VISITS: u64 = 32_000_000;

type SortTargetCell = OnceLock<Result<Arc<Value>, String>>;


/// A prepared sort term with no source access in its comparator.
pub(crate) struct BoundSortTerm<'a> {
    pub source: BoundSortSource<'a>,
    pub descending: bool,
    pub options: SortOptions,
}


/// A resident column or a key that must be read for each matching record.
pub(crate) enum BoundSortSource<'a> {
    Stored(&'a SortColumn),
    Computed(Box<ComputedSort>),
}


/// Query-scoped key extraction for one local or finite related path.
pub(crate) struct ComputedSort {
    pub path: QueryPath,
    pub related: Option<(Vec<BoundRelationHop>, QueryPath)>,
    pub reducer: Option<SortReducer>,
    transform: SortTransform,
    collator: CollatorBorrowed<'static>,
}


/// A bound, stable scalar transformation rather than an exact matcher.
enum SortTransform {
    Identity,
    CaseFold,
    Operation(Arc<dyn BindingOperation>),
}


/// Stable identity required when one order spans several collections.
#[derive(Clone, Debug, Eq, PartialEq)]
enum SortTransformIdentity {
    Identity,
    CaseFold,
    Custom {
        name: String,
        version: u32,
        transform: Transform,
    },
    Operation {
        name: String,
        version: u32,
    },
}


/// Sort terms for every selected collection, with a flag for source reads.
pub(crate) struct BoundSort<'a> {
    pub terms: Vec<Option<Vec<BoundSortTerm<'a>>>>,
    pub has_computed: bool,
    pub needs_source: bool,
}


/// Query-local target projections reused when several source links reach one target.
#[derive(Default)]
pub(crate) struct SortTargetCache {
    values: HashMap<(usize, u32), Arc<SortTargetCell>>,
    retained_bytes: u64,
    stream_limit: Option<u64>,
    cancellation: Option<crate::streaming::CancellationToken>,
    execution: Option<crate::streaming::ExecutionContext>,
    pub reads: u64,
    pub reuse: u64,
    pub edge_visits: u64,
    pub bytes_read: u64,
}


#[derive(Default)]
pub(crate) struct MergedSortTargetCache {
    values: HashMap<MergedRecordSlot, Arc<Value>>,
    retained_bytes: u64,
    execution: Option<crate::streaming::ExecutionContext>,
    pub edge_visits: u64,
    pub reads: u64,
    pub reuse: u64,
    pub bytes_read: u64,
}


impl MergedSortTargetCache {
    pub(crate) fn configure_execution(
        &mut self,
        execution: Option<&crate::streaming::ExecutionContext>,
    ) {
        self.execution = execution.cloned();
    }

    fn edge(&mut self) -> Result<(), String> {
        if let Some(execution) = &self.execution {
            execution.charge_edges(1)?;
        }
        self.edge_visits = self
            .edge_visits
            .checked_add(1)
            .ok_or("sort edge overflow")?;
        if self.edge_visits > MAX_SORT_EDGE_VISITS {
            return Err("sort relation edge budget exceeded".into());
        }
        Ok(())
    }

    fn load(
        &mut self,
        view: &MergedRelationView<'_>,
        address: MergedRecordSlot,
    ) -> Result<Option<Arc<Value>>, String> {
        if let Some(value) = self.values.get(&address) {
            self.reuse += 1;
            return Ok(Some(value.clone()));
        }
        let Some(current) = view.record(address)? else {
            return Ok(None);
        };
        if current.record.class == RecordClass::RawFallback {
            return Ok(None);
        }
        let estimated = current
            .record
            .range
            .len()
            .checked_mul(4)
            .and_then(|bytes| bytes.checked_add(128))
            .ok_or("sort target byte estimate overflow")?;
        let next = self
            .retained_bytes
            .checked_add(estimated)
            .ok_or("sort target byte budget overflow")?;
        if next > MAX_SORT_TARGET_BYTES {
            return Err("sort target cache budget exceeded".into());
        }
        let path = view.repository_root().join(&current.file.relative_path);
        if let Some(execution) = &self.execution {
            execution.check_string()?;
            execution.charge_reads(1)?;
            execution.charge_bytes(current.record.range.len())?;
        }
        let bytes = match crate::source::read_record_range_for_search(
            &path,
            current.file.revision,
            current.record.range,
            self.execution.as_ref(),
        ) {
            Ok(bytes) => bytes,
            Err(message) => {
                view.record_source_failure(current.file, current.record.range, message);
                return Ok(None);
            }
        };
        let extracted = crate::record_preparation::prepare_query_record_with_execution(
            current.record.class,
            &current.collection.source,
            &current.collection.definition.schema,
            &bytes,
            &crate::source::ScanProjection::full(false),
            false,
            self.execution.as_ref(),
        )?;
        let value = Arc::new(extracted.value);
        self.retained_bytes = next;
        self.reads += 1;
        self.bytes_read += bytes.len() as u64;
        self.values.insert(address, value.clone());
        Ok(Some(value))
    }
}


fn expression(field: &SortField) -> Result<SortExpression, String> {
    if let Some(expression) = &field.expression {
        return Ok(expression.clone());
    }
    // Keep legacy untyped strings as literal paths when they parse. A typed
    // SortExpression is the unambiguous programmatic way to request a binding.
    if QueryPath::parse(&field.field).is_ok() {
        return Ok(SortExpression {
            path: field.field.clone(),
            binding: None,
            reducer: None,
        });
    }
    let (path, binding) = split_path_binding(&field.field);
    let binding = binding.ok_or_else(|| format!("invalid sort field: {}", field.field))?;
    QueryPath::parse(path)?;
    if binding.is_empty() {
        return Err("empty sort binding".into());
    }
    Ok(SortExpression {
        path: path.into(),
        binding: Some(binding.into()),
        reducer: None,
    })
}

fn bind_transform(
    schema: &crate::reference::Schema,
    path: &QueryPath,
    requested: Option<&str>,
) -> Result<(SortTransform, SortTransformIdentity), String> {
    let name = requested.or_else(|| {
        schema
            .defaults
            .get(&path.field.segments)
            .map(String::as_str)
    });
    match name {
        None | Some("default") => Ok((SortTransform::Identity, SortTransformIdentity::Identity)),
        Some("lower") => Ok((SortTransform::CaseFold, SortTransformIdentity::CaseFold)),
        Some(name) => {
            let key = (path.field.segments.clone(), name.to_owned());
            if let Some(binding) = schema.custom.get(&key) {
                return Ok((
                    match binding.transform {
                        Transform::Identity => SortTransform::Identity,
                        Transform::CaseFold => SortTransform::CaseFold,
                    },
                    SortTransformIdentity::Custom {
                        name: binding.name.clone(),
                        version: binding.version,
                        transform: binding.transform,
                    },
                ));
            }
            let binding = schema
                .operations
                .get(&key)
                .ok_or_else(|| format!("unknown sort binding {name}"))?;
            if !binding.operation.sort_compatible() {
                return Err(format!(
                    "sort binding {name} does not produce a stable scalar"
                ));
            }
            Ok((
                SortTransform::Operation(binding.operation.clone()),
                SortTransformIdentity::Operation {
                    name: binding.name.clone(),
                    version: binding.version,
                },
            ))
        }
    }
}


impl<'a> BoundSort<'a> {
    pub fn bind(repo: &'a Repository, query: &Query) -> Result<Self, String> {
        let mut terms = Vec::with_capacity(repo.collections.len());
        let mut has_computed = false;
        let mut needs_source = false;
        let mut expected_config = vec![None; query.options.sort.len()];
        for (index, collection) in repo.collections.iter().enumerate() {
            if !query.collections.is_empty()
                && !query.collections.contains(&collection.definition.name)
            {
                terms.push(None);
                continue;
            }
            let mut selected = Vec::with_capacity(query.options.sort.len());
            for (position, field) in query.options.sort.iter().enumerate() {
                let expression = expression(field)?;
                let path = QueryPath::parse(&expression.path)?;
                let related = resolve_path(repo, index, &path)?;
                if related.as_ref().is_some_and(|(hops, _)| {
                    matches!(
                        expression.reducer,
                        Some(SortReducer::First | SortReducer::Last)
                    ) && hops
                        .iter()
                        .any(|hop| hop.direction == HopDirection::Backlink)
                }) {
                    return Err("FIRST/LAST over backlinks has no structural source order".into());
                }
                let terminal_index = related
                    .as_ref()
                    .and_then(|(hops, _)| hops.last())
                    .map_or(index, |hop| hop.to.0 as usize);
                let terminal = related.as_ref().map_or(&path, |(_, terminal)| terminal);
                let target = &repo.collections[terminal_index];
                let (transform, transform_identity) = bind_transform(
                    &target.definition.schema,
                    terminal,
                    expression.binding.as_deref(),
                )?;
                let column = if related.is_none()
                    && expression.reducer.is_none()
                    && expression.binding.is_none()
                    && !path.has_explicit_array_step
                    && matches!(&transform, SortTransform::Identity)
                {
                    collection.sorting.columns.get(&path.field)
                } else {
                    None
                };
                let (source, locale, options) = if let Some(column) = column {
                    (
                        BoundSortSource::Stored(column),
                        column.locale.clone(),
                        column.options,
                    )
                } else {
                    has_computed = true;
                    needs_source |= related.is_some() || path.field.root == PathRoot::Data;
                    let config = SortConfig {
                        locale: target.definition.sort_locale.clone(),
                        options: target.definition.sort_options,
                    };
                    let collator = make_collator(&config)?;
                    (
                        BoundSortSource::Computed(Box::new(ComputedSort {
                            path,
                            related,
                            reducer: expression.reducer,
                            transform,
                            collator,
                        })),
                        config.locale,
                        config.options,
                    )
                };
                let config = (locale, options, transform_identity);
                if expected_config[position]
                    .as_ref()
                    .is_some_and(|previous| previous != &config)
                {
                    return Err(format!(
                        "incompatible cross-collection sort: {}",
                        field.field
                    ));
                }
                expected_config[position] = Some(config);
                selected.push(BoundSortTerm {
                    source,
                    descending: field.descending,
                    options,
                });
            }
            terms.push(Some(selected));
        }
        Ok(Self {
            terms,
            has_computed,
            needs_source,
        })
    }

    /// Evaluate every requested key for a verified match before top-k selection.
    pub fn keys(
        &self,
        repo: &Repository,
        collection_index: usize,
        ctx: &RecordContext<'_>,
        target_cache: &Mutex<SortTargetCache>,
        external_targets: Option<&crate::relation_target_cache::TargetValueCache>,
    ) -> Result<Vec<(SortKey, bool, SortOptions)>, String> {
        self.terms[collection_index]
            .as_ref()
            .ok_or("sort collection is not selected")?
            .iter()
            .map(|term| {
                let key = match &term.source {
                    BoundSortSource::Stored(column) => {
                        column.keys[ctx.record.slot.0 as usize].clone()
                    }
                    BoundSortSource::Computed(computed) => {
                        if computed.related.is_some() {
                            computed.key(
                                repo,
                                collection_index,
                                ctx,
                                Some(target_cache),
                                external_targets,
                            )?
                        } else {
                            computed.key(repo, collection_index, ctx, None, external_targets)?
                        }
                    }
                };
                Ok((key, term.descending, term.options))
            })
            .collect()
    }
}


impl ComputedSort {
    fn key(
        &self,
        repo: &Repository,
        collection_index: usize,
        ctx: &RecordContext<'_>,
        target_cache: Option<&Mutex<SortTargetCache>>,
        external_targets: Option<&crate::relation_target_cache::TargetValueCache>,
    ) -> Result<SortKey, String> {
        if let Some((hops, terminal)) = &self.related {
            let mut reduced = SortAccumulator::new(self.reducer);
            self.visit_related(
                repo,
                hops,
                terminal,
                0,
                collection_index,
                ctx.record.slot,
                Some(ctx.value),
                target_cache.ok_or("related sort target cache missing")?,
                external_targets,
                &mut reduced,
            )?;
            return reduced.finish(self);
        }
        self.key_local(ctx)
    }

    /// Evaluate a computed key that only uses the current record. The caller
    /// may provide a base or a segment-local RecordContext.
    pub(crate) fn key_local(&self, ctx: &RecordContext<'_>) -> Result<SortKey, String> {
        if self.related.is_some() {
            return Err("related sort needs merged relation resolution".into());
        }
        if let PathRoot::Metadata(field) = &self.path.field.root {
            let value = metadata_value(*field, ctx)?;
            return self.reduce(&[&value], false);
        }
        if ctx.record.class == RecordClass::RawFallback {
            return Ok(SortKey::Unavailable);
        }
        let mut selected = Vec::new();
        let mut expanded = false;
        let _present = select_values(ctx.value, &self.path.steps, &mut selected, &mut expanded);
        self.reduce(&selected, expanded)
    }

    pub(crate) fn key_merged(
        &self,
        view: &MergedRelationView<'_>,
        address: MergedRecordSlot,
        ctx: &RecordContext<'_>,
        cache: &mut MergedSortTargetCache,
    ) -> Result<SortKey, String> {
        let Some((hops, terminal)) = &self.related else {
            return self.key_local(ctx);
        };
        let mut reduced = SortAccumulator::new(self.reducer);
        self.visit_related_merged(
            view,
            hops,
            terminal,
            0,
            address,
            Some(ctx.value),
            cache,
            &mut reduced,
        )?;
        reduced.finish(self)
    }

    /// A simple one-hop related sort can reuse the target's resident key.
    /// The target column has the same collection collation as this computed
    /// expression; array and transformed paths retain source evaluation.
    pub(crate) fn direct_related_target_column(&self, hop: &BoundRelationHop) -> Option<&str> {
        let (hops, terminal) = self.related.as_ref()?;
        if hops.as_slice() != std::slice::from_ref(hop)
            || self.reducer.is_some()
            || !matches!(self.transform, SortTransform::Identity)
            || terminal.field.root != PathRoot::Data
            || terminal.has_explicit_array_step
            || terminal.has_position
            || terminal.field.segments.len() != 1
            || terminal
                .steps
                .iter()
                .any(|step| !matches!(step, PathStep::Field(_)))
        {
            return None;
        }
        Some(&terminal.display)
    }

    #[allow(clippy::too_many_arguments)]
    fn visit_related_merged(
        &self,
        view: &MergedRelationView<'_>,
        hops: &[BoundRelationHop],
        terminal: &QueryPath,
        depth: usize,
        address: MergedRecordSlot,
        source_value: Option<&Value>,
        cache: &mut MergedSortTargetCache,
        reduced: &mut SortAccumulator,
    ) -> Result<(), String> {
        let Some(current) = view.record(address)? else {
            reduced.observe_unavailable()?;
            return Ok(());
        };
        if current.record.class == RecordClass::RawFallback {
            reduced.observe_unavailable()?;
            return Ok(());
        }
        if depth == hops.len() {
            if let PathRoot::Metadata(field) = terminal.field.root {
                let empty = Value::Null;
                let ctx = RecordContext {
                    value: source_value.unwrap_or(&empty),
                    source_bytes: None,
                    record: current.record,
                    file: current.file,
                    collection_name: &current.collection.definition.name,
                    repository_root: view.repository_root(),
                };
                reduced.observe_value(&metadata_value(field, &ctx)?, self)?;
            } else {
                let owned = if source_value.is_none() {
                    cache.load(view, address)?
                } else {
                    None
                };
                let Some(value) = source_value.or(owned.as_deref()) else {
                    reduced.observe_unavailable()?;
                    return Ok(());
                };
                let mut selected = Vec::new();
                let mut expanded = false;
                select_values(value, &terminal.steps, &mut selected, &mut expanded);
                reduced.expanded |= expanded;
                for value in selected {
                    reduced.observe_value(value, self)?;
                }
            }
            return Ok(());
        }
        let hop = &hops[depth];
        if hop.from != address.collection {
            return Err("merged sort relation hop collection mismatch".into());
        }
        let has_position = hop.selectors.iter().any(Option::is_some);
        reduced.expanded |= match hop.direction {
            HopDirection::Backlink => !has_position,
            HopDirection::Forward => {
                view.relation_cardinality(hop.relation)?
                    == crate::relation::RelationCardinality::Many
                    && !has_position
            }
        };
        match hop.direction {
            HopDirection::Forward => {
                let (edges, _present, null, _empty, unavailable) =
                    view.forward_edges(address, hop.relation.ordinal as usize)?;
                if unavailable {
                    reduced.observe_unavailable()?;
                    return Ok(());
                }
                if matches!(self.reducer, Some(SortReducer::First | SortReducer::Last)) {
                    let owned = if source_value.is_none() {
                        cache.load(view, address)?
                    } else {
                        None
                    };
                    let Some(value) = source_value.or(owned.as_deref()) else {
                        reduced.observe_unavailable()?;
                        return Ok(());
                    };
                    let mut positions = Vec::new();
                    visit_reference_occurrences(
                        value,
                        &current.collection.relations[hop.relation.ordinal as usize]
                            .declaration
                            .path,
                        &mut positions,
                        &mut |id, positions| {
                            if !positions_match(&hop.selectors, positions) {
                                return Ok(());
                            }
                            cache.edge()?;
                            if let Some(id) = id {
                                if let Some(target) = view.resolve_id(hop.to, id)? {
                                    self.visit_related_merged(
                                        view,
                                        hops,
                                        terminal,
                                        depth + 1,
                                        target,
                                        None,
                                        cache,
                                        reduced,
                                    )
                                } else {
                                    reduced.observe_unavailable()
                                }
                            } else {
                                reduced.observe_value(&Value::Null, self)
                            }
                        },
                    )?;
                } else {
                    for (id, positions) in edges {
                        if !positions_match(&hop.selectors, positions) {
                            continue;
                        }
                        cache.edge()?;
                        if let Some(target) = view.resolve_id(hop.to, id)? {
                            self.visit_related_merged(
                                view,
                                hops,
                                terminal,
                                depth + 1,
                                target,
                                None,
                                cache,
                                reduced,
                            )?;
                        } else {
                            reduced.observe_unavailable()?;
                        }
                    }
                    if null {
                        reduced.observe_value(&Value::Null, self)?;
                    }
                }
            }
            HopDirection::Backlink => {
                for (position, source) in view
                    .backlink_sources(hop.relation, &current.record.id.0)?
                    .into_iter()
                    .enumerate()
                {
                    cache.edge()?;
                    if hop.selectors.first().is_some_and(|selector| {
                        selector.is_some_and(|index| index as usize != position)
                    }) {
                        continue;
                    }
                    self.visit_related_merged(
                        view,
                        hops,
                        terminal,
                        depth + 1,
                        source,
                        None,
                        cache,
                        reduced,
                    )?;
                }
            }
        }
        Ok(())
    }

    fn scalar_key(&self, value: &Value) -> Result<SortKey, String> {
        let transformed;
        let selected = match &self.transform {
            SortTransform::Identity => value,
            SortTransform::CaseFold => {
                transformed = match value {
                    Value::String(text) => Value::String(text.chars().case_fold().collect()),
                    Value::Id(text) => Value::Id(text.chars().case_fold().collect()),
                    _ => value.clone(),
                };
                &transformed
            }
            SortTransform::Operation(operation) => {
                transformed = operation
                    .map_value(value)
                    .ok_or("sort transformation failed")?;
                &transformed
            }
        };
        match selected {
            Value::Null => Ok(SortKey::Null),
            Value::String(text) => {
                let mut key = Vec::new();
                self.collator
                    .write_sort_key_to(text, &mut key)
                    .map_err(|error| error.to_string())?;
                Ok(SortKey::String(key))
            }
            Value::Id(text) => Ok(SortKey::Id(text.clone())),
            Value::Number(number) => Ok(SortKey::Number(number.clone())),
            Value::Bool(value) => Ok(SortKey::Bool(*value)),
            Value::Date(value) => Ok(SortKey::Date(*value)),
            Value::Datetime(value) => Ok(SortKey::Datetime(*value)),
            Value::Unixtime(value) => Ok(SortKey::Unixtime(*value)),
            Value::Array(_) | Value::Object(_) => {
                Err(format!("unsupported sort value: {}", self.path.display))
            }
        }
    }

    fn reduce(&self, selected: &[&Value], expanded: bool) -> Result<SortKey, String> {
        if self.reducer.is_none()
            && selected
                .iter()
                .any(|value| matches!(value, Value::Array(_)))
        {
            return Err(format!("multi-valued sort field: {}", self.path.display));
        }
        let mut accumulated = SortAccumulator::new(self.reducer);
        accumulated.expanded = expanded;
        for value in selected {
            accumulated.observe_value(value, self)?;
        }
        accumulated.finish(self)
    }
}


// Every child must be visited to retain all source occurrences.
#[allow(clippy::unnecessary_fold)]
fn select_values<'a>(
    value: &'a Value,
    steps: &[PathStep],
    output: &mut Vec<&'a Value>,
    expanded: &mut bool,
) -> bool {
    let Some((step, rest)) = steps.split_first() else {
        output.push(value);
        return true;
    };
    match (step, value) {
        (PathStep::Field(name), Value::Object(fields)) => fields
            .get(name)
            .is_some_and(|next| select_values(next, rest, output, expanded)),
        (PathStep::Field(_), Value::Array(array)) => {
            *expanded = true;
            array.values.is_empty()
                || array.values.iter().fold(false, |found, child| {
                    select_values(child, steps, output, expanded) || found
                })
        }
        (PathStep::ArrayIndex(index), Value::Array(array)) => array
            .values
            .get(*index as usize)
            .is_some_and(|next| select_values(next, rest, output, expanded)),
        (PathStep::EachElement, Value::Array(array)) => {
            *expanded = true;
            array.values.is_empty()
                || array.values.iter().fold(false, |found, child| {
                    select_values(child, rest, output, expanded) || found
                })
        }
        _ => false,
    }
}


impl SortTargetCache {
    pub(crate) fn configure_execution(&mut self, execution: &crate::streaming::ExecutionContext) {
        self.execution = Some(execution.clone());
    }

    pub(crate) fn configure_stream(
        &mut self,
        max_buffered_bytes: usize,
        cancellation: &crate::streaming::CancellationToken,
    ) {
        self.stream_limit = Some((max_buffered_bytes / 4) as u64);
        self.cancellation = Some(cancellation.clone());
    }

    pub(crate) fn retained_bytes(&self) -> u64 {
        self.retained_bytes
    }

    pub(crate) fn reserve_headroom(&self) -> u64 {
        MAX_SORT_TARGET_BYTES.saturating_sub(self.retained_bytes)
    }

    fn is_active(
        cache: &Mutex<Self>,
        collection: CollectionId,
        slot: RecordSlot,
    ) -> Result<bool, String> {
        let state = cache.lock().map_err(|error| error.to_string())?;
        Ok(state
            .execution
            .as_ref()
            .and_then(|execution| execution.active_records())
            .is_none_or(|domains| domains[collection.0 as usize].contains(slot)))
    }

    fn resolve_target(
        cache: &Mutex<Self>,
        repo: &Repository,
        collection: CollectionId,
        target_id: &str,
        static_target: &RelationTarget,
    ) -> Result<Option<RecordSlot>, String> {
        let state = cache.lock().map_err(|error| error.to_string())?;
        if let Some(domains) = state
            .execution
            .as_ref()
            .and_then(|execution| execution.active_records())
        {
            Ok(repo.collections[collection.0 as usize]
                .id_lookup
                .get(target_id)
                .copied()
                .filter(|slot| domains[collection.0 as usize].contains(*slot)))
        } else {
            Ok(match static_target {
                RelationTarget::Resolved(slot) => Some(*slot),
                RelationTarget::Unresolved(_) => None,
            })
        }
    }

    fn edge(cache: &Mutex<Self>) -> Result<(), String> {
        let mut state = cache.lock().map_err(|error| error.to_string())?;
        if let Some(execution) = &state.execution {
            execution.charge_edges(1)?;
        }
        if state
            .cancellation
            .as_ref()
            .is_some_and(crate::streaming::CancellationToken::is_cancelled)
        {
            return Err("stream cancelled".into());
        }
        state.edge_visits = state
            .edge_visits
            .checked_add(1)
            .ok_or("sort edge counter overflow")?;
        if state.edge_visits > MAX_SORT_EDGE_VISITS {
            return Err(state.execution.as_ref().map_or_else(
                || "sort relation edge budget exceeded".to_owned(),
                |execution| {
                    execution.fail(
                        crate::streaming::ExecutionErrorKind::Resource,
                        "sort relation edge budget exceeded",
                    )
                },
            ));
        }
        Ok(())
    }

    fn external_reuse(cache: &Mutex<Self>) -> Result<(), String> {
        let mut state = cache.lock().map_err(|error| error.to_string())?;
        state.reuse += 1;
        Ok(())
    }

    fn load(
        cache: &Mutex<Self>,
        repo: &Repository,
        collection_index: usize,
        slot: RecordSlot,
    ) -> Result<Option<Arc<Value>>, String> {
        let collection = &repo.collections[collection_index];
        let record = collection
            .generation
            .records
            .get(slot.0 as usize)
            .ok_or("sort target slot is missing")?;
        let (cell, cancellation, execution) = {
            let mut state = cache.lock().map_err(|error| error.to_string())?;
            if state
                .execution
                .as_ref()
                .and_then(|execution| execution.active_records())
                .is_some_and(|domains| {
                    !domains[collection.generation.collection.0 as usize].contains(slot)
                })
            {
                return Ok(None);
            }
            if state
                .cancellation
                .as_ref()
                .is_some_and(crate::streaming::CancellationToken::is_cancelled)
            {
                return Err("stream cancelled".into());
            }
            if record.class == RecordClass::RawFallback {
                return Ok(None);
            }
            if let Some(execution) = &state.execution {
                execution.check_string()?;
                if record.range.len() > execution.limits.target_record_bytes {
                    return Err(execution.fail(
                        crate::streaming::ExecutionErrorKind::Resource,
                        "sort target record exceeds source read allowance",
                    ));
                }
            }
            let id = (collection_index, slot.0);
            let cell = if let Some(cell) = state.values.get(&id).cloned() {
                state.reuse += 1;
                cell
            } else {
                let estimated = record
                    .range
                    .len()
                    .checked_mul(4)
                    .and_then(|bytes| bytes.checked_add(128))
                    .ok_or("sort target byte estimate overflow")?;
                let next = state
                    .retained_bytes
                    .checked_add(estimated)
                    .ok_or("sort target byte budget overflow")?;
                if next > MAX_SORT_TARGET_BYTES {
                    return Err(state.execution.as_ref().map_or_else(
                        || "sort target cache budget exceeded".to_owned(),
                        |execution| {
                            execution.fail(
                                crate::streaming::ExecutionErrorKind::Resource,
                                "sort target cache budget exceeded",
                            )
                        },
                    ));
                }
                if state.stream_limit.is_some_and(|limit| next > limit) {
                    return Err(state.execution.as_ref().map_or_else(
                        || {
                            "stream resource: sort target cache exceeds buffered-byte limit"
                                .to_owned()
                        },
                        |execution| {
                            execution.fail(
                                crate::streaming::ExecutionErrorKind::Resource,
                                "stream resource: sort target cache exceeds buffered-byte limit",
                            )
                        },
                    ));
                }
                if let Some(execution) = &state.execution {
                    execution.charge_reads(1)?;
                    execution.charge_bytes(record.range.len())?;
                }
                // Reserve pending loads too, before any worker leaves the lock.
                state.retained_bytes = next;
                let cell = Arc::new(OnceLock::new());
                state.values.insert(id, cell.clone());
                cell
            };
            (cell, state.cancellation.clone(), state.execution.clone())
        };
        // Only callers for this target wait here; other targets can load in parallel.
        let result = cell.get_or_init(|| {
            let path = repo
                .files
                .native_path(record.file)
                .ok_or("sort target path is missing")?;
            let bytes = crate::source::read_record_range_for_search(
                &path,
                record.revision,
                record.range,
                execution.as_ref(),
            )?;
            if cancellation
                .as_ref()
                .is_some_and(crate::streaming::CancellationToken::is_cancelled)
            {
                return Err("stream cancelled".into());
            }
            let extracted = crate::record_preparation::prepare_query_record_with_execution(
                record.class,
                &collection.source,
                &collection.definition.schema,
                &bytes,
                &crate::source::ScanProjection::full(false),
                false,
                execution.as_ref(),
            )?;
            let mut state = cache.lock().map_err(|error| error.to_string())?;
            state.reads += 1;
            state.bytes_read += bytes.len() as u64;
            Ok(Arc::new(extracted.value))
        });
        if cancellation
            .as_ref()
            .is_some_and(crate::streaming::CancellationToken::is_cancelled)
        {
            return Err("stream cancelled".into());
        }
        result.clone().map(Some)
    }
}


/// One reduced key and the minimum state needed across relation occurrences.
struct SortAccumulator {
    reducer: Option<SortReducer>,
    count: u64,
    best: Option<SortKey>,
    last_raw: Option<LastValue>,
    saw_null: bool,
    saw_unavailable: bool,
    expanded: bool,
}


/// Only a scalar candidate or a compact error marker is retained for LAST.
enum LastValue {
    Scalar(Value),
    InvalidObject,
    Unavailable,
}


impl SortAccumulator {
    fn new(reducer: Option<SortReducer>) -> Self {
        Self {
            reducer,
            count: 0,
            best: None,
            last_raw: None,
            saw_null: false,
            saw_unavailable: false,
            expanded: false,
        }
    }

    fn observe_unavailable(&mut self) -> Result<(), String> {
        self.count = self
            .count
            .checked_add(1)
            .ok_or("sort occurrence overflow")?;
        self.saw_unavailable = true;
        match self.reducer {
            Some(SortReducer::First) if self.count > 1 => {}
            Some(SortReducer::Last) => {
                self.best = None;
                self.last_raw = Some(LastValue::Unavailable);
            }
            _ => {
                self.best.get_or_insert(SortKey::Unavailable);
            }
        }
        Ok(())
    }

    fn observe_value(&mut self, value: &Value, sort: &ComputedSort) -> Result<(), String> {
        if let Value::Array(array) = value {
            self.expanded = true;
            for child in &array.values {
                self.observe_value(child, sort)?;
            }
            return Ok(());
        }
        self.count = self
            .count
            .checked_add(1)
            .ok_or("sort occurrence overflow")?;
        match self.reducer {
            Some(SortReducer::First) if self.count > 1 => {}
            Some(SortReducer::Last) => {
                self.last_raw = Some(match value {
                    Value::Object(_) => LastValue::InvalidObject,
                    _ => LastValue::Scalar(value.clone()),
                });
                self.best = None;
            }
            Some(SortReducer::Min | SortReducer::Max) => {
                let key = sort.scalar_key(value)?;
                if matches!(key, SortKey::Null) {
                    self.saw_null = true;
                } else if self.best.as_ref().is_none_or(|old| {
                    let order = key.compare_with(old, false, SortOptions::default());
                    if self.reducer == Some(SortReducer::Min) {
                        order.is_lt()
                    } else {
                        order.is_gt()
                    }
                }) {
                    self.best = Some(key);
                }
            }
            _ => {
                if self.best.is_none() {
                    self.best = Some(sort.scalar_key(value)?);
                }
            }
        }
        Ok(())
    }

    fn finish(mut self, sort: &ComputedSort) -> Result<SortKey, String> {
        if self.count == 0 {
            return Ok(SortKey::Missing);
        }
        if self.reducer.is_none() && (self.expanded || self.count > 1) {
            return Err(format!("multi-valued sort field: {}", sort.path.display));
        }
        if self.reducer == Some(SortReducer::Last) {
            self.best = match self.last_raw {
                Some(LastValue::Scalar(value)) => Some(sort.scalar_key(&value)?),
                Some(LastValue::InvalidObject) => {
                    return Err(format!("unsupported sort value: {}", sort.path.display));
                }
                Some(LastValue::Unavailable) => Some(SortKey::Unavailable),
                None => self.best,
            };
        }
        if matches!(self.reducer, Some(SortReducer::Min | SortReducer::Max)) {
            if self.saw_unavailable {
                return Ok(SortKey::Unavailable);
            }
            return Ok(self.best.unwrap_or(if self.saw_null {
                SortKey::Null
            } else {
                SortKey::Missing
            }));
        }
        Ok(self.best.unwrap_or(SortKey::Missing))
    }
}


impl ComputedSort {
    #[allow(clippy::too_many_arguments)]
    fn visit_related(
        &self,
        repo: &Repository,
        hops: &[BoundRelationHop],
        terminal: &QueryPath,
        depth: usize,
        collection_index: usize,
        slot: RecordSlot,
        source_value: Option<&Value>,
        cache: &Mutex<SortTargetCache>,
        external_targets: Option<&crate::relation_target_cache::TargetValueCache>,
        reduced: &mut SortAccumulator,
    ) -> Result<(), String> {
        let collection = &repo.collections[collection_index];
        if !SortTargetCache::is_active(cache, collection.generation.collection, slot)? {
            reduced.observe_unavailable()?;
            return Ok(());
        }
        let record = collection
            .generation
            .records
            .get(slot.0 as usize)
            .ok_or("sort relation slot is missing")?;
        if record.class == RecordClass::RawFallback {
            reduced.observe_unavailable()?;
            return Ok(());
        }
        if depth == hops.len() {
            let external = match (source_value, terminal.steps.first()) {
                (None, Some(PathStep::Field(field))) if terminal.field.root == PathRoot::Data => {
                    external_targets.and_then(|targets| {
                        targets.get_projected_field(collection.generation.collection, slot, field)
                    })
                }
                _ => None,
            };
            if external.is_some() {
                SortTargetCache::external_reuse(cache)?;
            }
            let owned = if source_value.is_none() && external.is_none() {
                SortTargetCache::load(cache, repo, collection_index, slot)?
            } else {
                None
            };
            let value = source_value
                .or(external)
                .or(owned.as_deref())
                .ok_or("sort target value unavailable")?;
            let mut selected = Vec::new();
            let mut expanded = false;
            let _present = select_values(value, &terminal.steps, &mut selected, &mut expanded);
            reduced.expanded |= expanded;
            for value in selected {
                reduced.observe_value(value, self)?;
            }
            return Ok(());
        }
        let hop = &hops[depth];
        if hop.from.0 as usize != collection_index {
            return Err("sort relation hop collection mismatch".into());
        }
        let source_collection = &repo.collections[hop.relation.source.0 as usize];
        let relation = source_collection
            .relations
            .get(hop.relation.ordinal as usize)
            .ok_or("sort relation index is missing")?;
        if hop.direction == HopDirection::Forward && relation.unavailable().contains(slot.0) {
            reduced.observe_unavailable()?;
            return Ok(());
        }
        let has_position = hop.selectors.iter().any(Option::is_some);
        reduced.expanded |= match hop.direction {
            HopDirection::Backlink => !has_position,
            HopDirection::Forward => {
                relation.declaration.cardinality == crate::relation::RelationCardinality::Many
                    && !has_position
            }
        };
        match hop.direction {
            HopDirection::Forward => {
                if matches!(self.reducer, Some(SortReducer::First | SortReducer::Last)) {
                    // A FILTER projection may omit this intermediate relation
                    // field. Use the complete source record for occurrence order.
                    let owned = if source_value.is_none() {
                        SortTargetCache::load(cache, repo, collection_index, slot)?
                    } else {
                        None
                    };
                    let value = source_value
                        .or(owned.as_deref())
                        .ok_or("sort relation source unavailable")?;
                    let mut positions = Vec::new();
                    visit_reference_occurrences(
                        value,
                        &relation.declaration.path,
                        &mut positions,
                        &mut |id, positions| {
                            if !positions_match(&hop.selectors, positions) {
                                return Ok(());
                            }
                            SortTargetCache::edge(cache)?;
                            if let Some(id) = id {
                                if let Some(target) = repo.collections[hop.to.0 as usize]
                                    .id_lookup
                                    .get(id)
                                    .copied()
                                {
                                    if SortTargetCache::is_active(cache, hop.to, target)? {
                                        self.visit_related(
                                            repo,
                                            hops,
                                            terminal,
                                            depth + 1,
                                            hop.to.0 as usize,
                                            target,
                                            None,
                                            cache,
                                            external_targets,
                                            reduced,
                                        )
                                    } else {
                                        reduced.observe_unavailable()
                                    }
                                } else {
                                    reduced.observe_unavailable()
                                }
                            } else {
                                reduced.observe_value(&Value::Null, self)
                            }
                        },
                    )?;
                } else {
                    for edge in relation.edges(slot).ok_or("sort relation edges missing")? {
                        if !positions_match(&hop.selectors, &edge.positions) {
                            continue;
                        }
                        SortTargetCache::edge(cache)?;
                        match SortTargetCache::resolve_target(
                            cache,
                            repo,
                            hop.to,
                            edge.target_id(),
                            &edge.target,
                        )? {
                            Some(target) => self.visit_related(
                                repo,
                                hops,
                                terminal,
                                depth + 1,
                                hop.to.0 as usize,
                                target,
                                None,
                                cache,
                                external_targets,
                                reduced,
                            )?,
                            None => reduced.observe_unavailable()?,
                        }
                    }
                    if relation.nulls().contains(slot.0) {
                        reduced.observe_value(&Value::Null, self)?;
                    }
                }
            }
            HopDirection::Backlink => {
                let sources = repo.relation_reverse_sources(relation, &record.id.0);
                if let Some(sources) = sources {
                    if has_position {
                        let mut seen = 0usize;
                        for source in &source_collection.identity_order {
                            if !SortTargetCache::is_active(cache, hop.to, *source)? {
                                continue;
                            }
                            if !sources.contains(source.0) {
                                continue;
                            }
                            SortTargetCache::edge(cache)?;
                            if hop.selectors.first().is_none_or(|selector| {
                                selector.is_none_or(|index| index as usize == seen)
                            }) {
                                self.visit_related(
                                    repo,
                                    hops,
                                    terminal,
                                    depth + 1,
                                    hop.to.0 as usize,
                                    *source,
                                    None,
                                    cache,
                                    external_targets,
                                    reduced,
                                )?;
                            }
                            seen += 1;
                        }
                    } else {
                        for source in sources.iter() {
                            if !SortTargetCache::is_active(cache, hop.to, RecordSlot(source))? {
                                continue;
                            }
                            SortTargetCache::edge(cache)?;
                            self.visit_related(
                                repo,
                                hops,
                                terminal,
                                depth + 1,
                                hop.to.0 as usize,
                                RecordSlot(source),
                                None,
                                cache,
                                external_targets,
                                reduced,
                            )?;
                        }
                    }
                }
            }
        }
        Ok(())
    }
}


fn positions_match(selectors: &[Option<u32>], positions: &[u32]) -> bool {
    selectors.len() <= positions.len()
        && selectors
            .iter()
            .zip(positions)
            .all(|(selector, position)| selector.is_none_or(|index| index == *position))
}

fn visit_reference_occurrences(
    value: &Value,
    path: &[String],
    positions: &mut Vec<u32>,
    emit: &mut impl FnMut(Option<&str>, &[u32]) -> Result<(), String>,
) -> Result<(), String> {
    if let Value::Array(array) = value {
        for (index, child) in array.values.iter().enumerate() {
            positions.push(u32::try_from(index).map_err(|_| "sort relation array index overflow")?);
            visit_reference_occurrences(child, path, positions, emit)?;
            positions.pop();
        }
        return Ok(());
    }
    if let Some((head, tail)) = path.split_first() {
        if let Value::Object(fields) = value
            && let Some(child) = fields.get(head)
        {
            visit_reference_occurrences(child, tail, positions, emit)?;
        }
        return Ok(());
    }
    match value {
        Value::Id(id) | Value::String(id) => emit(Some(id), positions),
        Value::Null => emit(None, positions),
        _ => Ok(()),
    }
}

/// Approximate bytes retained by a prepared scalar key.
pub(crate) fn key_bytes(key: &SortKey) -> usize {
    match key {
        SortKey::String(bytes) => bytes.capacity(),
        SortKey::Id(text) => text.capacity(),
        _ => 0,
    }
}

/// Compare already prepared terms without reading source or allocating keys.
pub(crate) fn compare_parts(
    left: &[(SortKey, bool, SortOptions)],
    right: &[(SortKey, bool, SortOptions)],
) -> Ordering {
    for ((left, descending, options), (right, _, _)) in left.iter().zip(right) {
        let order = left.compare_with(right, *descending, *options);
        if !order.is_eq() {
            return order;
        }
    }
    Ordering::Equal
}

#[cfg(test)]
#[path = "sort_binding.test.rs"]
mod tests;
