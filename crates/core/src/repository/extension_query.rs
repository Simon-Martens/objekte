//! Exact execution of registered logical plans over selected source records.
//! The frontend compiles syntax only; source reads, Boolean evaluation,
//! bounded ordering, counts and page materialization belong to the engine.

use super::*;
use crate::{
    Direction, JsonFilter, JsonPathStep, Sort, TextFormat,
    extensions::{
        BindingContext, BoundTextQuery, CandidateConstraint, DecodedRecord, Extraction,
        LogicalPlan, LogicalPredicate, ScalarValue, TextPreparation, TypedValue,
    },
    file_picker::TypedKey,
    repository::{
        extension_derived_index::SelectedDerivedKey,
        extension_index::RepresentationIndexKey,
        ordered_result_cache::{OrderedCoverage, OrderedResultKey},
        predicate_cache::PredicateCacheKey,
    },
    sorting::SortValue,
    text::TextRules,
};
use std::{cmp::Ordering, collections::BinaryHeap, sync::Arc};


/// One projected source record. A column with no emitted scalar values is an
/// empty vector; null is represented by `TypedValue::Null`.
#[derive(Clone, Debug)]
pub struct ExtensionQueryRow {
    pub source: crate::extensions::SourceDescriptor,
    pub values: BTreeMap<String, Vec<TypedValue>>,
}


/// A page from a registered frontend. `identity` uses semantic descriptors
/// and parameters rather than process-local numeric handles.
#[derive(Debug)]
pub struct ExtensionQueryResult {
    pub rows: Vec<ExtensionQueryRow>,
    pub total: Option<usize>,
    pub truncated: bool,
    pub identity: String,
    pub elapsed_ms: f64,
    pub metrics: Metrics,
    pub query_plan: Vec<String>,
}


enum CompiledPredicate {
    Call {
        path: JsonPath,
        path_name: String,
        binding: BoundTextQuery,
        applicable: bool,
    },
    Derived {
        handle: u32,
        binding: crate::extensions::BoundValueExtractor,
        predicate: DerivedPredicate,
        key: Option<TypedKey>,
    },
    And(Vec<Self>),
    Or(Vec<Self>),
    Not(Box<Self>),
}


struct ExtensionCandidates {
    bits: Vec<u64>,
    text_only: bool,
}


impl CompiledPredicate {
    fn identity(&self) -> Value {
        match self {
            Self::Call {
                path_name,
                binding,
                applicable,
                ..
            } => serde_json::json!(["call", path_name, binding.identity, applicable]),
            Self::Derived {
                binding, predicate, ..
            } => serde_json::json!(["derived", binding.path, predicate]),
            Self::And(parts) => {
                serde_json::json!(["and", parts.iter().map(Self::identity).collect::<Vec<_>>()])
            }
            Self::Or(parts) => {
                serde_json::json!(["or", parts.iter().map(Self::identity).collect::<Vec<_>>()])
            }
            Self::Not(part) => serde_json::json!(["not", part.identity()]),
        }
    }

