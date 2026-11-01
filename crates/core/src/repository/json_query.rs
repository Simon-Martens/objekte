//! JSON filter execution across memory, grep, and source files.
use super::*;
use crate::{
    JsonFilter, JsonQuery, JsonQueryBackend, JsonQueryDependencies, JsonQueryResult,
    JsonQueryStreamSummary,
};
use crate::sorting::{SortBuffer, SortPlan, split_sort_clause};
use grep_searcher::{Sink, SinkMatch};
use std::ops::{Deref, DerefMut};


/// Search sink that stops after one regex match.
#[derive(Default)]
struct FirstMatch(bool);


impl Sink for FirstMatch {
    type Error = std::io::Error;

    fn matched(&mut self, _: &grep_searcher::Searcher, _: &SinkMatch<'_>) -> std::io::Result<bool> {
        self.0 = true;
        Ok(false)
    }
}


/// Per-file verification result and phase timings.
#[derive(Default)]
struct ScanOutcome {
    candidate: bool,
    matches: bool,
    source_bytes: u64,
    parse_nanos: u64,
    query_nanos: u64,
    file_open_nanos: u64,
    read_nanos: u64,
    regex_nanos: u64,
    hash_nanos: u64,
    projected: Option<(Document, String, Stamp)>,
}

type CandidateCallback<'a> =
    dyn FnMut(&Collection, &str, &Stamp, Document, String) -> Result<()> + 'a;


// INFO: Shared page sink for collected and streamed queries. Backends use its
// JsonQueryResult fields through Deref; streamed records are validated before
// emission and are not retained in the result vector.
struct QueryOutput<'a> {
    result: JsonQueryResult,
    returned: usize,
    streaming: bool,
    on_record: &'a mut dyn FnMut(usize, &Record) -> Result<()>,
    on_candidate: Option<&'a mut CandidateCallback<'a>>,
}


impl QueryOutput<'_> {
    fn push(&mut self, record: Record) -> Result<()> {
        (self.on_record)(self.returned + 1, &record)?;
        self.returned += 1;
        if !self.streaming {
            self.result.records.push(record);
        }
        Ok(())
    }

    fn returned(&self) -> usize {
        self.returned
    }

    fn push_candidate(
        &mut self,
        collection: &Collection,
        id: &str,
        stamp: &Stamp,
        document: Document,
        hash: String,
    ) -> Result<()> {
        self.on_candidate.as_mut().expect("candidate callback")(
            collection, id, stamp, document, hash,
        )?;
        self.returned += 1;
        Ok(())
    }
}


impl Deref for QueryOutput<'_> {
    type Target = JsonQueryResult;

    fn deref(&self) -> &Self::Target {
        &self.result
    }
}


impl DerefMut for QueryOutput<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.result
    }
}


mod exact;
mod memory;
mod source;


impl Repository {
    /// Query stored JSON paths in all registered collections, or an explicit scope.
    /// Ordering is (collection, id).
    pub fn query_json(&mut self, request: &JsonQuery) -> Result<JsonQueryResult> {
        let request = Self::normalized_json_sort(request)?;
        if !request.sort.is_empty() {
            return self.query_json_sorted(&request, &mut |_| Ok(()));
        }
        let (result, _) = self.query_json_inner(&request, false, &mut |_, _| Ok(()), None)?;
        Ok(result)
    }

    /// Emit validated page records in (collection, ID) order. Records are
    /// provisional until this method returns a summary; a later error means
    /// callers must discard the emitted page. Ordinals start at one.
    pub fn query_json_stream(
        &mut self,
        request: &JsonQuery,
        mut on_record: impl FnMut(usize, &Record) -> Result<()>,
    ) -> Result<JsonQueryStreamSummary> {
        let request = Self::normalized_json_sort(request)?;
        if !request.sort.is_empty() {
            let result = self.query_json_sorted(&request, &mut |_| Ok(()))?;
            for (index, record) in result.records.iter().enumerate() {
                on_record(index + 1, record)?;
            }
            return Ok(JsonQueryStreamSummary {
                returned: result.records.len(), total: result.total,
                truncated: result.truncated, elapsed_ms: result.elapsed_ms,
                dependencies: result.dependencies, filter_hash: result.filter_hash,
                metrics: result.metrics, query_plan: result.query_plan,
            });
        }
        let (result, returned) = self.query_json_inner(&request, true, &mut on_record, None)?;
        Ok(JsonQueryStreamSummary {
            returned,
            total: result.total,
            truncated: result.truncated,
            elapsed_ms: result.elapsed_ms,
            dependencies: result.dependencies,
            filter_hash: result.filter_hash,
            metrics: result.metrics,
            query_plan: result.query_plan,
        })
    }

