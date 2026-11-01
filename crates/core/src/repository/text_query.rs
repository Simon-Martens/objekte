//! Full-text search over all included JSON leaf values.

use super::*;
use crate::sorting::{SortBuffer, SortPlan, split_sort_clause};
use crate::{
    JsonFilter, TextHit, TextMode, TextQuery, TextQueryResult, TextQueryStreamSummary,
    text::{TextRules, render_text, visit_json_text},
};
use regex::Regex;
use std::collections::HashSet;
use unicode_casefold::UnicodeCaseFold;

/// Compiled matcher and conservative bigram prefilter for one request.
struct TextMatcher {
    regex: Regex,
    candidate_pattern: Option<String>,
    candidate_literal: bool,
    raw: bool,
    fold_word: bool,
}

/// Canonical query fields and, for returned hits, concrete indexed paths.
struct TextMatches {
    fields: Vec<String>,
    matched_paths: Vec<String>,
}

fn indexed_path(path: &str, indices: &[(usize, usize)]) -> String {
    if path.is_empty() {
        return "$value".into();
    }
    let mut result = String::with_capacity(path.len() + indices.len() * 3);
    if indices.first().is_some_and(|(offset, _)| *offset == 0) {
        result.push_str("$value");
    }
    let mut start = 0;
    for &(offset, index) in indices {
        // Offsets locate traversal-inserted [] markers. Splitting on the text
        // "[]" would mistake the same characters inside a JSON field name.
        result.push_str(&path[start..offset]);
        result.push('[');
        result.push_str(&index.to_string());
        result.push(']');
        start = offset + 2;
    }
    result.push_str(&path[start..]);
    result
}

impl TextMatcher {
    fn compile(request: &TextQuery) -> Result<Self> {
        if request.pattern.len() > 16 * 1024 || request.pattern.is_empty() {
            return Err(Error::Request(
                "text pattern must contain 1–16384 bytes".into(),
            ));
        }
        let raw = matches!(request.mode, TextMode::WordRaw | TextMode::RegexRaw);
        let word_mode = matches!(request.mode, TextMode::Word | TextMode::WordRaw);
        let fold_word = word_mode && !request.case_sensitive;
        let mut candidate_base = None;
        let pattern = if word_mode {
            let word = if raw {
                request.pattern.clone()
            } else {
                render_text(&request.pattern, crate::TextFormat::Plain)
                    .expect("plain rendering always succeeds")
            };
            if word.trim().is_empty() {
                return Err(Error::Request(
                    "word search needs non-whitespace text".into(),
                ));
            }
            let word = if fold_word {
                word.case_fold().collect::<String>()
            } else {
                word
            };
            let literal = regex::escape(&word);
            candidate_base = Some(word);
            literal
        } else {
            request.pattern.clone()
        };
        let regex = regex::RegexBuilder::new(&pattern)
            .case_insensitive(!request.case_sensitive && !fold_word)
            .build()
            .map_err(|error| Error::Request(format!("invalid text regex: {error}")))?;
        let candidate_base = candidate_base.as_deref().unwrap_or(&pattern);
        let candidate_pattern = if word_mode {
            Some(candidate_base.to_owned())
        } else if !candidate_base.is_ascii() {
            // Unicode regex folding may select byte sequences absent from a
            // case-folded projection. Keep those searches conservative.
            None
        } else if !request.case_sensitive {
            Some(format!("(?i:{candidate_base})"))
        } else {
            Some(candidate_base.to_owned())
        };
        Ok(Self {
            regex,
            candidate_pattern,
            candidate_literal: word_mode,
            raw,
            fold_word,
        })
    }

    fn matching_fields(
        &self,
        document: &Value,
        rules: &TextRules,
        selected: Option<&BTreeSet<&str>>,
        include_indices: bool,
    ) -> TextMatches {
        let mut fields = BTreeSet::new();
        let mut matched_paths = Vec::new();
        visit_json_text(
            document,
            rules,
            include_indices,
            &mut |name, indices, value, format| {
                if selected.is_some_and(|selected| !selected.contains(name)) {
                    return;
                }
                let text = if self.raw {
                    Some(value.to_owned())
                } else {
                    render_text(value, format)
                };
                if text.is_some_and(|text| {
                    if self.fold_word {
                        self.regex.is_match(&text.case_fold().collect::<String>())
                    } else {
                        self.regex.is_match(&text)
                    }
                }) {
                    fields.insert(name.to_owned());
                    if include_indices {
                        matched_paths.push(indexed_path(name, indices));
                    }
                }
            },
        );
        TextMatches {
            fields: fields.into_iter().collect(),
            matched_paths,
        }
    }