    /// A skipped source must be false without changing predicate error order.
    /// Only the first AND child may narrow; OR needs every branch; NOT never
    /// complements an approximate posting.
    fn candidate_bits(
        &self,
        repository: &Repository,
        collection: &Collection,
        selection: &str,
        source_count: usize,
        extensions: &crate::extensions::Extensions,
    ) -> Option<ExtensionCandidates> {
        match self {
            Self::Call {
                path_name,
                binding,
                applicable: true,
                ..
            } if binding.operator.can_skip_non_candidates() => {
                let CandidateConstraint::BigramLiteral {
                    literal,
                    representation,
                } = &binding.candidate
                else {
                    return None;
                };
                let compatible = representation == &binding.representation.descriptor().identity
                    || representation == "objekte:visible-normalized"
                        && binding.preparation == TextPreparation::VisibleNormalized;
                if !compatible {
                    return None;
                }
                let encoding = collection
                    .bigram_encoding
                    .unwrap_or(repository.options.bigram_encoding);
                let key = RepresentationIndexKey::new(
                    collection,
                    path_name,
                    &binding.representation.descriptor().identity,
                    selection,
                    extensions,
                    encoding,
                );
                repository
                    .representation_index_candidates(&key, literal)
                    .map(|bits| ExtensionCandidates {
                        bits,
                        text_only: true,
                    })
            }
            Self::And(parts) => parts.first()?.candidate_bits(
                repository,
                collection,
                selection,
                source_count,
                extensions,
            ),
            Self::Or(parts) if !parts.is_empty() => {
                let mut union = None::<ExtensionCandidates>;
                for part in parts {
                    let next = part.candidate_bits(
                        repository,
                        collection,
                        selection,
                        source_count,
                        extensions,
                    )?;
                    if let Some(combined) = &mut union {
                        for (word, extra) in combined.bits.iter_mut().zip(next.bits) {
                            *word |= extra;
                        }
                        combined.text_only &= next.text_only;
                    } else {
                        union = Some(next);
                    }
                }


                union
            }
            Self::Derived {
                binding,
                predicate,
                key,
                ..
            } => {
                let index_key =
                    SelectedDerivedKey::new(collection, &binding.path, selection, extensions);
                repository
                    .selected_derived_candidates(&index_key, predicate, key.as_ref(), source_count)
                    .map(|bits| ExtensionCandidates {
                        bits,
                        text_only: false,
                    })
            }
            Self::Not(_) | Self::Call { .. } | Self::Or(_) => None,
        }
    }

    fn evaluate(&self, record: &dyn DecodedRecord, scratch: &mut EvalScratch) -> Result<bool> {
        match self {
            Self::Call {
                path,
                binding,
                applicable,
                ..
            } => {
                if !applicable {
                    return Ok(false);
                }


                let mut matched = false;
                record.visit_path(path, &mut |value| {
                    if matched {
                        return Ok(());
                    }
                    let ScalarValue::Text(source) = value else {
                        return Ok(());
                    };
                    scratch.rendered.clear();
                    let Some(prepared) = binding
                        .representation
                        .render(source, &mut scratch.rendered)?
                    else {
                        return Ok(());
                    };
                    scratch.scanner.clear();
                    matched = match binding.preparation {
                        TextPreparation::Stored => binding
                            .operator
                            .matches(ScalarValue::Text(prepared), &mut scratch.scanner)?,
                        TextPreparation::VisibleNormalized => binding
                            .operator
                            .matches_prepared(ScalarValue::Text(prepared), &mut scratch.scanner)?,
                    };
                    Ok(())
                })?;
                Ok(matched)
            }
            Self::Derived {
                handle,
                binding,
                predicate,
                key,
            } => Ok(predicate.matches(
                derived_values(*handle, binding, record, scratch)?,
                key.as_ref(),
            )),
            Self::And(parts) => {
                for part in parts {
                    if !part.evaluate(record, scratch)? {
                        return Ok(false);
                    }
                }
                Ok(true)
            }
            Self::Or(parts) => {
                for part in parts {
                    if part.evaluate(record, scratch)? {
                        return Ok(true);
                    }
                }
                Ok(false)
            }

            Self::Not(part) => Ok(!part.evaluate(record, scratch)?),
        }
    }
}


#[derive(Default)]
struct EvalScratch {
    rendered: String,
    scanner: String,
    extracted: Vec<TypedValue>,
    derived: BTreeMap<u32, Vec<TypedKey>>,
}


fn derived_values<'a>(
    handle: u32,
    binding: &crate::extensions::BoundValueExtractor,
    record: &dyn DecodedRecord,
    scratch: &'a mut EvalScratch,
) -> Result<&'a [TypedKey]> {
    if !scratch.derived.contains_key(&handle) {
        scratch.extracted.clear();
        let state = binding.extractor.extract(record, &mut scratch.extracted)?;
        if state == Extraction::Missing && !scratch.extracted.is_empty() {
            return Err(Error::Request(format!(
                "extractor {} emitted values for a missing field",
                binding.identity
            )));
        }
        let keys = scratch
            .extracted
            .drain(..)
            .map(TypedKey::from_extracted)
            .collect::<Result<Vec<_>>>()?;
        scratch.derived.insert(handle, keys);
    }
    Ok(&scratch.derived[&handle])
}


