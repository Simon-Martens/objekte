//! Text queries over pinned provider inventories; native-only collections keep
//! the concrete text matcher, picker and streaming loops in the parent module.

use super::*;
use crate::{
    extensions::BindingContext,
    repository::{extension_index::RepresentationIndexKey, extension_query::source_location},
    sorting::{SortBuffer, SortRecord},
};

fn selected_matches(
    record: &Record,
    matcher: Option<&TextMatcher>,
    extension: Option<&BoundTextQuery>,
    binding: Option<&matcher::TextBindingPlan<'_>>,
    rules: &TextRules,
    fields: Option<&BTreeSet<&str>>,
    indices: bool,
) -> Result<TextMatches> {
    if record.state.status == RecordStatus::IncorrectJson {
        let matches = matcher.is_some_and(|matcher| {
            fields.is_none()
                && record
                    .raw
                    .as_ref()
                    .is_some_and(|raw| matcher.matches_malformed(raw))
        });
        return Ok(TextMatches {
            fields: if matches {
                vec!["$raw".into()]
            } else {
                Vec::new()
            },
            matched_paths: if matches && indices {
                vec!["$raw".into()]
            } else {
                Vec::new()
            },
        });
    }
    if let Some(extension) = extension {
        TextMatcher::matching_fields_extension(
            &record.data,
            rules,
            fields,
            indices,
            extension,
            binding.expect("extension binding exists"),
        )
    } else {
        Ok(matcher.expect("native matcher exists").matching_fields(
            &record.data,
            rules,
            fields,
            indices,
        ))
    }
}


impl Repository {
    fn selected_text_candidates(
        &self,
        request: &TextQuery,
        collection: &Collection,
        snapshot: &CustomSourceSnapshot,
        matcher: Option<&TextMatcher>,
        extension: Option<&BoundTextQuery>,
    ) -> Option<Vec<u64>> {
        if request.filter.is_some() || self.options.cache != CacheMode::Memory {
            return None;
        }
        let field = request
            .fields
            .as_ref()
            .filter(|fields| fields.len() == 1)?
            .first()?;
        let extensions = self.extensions.as_ref()?;
        let (representation, literal) = if let Some(extension) = extension {
            if !extension.allows_candidate_pruning() {
                return None;
            }
            let CandidateConstraint::BigramLiteral {
                literal,
                representation,
            } = &extension.candidate
            else {
                return None;
            };
            let compatible = representation == &extension.representation.descriptor().identity
                || representation == "objekte:visible-normalized"
                    && extension.preparation == TextPreparation::VisibleNormalized;
            if !compatible {
                return None;
            }
            (
                extension.representation.descriptor().identity.as_str(),
                literal.as_str(),
            )
        } else {
            let matcher = matcher?;
            if request.mode == crate::TextMode::Regex {
                return None;
            }
            let representation = if request.mode == crate::TextMode::Word {
                match crate::repository::extension_index::field_format(
                    collection,
                    &JsonFilter::parse_path(field).ok()?,
                ) {
                    "markdown" => "objekte:visible-markdown",
                    "html" => "objekte:visible-html",
                    "xml" => "objekte:visible-xml",
                    _ => "objekte:visible-plain",
                }
            } else {
                "objekte:stored"
            };
            (representation, matcher.candidate_pattern.as_deref()?)
        };
        let definition = extensions.representation_definition(representation).ok()?;
        if !definition.binds(&BindingContext {
            collection: &collection.name,
            path: field,
            format: crate::repository::extension_index::field_format(
                collection,
                &JsonFilter::parse_path(field).ok()?,
            ),
            schema: extensions
                .schema(&collection.name)
                .map(std::sync::Arc::as_ref),
        }) {
            return None;
        }
        let key = RepresentationIndexKey::new(
            collection,
            field,
            representation,
            &snapshot.fingerprint,
            extensions,
            collection
                .bigram_encoding
                .unwrap_or(self.options.bigram_encoding),
        );
        self.representation_index_candidates(&key, literal)
    }

