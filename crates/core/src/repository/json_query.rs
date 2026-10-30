//! JSON filter execution across memory, grep, and source files.
use super::*;
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
}


// INFO: Shared page sink for collected and streamed queries. Backends use its
// JsonQueryResult fields through Deref; streamed records are validated before
// emission and are not retained in the result vector.
struct QueryOutput<'a> {
    result: JsonQueryResult,
    returned: usize,
    streaming: bool,
    on_record: &'a mut dyn FnMut(usize, &Record) -> Result<()>,
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
    /// Query schema-declared JSON paths in all registered collections, or an explicit scope.
    /// Ordering is (collection, id).
    pub fn query_json(&mut self, request: &JsonQuery) -> Result<JsonQueryResult> {
        let (result, _) = self.query_json_inner(request, false, &mut |_, _| Ok(()))?;
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
        let (result, returned) = self.query_json_inner(request, true, &mut on_record)?;
        Ok(JsonQueryStreamSummary {
            returned,
            total: result.total,
            dependencies: result.dependencies,
            filter_hash: result.filter_hash,
            metrics: result.metrics,
            query_plan: result.query_plan,
        })
    }

    fn query_json_inner(
        &mut self,
        request: &JsonQuery,
        streaming: bool,
        on_record: &mut dyn FnMut(usize, &Record) -> Result<()>,
    ) -> Result<(JsonQueryResult, usize)> {
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
        if request.collections.is_some()
            && let Some(c) = collections.iter().find(|c| !filter.follows_schema(c))
        {
            return Err(Error::Request(format!(
                "filter does not follow the current schema of collection {}",
                c.name
            )));
        }
        let searchable = collections
            .iter()
            .filter(|c| filter.follows_schema(c))
            .cloned()
            .collect::<Vec<_>>();
        let mut result = QueryOutput {
            result: JsonQueryResult {
                records: Vec::new(),
                total: request.count.then_some(0),
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
        };
        if !collections.is_empty() {
            match request.backend {
                JsonQueryBackend::Grep if !searchable.is_empty() => {
                    self.query_json_grep(request, &filter, &searchable, &mut result)?
                }
                JsonQueryBackend::Auto | JsonQueryBackend::Bigram if !searchable.is_empty() => {
                    self.query_json_bigram(request, &filter, &searchable, &mut result)?
                }
                JsonQueryBackend::Direct if !searchable.is_empty() => {
                    self.query_json_direct(request, &filter, &searchable, &mut result)?
                }
                _ => {}
            }
        }
        result.metrics = self.metrics.since(&before);
        Ok((result.result, result.returned))
    }
}