enum CompiledSort {
    Id,
    Derived {
        handle: u32,
        binding: crate::extensions::BoundValueExtractor,
    },
}


impl CompiledSort {
    fn value(
        &self,
        source: &crate::extensions::SourceDescriptor,
        record: &dyn DecodedRecord,
        scratch: &mut EvalScratch,
    ) -> Result<SortValue> {
        match self {
            Self::Id => Ok(SortValue::String(source.record_id.as_bytes().to_vec())),
            Self::Derived { handle, binding } => {
                let values = derived_values(*handle, binding, record, scratch)?;
                if values.len() > 1 {
                    return Err(Error::Request(format!(
                        "derived sort {} requires one value per record",
                        binding.identity
                    )));
                }
                Ok(match values.first() {
                    None | Some(TypedKey::Missing) => SortValue::Missing,
                    Some(TypedKey::Null) => SortValue::Null,
                    Some(TypedKey::Bool(value)) => SortValue::Bool(*value),
                    Some(TypedKey::Number(value)) => SortValue::Number(value.clone()),
                    Some(TypedKey::String(value)) => SortValue::String(value.as_bytes().to_vec()),
                })
            }
        }
    }
}


#[derive(Clone)]
struct Ranked {
    slot: u32,
    source: crate::extensions::SourceDescriptor,
    keys: Vec<SortValue>,
    directions: Arc<Vec<Direction>>,
}


impl Ord for Ranked {
    fn cmp(&self, other: &Self) -> Ordering {
        for ((left, right), direction) in self
            .keys
            .iter()
            .zip(&other.keys)
            .zip(self.directions.iter())
        {
            let order = left.compare(right, *direction);
            if order != Ordering::Equal {
                return order;
            }
        }
        self.source
            .collection
            .cmp(&other.source.collection)
            .then_with(|| self.source.record_id.cmp(&other.source.record_id))
    }
}


impl PartialOrd for Ranked {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}


impl PartialEq for Ranked {
    fn eq(&self, other: &Self) -> bool {
        self.cmp(other) == Ordering::Equal
    }
}


impl Eq for Ranked {}


fn compile_predicate(
    extensions: &crate::extensions::Extensions,
    collection: &Collection,
    schema: Option<&Value>,
    formats: &BTreeMap<String, TextFormat>,
    predicate: &LogicalPredicate,
) -> Result<CompiledPredicate> {
    Ok(match predicate {
        LogicalPredicate::Call {
            operator,
            representation,
            path,
            parameters,
        } => {
            let operator = extensions.text_handle_identity(*operator, true)?;
            let representation = extensions.text_handle_identity(*representation, false)?;
            let binding = extensions.bind_text(representation, operator, parameters)?;
            let parsed = JsonFilter::parse_path(path)?;
            let binding_path = binding_path(&parsed);
            let format = formats
                .get(&binding_path)
                .copied()
                .unwrap_or(TextFormat::Plain);
            let format = match format {
                TextFormat::Plain => "plain",
                TextFormat::Markdown => "markdown",
                TextFormat::Html => "html",
                TextFormat::Xml => "xml",
            };
            let applicable = binding.representation.binds(&BindingContext {
                collection: &collection.name,
                path: &binding_path,
                format,
                schema,
            });
            CompiledPredicate::Call {
                path: parsed,
                path_name: path.clone(),
                binding,
                applicable,
            }
        }
        LogicalPredicate::Derived {
            extractor,
            predicate,
        } => {
            let name = extensions.extractor_handle_identity(*extractor)?;
            CompiledPredicate::Derived {
                handle: *extractor,
                binding: extensions.bind_value(name, &collection.name, schema)?,
                key: predicate.key()?,
                predicate: predicate.clone(),
            }
        }
        LogicalPredicate::And(parts) => CompiledPredicate::And(
            parts
                .iter()
                .map(|part| compile_predicate(extensions, collection, schema, formats, part))
                .collect::<Result<Vec<_>>>()?,
        ),
        LogicalPredicate::Or(parts) => CompiledPredicate::Or(
            parts
                .iter()
                .map(|part| compile_predicate(extensions, collection, schema, formats, part))
                .collect::<Result<Vec<_>>>()?,
        ),
        LogicalPredicate::Not(part) => CompiledPredicate::Not(Box::new(compile_predicate(
            extensions, collection, schema, formats, part,
        )?)),
    })
}

