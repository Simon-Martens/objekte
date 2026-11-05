//! Text queries over pinned provider inventories; native-only collections keep
//! the concrete text matcher, picker and streaming loops in the parent module.

use super::*;
use crate::{
    extensions::{
        BindingContext, BoundTextQuery, CandidateConstraint, TextComparison, TextCompileRequest,
        TextSyntax,
    },
    repository::{extension_index::RepresentationIndexKey, extension_query::source_location},
    sorting::{SortBuffer, SortRecord},
};

type SelectedHitCallback<'a> = dyn FnMut(usize, &TextHit) -> Result<()> + 'a;


/// Sources and field scope needed for one selected-record verification.
struct SelectedMatchScope<'a, 'b> {
    source: &'a crate::extensions::SourceDescriptor,
    extensions: &'a crate::extensions::Extensions,
    fields: Option<&'a BTreeSet<&'b str>>,
    indices: bool,
}


fn selected_matches(
    record: &Record,
    scope: SelectedMatchScope<'_, '_>,
    matcher: Option<&TextMatcher>,
    extension: Option<&BoundTextQuery>,
    binding: Option<&matcher::TextBindingPlan<'_>>,
    definitions: Option<&BTreeMap<String, Vec<BoundTextQuery>>>,
    rules: &TextRules,
) -> Result<TextMatches> {
    if record.state.status.is_malformed() {
        let matches = if let (Some(matcher), None, Some(raw)) =
            (matcher, scope.fields, record.raw.as_ref())
        {
            scope
                .extensions
                .decoder(scope.source)?
                .malformed_text(raw)?
                .is_some_and(|text| matcher.matches_malformed_text(&text))
        } else {
            false
        };
        return Ok(TextMatches {
            fields: if matches {
                vec!["$raw".into()]
            } else {
                Vec::new()
            },
            matched_paths: if matches && scope.indices {
                vec!["$raw".into()]
            } else {
                Vec::new()
            },
            matched_definitions: Vec::new(),
        });
    }
    if let Some(definitions) = definitions.filter(|definitions| !definitions.is_empty()) {
        matcher
            .expect("definition matcher exists")
            .matching_fields_definitions(
                &record.data,
                rules,
                scope.fields,
                scope.indices,
                definitions,
            )
    } else if let Some(extension) = extension {
        TextMatcher::matching_fields_extension(
            &record.data,
            rules,
            scope.fields,
            scope.indices,
            extension,
            binding.expect("extension binding exists"),
        )
    } else {
        Ok(matcher.expect("native matcher exists").matching_fields(
            &record.data,
            rules,
            scope.fields,
            scope.indices,
        ))
    }
}


impl Repository {
    fn bind_selected_text_definitions(
        &self,
        request: &TextQuery,
        collection: &Collection,
    ) -> Result<BTreeMap<String, Vec<BoundTextQuery>>> {
        let Some(extensions) = self.extensions.as_ref() else {
            return Ok(BTreeMap::new());
        };
        let mut selected = BTreeMap::new();
        if let Some(fields) = &request.fields {
            for field in fields {
                let parsed = crate::text::parse_text_path(field.path())?;
                let declared = crate::schema::field_at_path(&collection.fields, &parsed)
                    .is_some_and(|field| field.text_search.is_some());
                if field.search().is_none() && !declared {
                    continue;
                }
                if request.mode == crate::TextMode::WordRaw {
                    return Err(Error::Request(
                        "explicit text definition conflicts with raw-word mode".into(),
                    ));
                }
                let canonical = crate::repository::extension_index::binding_path(&parsed);
                let request = TextCompileRequest {
                    pattern: request.pattern.clone(),
                    comparison: TextComparison::Contains,
                    syntax: if request.mode == crate::TextMode::Regex {
                        TextSyntax::Regex
                    } else {
                        TextSyntax::Literal
                    },
                    case_sensitive: request.case_sensitive.then_some(true),
                };
                let bound = extensions.bind_text_definition(
                    collection,
                    field.path(),
                    field.search(),
                    &request,
                )?;
                let bindings: &mut Vec<BoundTextQuery> = selected.entry(canonical).or_default();
                if !bindings
                    .iter()
                    .any(|existing| existing.identity == bound.identity)
                {
                    bindings.push(bound);
                }
            }
        } else {
            for path in crate::schema::text_search_paths(&collection.fields) {
                if request.mode == crate::TextMode::WordRaw {
                    return Err(Error::Request(
                        "configured text definition conflicts with raw-word mode".into(),
                    ));
                }
                let request = TextCompileRequest {
                    pattern: request.pattern.clone(),
                    comparison: TextComparison::Contains,
                    syntax: if request.mode == crate::TextMode::Regex {
                        TextSyntax::Regex
                    } else {
                        TextSyntax::Literal
                    },
                    case_sensitive: request.case_sensitive.then_some(true),
                };
                let parsed = JsonFilter::parse_path(&path)?;
                let canonical = crate::repository::extension_index::binding_path(&parsed);
                selected.insert(
                    canonical,
                    vec![extensions.bind_text_definition(collection, &path, None, &request)?],
                );
            }
        }
        Ok(selected)
    }