    fn matches_malformed(&self, bytes: &[u8]) -> bool {
        let text = crate::text::malformed_search_text(bytes);
        let text = if self.raw {
            text
        } else {
            render_text(&text, crate::TextFormat::Plain).expect("plain text renders")
        };
        if self.fold_word {
            self.regex.is_match(&text.case_fold().collect::<String>())
        } else {
            self.regex.is_match(&text)
        }
    }

    /// Normal postings retain scalar values but do not normalize whitespace.
    /// Broad or whitespace-sensitive patterns therefore scan all file slots.
    fn regular_candidate_pattern(&self) -> Option<&str> {
        let pattern = self.candidate_pattern.as_deref()?;
        if pattern.chars().any(char::is_whitespace)
            || [r"\s", r"\n", r"\r", r"\t", r"\x", r"\u"]
                .iter()
                .any(|escape| pattern.contains(escape))
        {
            None
        } else {
            Some(pattern)
        }
    }
}

impl Repository {
    /// Build a collection's separate full-text index if it is not warm yet.
    pub fn warm_text_index(&mut self, collection: &str) -> Result<Metrics> {
        let before = self.metrics.clone();
        self.reload()?;
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let started = Instant::now();
        if self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists")
            .ensure_text_index()?
        {
            self.metrics.fts_open_nanos += nanos(started);
        }
        Ok(self.metrics.since(&before))
    }

