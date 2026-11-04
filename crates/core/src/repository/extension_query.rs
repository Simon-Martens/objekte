//! Exact execution of registered logical plans over selected source records.
//! The frontend compiles syntax only; source reads, Boolean evaluation,
//! bounded ordering, counts and page materialization belong to the engine.

use super::*;
use crate::{
    Direction, JsonFilter, Sort, TextFormat,
    extensions::{
        BindingContext, BoundTextQuery, DecodedRecord, Extraction, LogicalPlan, LogicalPredicate,
        ScalarValue, TextPreparation, TypedValue,
    },
    file_picker::TypedKey,
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
            let format = formats.get(path).copied().unwrap_or(TextFormat::Plain);
            let format = match format {
                TextFormat::Plain => "plain",
                TextFormat::Markdown => "markdown",
                TextFormat::Html => "html",
                TextFormat::Xml => "xml",
            };
            let applicable = binding.representation.binds(&BindingContext {
                collection: &collection.name,
                path,
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


impl Repository {
    /// Compile an application request through a registered frontend, then run
    /// its logical plan over engine-selected, revision-checked source records.
    /// Custom-format plans currently use exact scan fallback; they do not
    /// borrow the JSON-only picker or publish a reusable query-cache entry.
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
                identities,
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
        let mut best = BinaryHeap::new();
        let mut matches = 0usize;
        let cap = self.options.total_limit;
        let mut scratch = EvalScratch::default();
        for (collection, predicate, sorts) in &compiled {
            let selected = if self.options.freshness == Freshness::MetadataOnOpen {
                if let Some(snapshot) = self.custom_source_snapshot.get(&collection.name) {
                    snapshot.clone()
                } else {
                    let selected = self.source_descriptors(&collection.name)?;
                    self.metrics.enumerations += 1;
                    selected
                }
            } else {
                let selected = self.source_descriptors(&collection.name)?;
                self.metrics.enumerations += 1;
                selected
            };
            for source in &selected {
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
                        let mut keys = Vec::new();
                        for sort in sorts {
                            keys.push(sort.value(source, record, &mut scratch)?);
                        }
                        Some(keys)
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
                let row = Ranked {
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
            if self.options.freshness != Freshness::MetadataOnOpen {
                let latest = self.source_descriptors(&collection.name)?;
                self.metrics.enumerations += 1;
                if latest != selected {
                    return Err(Error::Unstable(collection.path.clone()));
                }
            }
        }
        let ordered = best.into_sorted_vec();
        let end = ordered.len().min(plan.offset.saturating_add(plan.limit));
        let page = ordered.get(plan.offset..end).unwrap_or(&[]);
        let mut rows = Vec::with_capacity(page.len());
        for ranked in page {
            let source = &ranked.source;
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
                source: source.clone(),
                values: projected
                    .ok_or_else(|| Error::Request("format decoder emitted no record".into()))?,
            });
        }
        Ok(ExtensionQueryResult {
            rows,
            total: plan.count.then_some(matches),
            truncated: matches > cap,
            identity,
            elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
            metrics: self.metrics.since(&metrics_before),
            query_plan: vec!["extension-exact-scan".into()],
        })
    }
}