    fn selected_definition_candidates(
        &self,
        collection: &Collection,
        snapshot: &CustomSourceSnapshot,
        _fields: Option<&[crate::TextFieldSelector]>,
        definitions: &BTreeMap<String, Vec<BoundTextQuery>>,
        filter: Option<&JsonFilter>,
    ) -> Option<Vec<u64>> {
        if self.options.cache != CacheMode::Memory || definitions.is_empty() {
            return None;
        }
        let extensions = self.extensions.as_ref()?;
        if filter.is_some_and(|filter| {
            filter.has_relations()
                || !filter.registered_text_pruning_safe()
                || filter.needs_selected_cardinality_check_in(|path| {
                    let key = crate::repository::extension_derived_index::SelectedDerivedKey::new(
                        collection,
                        path,
                        &snapshot.fingerprint,
                        extensions,
                    );
                    self.selected_derived_indexes
                        .get(&key)
                        .is_some_and(|posting| posting.index.scalar_safe(path))
                })
        }) {
            return None;
        }
        let mut union = None::<Vec<u64>>;
        for (path, bindings) in definitions {
            for bound in bindings {
                if !bound.allows_candidate_pruning() {
                    return None;
                }
                let CandidateConstraint::BigramLiteral {
                    literal,
                    representation,
                } = &bound.candidate
                else {
                    return None;
                };
                let compatible = representation == &bound.representation.descriptor().identity
                    || representation == "objekte:visible-normalized"
                        && bound.preparation
                            == crate::extensions::TextPreparation::VisibleNormalized
                    || representation == "objekte:stored"
                        && bound.preparation == crate::extensions::TextPreparation::Stored;
                if !compatible {
                    return None;
                }
                let key = RepresentationIndexKey::new(
                    collection,
                    bound.index_path.as_deref().unwrap_or(path),
                    &bound.representation.descriptor().identity,
                    &snapshot.fingerprint,
                    extensions,
                    collection
                        .bigram_encoding
                        .unwrap_or(self.options.bigram_encoding),
                );
                let bits = self.representation_index_candidates(&key, literal)?;


                union = Some(match union {
                    Some(mut current) => {
                        for (word, extra) in current.iter_mut().zip(bits) {
                            *word |= extra;
                        }
                        current
                    }
                    None => bits,
                });
            }
        }


        union
    }

