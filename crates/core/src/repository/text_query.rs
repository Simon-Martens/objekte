//! Full-text search over all included JSON leaf values.

use super::*;
use crate::extensions::{
    BoundTextQuery, CandidateConstraint, NativeTextProjection, TextPreparation,
};
use crate::file_picker::SlotSelection;
use crate::repository::{
    ordered_result_cache::{OrderedCoverage, OrderedResultKey},
    predicate_cache::PredicateCacheKey,
};
use crate::sorting::split_sort_clause;
use crate::{
    JsonFilter, TextHit, TextQuery, TextQueryResult, TextQueryStreamSummary, text::TextRules,
};
use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};

mod matcher;
mod selected;
mod sorted;

use matcher::{TextMatcher, TextMatches};


struct TextScanOutcome {
    fields: Vec<String>,
    metrics: Metrics,
    reusable: Option<Box<(Document, Vec<u8>)>>,
    cached_match: Option<bool>,
}


impl Repository {
    /// Build a collection's separate full-text index if it is not warm yet.
    pub fn warm_text_index(&mut self, collection: &str) -> Result<Metrics> {
        let before = self.metrics.clone();
        if self.selected_route(&[collection.to_owned()])?.is_some() {
            self.warm_selected_indexes(collection, true, false)?;
            return Ok(self.metrics.since(&before));
        }
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let started = Instant::now();
        if self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists")
            .ensure_text_index()?
        {
            self.query_cache.invalidate(collection);
            self.metrics.fts_open_nanos += nanos(started);
        }
        Ok(self.metrics.since(&before))
    }