    /// Publish provisional ordered page snapshots. Rows can move until completion.
    pub fn query_json_stream_updates(
        &mut self,
        request: &JsonQuery,
        mut on_page: impl FnMut(&[Record]) -> Result<()>,
    ) -> Result<JsonQueryStreamSummary> {
        let request = Self::normalized_json_sort(request)?;
        if request.sort.is_empty() {
            let mut page = Vec::new();
            return self.query_json_stream(&request, |_, record| {
                page.push(record.clone());
                on_page(&page)
            });
        }
        let result = self.query_json_sorted(&request, &mut on_page)?;
        Ok(JsonQueryStreamSummary {
            returned: result.records.len(), total: result.total,
            truncated: result.truncated, elapsed_ms: result.elapsed_ms,
            dependencies: result.dependencies, filter_hash: result.filter_hash,
            metrics: result.metrics, query_plan: result.query_plan,
        })
    }

    fn normalized_json_sort(request: &JsonQuery) -> Result<JsonQuery> {
        let (filter, inline) = split_sort_clause(&request.filter)?;
        if !inline.is_empty() && !request.sort.is_empty() {
            return Err(Error::Request("sort specified both in query text and request".into()));
        }
        let mut request = request.clone();
        request.filter = filter;
        if !inline.is_empty() { request.sort = inline; }
        Ok(request)
    }

    fn query_json_sorted(
        &mut self,
        request: &JsonQuery,
        on_page: &mut dyn FnMut(&[Record]) -> Result<()>,
    ) -> Result<JsonQueryResult> {
        let started = Instant::now();
        let before = self.metrics.clone();
        if self.options.total_limit == 0 {
            return Err(Error::Request("repository total_limit must be positive".into()));
        }
        self.reload()?;
        let names = request.collections.clone().unwrap_or_else(|| self.model.collections.keys().cloned().collect::<Vec<_>>());
        let mut plans = BTreeMap::new();
        let filter = JsonFilter::compile(&request.filter, &request.parameters)?;
        let mut indexed_order = matches!(request.backend, JsonQueryBackend::Auto | JsonQueryBackend::Bigram)
            && self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest;
        for name in &names {
            let collection = self.collection(name)?;
            plans.insert(name.clone(), SortPlan::compile(&collection, &request.sort)?);
            if indexed_order {
                self.ensure_picker(&collection)?;
                let picker = &self.file_pickers[name];
                let scoped = filter.for_fields(&collection.fields);
                let risky = scoped.needs_cardinality_check_in(|path| picker.typed_scalar_safe(path))
                    || scoped.top_level_scalar_fields().iter().any(|field| picker.has_array_value(field));
                indexed_order = picker.has_sort_index(&request.sort) && !risky;
            }
        }
        let mut buffer = SortBuffer::new(plans, request.offset, request.limit, self.options.total_limit);
        let mut scan = request.clone();
        scan.offset = 0;
        scan.limit = if indexed_order { self.options.total_limit.saturating_add(1).min(i64::MAX as usize) }
            else { i64::MAX as usize };
        scan.count = !indexed_order;
        let mut selected_validation = Metrics::default();
        let root = self.model.root.clone();
        let (mut result, _) = if matches!(scan.backend, JsonQueryBackend::Auto | JsonQueryBackend::Bigram) {
            let mut on_candidate = |collection: &Collection, id: &str, stamp: &Stamp, mut document: Document, hash: String| {
                let keys = buffer.project(&collection.name, id, &document.data)?;
                let changed = buffer.insert_keys(&collection.name, id, keys, || {
                    if collection.schema.is_some() {
                        let started = Instant::now();
                        document.validate(collection);
                        selected_validation.validations += 1;
                        selected_validation.validation_nanos += nanos(started);
                    } else {
                        document.state.status = RecordStatus::NoSchema;
                    }
                    Record {
                        collection: collection.name.clone(), id: id.into(),
                        path: collection.path.join(format!("{id}.json")).strip_prefix(&root)
                            .expect("record is within repository").to_string_lossy().into_owned(),
                        size_bytes: stamp.size, modified_unix_nanos: stamp.modified.clone(),
                        hash, data: document.data, state: document.state, raw: None,
                    }
                });
                if changed && (buffer.seen() <= 16 || buffer.seen().is_multiple_of(64)) {
                    let page = buffer.page().cloned().collect::<Vec<_>>();
                    on_page(&page)?;
                }
                Ok(())
            };
            self.query_json_inner(&scan, true, &mut |_, _| Ok(()), Some(&mut on_candidate))?
        } else {
            self.query_json_inner(&scan, true, &mut |_, record| {
                let changed = buffer.insert(&record.collection, &record.id, &record.data, || record.clone())?;
                if changed && (buffer.seen() <= 16 || buffer.seen().is_multiple_of(64)) {
                    let page = buffer.page().cloned().collect::<Vec<_>>();
                    on_page(&page)?;
                }
                Ok(())
            }, None)?
        };
        self.metrics.add_from(&selected_validation);
        let page = buffer.page().cloned().collect::<Vec<_>>();
        on_page(&page)?;
        result.total = buffer.total(request.count);
        result.truncated = buffer.truncated();
        result.records = buffer.into_page();
        result.metrics = self.metrics.since(&before);
        result.elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        result.query_plan.push("locale-aware sorted page; bounded result prefix".into());
        Ok(result)
    }

