//! JSON filter execution across memory, grep, and source files.
use super::*;
use crate::sorting::split_sort_clause;
use crate::{
    JsonFilter, JsonQuery, JsonQueryBackend, JsonQueryDependencies, JsonQueryResult,
    JsonQueryStreamSummary,
};
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
    validation_nanos: u64,
    validated: bool,
    /// A retained match, with source bytes only for malformed JSON.
    projected: Option<(Document, String, Stamp, Option<Vec<u8>>)>,
    cache_skipped: bool,
    cached_terms: u64,
    term_results: smallvec::SmallVec<[u8; 4]>,
}


type CandidateCallback<'a> =
    dyn FnMut(&Collection, &str, &Stamp, Document, String, Option<Vec<u8>>) -> Result<()> + 'a;
type OwnedRecordCallback<'a> = dyn FnMut(Record) -> Result<()> + 'a;


// INFO: Shared page sink for collected and streamed queries. Backends use its
// JsonQueryResult fields through Deref; streamed records are validated before
// emission and are not retained in the result vector.
struct QueryOutput<'a> {
    result: JsonQueryResult,
    returned: usize,
    streaming: bool,
    on_record: &'a mut dyn FnMut(usize, &Record) -> Result<()>,
    on_owned: Option<&'a mut OwnedRecordCallback<'a>>,
    on_candidate: Option<&'a mut CandidateCallback<'a>>,
}