    pub(super) fn search_text_selected(
        &mut self,
        request: &TextQuery,
        selected: Vec<(Collection, CustomSourceSnapshot)>,
        extension: Option<&BoundTextQuery>,
        cache_allowed: bool,
    ) -> Result<(TextQueryResult, Option<PendingSelectedOrder>)> {
        let started = Instant::now();
        let before = self.metrics.clone();
        if request.limit == 0
            || request.limit > i64::MAX as usize
            || request.offset > i64::MAX as usize
            || self.options.total_limit == 0
        {
            return Err(Error::Request(
                "invalid text pagination or total_limit".into(),
            ));
        }
        let matcher = extension
            .is_none()
            .then(|| TextMatcher::compile(request))
            .transpose()?;
        let filter = request
            .filter
            .as_ref()
            .map(|source| {
                if source.trim().is_empty() {
                    return Err(Error::Request("text filter must not be empty".into()));
                }
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
        let requested_fields = request
            .fields
            .as_ref()
            .map(|fields| fields.iter().map(String::as_str).collect::<BTreeSet<_>>());
        if requested_fields.as_ref().is_some_and(BTreeSet::is_empty) {
            return Err(Error::Request("text field selection is empty".into()));
        }
        let mut bound_filters = Vec::with_capacity(selected.len());
        let mut plans = BTreeMap::new();
        for (collection, _) in &selected {
            bound_filters.push(
                filter
                    .as_ref()
                    .map(|filter| self.bind_filter(filter, collection))
                    .transpose()?,
            );
            if !request.sort.is_empty() {
                plans.insert(
                    collection.name.clone(),
                    self.sort_plan(collection, &request.sort)?,
                );
            }
        }
        let mut offsets = Vec::with_capacity(selected.len() + 1);
        offsets.push(0_usize);
        for (_, snapshot) in &selected {
            let next = offsets
                .last()
                .unwrap()
                .checked_add(snapshot.sources.len())
                .ok_or_else(|| Error::Request("selected source table exceeds 2^32 slots".into()))?;
            if next > u32::MAX as usize {
                return Err(Error::Request(
                    "selected source table exceeds 2^32 slots".into(),
                ));
            }
            offsets.push(next);
        }
        let names = selected
            .iter()
            .map(|(collection, _)| collection.name.clone())
            .collect::<Vec<_>>();
        let ordered_key = OrderedResultKey {
            predicate: PredicateCacheKey {
                collection: format!("@frontend:{}", names.join("\0")),
                expression: serde_json::to_vec(&(
                    "ordinary-selected-text/v1",
                    &request.pattern,
                    request.mode,
                    request.case_sensitive,
                    &request.fields,
                    filter.as_ref().map(JsonFilter::cache_expression),
                    extension.map(|binding| &binding.identity),
                    selected
                        .iter()
                        .map(|(collection, snapshot)| {
                            (
                                &collection.mapping_hash,
                                &collection.schema_hash,
                                &snapshot.definitions,
                                &snapshot.source_generation,
                            )
                        })
                        .collect::<Vec<_>>(),
                ))
                .expect("selected text identity serializes"),
            },
            sort: serde_json::to_vec(&request.sort).expect("sort identity serializes"),
        };
        let can_cache = cache_allowed
            && self.options.cache == CacheMode::Memory
            && self.options.freshness != Freshness::HashOnRequest
            && self.query_cache.enabled()
            && extension.is_none_or(|binding| binding.cacheable)
            && filter.as_ref().is_none_or(|filter| !filter.has_relations())
            && !plans.values().any(crate::sorting::SortPlan::has_relations)
            && selected.iter().all(|(_, snapshot)| snapshot.cacheable);
        let cached_page = can_cache
            .then(|| {
                self.query_cache
                    .completed_ordered_page(&ordered_key, request.offset, request.limit)
            })
            .flatten();
        let cache_hit = cached_page.is_some();
        if !cache_hit
            && extension.is_none()
            && request.filter.is_none()
            && request.mode != crate::TextMode::Regex
            && matcher
                .as_ref()
                .and_then(|matcher| matcher.candidate_pattern.as_ref())
                .is_some_and(|pattern| pattern.len() >= 2)
            && selected.iter().all(|(_, snapshot)| snapshot.cacheable)
            && let Some(fields) = &request.fields
            && fields.len() == 1
        {
            let field = &fields[0];
            let path = JsonFilter::parse_path(field)?;
            for (collection, _) in &selected {
                let representation = if request.mode == crate::TextMode::Word {
                    match crate::repository::extension_index::field_format(collection, &path) {
                        "markdown" => "objekte:visible-markdown",
                        "html" => "objekte:visible-html",
                        "xml" => "objekte:visible-xml",
                        _ => "objekte:visible-plain",
                    }
                } else {
                    "objekte:stored"
                };
                self.warm_selected_text_field(&collection.name, representation, field)?;
            }
        }
        let mut matches = 0_usize;
        let mut ordered_slots = Vec::new();
        let mut used_candidates = false;
        if let Some((total, _)) = &cached_page {
            matches = *total;
            self.metrics.ordered_result_hits += 1;
        } else {
            let cap = self.options.total_limit;
            let mut sorted =
                (!request.sort.is_empty()).then(|| SortBuffer::<u32>::new(plans, 0, cap, cap));
            let needs_status = bound_filters.iter().flatten().any(JsonFilter::uses_status)
                || sorted.as_ref().is_some_and(SortBuffer::uses_status);
            let mut bytes = Vec::new();
            for (collection_index, (collection, snapshot)) in selected.iter().enumerate() {
                let rules = TextRules::new(&collection.fields);
                let schema = self
                    .extensions
                    .as_ref()
                    .and_then(|extensions| extensions.schema(&collection.name))
                    .cloned();
                let binding = extension.map(|extension| {
                    matcher::TextBindingPlan::new(
                        &collection.name,
                        schema.as_deref(),
                        &rules,
                        extension,
                    )
                });
                let candidates = self.selected_text_candidates(
                    request,
                    collection,
                    snapshot,
                    matcher.as_ref(),
                    extension,
                );
                used_candidates |= candidates.is_some();
                let mut positions = (0..snapshot.sources.len()).collect::<Vec<_>>();
                if sorted.is_none() {
                    positions.sort_unstable_by(|&left, &right| {
                        let a = &snapshot.sources[left];
                        let b = &snapshot.sources[right];
                        let modified = |source: &crate::extensions::SourceDescriptor| {
                            source
                                .metadata
                                .get("modified_unix_nanos")
                                .and_then(|value| value.parse::<i128>().ok())
                                .unwrap_or(0)
                        };
                        modified(b)
                            .cmp(&modified(a))
                            .then_with(|| a.record_id.cmp(&b.record_id))
                    });
                }
                for index in positions {
                    if candidates.as_ref().is_some_and(|bits| {
                        bits.get(index / 64)
                            .is_some_and(|word| word & (1 << (index % 64)) == 0)
                    }) {
                        continue;
                    }
                    let source = &snapshot.sources[index];
                    let record =
                        self.selected_record(collection, source, &mut bytes, needs_status)?;
                    let stamp = Stamp {
                        size: record.size_bytes,
                        modified: record.modified_unix_nanos.clone(),
                        identity: source.revision.clone(),
                    };
                    let created = source
                        .metadata
                        .get("created_unix_nanos")
                        .and_then(|value| value.parse::<i128>().ok());
                    if let Some(filter) = &bound_filters[collection_index]
                        && !filter.matches_source(
                            &record.data,
                            &record.id,
                            &record.path,
                            &stamp,
                            created,
                            record.state.status,
                        )?
                    {
                        continue;
                    }
                    let fields = selected_matches(
                        &record,
                        matcher.as_ref(),
                        extension,
                        binding.as_ref(),
                        &rules,
                        requested_fields.as_ref(),
                        false,
                    )?;
                    if fields.fields.is_empty() {
                        continue;
                    }
                    matches += 1;
                    let slot = u32::try_from(offsets[collection_index] + index)
                        .expect("checked source slots");
                    if let Some(buffer) = &mut sorted {
                        let sort_record = SortRecord {
                            id: &record.id,
                            collection_path: &collection.path,
                            root: &self.model.root,
                            size: record.size_bytes,
                            modified: &record.modified_unix_nanos,
                            status: record.state.status,
                            locator: Some(&source.locator),
                            created,
                        };
                        let keys = buffer.project(&collection.name, &sort_record, &record.data)?;
                        buffer.insert_keys(&collection.name, &record.id, keys, || slot);
                    } else if ordered_slots.len() < cap {
                        ordered_slots.push(slot);
                    }
                }
            }
            if let Some(buffer) = sorted {
                ordered_slots = buffer.into_page();
            }
        }
        let end = ordered_slots
            .len()
            .min(request.offset.saturating_add(request.limit));
        let page = cached_page.as_ref().map_or_else(
            || ordered_slots.get(request.offset..end).unwrap_or(&[]),
            |(_, page)| page.as_slice(),
        );
        let mut hits = Vec::with_capacity(page.len());
        let mut bytes = Vec::new();
        for &slot in page {
            let (collection_index, local) = source_location(&offsets, slot)
                .ok_or_else(|| Error::Cache("selected source slot is unavailable".into()))?;
            let (collection, snapshot) = &selected[collection_index];
            let source = &snapshot.sources[local];
            let record = self.selected_record(collection, source, &mut bytes, true)?;
            let stamp = Stamp {
                size: record.size_bytes,
                modified: record.modified_unix_nanos.clone(),
                identity: source.revision.clone(),
            };
            let created = source
                .metadata
                .get("created_unix_nanos")
                .and_then(|value| value.parse::<i128>().ok());
            if let Some(filter) = &bound_filters[collection_index]
                && !filter.matches_source(
                    &record.data,
                    &record.id,
                    &record.path,
                    &stamp,
                    created,
                    record.state.status,
                )?
            {
                return Err(Error::Unstable(source.locator.clone()));
            }
            let rules = TextRules::new(&collection.fields);
            let schema = self
                .extensions
                .as_ref()
                .and_then(|extensions| extensions.schema(&collection.name))
                .cloned();
            let binding = extension.map(|extension| {
                matcher::TextBindingPlan::new(
                    &collection.name,
                    schema.as_deref(),
                    &rules,
                    extension,
                )
            });
            let fields = selected_matches(
                &record,
                matcher.as_ref(),
                extension,
                binding.as_ref(),
                &rules,
                requested_fields.as_ref(),
                true,
            )?;
            if fields.fields.is_empty() {
                return Err(Error::Unstable(source.locator.clone()));
            }
            hits.push(TextHit {
                record,
                fields: fields.fields,
                matched_paths: fields.matched_paths,
            });
        }
        if self.options.freshness != Freshness::MetadataOnOpen {
            for (collection, snapshot) in &selected {
                let latest = self.source_descriptors(&collection.name)?;
                self.metrics.enumerations += 1;
                if latest != *snapshot.sources {
                    return Err(Error::Unstable(collection.path.clone()));
                }
            }
        }
        let pending = (can_cache && !cache_hit && matches <= self.options.total_limit)
            .then_some((ordered_key, ordered_slots));
        Ok((
            TextQueryResult {
                hits,
                total: request
                    .count
                    .then_some(matches.min(self.options.total_limit)),
                truncated: matches > self.options.total_limit,
                elapsed_ms: started.elapsed().as_secs_f64() * 1000.0,
                metrics: self.metrics.since(&before),
                query_plan: vec![
                    if cache_hit {
                        "selected-text-ordered-cache"
                    } else if used_candidates {
                        "selected-text-index-candidates"
                    } else {
                        "selected-text-exact-scan"
                    }
                    .into(),
                ],
            },
            pending,
        ))
    }
}