    fn query_json_inner<'a>(
        &mut self,
        request: &JsonQuery,
        streaming: bool,
        on_record: &'a mut dyn FnMut(usize, &Record) -> Result<()>,
        on_candidate: Option<&'a mut CandidateCallback<'a>>,
    ) -> Result<(JsonQueryResult, usize)> {
        let started = Instant::now();
        let before = self.metrics.clone();
        let t = Instant::now();
        if request.limit == 0
            || request.limit > i64::MAX as usize
            || request.offset > i64::MAX as usize
        {
            return Err(Error::Request(
                "query limit must be positive and pagination must fit i64".into(),
            ));
        }
        if self.options.total_limit == 0 {
            return Err(Error::Request("repository total_limit must be positive".into()));
        }
        let mut bounded = request.clone();
        if request.sort.is_empty() {
            bounded.limit = request.limit.min(self.options.total_limit.saturating_sub(request.offset));
        }
        let request = &bounded;
        let filter = JsonFilter::compile(&request.filter, &request.parameters)?;
        self.metrics.filter_compile_nanos += nanos(t);
        self.reload()?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect())
            .into_iter()
            .collect::<BTreeSet<_>>();
        let collections = names
            .iter()
            .map(|name| self.collection(name))
            .collect::<Result<Vec<_>>>()?;
        let mut result = QueryOutput {
            result: JsonQueryResult {
                records: Vec::new(),
                total: request.count.then_some(0),
                truncated: false,
                elapsed_ms: 0.0,
                dependencies: JsonQueryDependencies {
                    collections: names.into_iter().collect(),
                    paths: filter.paths().to_vec(),
                },
                filter_hash: filter.hash().into(),
                metrics: Metrics::default(),
                query_plan: Vec::new(),
            },
            returned: 0,
            streaming,
            on_record,
            on_candidate,
        };
        if !collections.is_empty() {
            match request.backend {
                JsonQueryBackend::Grep => {
                    self.query_json_grep(request, &filter, &collections, &mut result)?
                }
                JsonQueryBackend::Auto | JsonQueryBackend::Bigram => {
                    self.query_json_bigram(request, &filter, &collections, &mut result)?
                }
                JsonQueryBackend::Direct => {
                    self.query_json_direct(request, &filter, &collections, &mut result)?
                }
            }
        }
        if let Some(total) = result.total {
            result.truncated = total > self.options.total_limit;
            result.total = Some(total.min(self.options.total_limit));
        }
        result.metrics = self.metrics.since(&before);
        result.elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        Ok((result.result, result.returned))
    }
}