fn binding_path(path: &JsonPath) -> String {
    let mut result = String::new();
    for step in path {
        match step {
            JsonPathStep::Key(key) => {
                if !result.is_empty() {
                    result.push('.');
                }
                result.push_str(key);
            }
            JsonPathStep::Index(_) | JsonPathStep::Each => result.push_str("[]"),
        }
    }
    result
}

fn compile_sorts(
    extensions: &crate::extensions::Extensions,
    collection: &Collection,
    schema: Option<&Value>,
    sorts: &[Sort],
) -> Result<Vec<CompiledSort>> {
    sorts
        .iter()
        .map(|sort| {
            if sort.field == "@id" {
                return Ok(CompiledSort::Id);
            }
            if let Some(name) = sort.field.strip_prefix("@derived:") {
                let handle = extensions
                    .bindings()
                    .extractor(name)
                    .ok_or_else(|| Error::Request(format!("unknown value extractor {name}")))?;
                let binding = extensions.bind_value(name, &collection.name, schema)?;
                return Ok(CompiledSort::Derived { handle, binding });
            }
            Err(Error::Request(format!(
                "unsupported extension sort field {}",
                sort.field
            )))
        })
        .collect()
}

fn project_columns(
    record: &dyn DecodedRecord,
    source: &crate::extensions::SourceDescriptor,
    columns: &[(String, Option<JsonPath>)],
) -> Result<BTreeMap<String, Vec<TypedValue>>> {
    let mut projected = BTreeMap::new();
    for (name, path) in columns {
        let values = if let Some(path) = path {
            let mut values = Vec::new();
            record.visit_path(path, &mut |value| {
                values.push(match value {
                    ScalarValue::Null => TypedValue::Null,
                    ScalarValue::Bool(value) => TypedValue::Bool(value),
                    ScalarValue::Number(value) => TypedValue::Number(value.into()),
                    ScalarValue::Text(value) => TypedValue::Text(value.into()),
                });
                Ok(())
            })?;
            values
        } else if name == "@id" {
            vec![TypedValue::Text(source.record_id.clone())]
        } else {
            vec![TypedValue::Text(source.collection.clone())]
        };
        projected.insert(name.clone(), values);
    }
    Ok(projected)
}

fn validate_logical_shape(plan: &LogicalPlan) -> Result<()> {
    if plan.collections.len() > 1024 || plan.sort.len() > 64 || plan.columns.len() > 1024 {
        return Err(Error::Request(
            "logical query exceeds scope or projection limit".into(),
        ));
    }
    let mut nodes = 0;
    let mut stack = vec![(&plan.predicate, 0usize)];
    while let Some((predicate, depth)) = stack.pop() {
        nodes += 1;
        if nodes > 256 || depth > 32 {
            return Err(Error::Request(
                "logical query expression limit exceeded".into(),
            ));
        }
        match predicate {
            LogicalPredicate::And(parts) | LogicalPredicate::Or(parts) => {
                stack.extend(parts.iter().map(|part| (part, depth + 1)));
            }
            LogicalPredicate::Not(part) => stack.push((part, depth + 1)),
            LogicalPredicate::Call { .. } | LogicalPredicate::Derived { .. } => {}
        }
    }
    Ok(())
}