    /// Warm all collections' full-text indexes.
    pub fn warm_all_text_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.warm_text_index(name)?)))
            .collect()
    }

    /// Refresh a collection and rebuild its separate full-text index if eligible.
    pub fn rebuild_text_index(&mut self, collection: &str) -> Result<Metrics> {
        let before = self.metrics.clone();
        // Explicit rebuilding is also an explicit source refresh under
        // MetadataOnOpen. Selection and native slots must advance together.
        self.refresh(collection)?;
        if self.selected_route(&[collection.to_owned()])?.is_some() {
            self.rebuild_selected_text_indexes(collection)?;
            self.query_cache.invalidate(collection);
            return Ok(self.metrics.since(&before));
        }
        let c = self.collection(collection)?;
        self.ensure_picker(&c)?;
        let started = Instant::now();
        let picker = self
            .file_pickers
            .get_mut(collection)
            .expect("picker exists");
        picker.rebuild_text_index()?;
        self.query_cache.invalidate(collection);
        if picker.text_index_bytes().is_some() {
            self.metrics.fts_open_nanos += nanos(started);
        }
        Ok(self.metrics.since(&before))
    }

    /// Rebuild every collection's full-text index.
    pub fn rebuild_all_text_indexes(&mut self) -> Result<BTreeMap<String, Metrics>> {
        let names = self.model.collections.keys().cloned().collect::<Vec<_>>();
        names
            .iter()
            .map(|name| Ok((name.clone(), self.rebuild_text_index(name)?)))
            .collect()
    }

    /// Search rendered words, rendered regex, or raw-markup regex newest first.
    pub fn search_text(&mut self, request: &TextQuery) -> Result<TextQueryResult> {
        let request = Self::normalized_filter_sort(request)?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect());
        if let Some(selected) = self.selected_route(&names)? {
            let (result, pending, _) =
                self.search_text_selected(&request, selected, None, true, None)?;
            if let Some((key, slots)) = pending {
                self.query_cache.insert_ordered(key, slots);
            }
            return Ok(result);
        }
        if !request.sort.is_empty() {
            return self.search_text_sorted::<false>(&request, None, None);
        }
        self.search_text_inner::<false>(&request, false, &mut |_, _| Ok(()), None, None)
            .map(|(result, _)| result)
    }

    /// Search the same engine-managed source, ordering, and cache path with a
    /// registered representation and exact operator. The request's `pattern`
    /// is passed to the operator as a JSON string at compile time.
    pub fn search_text_with(
        &mut self,
        request: &TextQuery,
        representation: &str,
        operator: &str,
    ) -> Result<TextQueryResult> {
        let request = Self::normalized_filter_sort(request)?;
        let extension = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no text extensions are registered".into()))?
            .bind_text(
                representation,
                operator,
                &Value::String(request.pattern.clone()),
            )?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect());
        if let Some(selected) = self.selected_route(&names)? {
            let (result, pending, _) =
                self.search_text_selected(&request, selected, Some(&extension), true, None)?;
            if let Some((key, slots)) = pending {
                self.query_cache.insert_ordered(key, slots);
            }
            return Ok(result);
        }
        if !request.sort.is_empty() {
            return self.search_text_sorted::<true>(&request, None, Some(&extension));
        }
        self.search_text_inner::<true>(&request, false, &mut |_, _| Ok(()), None, Some(&extension))
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
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect());
        if let Some(selected) = self.selected_route(&names)? {
            let (result, pending, returned) = self.search_text_selected(
                &request,
                selected,
                None,
                self.options.stream_query_cache,
                Some(&mut on_hit),
            )?;
            if let Some((key, slots)) = pending {
                self.query_cache.insert_ordered(key, slots);
            }
            return Ok(TextQueryStreamSummary {
                returned,
                total: result.total,
                truncated: result.truncated,
                elapsed_ms: result.elapsed_ms,
                metrics: result.metrics,
                query_plan: result.query_plan,
            });
        }
        if !request.sort.is_empty() {
            let result = self.search_text_sorted::<false>(&request, None, None)?;
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
        let (result, returned) = self.search_text_inner::<false>(
            &request,
            true,
            &mut |index, hit| on_hit(index, &hit),
            None,
            None,
        )?;
        Ok(TextQueryStreamSummary {
            returned,
            total: result.total,
            truncated: result.truncated,
            elapsed_ms: result.elapsed_ms,
            metrics: result.metrics,
            query_plan: result.query_plan,
        })
    }

    /// Stream registered text/operator matches using the ordinary provisional
    /// hit and final-summary contract. Cache reuse follows `stream_query_cache`.
    pub fn search_text_stream_with(
        &mut self,
        request: &TextQuery,
        representation: &str,
        operator: &str,
        mut on_hit: impl FnMut(usize, &TextHit) -> Result<()>,
    ) -> Result<TextQueryStreamSummary> {
        let request = Self::normalized_filter_sort(request)?;
        let extension = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no text extensions are registered".into()))?
            .bind_text(
                representation,
                operator,
                &Value::String(request.pattern.clone()),
            )?;
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect());
        if let Some(selected) = self.selected_route(&names)? {
            let (result, pending, returned) = self.search_text_selected(
                &request,
                selected,
                Some(&extension),
                self.options.stream_query_cache,
                Some(&mut on_hit),
            )?;
            if let Some((key, slots)) = pending {
                self.query_cache.insert_ordered(key, slots);
            }
            return Ok(TextQueryStreamSummary {
                returned,
                total: result.total,
                truncated: result.truncated,
                elapsed_ms: result.elapsed_ms,
                metrics: result.metrics,
                query_plan: result.query_plan,
            });
        }
        if !request.sort.is_empty() {
            let result = self.search_text_sorted::<true>(&request, None, Some(&extension))?;
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
        let (result, returned) = self.search_text_inner::<true>(
            &request,
            true,
            &mut |index, hit| on_hit(index, &hit),
            None,
            Some(&extension),
        )?;
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
        let names = request
            .collections
            .clone()
            .unwrap_or_else(|| self.model.collections.keys().cloned().collect());
        if let Some(selected) = self.selected_route(&names)? {
            let mut page = Vec::new();
            let (result, pending, returned) = self.search_text_selected(
                &request,
                selected,
                None,
                self.options.stream_query_cache,
                Some(&mut |_, hit| {
                    page.push(hit.clone());
                    on_page(&page)
                }),
            )?;
            if let Some((key, slots)) = pending {
                self.query_cache.insert_ordered(key, slots);
            }
            return Ok(TextQueryStreamSummary {
                returned,
                total: result.total,
                truncated: result.truncated,
                elapsed_ms: result.elapsed_ms,
                metrics: result.metrics,
                query_plan: result.query_plan,
            });
        }
        if request.sort.is_empty() {
            let mut page = Vec::new();
            let (result, returned) = self.search_text_inner::<false>(
                &request,
                true,
                &mut |_, hit| {
                    page.push(hit);
                    on_page(&page)
                },
                None,
                None,
            )?;
            return Ok(TextQueryStreamSummary {
                returned,
                total: result.total,
                truncated: result.truncated,
                elapsed_ms: result.elapsed_ms,
                metrics: result.metrics,
                query_plan: result.query_plan,
            });
        }
        let result = self.search_text_sorted::<false>(&request, Some(&mut on_page), None)?;
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
        // Newest first with ID tie-breaks is already the unsorted order.
        if crate::sorting::is_default_order(&request.sort) {
            request.sort.clear();
        }
        Ok(request)
    }

    fn search_text_inner<const CUSTOM: bool>(
        &mut self,
        request: &TextQuery,
        streaming: bool,
        on_hit: &mut dyn FnMut(usize, TextHit) -> Result<()>,
        stop_after: Option<usize>,
        extension: Option<&BoundTextQuery>,
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
        let matcher = if CUSTOM {
            None
        } else {
            Some(TextMatcher::compile(request)?)
        };
        let filter = request
            .filter
            .as_ref()
            .map(|source| {
                if source.trim().is_empty() {
                    return Err(Error::Request("text filter must not be empty".into()));
                }
                // `:fts` in the filter matches like the search itself.
                JsonFilter::compile_with_fts(
                    source,
                    &request.parameters,
                    crate::FtsOptions {
                        mode: request.mode,
                        case_sensitive: request.case_sensitive,
                    },
                )
            })
            .transpose()?;
        if filter.is_none() && !request.parameters.is_empty() {
            return Err(Error::Request("text parameters require a filter".into()));
        }
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
        let mut pending_cache = Vec::new();
        let mut pending_ordered = Vec::new();
        let evictions_before = self.query_cache.evictions();
        for name in names {
            if stop_after.is_some_and(|limit| matched >= limit) {
                break;
            }
            let c = self.collection(&name)?;
            let extension_schema = if CUSTOM {
                self.extensions
                    .as_ref()
                    .and_then(|extensions| extensions.schema(&name))
                    .cloned()
            } else {
                None
            };
            let collection_filter = filter
                .as_ref()
                .map(|filter| self.bind_filter(filter, &c))
                .transpose()?;
            let relative_collection = c
                .path
                .strip_prefix(&self.model.root)
                .expect("scoped collection")
                .to_path_buf();
            let rules = TextRules::new(&c.fields);
            let extension_binding = if CUSTOM {
                Some(matcher::TextBindingPlan::new(
                    &name,
                    extension_schema.as_deref(),
                    &rules,
                    extension.expect("custom text definition is bound"),
                ))
            } else {
                None
            };
            let source = Source::open_scoped(&c.path, c.recursive)?;
            let mut optimistic = false;
            // Postings only narrow the candidate list. The source check below
            // still decides whether a file matches the complete text query.
            let mut inventory = None;
            let mut selection: Option<SlotSelection> = None;
            if self.options.cache == CacheMode::Disabled {
                inventory = Some(source.inventory()?.into_iter().collect::<Vec<_>>());
            } else {
                self.warm_text_index(&name)?;
                let picker = &self.file_pickers[&name];
                optimistic = !picker.verified();
                let has_text_index = picker.text_index_bytes().is_some();
                used_text_index |= has_text_index;
                used_regular_index |= !has_text_index;
                selection = Some(if CUSTOM {
                    let extension = extension.expect("custom text definition is bound");
                    let (candidate, raw) = match &extension.candidate {
                        CandidateConstraint::BigramLiteral {
                            literal,
                            representation,
                        } if extension.allows_candidate_pruning()
                            && representation == "objekte:stored"
                            && extension.native_index_projection
                                == Some(NativeTextProjection::Stored)
                            && extension.preparation == TextPreparation::Stored =>
                        {
                            (Some(literal.as_str()), true)
                        }
                        CandidateConstraint::BigramLiteral {
                            literal,
                            representation,
                        } if extension.allows_candidate_pruning()
                            && representation == "objekte:visible-normalized"
                            && extension.preparation == TextPreparation::VisibleNormalized
                            && matches!(
                                extension.native_index_projection,
                                Some(NativeTextProjection::VisibleNormalized(_))
                            )
                            && has_text_index =>
                        {
                            (Some(literal.as_str()), false)
                        }
                        CandidateConstraint::BigramLiteral {
                            literal,
                            representation,
                        } if extension.allows_candidate_pruning()
                            && representation == "objekte:visible-normalized"
                            && extension.preparation == TextPreparation::VisibleNormalized
                            && extension.native_index_projection
                                == Some(NativeTextProjection::VisibleNormalized(
                                    crate::TextFormat::Plain,
                                ))
                            && !has_text_index =>
                        {
                            // On schemaless plain text, each complete word
                            // remains a necessary substring of stored bytes.
                            let word = literal
                                .split(' ')
                                .max_by_key(|word| word.len())
                                .filter(|word| !word.is_empty());
                            (word, true)
                        }
                        _ => (None, true),
                    };
                    picker.text_candidate_selection(
                        candidate,
                        raw,
                        true,
                        requested_fields.as_ref(),
                        |_, _| true,
                    )
                } else {
                    let matcher = matcher.as_ref().expect("native text matcher exists");
                    picker.text_candidate_selection(
                        if has_text_index {
                            matcher.candidate_pattern.as_deref()
                        } else {
                            matcher.regular_candidate_pattern()
                        },
                        matcher.raw,
                        matcher.candidate_literal,
                        requested_fields.as_ref(),
                        |value, format| matcher.matches_value(value, format),
                    )
                });
            }
            if let Some(filter) = &collection_filter
                && self.options.cache != CacheMode::Disabled
                && selection
                    .as_ref()
                    .is_some_and(|selection| selection.len() > 4096)
            {
                let picker = &self.file_pickers[&name];
                let scalar_fields = filter.top_level_scalar_fields();
                let risk = filter.needs_cardinality_check_in(|path| picker.typed_scalar_safe(path))
                    || scalar_fields
                        .iter()
                        .any(|field| picker.has_array_value(field));
                if !risk {
                    let required = filter.required_top_level_fields();
                    let values_prefilter = filter.values_prefilter_with_path();
                    let values_prefilter = (!filter
                        .references_indexed_field(picker.typed_fields())
                        || values_prefilter
                            .as_ref()
                            .and_then(|(_, _, path)| path.as_ref())
                            .is_some_and(|path| picker.has_text_field_path(path)))
                    .then_some(values_prefilter)
                    .flatten();
                    let typed_plan = filter.typed_plan();
                    if values_prefilter.is_some() || typed_plan.is_some() || !required.is_empty() {
                        let allowed = picker.candidate_slot_selection_for_path(
                            values_prefilter
                                .as_ref()
                                .map(|(_, pattern, _)| pattern.as_str()),
                            values_prefilter
                                .as_ref()
                                .and_then(|(_, _, path)| path.as_ref()),
                            typed_plan.as_ref(),
                            &required,
                        );
                        if allowed.len() < selection.as_ref().unwrap().len() {
                            selection.as_mut().unwrap().intersect(picker, allowed);
                            used_filter_index = true;
                        }
                    }
                }
            }
            if request.sort.is_empty()
                && let Some(inventory) = inventory.as_mut()
            {
                inventory.sort_unstable_by(|a, b| {
                    let a_time = a.1.modified.parse::<i128>().expect("validated timestamp");
                    let b_time = b.1.modified.parse::<i128>().expect("validated timestamp");
                    b_time.cmp(&a_time).then_with(|| a.0.cmp(&b.0))
                });
            } else if let Some(selection) = selection.as_mut() {
                selection.order_sparse(
                    &self.file_pickers[&name],
                    request.sort.is_empty(),
                    &request.sort,
                );
            }
            let cache_key = if self.options.cache == CacheMode::Memory
                && self.options.freshness != Freshness::HashOnRequest
                && self.query_cache.enabled()
                && (!streaming || self.options.stream_query_cache)
                && (!CUSTOM
                    || extension
                        .expect("custom text definition is bound")
                        .cacheable)
            {
                let picker = &self.file_pickers[&name];
                // Linked records can change without this collection changing.
                let safe_filter = collection_filter.as_ref().is_none_or(|filter| {
                    !filter.has_metadata()
                        && !filter.has_relations()
                        && !filter.needs_cardinality_check_in(|path| picker.typed_scalar_safe(path))
                        && !filter
                            .top_level_scalar_fields()
                            .iter()
                            .any(|field| picker.has_array_value(field))
                });
                (picker.verified() && picker.slot_count() <= u32::MAX as usize && safe_filter).then(
                    || PredicateCacheKey {
                        collection: name.clone(),
                        expression: if CUSTOM {
                            let extension = extension.expect("custom text definition is bound");
                            serde_json::to_vec(&(
                                "text-extension-query/v1",
                                &extension.identity,
                                &requested_fields,
                                collection_filter.as_ref().map(JsonFilter::cache_expression),
                            ))
                            .expect("extension text query key serializes")
                        } else {
                            serde_json::to_vec(&(
                                "text-query/v1",
                                &request.pattern,
                                request.mode,
                                request.case_sensitive,
                                &requested_fields,
                                collection_filter.as_ref().map(JsonFilter::cache_expression),
                            ))
                            .expect("text query key serializes")
                        },
                    },
                )
            } else {
                None
            };
            if let Some(key) = cache_key.as_ref()
                && (request.sort.is_empty()
                    || self.file_pickers[&name].has_sort_index(&request.sort))
            {
                let picker = &self.file_pickers[&name];
                let order_key = OrderedResultKey {
                    predicate: key.clone(),
                    sort: serde_json::to_vec(&request.sort).expect("sort serializes"),
                };
                let complete_bits = self
                    .query_cache
                    .get(key)
                    .and_then(|entry| entry.complete.then(|| entry.matched.clone()));
                let mut cached_page = None;
                if let Some((slots, coverage)) = self.query_cache.ordered(&order_key) {
                    let start = request.offset.saturating_sub(matched).min(slots.len());
                    let remaining = request.limit.saturating_sub(returned);
                    let capped_count = request.count
                        && coverage == OrderedCoverage::CappedPrefix
                        && request
                            .collections
                            .as_ref()
                            .is_some_and(|names| names.len() == 1)
                        && stop_after.is_some();
                    if coverage == OrderedCoverage::Complete
                        || ((!request.count || capped_count)
                            && remaining > 0
                            && start.saturating_add(remaining) <= slots.len())
                    {
                        let end = start.saturating_add(remaining).min(slots.len());
                        cached_page = Some((slots.len(), slots[start..end].to_vec(), capped_count));
                        self.metrics.ordered_result_hits += 1;
                    }
                }
                if cached_page.is_none()
                    && let Some(bits) = complete_bits
                {
                    let mut cached_selection = SlotSelection::from_bits(picker, Some(bits));
                    cached_selection.order_sparse(picker, request.sort.is_empty(), &request.sort);
                    let slots = cached_selection
                        .ordered(picker, request.sort.is_empty(), &request.sort)
                        .map(|slot| u32::try_from(slot).expect("picker slots fit u32"))
                        .collect::<Vec<_>>();
                    let start = request.offset.saturating_sub(matched).min(slots.len());
                    let end = start
                        .saturating_add(request.limit.saturating_sub(returned))
                        .min(slots.len());
                    let page = slots[start..end].to_vec();
                    let count = slots.len();
                    self.query_cache.insert_ordered(order_key, slots);
                    cached_page = Some((count, page, false));
                }
                if let Some((count, page_slots, capped_count)) = cached_page {
                    self.metrics.predicate_cache_hits += 1;
                    for slot in page_slots {
                        let (id, stamp) = picker.view_record(slot as usize);
                        let raw = Self::read_raw(&mut self.metrics, &source, id, Some(stamp))?;
                        let document =
                            Self::parse_value(&mut self.metrics, &c, id, &raw.json, true);
                        if let Some(filter) = &collection_filter
                            && !filter.matches_source(
                                &document.data,
                                id,
                                "",
                                &raw.stamp,
                                raw.created,
                                document.state.status,
                            )?
                        {
                            return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                        }
                        let malformed = document.state.status == RecordStatus::IncorrectJson;
                        let verified = if malformed {
                            if !CUSTOM
                                && requested_fields.is_none()
                                && matcher
                                    .as_ref()
                                    .expect("native text matcher exists")
                                    .matches_malformed(&raw.json)
                            {
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
                        } else if CUSTOM {
                            TextMatcher::matching_fields_extension(
                                &document.data,
                                &rules,
                                requested_fields.as_ref(),
                                true,
                                extension.expect("custom text definition is bound"),
                                extension_binding
                                    .as_ref()
                                    .expect("custom text binding exists"),
                            )?
                        } else {
                            matcher
                                .as_ref()
                                .expect("native text matcher exists")
                                .matching_fields(
                                    &document.data,
                                    &rules,
                                    requested_fields.as_ref(),
                                    true,
                                )
                        };
                        if verified.fields.is_empty() {
                            return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                        }
                        let hit = TextHit {
                            record: Record {
                                collection: name.clone(),
                                id: id.to_owned(),
                                path: self.path(&c, id),
                                size_bytes: raw.stamp.size,
                                modified_unix_nanos: raw.stamp.modified,
                                hash: raw.hash,
                                data: document.data,
                                state: document.state,
                                raw: malformed.then_some(raw.json),
                            },
                            fields: verified.fields,
                            matched_paths: verified.matched_paths,
                        };
                        if streaming {
                            on_hit(returned + 1, hit)?;
                        } else {
                            hits.push(hit);
                        }
                        returned += 1;
                    }
                    matched += if capped_count {
                        self.options.total_limit.saturating_add(1)
                    } else {
                        count
                    };
                    source.finish()?;
                    if stop_after.is_some_and(|limit| matched >= limit)
                        || !request.count && returned >= request.limit
                    {
                        break;
                    }
                    continue;
                }
            }
            self.metrics.fts_candidates += inventory.as_ref().map_or_else(
                || selection.as_ref().expect("memory candidates").len(),
                Vec::len,
            ) as u64;
            let cache_snapshot = cache_key
                .as_ref()
                .and_then(|key| self.query_cache.get(key).cloned());
            let mut observed = cache_key.as_ref().map(|_| {
                let words = self.file_pickers[&name].slot_count().div_ceil(64);
                (vec![0_u64; words], vec![0_u64; words])
            });
            let mut ordered_matches = Vec::new();
            let max_ordered_slots = self.options.query_cache_bytes / 8;
            let mut ordered_truncated = false;
            let mut complete = false;
            let mut candidates: Box<dyn Iterator<Item = (Option<usize>, &str, &Stamp)> + '_> =
                if let Some(inventory) = inventory.as_ref() {
                    Box::new(
                        inventory
                            .iter()
                            .map(|(id, stamp)| (None, id.as_str(), stamp)),
                    )
                } else {
                    let picker = &self.file_pickers[&name];
                    Box::new(
                        selection
                            .as_ref()
                            .expect("memory candidates")
                            .ordered(picker, request.sort.is_empty(), &request.sort)
                            .map(move |slot| {
                                let (id, stamp) = picker.view_record(slot);
                                (Some(slot), id, stamp)
                            }),
                    )
                };
            // Verify a batch in parallel, then inspect its outcomes in the
            // candidate order so sorted pages and callbacks remain ordered.
            loop {
                let batch = candidates.by_ref().take(256).collect::<Vec<_>>();
                if batch.is_empty() {
                    complete = true;
                    break;
                }
                let retain_page = request.sort.is_empty()
                    && returned < request.limit
                    && matched.saturating_add(batch.len()) > request.offset;
                let retain_limit = batch.len().min(64);
                let retained_bytes = AtomicUsize::new(0);
                let outcomes = batch
                    .par_iter()
                    .enumerate()
                    // Reuse each buffer for several files while keeping enough
                    // parallel tasks for broad candidate batches.
                    .with_min_len(4)
                    .map_init(
                        Vec::new,
                        |bytes, (position, (slot, id, stamp))| -> Result<TextScanOutcome> {
                            let mut metrics = Metrics::default();
                            if let Some(matches) = slot.and_then(|slot| {
                                cache_snapshot.as_ref().and_then(|entry| entry.known(slot))
                            }) {
                                return Ok(TextScanOutcome {
                                    fields: Vec::new(),
                                    metrics,
                                    reusable: None,
                                    cached_match: Some(matches),
                                });
                            }
                            let read = match Self::read_verified_into(
                                &mut metrics,
                                &source,
                                id,
                                (!optimistic).then_some(stamp),
                                bytes,
                            ) {
                                Ok(raw) => raw,
                                Err(Error::Io { source: error, .. })
                                    if optimistic
                                        && error.kind() == std::io::ErrorKind::NotFound =>
                                {
                                    return Ok(TextScanOutcome {
                                        fields: Vec::new(),
                                        metrics,
                                        reusable: None,
                                        cached_match: None,
                                    });
                                }
                                Err(error) => return Err(error),
                            };
                            let mut document =
                                Self::parse_value(&mut metrics, &c, id, bytes, false);
                            if collection_filter
                                .as_ref()
                                .is_some_and(JsonFilter::uses_status)
                                && document.state.status != RecordStatus::IncorrectJson
                            {
                                let started = Instant::now();
                                document.validate(&c);
                                if c.schema.is_some() {
                                    metrics.validations += 1;
                                    metrics.validation_nanos += nanos(started);
                                }
                            }
                            let started = Instant::now();
                            let passes_filter = match &collection_filter {
                                Some(filter) => {
                                    let path = filter.uses_source_path().then(|| {
                                        relative_collection
                                            .join(format!("{id}.json"))
                                            .to_string_lossy()
                                            .into_owned()
                                    });
                                    filter.matches_source(
                                        &document.data,
                                        id,
                                        path.as_deref().unwrap_or(""),
                                        &read.stamp,
                                        read.created,
                                        document.state.status,
                                    )?
                                }
                                None => true,
                            };
                            let fields = if !passes_filter {
                                Vec::new()
                            } else if document.state.status == RecordStatus::IncorrectJson {
                                if !CUSTOM
                                    && requested_fields.is_none()
                                    && matcher
                                        .as_ref()
                                        .expect("native text matcher exists")
                                        .matches_malformed(bytes)
                                {
                                    vec!["$raw".into()]
                                } else {
                                    Vec::new()
                                }
                            } else if CUSTOM {
                                TextMatcher::matching_fields_extension(
                                    &document.data,
                                    &rules,
                                    requested_fields.as_ref(),
                                    false,
                                    extension.expect("custom text definition is bound"),
                                    extension_binding
                                        .as_ref()
                                        .expect("custom text binding exists"),
                                )?
                                .fields
                            } else {
                                matcher
                                    .as_ref()
                                    .expect("native text matcher exists")
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
                            let reusable = if !fields.is_empty()
                                && retain_page
                                && position < retain_limit
                                && bytes.len() <= 1024 * 1024
                                && bytes.capacity() <= bytes.len().saturating_mul(4).max(4096)
                            {
                                let cost = bytes.capacity().saturating_mul(8);
                                if retained_bytes
                                    .try_update(
                                        AtomicOrdering::Relaxed,
                                        AtomicOrdering::Relaxed,
                                        |used| {
                                            used.checked_add(cost)
                                                .filter(|&next| next <= 8 * 1024 * 1024)
                                        },
                                    )
                                    .is_ok()
                                {
                                    Some(Box::new((document, std::mem::take(bytes))))
                                } else {
                                    None
                                }
                            } else {
                                None
                            };
                            if bytes.capacity() > 1024 * 1024 {
                                *bytes = Vec::new();
                            }
                            Ok(TextScanOutcome {
                                fields,
                                metrics,
                                reusable,
                                cached_match: None,
                            })
                        },
                    )
                    .collect::<Result<Vec<_>>>()?;
                let mut page = Vec::new();
                for (
                    (slot, id, stamp),
                    TextScanOutcome {
                        fields,
                        metrics,
                        reusable,
                        cached_match,
                    },
                ) in batch.iter().zip(outcomes)
                {
                    self.metrics.add_from(&metrics);
                    if cached_match.is_some() {
                        self.metrics.partial_evaluation_hits += 1;
                    }
                    let is_match = cached_match.unwrap_or(!fields.is_empty());
                    if let (Some(slot), Some((evaluated, matches))) = (*slot, observed.as_mut()) {
                        let bit = 1_u64 << (slot % 64);
                        evaluated[slot / 64] |= bit;
                        if is_match {
                            matches[slot / 64] |= bit;
                        }
                    }
                    if is_match {
                        if let Some(slot) = *slot
                            && cache_key.is_some()
                        {
                            if ordered_matches.len() < max_ordered_slots {
                                ordered_matches
                                    .push(u32::try_from(slot).expect("picker slots fit u32"));
                            } else {
                                ordered_truncated = true;
                            }
                        }
                        if matched >= request.offset && returned + page.len() < request.limit {
                            page.push((
                                (*id).to_owned(),
                                (*stamp).clone(),
                                fields,
                                reusable,
                                cached_match.is_some(),
                            ));
                        }
                        matched += 1;
                    }
                }
                // Only page hits need a second read for full matched paths,
                // hashes, validation state, and a returned Record.
                for (id, stamp, matched_fields, reusable, from_cache) in page {
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
                    let document = if let Some(reusable) = reusable
                        && reusable.1 == raw.json
                    {
                        let (mut document, _) = *reusable;
                        if c.schema.is_none()
                            && document.state.status != RecordStatus::IncorrectJson
                        {
                            document.state.status = RecordStatus::NoSchema;
                        }
                        if c.schema.is_some() && document.state.status == RecordStatus::Pending {
                            let started = Instant::now();
                            document.validate(&c);
                            self.metrics.validations += 1;
                            self.metrics.validation_nanos += nanos(started);
                        }
                        document
                    } else {
                        Self::parse_value(&mut self.metrics, &c, &id, &raw.json, true)
                    };
                    let malformed = document.state.status == RecordStatus::IncorrectJson;
                    if let Some(filter) = &collection_filter {
                        let path = filter.uses_source_path().then(|| self.path(&c, &id));
                        let passes = filter.matches_source(
                            &document.data,
                            &id,
                            path.as_deref().unwrap_or(""),
                            &raw.stamp,
                            raw.created,
                            document.state.status,
                        )?;
                        if !passes {
                            if optimistic {
                                continue;
                            }
                            return Err(Error::Unstable(source.path.join(format!("{id}.json"))));
                        }
                    }
                    let verified = if malformed {
                        if !CUSTOM
                            && requested_fields.is_none()
                            && matcher
                                .as_ref()
                                .expect("native text matcher exists")
                                .matches_malformed(&raw.json)
                        {
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
                    } else if CUSTOM {
                        TextMatcher::matching_fields_extension(
                            &document.data,
                            &rules,
                            requested_fields.as_ref(),
                            true,
                            extension.expect("custom text definition is bound"),
                            extension_binding
                                .as_ref()
                                .expect("custom text binding exists"),
                        )?
                    } else {
                        matcher
                            .as_ref()
                            .expect("native text matcher exists")
                            .matching_fields(
                                &document.data,
                                &rules,
                                requested_fields.as_ref(),
                                true,
                            )
                    };
                    if (!from_cache && verified.fields != matched_fields)
                        || verified.fields.is_empty()
                    {
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
                        fields: if from_cache {
                            verified.fields
                        } else {
                            matched_fields
                        },
                        matched_paths: verified.matched_paths,
                    };
                    if streaming {
                        on_hit(returned + 1, hit)?;
                    } else {
                        hits.push(hit);
                    }
                    returned += 1;
                }
                if stop_after.is_some_and(|limit| matched >= limit)
                    || !request.count && returned >= request.limit
                {
                    break;
                }
            }
            source.finish()?;
            if let (Some(key), Some((evaluated, matches))) = (cache_key, observed) {
                if request.sort.is_empty() || self.file_pickers[&name].has_sort_index(&request.sort)
                {
                    let coverage = if ordered_truncated {
                        OrderedCoverage::Prefix
                    } else if complete {
                        OrderedCoverage::Complete
                    } else if stop_after.is_some_and(|limit| matched >= limit) {
                        OrderedCoverage::CappedPrefix
                    } else {
                        OrderedCoverage::Prefix
                    };
                    pending_ordered.push((
                        OrderedResultKey {
                            predicate: key.clone(),
                            sort: serde_json::to_vec(&request.sort).expect("sort serializes"),
                        },
                        ordered_matches,
                        coverage,
                    ));
                }
                pending_cache.push((key, evaluated, matches, complete));
            }
            if stop_after.is_some_and(|limit| matched >= limit)
                || !request.count && returned >= request.limit
            {
                break;
            }
        }
        for (key, evaluated, matches, complete) in pending_cache {
            self.query_cache
                .record_bits(key, evaluated, matches, complete);
        }
        for (key, slots, coverage) in pending_ordered {
            self.query_cache
                .insert_ordered_with_coverage(key, slots, coverage);
        }
        self.metrics.cache_evictions += self.query_cache.evictions() - evictions_before;
        let mut query_plan = vec![if self.options.cache == CacheMode::Disabled {
            "direct full-text source scan; source verification".into()
        } else if used_regular_index && used_text_index {
            "normal and full-text bigram candidates; source verification".into()
        } else if used_regular_index {
            "normal bigram candidates; source text verification".into()
        } else if matcher.as_ref().is_some_and(|matcher| matcher.raw) {
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