impl QueryOutput<'_> {
    fn push(&mut self, record: Record) -> Result<()> {
        if let Some(callback) = self.on_owned.as_mut() {
            callback(record)?;
        } else {
            (self.on_record)(self.returned + 1, &record)?;
            if !self.streaming {
                self.result.records.push(record);
            }
        }
        self.returned += 1;
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
        raw: Option<Vec<u8>>,
    ) -> Result<()> {
        self.on_candidate.as_mut().expect("candidate callback")(
            collection, id, stamp, document, hash, raw,
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
mod linked;
mod memory;
mod selected;
mod sorted;
mod source;


impl Repository {
    /// Query stored JSON paths in all registered collections, or an explicit scope.
    /// Results follow the requested sort, or modification time by default.
    pub fn query_json(&mut self, request: &JsonQuery) -> Result<JsonQueryResult> {
        let request = Self::normalized_json_sort(request)?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect());
        if let Some(selected) = self.selected_json_route(&request, &names)? {
            let (result, pending, _) = self.query_json_selected(&request, selected, true, None)?;
            if let Some((key, slots)) = pending {
                self.query_cache.insert_ordered(key, slots);
            }
            return Ok(result);
        }
        if !request.sort.is_empty() {
            return self.query_json_sorted(&request, None);
        }
        let (result, _) =
            self.query_json_inner(&request, false, &mut |_, _| Ok(()), None, None, None)?;
        Ok(result)
    }

    /// Emit validated page records in query order. Records are
    /// provisional until this method returns a summary; a later error means
    /// callers must discard the emitted page. Ordinals start at one.
    pub fn query_json_stream(
        &mut self,
        request: &JsonQuery,
        mut on_record: impl FnMut(usize, &Record) -> Result<()>,
    ) -> Result<JsonQueryStreamSummary> {
        let request = Self::normalized_json_sort(request)?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect());
        if let Some(selected) = self.selected_json_route(&request, &names)? {
            let (result, pending, returned) = self.query_json_selected(
                &request,
                selected,
                self.options.stream_query_cache,
                Some(&mut on_record),
            )?;
            if let Some((key, slots)) = pending {
                self.query_cache.insert_ordered(key, slots);
            }
            return Ok(JsonQueryStreamSummary {
                returned,
                total: result.total,
                truncated: result.truncated,
                elapsed_ms: result.elapsed_ms,
                dependencies: result.dependencies,
                filter_hash: result.filter_hash,
                metrics: result.metrics,
                query_plan: result.query_plan,
            });
        }
        if !request.sort.is_empty() {
            let result = self.query_json_sorted(&request, None)?;
            for (index, record) in result.records.iter().enumerate() {
                on_record(index + 1, record)?;
            }
            return Ok(JsonQueryStreamSummary {
                returned: result.records.len(),
                total: result.total,
                truncated: result.truncated,
                elapsed_ms: result.elapsed_ms,
                dependencies: result.dependencies,
                filter_hash: result.filter_hash,
                metrics: result.metrics,
                query_plan: result.query_plan,
            });
        }
        let (result, returned) =
            self.query_json_inner(&request, true, &mut on_record, None, None, None)?;
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
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect());
        if let Some(selected) = self.selected_json_route(&request, &names)? {
            let mut page = Vec::new();
            let (result, pending, returned) = self.query_json_selected(
                &request,
                selected,
                self.options.stream_query_cache,
                Some(&mut |_, record| {
                    page.push(record.clone());
                    on_page(&page)
                }),
            )?;
            if let Some((key, slots)) = pending {
                self.query_cache.insert_ordered(key, slots);
            }
            return Ok(JsonQueryStreamSummary {
                returned,
                total: result.total,
                truncated: result.truncated,
                elapsed_ms: result.elapsed_ms,
                dependencies: result.dependencies,
                filter_hash: result.filter_hash,
                metrics: result.metrics,
                query_plan: result.query_plan,
            });
        }
        if request.sort.is_empty() {
            let mut page = Vec::new();
            let (result, returned) = self.query_json_inner(
                &request,
                true,
                &mut |_, _| Ok(()),
                Some(&mut |record| {
                    page.push(record);
                    on_page(&page)
                }),
                None,
                None,
            )?;
            return Ok(JsonQueryStreamSummary {
                returned,
                total: result.total,
                truncated: result.truncated,
                elapsed_ms: result.elapsed_ms,
                dependencies: result.dependencies,
                filter_hash: result.filter_hash,
                metrics: result.metrics,
                query_plan: result.query_plan,
            });
        }
        let result = self.query_json_sorted(&request, Some(&mut on_page))?;
        Ok(JsonQueryStreamSummary {
            returned: result.records.len(),
            total: result.total,
            truncated: result.truncated,
            elapsed_ms: result.elapsed_ms,
            dependencies: result.dependencies,
            filter_hash: result.filter_hash,
            metrics: result.metrics,
            query_plan: result.query_plan,
        })
    }

    fn selected_json_route(
        &mut self,
        request: &JsonQuery,
        names: &[String],
    ) -> Result<Option<Vec<(Collection, CustomSourceSnapshot)>>> {
        if let Some(selected) = self.selected_route(names)? {
            return Ok(Some(selected));
        }
        if !request.filter.contains(':') {
            return Ok(None);
        }
        let configured = names
            .iter()
            .any(|name| self.text_definition_collections.contains(name));
        if !self.has_explicit_text_selector(&request.filter)
            && !(configured && request.filter.contains(":fts"))
        {
            return Ok(None);
        }
        let filter =
            JsonFilter::compile_with_fts(&request.filter, &request.parameters, request.fts)?;
        let selected = names.iter().any(|name| {
            self.model
                .collections
                .get(name)
                .is_some_and(|collection| filter.has_registered_text_terms(&collection.fields))
        });
        if selected {
            Ok(Some(self.selected_text_definition_route(names)?))
        } else {
            Ok(None)
        }
    }

    fn has_explicit_text_selector(&self, filter: &str) -> bool {
        let mut quote = None;
        let mut escaped = false;
        let mut chars = filter.chars().peekable();
        while let Some(ch) = chars.next() {
            if let Some(open) = quote {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == open {
                    quote = None;
                }
                continue;
            }
            if matches!(ch, '\'' | '"') {
                quote = Some(ch);
                continue;
            }
            if ch != ':' {
                continue;
            }
            let mut name = String::new();
            while chars
                .peek()
                .is_some_and(|next| next.is_ascii_alphanumeric() || *next == '_')
            {
                name.push(chars.next().expect("peeked modifier"));
            }
            if !name.is_empty()
                && !matches!(
                    name.as_str(),
                    "fts" | "length" | "each" | "lower" | "ignorecase"
                )
            {
                return true;
            }
        }
        false
    }

    fn normalized_json_sort(request: &JsonQuery) -> Result<JsonQuery> {
        let (filter, inline) = split_sort_clause(&request.filter)?;
        if !inline.is_empty() && !request.sort.is_empty() {
            return Err(Error::Request(
                "sort specified both in query text and request".into(),
            ));
        }
        let mut request = request.clone();
        request.filter = filter;
        if !inline.is_empty() {
            request.sort = inline;
        }
        // Newest first with ID tie-breaks is already the unsorted order.
        if crate::sorting::is_default_order(&request.sort) {
            request.sort.clear();
        }
        Ok(request)
    }

    fn query_json_inner<'a>(
        &mut self,
        request: &JsonQuery,
        streaming: bool,
        on_record: &'a mut dyn FnMut(usize, &Record) -> Result<()>,
        on_owned: Option<&'a mut OwnedRecordCallback<'a>>,
        on_candidate: Option<&'a mut CandidateCallback<'a>>,
        stop_after: Option<usize>,
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
            return Err(Error::Request(
                "repository total_limit must be positive".into(),
            ));
        }
        let mut bounded = request.clone();
        if request.sort.is_empty() {
            bounded.limit = request
                .limit
                .min(self.options.total_limit.saturating_sub(request.offset));
        }
        let request = &bounded;
        let filter =
            JsonFilter::compile_with_fts(&request.filter, &request.parameters, request.fts)?;
        self.metrics.filter_compile_nanos += nanos(t);
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
                    metadata_fields: filter.metadata_fields().to_vec(),
                },
                filter_hash: filter.hash().into(),
                metrics: Metrics::default(),
                query_plan: Vec::new(),
            },
            returned: 0,
            streaming,
            on_record,
            on_owned,
            on_candidate,
        };
        if !collections.is_empty() {
            match request.backend {
                JsonQueryBackend::Grep => {
                    self.query_json_grep(request, &filter, &collections, &mut result)?
                }
                JsonQueryBackend::Auto | JsonQueryBackend::Bigram => {
                    self.query_json_bigram(request, &filter, &collections, &mut result, stop_after)?
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