    /// Reuse the concrete picker's text planner for its native partition.
    /// Uncovered custom slots remain candidates for exact decoding.
    fn selected_native_text_candidates(
        &mut self,
        collection: &Collection,
        snapshot: &CustomSourceSnapshot,
        matcher: Option<&TextMatcher>,
        extension: Option<&BoundTextQuery>,
        filter: Option<&JsonFilter>,
        requested_fields: Option<&BTreeSet<&str>>,
    ) -> Result<Option<Vec<u64>>> {
        if self.options.cache != CacheMode::Memory || snapshot.native_count == 0 {
            return Ok(None);
        }


        self.ensure_picker(collection)?;
        let started = Instant::now();
        if self
            .file_pickers
            .get_mut(&collection.name)
            .expect("picker exists")
            .ensure_text_index()?
        {
            self.query_cache.invalidate(&collection.name);
            self.metrics.fts_open_nanos += nanos(started);
        }
        let picker = &self.file_pickers[&collection.name];
        if filter.is_some_and(|filter| {
            filter.has_relations()
                || !filter.registered_text_pruning_safe()
                || filter.needs_cardinality_check_in(|path| picker.typed_scalar_safe(path))
                || filter
                    .top_level_scalar_fields()
                    .iter()
                    .any(|field| picker.has_array_value(field))
        }) {
            return Ok(None);
        }
        let has_text_index = picker.text_index_bytes().is_some();
        let selection = if let Some(extension) = extension {
            let (candidate, raw) = match &extension.candidate {
                CandidateConstraint::BigramLiteral {
                    literal,
                    representation,
                } if extension.allows_candidate_pruning()
                    && representation == "objekte:stored"
                    && extension.native_index_projection == Some(NativeTextProjection::Stored)
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
                    let word = literal
                        .split(' ')
                        .max_by_key(|word| word.len())
                        .filter(|word| !word.is_empty());
                    (word, true)
                }
                _ => (None, true),
            };
            picker.text_candidate_selection(candidate, raw, true, requested_fields, |_, _| true)
        } else {
            let matcher = matcher.expect("native text matcher exists");
            picker.text_candidate_selection(
                if has_text_index {
                    matcher.candidate_pattern.as_deref()
                } else {
                    matcher.regular_candidate_pattern()
                },
                matcher.raw,
                matcher.candidate_literal,
                requested_fields,
                |value, format| matcher.matches_value(value, format),
            )
        };
        if selection.is_all() || selection.len().saturating_mul(2) >= snapshot.native_count {
            return Ok(None);
        }
        Ok(snapshot.native_candidate_bits(picker, &selection, &mut self.metrics))
    }

    fn selected_text_candidates(
        &self,
        request: &TextQuery,
        collection: &Collection,
        snapshot: &CustomSourceSnapshot,
        matcher: Option<&TextMatcher>,
        extension: Option<&BoundTextQuery>,
        filter: Option<&JsonFilter>,
    ) -> Option<Vec<u64>> {
        if self.options.cache != CacheMode::Memory {
            return None;
        }
        let fields = request
            .fields
            .as_ref()
            .filter(|fields| !fields.is_empty())?;
        let extensions = self.extensions.as_ref()?;
        // A filter is evaluated before text matching. Its potential errors
        // cannot be hidden by a text posting for a nonmatching record.
        if filter.is_some_and(|filter| {
            filter.has_relations()
                || !filter.registered_text_pruning_safe()
                || filter.needs_selected_cardinality_check_in(|path| {
                    let key = crate::repository::extension_derived_index::SelectedDerivedKey::new(
                        collection,
                        path,
                        &snapshot.fingerprint,
                        extensions,
                    );
                    self.selected_derived_indexes
                        .get(&key)
                        .is_some_and(|posting| posting.index.scalar_safe(path))
                })
        }) {
            return None;
        }
        let literal = if let Some(extension) = extension {
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
            literal.as_str()
        } else {
            let matcher = matcher?;
            if request.mode == crate::TextMode::Regex {
                return None;
            }
            matcher.candidate_pattern.as_deref()?
        };
        let mut union: Option<Vec<u64>> = None;
        for field in fields {
            let parsed = crate::text::parse_text_path(field).ok()?;
            let format = crate::repository::extension_index::field_format(collection, &parsed);
            let representation = if let Some(extension) = extension {
                extension.representation.descriptor().identity.as_str()
            } else if request.mode == crate::TextMode::Word {
                match format {
                    "markdown" => "objekte:visible-markdown",
                    "html" => "objekte:visible-html",
                    "xml" => "objekte:visible-xml",
                    _ => "objekte:visible-plain",
                }
            } else {
                "objekte:stored"
            };
            let definition = extensions.representation_definition(representation).ok()?;
            if !definition.binds(&BindingContext {
                collection: &collection.name,
                path: field,
                format,
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
            let bits = self.representation_index_candidates(&key, literal)?;


            union = Some(match union {
                Some(mut current) => {
                    for (left, right) in current.iter_mut().zip(bits) {
                        *left |= right;
                    }
                    current
                }
                None => bits,
            });
        }


        union
    }

    pub(super) fn search_text_selected(
        &mut self,
        request: &TextQuery,
        selected: Vec<(Collection, CustomSourceSnapshot)>,
        extension: Option<&BoundTextQuery>,
        cache_allowed: bool,
        mut on_hit: Option<&mut SelectedHitCallback<'_>>,
    ) -> Result<(TextQueryResult, Option<PendingSelectedOrder>, usize)> {
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
        let canonical_field_paths = request
            .fields
            .as_ref()
            .map(|fields| {
                fields
                    .iter()
                    .map(|field| {
                        let path = crate::text::parse_text_path(field.path())?;
                        Ok(crate::repository::extension_index::binding_path(&path))
                    })
                    .collect::<Result<Vec<_>>>()
            })
            .transpose()?;
        let requested_fields = canonical_field_paths
            .as_ref()
            .map(|fields| fields.iter().map(String::as_str).collect::<BTreeSet<_>>());
        if requested_fields.as_ref().is_some_and(BTreeSet::is_empty) {
            return Err(Error::Request("text field selection is empty".into()));
        }
        let mut bound_filters = Vec::with_capacity(selected.len());
        let mut bound_definitions = Vec::with_capacity(selected.len());
        let mut plans = BTreeMap::new();
        for (collection, _) in &selected {
            bound_definitions.push(if extension.is_none() {
                self.bind_selected_text_definitions(request, collection)?
            } else {
                BTreeMap::new()
            });
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
        let semantic_fields = selected
            .iter()
            .enumerate()
            .map(|(index, (collection, _))| {
                (
                    collection.name.as_str(),
                    request.fields.as_ref().map(|fields| {
                        fields
                            .iter()
                            .map(|field| {
                                let canonical =
                                    crate::text::parse_text_path(field.path()).ok().map(|path| {
                                        crate::repository::extension_index::binding_path(&path)
                                    });
                                let identities = canonical
                                    .as_ref()
                                    .and_then(|path| bound_definitions[index].get(path))
                                    .map(|bindings| {
                                        bindings
                                            .iter()
                                            .map(|bound| bound.identity.as_str())
                                            .collect::<Vec<_>>()
                                    });
                                (field.path(), identities)
                            })
                            .collect::<Vec<_>>()
                    }),
                    bound_definitions[index]
                        .iter()
                        .map(|(path, bindings)| {
                            (
                                path.as_str(),
                                bindings
                                    .iter()
                                    .map(|bound| bound.identity.as_str())
                                    .collect::<Vec<_>>(),
                            )
                        })
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>();
        let ordered_key = OrderedResultKey {
            predicate: PredicateCacheKey {
                collection: format!("@frontend:{}", names.join("\0")),
                expression: serde_json::to_vec(&(
                    "ordinary-selected-text/v2",
                    &request.pattern,
                    request.mode,
                    request.case_sensitive,
                    &semantic_fields,
                    bound_filters
                        .iter()
                        .map(|filter| filter.as_ref().map(JsonFilter::cache_expression))
                        .collect::<Vec<_>>(),
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
            && bound_definitions
                .iter()
                .flat_map(|definitions| definitions.values().flatten())
                .all(|binding| binding.cacheable)
            && bound_filters
                .iter()
                .flatten()
                .all(JsonFilter::registered_text_cacheable)
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
            && bound_definitions.iter().all(BTreeMap::is_empty)
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
            let path = crate::text::parse_text_path(field)?;
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
        let mut decisions = Vec::new();
        let mut returned = 0_usize;
        let stream_early = on_hit.is_some() && request.sort.is_empty() && !cache_hit;
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
                let projection_roots =
                    crate::repository::extension_sources::selected_projection_roots(
                        bound_filters[collection_index].as_ref(),
                        canonical_field_paths.as_deref(),
                        &request.sort,
                        true,
                    );
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
                let defer_scalar = bound_filters[collection_index]
                    .as_ref()
                    .and_then(|filter| {
                        self.selected_field_candidate_estimate(collection, snapshot, filter)
                    })
                    .is_some_and(|estimate| estimate.saturating_mul(16) > snapshot.sources.len());
                let mut scalar = if defer_scalar {
                    None
                } else if let Some(filter) = bound_filters[collection_index].as_ref() {
                    self.selected_scalar_candidates(collection, snapshot, filter)?
                } else {
                    None
                };
                let scalar_count = scalar.as_ref().map(|bits| {
                    bits.iter()
                        .map(|word| word.count_ones() as usize)
                        .sum::<usize>()
                });
                // A selective scalar posting is already an economical seed.
                // Exact text verification still runs on each surviving record.
                let use_scalar_seed = scalar_count
                    .is_some_and(|count| count.saturating_mul(16) <= snapshot.sources.len());
                let mut native_text_seed = false;
                let mut candidates = if use_scalar_seed {
                    scalar.take()
                } else {
                    let definitions = &bound_definitions[collection_index];
                    let selected_text = if definitions.is_empty() {
                        self.selected_text_candidates(
                            request,
                            collection,
                            snapshot,
                            matcher.as_ref(),
                            extension,
                            bound_filters[collection_index].as_ref(),
                        )
                    } else {
                        self.selected_definition_candidates(
                            collection,
                            snapshot,
                            request.fields.as_deref(),
                            definitions,
                            bound_filters[collection_index].as_ref(),
                        )
                    };
                    let selected_count = selected_text.as_ref().map(|bits| {
                        bits.iter()
                            .map(|word| word.count_ones() as usize)
                            .sum::<usize>()
                    });
                    let has_unbound_fields = request.fields.as_ref().is_none_or(|fields| {
                        fields.iter().any(|field| {
                            crate::text::parse_text_path(field.path())
                                .ok()
                                .map(|path| crate::repository::extension_index::binding_path(&path))
                                .is_none_or(|path| !definitions.contains_key(&path))
                        })
                    });
                    let native_text = if !definitions.is_empty() && has_unbound_fields
                        || definitions.is_empty()
                            && selected_count.is_none_or(|count| {
                                count.saturating_mul(8) > snapshot.sources.len()
                            }) {
                        self.selected_native_text_candidates(
                            collection,
                            snapshot,
                            matcher.as_ref(),
                            extension,
                            bound_filters[collection_index].as_ref(),
                            requested_fields.as_ref(),
                        )?
                    } else {
                        None
                    };
                    let native_count = native_text.as_ref().map(|bits| {
                        bits.iter()
                            .map(|word| word.count_ones() as usize)
                            .sum::<usize>()
                    });
                    if !definitions.is_empty() {
                        if has_unbound_fields {
                            native_text_seed = native_text.is_some();
                            match (selected_text, native_text) {
                                (Some(mut selected), Some(native)) => {
                                    for (left, right) in selected.iter_mut().zip(native) {
                                        *left |= right;
                                    }
                                    Some(selected)
                                }
                                _ => None,
                            }
                        } else {
                            selected_text
                        }
                    } else {
                        native_text_seed = native_count.is_some_and(|native| {
                            selected_count.is_none_or(|selected| native < selected)
                        });
                        if native_text_seed {
                            native_text
                        } else {
                            selected_text
                        }
                    }
                };
                if bound_filters[collection_index].is_some() {
                    let text_count = candidates.as_ref().map(|bits| {
                        bits.iter()
                            .map(|word| word.count_ones() as usize)
                            .sum::<usize>()
                    });
                    // A rare text posting needs no additional materialized
                    // bitset; broad text may benefit from scalar intersection.
                    if !use_scalar_seed
                        && text_count
                            .is_none_or(|count| count.saturating_mul(16) > snapshot.sources.len())
                    {
                        if scalar.is_none() && defer_scalar {
                            scalar = self.selected_scalar_candidates(
                                collection,
                                snapshot,
                                bound_filters[collection_index]
                                    .as_ref()
                                    .expect("bound filter"),
                            )?;
                        }
                        if let Some(scalar) = scalar.take() {
                            if let Some(bits) = &mut candidates {
                                for (left, right) in bits.iter_mut().zip(scalar) {
                                    *left &= right;
                                }
                            } else {
                                candidates = Some(scalar);
                            }
                        }
                    }
                }
                decisions.push(format!(
                    "{}:{}",
                    collection.name,
                    if use_scalar_seed {
                        "scalar-seed"
                    } else if native_text_seed {
                        "native-text-seed"
                    } else if candidates.is_some() {
                        "selected-text-seed"
                    } else {
                        "scan"
                    },
                ));
                used_candidates |= candidates.is_some();
                for index in snapshot.scan_slots(candidates.as_deref(), sorted.is_none()) {
                    self.metrics.selected_candidate_visits += 1;
                    self.metrics.selected_descriptor_visits += 1;
                    let source = &snapshot.sources[index];
                    let record = self.selected_record_for_evaluation(
                        collection,
                        source,
                        &mut bytes,
                        needs_status,
                        projection_roots.as_ref(),
                    )?;
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
                        SelectedMatchScope {
                            source,
                            extensions: self
                                .extensions
                                .as_deref()
                                .expect("selected source registry"),
                            fields: requested_fields.as_ref(),
                            indices: false,
                        },
                        matcher.as_ref(),
                        extension,
                        binding.as_ref(),
                        Some(&bound_definitions[collection_index]),
                        &rules,
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
                    if stream_early
                        && matches <= cap
                        && matches > request.offset
                        && matches - request.offset <= request.limit
                    {
                        let checked = self.selected_record(collection, source, &mut bytes, true)?;
                        let checked_stamp = Stamp {
                            size: checked.size_bytes,
                            modified: checked.modified_unix_nanos.clone(),
                            identity: source.revision.clone(),
                        };
                        if let Some(filter) = &bound_filters[collection_index]
                            && !filter.matches_source(
                                &checked.data,
                                &checked.id,
                                &checked.path,
                                &checked_stamp,
                                created,
                                checked.state.status,
                            )?
                        {
                            return Err(Error::Unstable(source.locator.clone()));
                        }
                        let fields = selected_matches(
                            &checked,
                            SelectedMatchScope {
                                source,
                                extensions: self
                                    .extensions
                                    .as_deref()
                                    .expect("selected source registry"),
                                fields: requested_fields.as_ref(),
                                indices: true,
                            },
                            matcher.as_ref(),
                            extension,
                            binding.as_ref(),
                            Some(&bound_definitions[collection_index]),
                            &rules,
                        )?;
                        if fields.fields.is_empty() {
                            return Err(Error::Unstable(source.locator.clone()));
                        }
                        on_hit.as_mut().expect("stream callback")(
                            returned + 1,
                            &TextHit {
                                record: checked,
                                fields: fields.fields,
                                matched_paths: fields.matched_paths,
                                matched_definitions: fields.matched_definitions,
                            },
                        )?;
                        returned += 1;
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
        for &slot in if stream_early { &[][..] } else { page } {
            let (collection_index, local) = source_location(&offsets, slot)
                .ok_or_else(|| Error::Cache("selected source slot is unavailable".into()))?;
            let (collection, snapshot) = &selected[collection_index];
            self.metrics.selected_descriptor_visits += 1;
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
                SelectedMatchScope {
                    source,
                    extensions: self
                        .extensions
                        .as_deref()
                        .expect("selected source registry"),
                    fields: requested_fields.as_ref(),
                    indices: true,
                },
                matcher.as_ref(),
                extension,
                binding.as_ref(),
                Some(&bound_definitions[collection_index]),
                &rules,
            )?;
            if fields.fields.is_empty() {
                return Err(Error::Unstable(source.locator.clone()));
            }
            let hit = TextHit {
                record,
                fields: fields.fields,
                matched_paths: fields.matched_paths,
                matched_definitions: fields.matched_definitions,
            };
            if let Some(callback) = on_hit.as_mut() {
                callback(returned + 1, &hit)?;
            } else {
                hits.push(hit);
            }
            returned += 1;
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
                ]
                .into_iter()
                .chain(decisions)
                .collect(),
            },
            pending,
            returned,
        ))


    }
}