    /// Warm all collections' full-text indexes.
    pub fn warm_all_text_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        self.reload()?;
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.warm_text_index(name)?)))
            .collect()
    }

    /// Refresh a collection and rebuild its separate full-text index if eligible.
    pub fn rebuild_text_index(&mut self, collection: &str) -> Result<Metrics> {
        let before = self.metrics.clone();
        self.reload()?;
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let started = Instant::now();
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        picker.rebuild_text_index()?;
        if picker.text_index_bytes().is_some() {
            self.metrics.fts_open_nanos += nanos(started);
        }
        Ok(self.metrics.since(&before))
    }

    /// Rebuild every collection's full-text index.
    pub fn rebuild_all_text_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        self.reload()?;
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.rebuild_text_index(name)?)))
            .collect()
    }

    /// Search rendered words, rendered regex, or raw-markup regex newest first.
    pub fn search_text(&mut self, request: &TextQuery) -> Result<TextQueryResult> {
        let request = Self::normalized_filter_sort(request)?;
        if !request.sort.is_empty() {
            return self.search_text_sorted(&request, &mut |_| Ok(()));
        }
        self.search_text_inner(&request, false, &mut |_, _| Ok(()), None)
            .map(|(result, _)| result)
    }

    /// Emit page hits as they are verified. Hits remain provisional until the
    /// final summary; a later source error invalidates earlier callback data.
    pub fn search_text_stream(
        &mut self,
        request: &TextQuery,
        mut on_hit: impl FnMut(usize, &TextHit) -> Result<()>,
    ) -> Result<TextQueryStreamSummary> {
        let request = Self::normalized_filter_sort(request)?;
        if !request.sort.is_empty() {
            let result = self.search_text_sorted(&request, &mut |_| Ok(()))?;
            for (index, hit) in result.hits.iter().enumerate() {
                on_hit(index + 1, hit)?;
            }
            return Ok(TextQueryStreamSummary {
                returned: result.hits.len(),
                total: result.total,
                truncated: result.truncated,
                elapsed_ms: result.elapsed_ms,
                metrics: result.metrics,
                query_plan: result.query_plan,
            });
        }
        let (result, returned) = self.search_text_inner(&request, true, &mut on_hit, None)?;
        Ok(TextQueryStreamSummary {
            returned,
            total: result.total,
            truncated: result.truncated,
            elapsed_ms: result.elapsed_ms,
            metrics: result.metrics,
            query_plan: result.query_plan,
        })
    }

    /// Publish provisional sorted page snapshots while verification proceeds.
    pub fn search_text_stream_updates(
        &mut self,
        request: &TextQuery,
        mut on_page: impl FnMut(&[TextHit]) -> Result<()>,
    ) -> Result<TextQueryStreamSummary> {
        let request = Self::normalized_filter_sort(request)?;
        if request.sort.is_empty() {
            let mut page = Vec::new();
            return self.search_text_stream(&request, |_, hit| {
                page.push(hit.clone());
                on_page(&page)
            });
        }
        let result = self.search_text_sorted(&request, &mut on_page)?;
        Ok(TextQueryStreamSummary {
            returned: result.hits.len(),
            total: result.total,
            truncated: result.truncated,
            elapsed_ms: result.elapsed_ms,
            metrics: result.metrics,
            query_plan: result.query_plan,
        })
    }

    fn normalized_filter_sort(request: &TextQuery) -> Result<TextQuery> {
        let mut request = request.clone();
        if let Some(filter) = request.filter.as_ref() {
            let (predicate, inline) = split_sort_clause(filter)?;
            if !inline.is_empty() && !request.sort.is_empty() {
                return Err(Error::Request(
                    "sort specified both in JSON filter and request".into(),
                ));
            }
            request.filter = Some(predicate);
            if !inline.is_empty() {
                request.sort = inline;
            }
        }
        Ok(request)
    }

    fn search_text_sorted(
        &mut self,
        request: &TextQuery,
        on_page: &mut dyn FnMut(&[TextHit]) -> Result<()>,
    ) -> Result<TextQueryResult> {
        let started = Instant::now();
        let before = self.metrics.clone();
        if self.options.total_limit == 0 {
            return Err(Error::Request(
                "repository total_limit must be positive".into(),
            ));
        }
        self.reload()?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect::<Vec<_>>());
        let mut plans = BTreeMap::new();
        let mut indexed_order = self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest;
        for name in &names {
            let collection = self.collection(name)?;
            plans.insert(name.clone(), SortPlan::compile(&collection, &request.sort)?);
            if indexed_order {
                self.ensure_picker(&collection)?;
                indexed_order = self.file_pickers[name].has_sort_index(&request.sort)
                    && self.file_pickers[name].verified();
            }
        }
        if indexed_order && request.offset < self.options.total_limit && request.limit > 0 {
            return self.search_text_indexed_order(request, on_page, started, &before);
        }
        let mut buffer = SortBuffer::new(
            plans,
            request.offset,
            request.limit,
            self.options.total_limit,
        );
        let mut scan = request.clone();
        scan.offset = 0;
        scan.limit = if indexed_order {
            self.options
                .total_limit
                .saturating_add(1)
                .min(i64::MAX as usize)
        } else {
            i64::MAX as usize
        };
        scan.count = !indexed_order;
        let (mut result, _) = self.search_text_inner(&scan, true, &mut |_, hit| {
            let record = &hit.record;
            let changed =
                buffer.insert(&record.collection, &record.id, &record.data, || hit.clone())?;
            if changed && (buffer.seen() <= 16 || buffer.seen().is_multiple_of(64)) {
                let page = buffer.page().cloned().collect::<Vec<_>>();
                on_page(&page)?;
            }
            Ok(())
        }, None)?;
        let page = buffer.page().cloned().collect::<Vec<_>>();
        on_page(&page)?;
        result.total = buffer.total(request.count);
        result.truncated = buffer.truncated();
        result.hits = buffer.into_page();
        result.metrics = self.metrics.since(&before);
        result.elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        result
            .query_plan
            .push("locale-aware sorted page; bounded result prefix".into());
        Ok(result)
    }

    fn search_text_indexed_order(
        &mut self,
        request: &TextQuery,
        on_page: &mut dyn FnMut(&[TextHit]) -> Result<()>,
        started: Instant,
        before: &Metrics,
    ) -> Result<TextQueryResult> {
        let mut scan = request.clone();
        scan.count = true;
        scan.limit = request.limit.min(self.options.total_limit - request.offset);
        let stop_after = self.options.total_limit.saturating_add(1);
        let mut page = Vec::new();
        let (mut result, _) = self.search_text_inner(
            &scan,
            true,
            &mut |_, hit| {
                page.push(hit.clone());
                on_page(&page)
            },
            Some(stop_after),
        )?;
        on_page(&page)?;
        result.hits = page;
        if !request.count {
            result.total = None;
        }
        result.metrics = self.metrics.since(before);
        result.elapsed_ms = started.elapsed().as_secs_f64() * 1_000.0;
        result
            .query_plan
            .push("indexed sort order; direct verified page".into());
        Ok(result)
    }

    fn search_text_inner(
        &mut self,
        request: &TextQuery,
        streaming: bool,
        on_hit: &mut dyn FnMut(usize, &TextHit) -> Result<()>,
        stop_after: Option<usize>,
    ) -> Result<(TextQueryResult, usize)> {
        let started = Instant::now();
        let before = self.metrics.clone();
        if request.limit == 0
            || request.limit > i64::MAX as usize
            || request.offset > i64::MAX as usize
        {
            return Err(Error::Request("invalid text pagination".into()));
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
        let matcher = TextMatcher::compile(request)?;
        let filter = request
            .filter
            .as_ref()
            .map(|source| {
                if source.trim().is_empty() {
                    return Err(Error::Request("text filter must not be empty".into()));
                }
                JsonFilter::compile(source, &request.parameters)
            })
            .transpose()?;
        if filter.is_none() && !request.parameters.is_empty() {
            return Err(Error::Request("text parameters require a filter".into()));
        }
        self.reload()?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect())
            .into_iter()
            .collect::<BTreeSet<_>>();
        let requested_fields = request
            .fields
            .as_ref()
            .map(|fields| fields.iter().map(String::as_str).collect::<BTreeSet<_>>());
        if requested_fields.as_ref().is_some_and(BTreeSet::is_empty) {
            return Err(Error::Request("text field selection is empty".into()));
        }
        let mut matched = 0;
        let mut hits = Vec::new();
        let mut returned = 0;
        let mut used_regular_index = false;
        let mut used_text_index = false;
        let mut used_filter_index = false;
        for name in names {
            if stop_after.is_some_and(|limit| matched >= limit) {
                break;
            }
            let c = self.collection(&name)?;
            let collection_filter = filter.as_ref().map(|filter| filter.for_fields(&c.fields));
            let rules = TextRules::new(&c.fields);
            let source = Source::open_scoped(&c.path, c.recursive)?;
            let mut optimistic = false;
            let mut candidates = if self.options.cache == CacheMode::Disabled {
                source.inventory()?.into_iter().collect::<Vec<_>>()
            } else {
                self.warm_text_index(&name)?;
                let picker = &self.file_pickers[&name];
                optimistic = !picker.verified();
                let has_text_index = picker.text_index_bytes().is_some();
                used_text_index |= has_text_index;
                used_regular_index |= !has_text_index;
                picker.text_candidates(
                    if has_text_index {
                        matcher.candidate_pattern.as_deref()
                    } else {
                        matcher.regular_candidate_pattern()
                    },
                    matcher.raw,
                    matcher.candidate_literal,
                    request.sort.is_empty(),
                )
            };
            if let Some(filter) = &collection_filter
                && self.options.cache != CacheMode::Disabled
                && candidates.len() > 4096
            {
                let picker = &self.file_pickers[&name];
                let scalar_fields = filter.top_level_scalar_fields();
                let risk = filter.needs_cardinality_check_in(|path| picker.typed_scalar_safe(path))
                    || scalar_fields
                        .iter()
                        .any(|field| picker.has_array_value(field));
                if !risk {
                    let required = filter.required_top_level_fields();
                    let values_pattern = filter.values_prefilter().map(|(_, pattern)| pattern);
                    let typed_plan = filter.typed_plan();
                    if values_pattern.is_some() || typed_plan.is_some() || !required.is_empty() {
                        let allowed = picker.candidate_records(
                            values_pattern.as_deref(),
                            typed_plan.as_ref(),
                            &required,
                        );
                        if allowed.len() < candidates.len() {
                            let allowed = allowed
                                .into_iter()
                                .map(|(id, _)| id)
                                .collect::<HashSet<_>>();
                            candidates.retain(|(id, _)| allowed.contains(id.as_str()));
                            used_filter_index = true;
                        }
                    }
                }
            }
            if request.sort.is_empty() && self.options.cache == CacheMode::Disabled {
                candidates.sort_unstable_by(|a, b| {
                    let a_time = a.1.modified.parse::<i128>().expect("validated timestamp");
                    let b_time = b.1.modified.parse::<i128>().expect("validated timestamp");
                    b_time.cmp(&a_time).then_with(|| a.0.cmp(&b.0))
                });
            } else if !request.sort.is_empty() && self.options.cache != CacheMode::Disabled {
                self.file_pickers[&name].order_candidates(&mut candidates, &request.sort);
            }
            self.metrics.fts_candidates += candidates.len() as u64;
            for batch in candidates.chunks(256) {
                let outcomes = batch
                    .par_iter()
                    .map(|(id, stamp)| -> Result<(Vec<String>, Metrics)> {
                        let mut metrics = Metrics::default();
                        let raw = match Self::read_raw(
                            &mut metrics,
                            &source,
                            id,
                            (!optimistic).then_some(stamp),
                        ) {
                            Ok(raw) => raw,
                            Err(Error::Io { source: error, .. })
                                if optimistic && error.kind() == std::io::ErrorKind::NotFound =>
                            {
                                return Ok((Vec::new(), metrics));
                            }
                            Err(error) => return Err(error),
                        };
                        let document = Self::parse_value(&mut metrics, &c, id, &raw.json, false);
                        let started = Instant::now();
                        let passes_filter = match &collection_filter {
                            Some(_) if document.state.status == RecordStatus::IncorrectJson => {
                                false
                            }
                            Some(filter) => filter.matches(&document.data)?,
                            None => true,
                        };
                        let fields = if !passes_filter {
                            Vec::new()
                        } else if document.state.status == RecordStatus::IncorrectJson {
                            if requested_fields.is_none() && matcher.matches_malformed(&raw.json) {
                                vec!["$raw".into()]
                            } else {
                                Vec::new()
                            }
                        } else {
                            matcher
                                .matching_fields(
                                    &document.data,
                                    &rules,
                                    requested_fields.as_ref(),
                                    false,
                                )
                                .fields
                        };
                        metrics.query_nanos += nanos(started);
                        metrics.grep_scans += 1;
                        metrics.grep_bytes += stamp.size;
                        Ok((fields, metrics))
                    })
                    .collect::<Result<Vec<_>>>()?;
                let mut page = Vec::new();
                for ((id, stamp), (fields, metrics)) in batch.iter().zip(outcomes) {
                    self.metrics.add_from(&metrics);
                    if !fields.is_empty() {
                        if matched >= request.offset && returned + page.len() < request.limit {
                            page.push((id.clone(), stamp.clone(), fields));
                        }
                        matched += 1;
                    }
                }
                for (id, stamp, matched_fields) in page {
                    let raw = match Self::read_raw(
                        &mut self.metrics,
                        &source,
                        &id,
                        (!optimistic).then_some(&stamp),
                    ) {
                        Ok(raw) => raw,
                        Err(Error::Io { source: error, .. })
                            if optimistic && error.kind() == std::io::ErrorKind::NotFound =>
                        {
                            continue;
                        }
                        Err(error) => return Err(error),
                    };
                    let document = Self::parse_value(&mut self.metrics, &c, &id, &raw.json, true);
                    let malformed = document.state.status == RecordStatus::IncorrectJson;
                    if let Some(filter) = &collection_filter {
                        let passes = !malformed && filter.matches(&document.data)?;
                        if !passes {
                            if optimistic {
                                continue;
                            }
                            return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                        }
                    }
                    let verified = if malformed {
                        if requested_fields.is_none() && matcher.matches_malformed(&raw.json) {
                            TextMatches {
                                fields: vec!["$raw".into()],
                                matched_paths: vec!["$raw".into()],
                            }
                        } else {
                            TextMatches {
                                fields: Vec::new(),
                                matched_paths: Vec::new(),
                            }
                        }
                    } else {
                        matcher.matching_fields(
                            &document.data,
                            &rules,
                            requested_fields.as_ref(),
                            true,
                        )
                    };
                    if verified.fields != matched_fields {
                        if optimistic {
                            continue;
                        }
                        return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                    }
                    let hit = TextHit {
                        record: Record {
                            collection: name.clone(),
                            id: id.clone(),
                            path: self.path(&c, &id),
                            size_bytes: raw.stamp.size,
                            modified_unix_nanos: raw.stamp.modified.clone(),
                            hash: raw.hash,
                            data: document.data,
                            state: document.state,
                            raw: malformed.then_some(raw.json),
                        },
                        fields: matched_fields,
                        matched_paths: verified.matched_paths,
                    };
                    on_hit(returned + 1, &hit)?;
                    returned += 1;
                    if !streaming {
                        hits.push(hit);
                    }
                }
                if stop_after.is_some_and(|limit| matched >= limit)
                    || !request.count && returned >= request.limit {
                    break;
                }
            }
            source.finish()?;
            if stop_after.is_some_and(|limit| matched >= limit)
                || !request.count && returned >= request.limit {
                break;
            }
        }
        let mut query_plan = vec![if self.options.cache == CacheMode::Disabled {
            "direct full-text source scan; source verification".into()
        } else if used_regular_index && used_text_index {
            "normal and full-text bigram candidates; source verification".into()
        } else if used_regular_index {
            "normal bigram candidates; source text verification".into()
        } else if matcher.raw {
            "raw markup full-text bigram candidates; source verification".into()
        } else {
            "rendered full-text bigram candidates; source verification".into()
        }];
        if used_filter_index {
            query_plan
                .push("JSON filter index candidates; filter before full-text verification".into());
        } else if filter.is_some() {
            query_plan.push("JSON filter before full-text verification".into());
        }
        Ok((
            TextQueryResult {
                hits,
                total: request
                    .count
                    .then_some(matched.min(self.options.total_limit)),
                truncated: matched > self.options.total_limit,
                elapsed_ms: started.elapsed().as_secs_f64() * 1_000.0,
                metrics: self.metrics.since(&before),
                query_plan,
            },
            returned,
        ))
    }
}