impl Repository {
    /// Compile an application request through a registered frontend, then run
    /// its logical plan over engine-selected, revision-checked source records.
    /// Custom-format plans use exact scan fallback. Complete ordered results
    /// may be cached under their selected source revisions and bound semantics.
    pub fn query_frontend(
        &mut self,
        frontend: &str,
        request: &Value,
    ) -> Result<ExtensionQueryResult> {
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no query frontends are registered".into()))?;
        let frontend = extensions.frontend(frontend)?;
        let plan = frontend.compile(request, &extensions.bindings())?;
        let descriptor = frontend.descriptor().clone();
        self.query_logical_inner(&descriptor, request, &plan)
    }

    fn query_logical_inner(
        &mut self,
        frontend: &crate::extensions::Descriptor,
        request: &Value,
        plan: &LogicalPlan,
    ) -> Result<ExtensionQueryResult> {
        let started = Instant::now();
        let metrics_before = self.metrics.clone();
        if plan.collections.is_empty()
            || plan.limit == 0
            || plan.limit > i64::MAX as usize
            || plan.offset > i64::MAX as usize
            || plan.collections.iter().collect::<BTreeSet<_>>().len() != plan.collections.len()
            || plan.columns.iter().collect::<BTreeSet<_>>().len() != plan.columns.len()
        {
            return Err(Error::Request(
                "invalid logical query scope, columns or page".into(),
            ));
        }
        validate_logical_shape(plan)?;
        let columns = plan
            .columns
            .iter()
            .map(|name| {
                let path = if matches!(name.as_str(), "@id" | "@collection") {
                    None
                } else {
                    Some(JsonFilter::parse_path(name)?)
                };
                Ok((name.clone(), path))
            })
            .collect::<Result<Vec<_>>>()?;
        let extensions = self.extensions.as_ref().expect("frontend was registered");
        let mut compiled = Vec::new();
        let mut identities = Vec::new();
        for name in &plan.collections {
            let collection = self.model.collection(name)?.clone();
            let schema = extensions.schema(name).map(Arc::as_ref);
            let formats = TextRules::new(&collection.fields)
                .declared_text_paths()
                .into_iter()
                .collect::<BTreeMap<_, _>>();
            let predicate =
                compile_predicate(extensions, &collection, schema, &formats, &plan.predicate)?;
            let sorts = compile_sorts(extensions, &collection, schema, &plan.sort)?;
            identities.push((
                name.clone(),
                collection.mapping_hash.clone(),
                collection.schema_hash.clone(),
                predicate.identity(),
            ));
            compiled.push((collection, predicate, sorts));
        }
        let identity = crate::config::fingerprint(
            "logical-query/v1",
            &(
                frontend,
                extensions.fingerprint(),
                request,
                &identities,
                &plan.sort,
                &plan.columns,
                plan.offset,
                plan.limit,
                plan.count,
            ),
        );
        let directions = Arc::new(
            plan.sort
                .iter()
                .map(|sort| sort.direction)
                .collect::<Vec<_>>(),
        );
        let mut selections = Vec::with_capacity(compiled.len());
        for (collection, _, _) in &compiled {
            let selected = if self.options.freshness == Freshness::MetadataOnOpen {
                let snapshot =
                    if let Some(snapshot) = self.custom_source_snapshot.get(&collection.name) {
                        snapshot.clone()
                    } else {
                        let sources = self.source_descriptors(&collection.name)?;
                        self.metrics.enumerations += 1;
                        let snapshot = CustomSourceSnapshot::new(sources);
                        self.custom_source_snapshot
                            .insert(collection.name.clone(), snapshot.clone());
                        snapshot
                    };
                (Arc::clone(&snapshot.sources), snapshot.fingerprint)
            } else {
                let sources = self.source_descriptors(&collection.name)?;
                self.metrics.enumerations += 1;
                let fingerprint = crate::config::fingerprint("selected-sources/v1", &sources);
                (Arc::new(sources), fingerprint)
            };
            selections.push(selected);
        }
        let all_sources = selections
            .iter()
            .flat_map(|(sources, _)| sources.iter())
            .collect::<Vec<_>>();
        if all_sources.len() > u32::MAX as usize {
            return Err(Error::Request(
                "logical query source table exceeds 2^32 slots".into(),
            ));
        }
        let source_fingerprints = selections
            .iter()
            .map(|(_, fingerprint)| fingerprint)
            .collect::<Vec<_>>();
        let ordered_key = OrderedResultKey {
            predicate: PredicateCacheKey {
                collection: format!("@frontend:{}", plan.collections.join("\0")),
                expression: serde_json::to_vec(&(
                    frontend,
                    extensions.fingerprint(),
                    &identities,
                    source_fingerprints,
                ))
                .expect("logical predicate identity serializes"),
            },
            sort: serde_json::to_vec(&plan.sort).expect("logical sort identity serializes"),
        };
        let can_cache = self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest
            && self.query_cache.enabled()
            && frontend.cacheable()
            && extensions
                .descriptors()
                .into_iter()
                .all(|item| item.cacheable());
        let cached = can_cache
            .then(|| {
                self.query_cache
                    .ordered(&ordered_key)
                    .and_then(|(slots, coverage)| {
                        (coverage == OrderedCoverage::Complete).then(|| slots.to_vec())
                    })
            })
            .flatten();
        let cache_hit = cached.is_some();
        let indexed_order = if cache_hit
            || compiled.len() != 1
            || plan.sort.len() != 1
            || self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            None
        } else if let CompiledSort::Derived { binding, .. } = &compiled[0].2[0] {
            let key = SelectedDerivedKey::new(
                &compiled[0].0,
                &binding.path,
                &selections[0].1,
                extensions,
            );
            self.selected_derived_indexes
                .get_mut(&key)
                .and_then(|posting| posting.order(&key.path, plan.sort[0].direction))
        } else {
            None
        };
        let candidates = if cache_hit {
            Vec::new()
        } else {
            compiled
                .iter()
                .zip(&selections)
                .map(|((collection, predicate, _), (selected, selection))| {
                    predicate.candidate_bits(
                        self,
                        collection,
                        selection,
                        selected.len(),
                        extensions,
                    )
                })
                .collect::<Vec<_>>()
        };
        let used_index = candidates.iter().any(Option::is_some);
        let mut best = BinaryHeap::new();
        let mut matches = 0usize;
        let cap = self.options.total_limit;
        let mut scratch = EvalScratch::default();
        let mut pending_insert = None;
        let mut indexed_membership = indexed_order
            .as_ref()
            .map(|_| vec![0_u64; all_sources.len().div_ceil(64)]);
        let ordered_slots = if let Some(slots) = cached {
            self.metrics.ordered_result_hits += 1;
            matches = slots.len();
            slots
        } else {
            let mut offset = 0usize;
            for (((_, predicate, sorts), (selected, _)), candidate) in
                compiled.iter().zip(&selections).zip(&candidates)
            {
                for (index, source) in selected.iter().enumerate() {
                    if candidate.as_ref().is_some_and(|candidate| {
                        candidate
                            .bits
                            .get(index / 64)
                            .is_none_or(|word| word & (1 << (index % 64)) == 0)
                    }) {
                        continue;
                    }
                    if candidate
                        .as_ref()
                        .is_some_and(|candidate| candidate.text_only)
                    {
                        self.metrics.fts_candidates += 1;
                    }
                    let bytes = self.read_selected_source_bytes(source)?;
                    self.metrics.source_reads += 1;
                    self.metrics.source_bytes += bytes.len() as u64;
                    let decoder = extensions.decoder(source)?;
                    let mut outcome = None;
                    self.metrics.parses += 1;
                    decoder.decode(&bytes, &mut |record| {
                        if outcome.is_some() {
                            return Err(Error::Request(
                                "format decoder emitted multiple records".into(),
                            ));
                        }
                        scratch.derived.clear();
                        let matched = predicate.evaluate(record, &mut scratch)?;
                        let keys = if matched {
                            if indexed_order.is_some() {
                                Some(Vec::new())
                            } else {
                                let mut keys = Vec::new();
                                for sort in sorts {
                                    keys.push(sort.value(source, record, &mut scratch)?);
                                }
                                Some(keys)
                            }
                        } else {
                            None
                        };
                        outcome = Some(keys);
                        Ok(())
                    })?;
                    let Some(matched) = outcome else {
                        return Err(Error::Request("format decoder emitted no record".into()));
                    };
                    let Some(keys) = matched else { continue };
                    matches += 1;
                    if let Some(membership) = &mut indexed_membership {
                        let slot = offset + index;
                        membership[slot / 64] |= 1 << (slot % 64);
                        continue;
                    }
                    let row = Ranked {
                        slot: u32::try_from(offset + index).expect("checked source slot limit"),
                        source: source.clone(),
                        keys,
                        directions: Arc::clone(&directions),
                    };
                    if best.len() < cap {
                        best.push(row);
                    } else if best.peek().is_some_and(|worst| row < *worst) {
                        best.pop();
                        best.push(row);
                    }
                }
                offset += selected.len();
            }
            let slots = if let (Some(order), Some(membership)) =
                (indexed_order.as_ref(), indexed_membership.as_ref())
            {
                order
                    .iter()
                    .copied()
                    .filter(|slot| {
                        let slot = *slot as usize;
                        membership[slot / 64] & (1 << (slot % 64)) != 0
                    })
                    .take(cap)
                    .collect::<Vec<_>>()
            } else {
                best.into_sorted_vec()
                    .into_iter()
                    .map(|row| row.slot)
                    .collect::<Vec<_>>()
            };
            if can_cache && matches <= cap {
                pending_insert = Some((ordered_key, slots.clone()));
            }
            slots
        };
        let end = ordered_slots
            .len()
            .min(plan.offset.saturating_add(plan.limit));
        let page = ordered_slots.get(plan.offset..end).unwrap_or(&[]);
        let mut rows = Vec::with_capacity(page.len());
        for &slot in page {
            let source = all_sources
                .get(slot as usize)
                .ok_or_else(|| Error::Cache("logical source slot is unavailable".into()))?;
            let bytes = self.read_selected_source_bytes(source)?;
            self.metrics.source_reads += 1;
            self.metrics.source_bytes += bytes.len() as u64;
            let decoder = extensions.decoder(source)?;
            let mut projected = None;
            self.metrics.parses += 1;
            decoder.decode(&bytes, &mut |record| {
                if projected.is_some() {
                    return Err(Error::Request(
                        "format decoder emitted multiple records".into(),
                    ));
                }
                projected = Some(project_columns(record, source, &columns)?);
                Ok(())
            })?;
            rows.push(ExtensionQueryRow {
                source: (*source).clone(),
                values: projected
                    .ok_or_else(|| Error::Request("format decoder emitted no record".into()))?,
            });
        }
        if self.options.freshness != Freshness::MetadataOnOpen {
            for ((collection, _, _), (selected, _)) in compiled.iter().zip(&selections) {
                let latest = self.source_descriptors(&collection.name)?;
                self.metrics.enumerations += 1;
                if latest != **selected {
                    return Err(Error::Unstable(collection.path.clone()));
                }
            }
        }
        if let Some((key, slots)) = pending_insert {
            self.query_cache.insert_ordered(key, slots);
        }
        Ok(ExtensionQueryResult {
            rows,
            total: plan.count.then_some(matches),
            truncated: matches > cap,
            identity,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            metrics: self.metrics.since(&metrics_before),
            query_plan: vec![if cache_hit {
                "extension-ordered-cache".into()
            } else if indexed_order.is_some() {
                "extension-indexed-order".into()
            } else if used_index {
                "extension-index-candidates".into()
            } else {
                "extension-exact-scan".into()
            }],
        })
    }
}
