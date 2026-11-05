//! Fused explicit projection build over the selected provider source table.
//! A source is read and decoded once for every requested text/typed output.

use super::*;
use crate::{
    JsonFilter,
    bigram::{BigramIndexBuilder, SKIP_INDEX_MIN_DENSITY_PCT},
    extensions::{
        BindingContext, BoundValueExtractor, DecodeOutcome, Extraction, LogicalNode, ScalarValue,
        TextRepresentation, TypedValue,
    },
    file_picker::TypedKey,
    repository::{
        extension_derived_index::{SelectedDerivedKey, SelectedDerivedPosting},
        extension_index::{
            RepresentationIndex, RepresentationIndexKey, add_text, base_source_fingerprint,
            binding_path, field_format, observe_bigrams,
        },
    },
};
use std::sync::Arc;


/// One requested process-local posting over the selected source table.
/// Build several requests together to share each provider read and decode.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum SelectedIndexRequest<'a> {
    Text {
        representation: &'a str,
        path: &'a str,
    },
    Derived {
        extractor: &'a str,
    },
    Field {
        path: &'a str,
    },
}


struct TextBuild {
    key: RepresentationIndexKey,
    path: JsonPath,
    definition: Arc<dyn TextRepresentation>,
    main: BigramIndexBuilder,
    skip: BigramIndexBuilder,
    rendered: String,
    content: String,
    observed_bigrams: Vec<u64>,
    malformed_slots: Vec<u32>,
}


struct DerivedBuild {
    key: SelectedDerivedKey,
    binding: BoundValueExtractor,
    rows: Vec<(TypedKey, usize)>,
    present: Vec<u64>,
    scalar_safe: bool,
    output: Vec<TypedValue>,
}


/// One ordinary logical-field posting over the selected source generation.
struct FieldBuild {
    key: SelectedDerivedKey,
    path: JsonPath,
    pointer: Option<String>,
    rows: Vec<(TypedKey, usize)>,
    present: Vec<u64>,
    scalar_safe: bool,
    output: Vec<TypedKey>,
    implicit_equivalent: bool,
}


struct ReuseMap {
    old_to_new: Vec<Option<u32>>,
    changed: Vec<bool>,
}


impl ReuseMap {
    fn new(
        old: &[crate::extensions::SourceDescriptor],
        new: &[crate::extensions::SourceDescriptor],
    ) -> Option<Self> {
        let mut old_to_new = vec![None; old.len()];
        let mut changed = vec![true; new.len()];
        for (old_slot, source) in old.iter().enumerate() {
            if let Ok(new_slot) = new.binary_search_by(|next| next.record_id.cmp(&source.record_id))
                && new[new_slot] == *source
            {
                old_to_new[old_slot] = Some(new_slot as u32);
                changed[new_slot] = false;
            }
        }
        let delta = changed.iter().filter(|&&changed| changed).count()
            + old_to_new.iter().filter(|slot| slot.is_none()).count();
        (delta.saturating_mul(4) <= new.len().max(1)).then_some(Self {
            old_to_new,
            changed,
        })
    }

    fn remap_rows(
        &self,
        rows: Vec<(TypedKey, usize)>,
        old_present: &[u64],
        slots: usize,
    ) -> (Vec<(TypedKey, usize)>, Vec<u64>) {
        let rows = rows
            .into_iter()
            .filter_map(|(key, old_slot)| {
                self.old_to_new
                    .get(old_slot)
                    .copied()
                    .flatten()
                    .map(|slot| (key, slot as usize))
            })
            .collect();
        let mut present = vec![0_u64; slots.div_ceil(64)];
        for (old_slot, mapped) in self.old_to_new.iter().enumerate() {
            if let Some(slot) = mapped
                && old_present
                    .get(old_slot / 64)
                    .is_some_and(|word| word & (1 << (old_slot % 64)) != 0)
            {
                present[*slot as usize / 64] |= 1 << (*slot % 64);
            }
        }
        (rows, present)
    }
}


struct TextReuse {
    old_key: RepresentationIndexKey,
    new_key: RepresentationIndexKey,
    base_to_current: Vec<Option<u32>>,
    overlay_slots: Vec<u32>,
    malformed_slots: Vec<u32>,
}


fn simple_pointer(path: &JsonPath) -> Option<String> {
    let mut pointer = String::new();
    for step in path {
        let crate::JsonPathStep::Key(key) = step else {
            return None;
        };
        pointer.push('/');
        pointer.push_str(&key.replace('~', "~0").replace('/', "~1"));
    }
    Some(pointer)
}


impl Repository {
    /// Build the requested postings in one source pass. Already built keys
    /// are reused; any failure leaves all newly requested keys unpublished.
    /// The default JSON picker path does not call this method.
    pub fn build_selected_indexes(
        &mut self,
        collection: &str,
        requests: &[SelectedIndexRequest<'_>],
    ) -> Result<usize> {
        self.build_selected_indexes_reusing(collection, requests, None)
    }

    pub(super) fn build_selected_indexes_reusing(
        &mut self,
        collection: &str,
        requests: &[SelectedIndexRequest<'_>],
        previous: Option<&CustomSourceSnapshot>,
    ) -> Result<usize> {
        if requests.is_empty() {
            return Err(Error::Request(
                "selected index request list is empty".into(),
            ));
        }
        if requests.len() > 4096 {
            return Err(Error::Request(
                "selected index build exceeds 4096 outputs".into(),
            ));
        }
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return Err(Error::Request(
                "selected indexes require memory cache and non-hash freshness".into(),
            ));
        }
        let selected_collection = self.model.collection(collection)?.clone();
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no extensions are registered".into()))?;
        let selected = if self.options.freshness == Freshness::MetadataOnOpen {
            if let Some(snapshot) = self.custom_source_snapshot.get(collection) {
                Arc::clone(&snapshot.sources)
            } else {
                self.metrics.enumerations += 1;
                Arc::new(self.source_descriptors(collection)?)
            }
        } else {
            self.metrics.enumerations += 1;
            Arc::new(self.source_descriptors(collection)?)
        };
        if selected.len() > u32::MAX as usize {
            return Err(Error::Request(
                "selected index source table exceeds 2^32 slots".into(),
            ));
        }
        let selection = extensions
            .selection_dependencies(&selected_collection, selected.as_ref())?
            .0;
        let encoding = selected_collection
            .bigram_encoding
            .unwrap_or(self.options.bigram_encoding);
        let reuse = previous.and_then(|old| ReuseMap::new(&old.sources, &selected));
        let mut text_builds = Vec::new();
        let mut text_reuses = Vec::new();
        let mut derived_builds = Vec::new();
        let mut field_builds = Vec::new();
        let mut reuse_failed = false;
        let mut requested = BTreeSet::new();
        for request in requests {
            if !requested.insert(*request) {
                return Err(Error::Request("duplicate selected index request".into()));
            }
            match request {
                SelectedIndexRequest::Text {
                    representation,
                    path,
                } => {
                    let definition = extensions.representation_definition(representation)?;
                    if !definition.descriptor().cacheable() {
                        return Err(Error::Request(format!(
                            "volatile representation {representation} cannot be indexed"
                        )));
                    }
                    let parsed = crate::text::parse_text_path(path)?;
                    if !definition.binds_explicit(&BindingContext {
                        collection,
                        path: &binding_path(&parsed),
                        format: field_format(&selected_collection, &parsed),
                        schema: extensions.schema(collection).map(Arc::as_ref),
                    }) {
                        return Err(Error::Request(format!(
                            "representation {representation} does not bind to {collection}.{path}"
                        )));
                    }
                    let key = RepresentationIndexKey::new(
                        &selected_collection,
                        path,
                        representation,
                        &selection,
                        extensions,
                        encoding,
                    );
                    if self.extension_text_indexes.contains_key(&key) {
                        continue;
                    }
                    if let (Some(old), Some(reuse)) = (previous, reuse.as_ref()) {
                        let old_key = RepresentationIndexKey::new(
                            &selected_collection,
                            path,
                            representation,
                            &old.fingerprint,
                            extensions,
                            encoding,
                        );
                        if let Some(posting) = self.extension_text_indexes.get(&old_key)
                            && old_key.semantics == key.semantics
                        {
                            let base_to_current = (0..posting.filter.file_count())
                                .map(|base| {
                                    let old_slot = posting
                                        .base_to_current
                                        .as_ref()
                                        .map_or(Some(base as u32), |map| {
                                            map.get(base).copied().flatten()
                                        })?;
                                    reuse.old_to_new.get(old_slot as usize).copied().flatten()
                                })
                                .collect::<Vec<_>>();
                            let mut covered = vec![false; selected.len()];
                            for &slot in base_to_current.iter().flatten() {
                                covered[slot as usize] = true;
                            }
                            let overlay_slots = covered
                                .iter()
                                .enumerate()
                                .filter(|(_, covered)| !**covered)
                                .map(|(slot, _)| slot as u32)
                                .collect::<Vec<_>>();
                            if overlay_slots.len().saturating_mul(4) <= selected.len().max(1) {
                                text_reuses.push(TextReuse {
                                    old_key,
                                    new_key: key,
                                    base_to_current,
                                    overlay_slots,
                                    malformed_slots: posting
                                        .malformed_slots
                                        .iter()
                                        .filter_map(|slot| {
                                            reuse.old_to_new.get(*slot as usize).copied().flatten()
                                        })
                                        .collect(),
                                });
                                continue;
                            }
                        }
                        reuse_failed = true;
                        continue;
                    }
                    text_builds.push(TextBuild {
                        key,
                        path: parsed,
                        definition,
                        main: BigramIndexBuilder::new_with_encoding(selected.len(), encoding),
                        skip: BigramIndexBuilder::new_with_encoding(selected.len(), encoding),
                        rendered: String::new(),
                        content: String::new(),
                        observed_bigrams: vec![0; 1024],
                        malformed_slots: Vec::new(),
                    });
                }
                SelectedIndexRequest::Derived { extractor } => {
                    let binding = extensions.bind_value(
                        extractor,
                        collection,
                        extensions.schema(collection).map(Arc::as_ref),
                    )?;
                    if !binding.cacheable {
                        return Err(Error::Request(format!(
                            "volatile extractor {extractor} cannot be indexed"
                        )));
                    }
                    let key = SelectedDerivedKey::new(
                        &selected_collection,
                        &binding.path,
                        &selection,
                        extensions,
                    );
                    if self.selected_derived_indexes.contains_key(&key) {
                        continue;
                    }
                    let mut rows = Vec::new();
                    let mut present = vec![0; selected.len().div_ceil(64)];
                    let mut scalar_safe = true;
                    if let (Some(old), Some(reuse)) = (previous, reuse.as_ref()) {
                        let old_key = SelectedDerivedKey::new(
                            &selected_collection,
                            &binding.path,
                            &old.fingerprint,
                            extensions,
                        );
                        if old_key.semantics == key.semantics
                            && let Some(posting) = self.selected_derived_indexes.get(&old_key)
                            && let Some(old_posting) = posting.index.export_dynamic(&binding.path)
                        {
                            (rows, present) = reuse.remap_rows(
                                old_posting.rows,
                                &old_posting.present,
                                selected.len(),
                            );
                            scalar_safe = old_posting.scalar_safe;
                        } else {
                            reuse_failed = true;
                            continue;
                        }
                    }
                    derived_builds.push(DerivedBuild {
                        key,
                        binding,
                        rows,
                        present,
                        scalar_safe,
                        output: Vec::new(),
                    });
                }
                SelectedIndexRequest::Field { path } => {
                    let parsed = JsonFilter::parse_path(path)?;
                    // This is an explicit build request. Automatic warmup
                    // still submits only configured paths.
                    let key = SelectedDerivedKey::new(
                        &selected_collection,
                        &parsed,
                        &selection,
                        extensions,
                    );
                    if self.selected_derived_indexes.contains_key(&key) {
                        continue;
                    }
                    let mut rows = Vec::new();
                    let mut present = vec![0; selected.len().div_ceil(64)];
                    let mut scalar_safe = true;
                    let mut implicit_equivalent = simple_pointer(&parsed).is_some();
                    if let (Some(old), Some(reuse)) = (previous, reuse.as_ref()) {
                        let old_key = SelectedDerivedKey::new(
                            &selected_collection,
                            &parsed,
                            &old.fingerprint,
                            extensions,
                        );
                        if old_key.semantics == key.semantics
                            && let Some(posting) = self.selected_derived_indexes.get(&old_key)
                            && let Some(old_posting) = posting.index.export_dynamic(&parsed)
                        {
                            (rows, present) = reuse.remap_rows(
                                old_posting.rows,
                                &old_posting.present,
                                selected.len(),
                            );
                            scalar_safe = old_posting.scalar_safe;
                            implicit_equivalent &= old_posting.implicit_equivalent;
                        } else {
                            reuse_failed = true;
                            continue;
                        }
                    }
                    field_builds.push(FieldBuild {
                        key,
                        pointer: simple_pointer(&parsed),
                        path: parsed,
                        rows,
                        present,
                        scalar_safe,
                        output: Vec::new(),
                        implicit_equivalent,
                    });
                }
            }
        }
        if reuse_failed {
            return self.build_selected_indexes_reusing(collection, requests, None);
        }
        if text_builds.is_empty()
            && text_reuses.is_empty()
            && derived_builds.is_empty()
            && field_builds.is_empty()
        {
            return Ok(selected.len());
        }
        let mut bytes = Vec::new();
        for (slot, source) in selected.iter().enumerate() {
            if text_builds.is_empty() && derived_builds.is_empty() && field_builds.is_empty() {
                break;
            }
            if reuse.as_ref().is_some_and(|map| !map.changed[slot]) {
                continue;
            }
            let decoder = extensions.decoder(source)?;
            if !decoder.descriptor().cacheable() {
                return Err(Error::Request(format!(
                    "volatile decoder {} cannot be indexed",
                    decoder.descriptor().identity
                )));
            }
            if let Some(provider) = extensions
                .source_providers()
                .find(|provider| provider.descriptor().identity == source.provider)
                && !provider.descriptor().cacheable()
            {
                return Err(Error::Request(format!(
                    "volatile source provider {} cannot be indexed",
                    provider.descriptor().identity
                )));
            }
            self.read_selected_source_bytes_into(source, &mut bytes)?;
            self.metrics.source_reads += 1;
            self.metrics.source_bytes += bytes.len() as u64;
            for build in &mut text_builds {
                build.content.clear();
            }
            self.metrics.parses += 1;
            let outcome =
                crate::extensions::format::decode_checked(decoder, &bytes, &mut |record| {
                    for build in &mut text_builds {
                        add_text(
                            record,
                            &build.path,
                            build.definition.as_ref(),
                            &mut build.rendered,
                            &mut build.content,
                        )?;
                    }
                    for build in &mut derived_builds {
                        build.output.clear();
                        let state = build.binding.extractor.extract(record, &mut build.output)?;
                        if state == Extraction::Missing && !build.output.is_empty() {
                            return Err(Error::Request(format!(
                                "extractor {} emitted values for a missing field",
                                build.binding.identity
                            )));
                        }
                        if state == Extraction::Present {
                            build.present[slot / 64] |= 1 << (slot % 64);
                        } else {
                            build.rows.push((TypedKey::Missing, slot));
                        }
                        build.scalar_safe &= build.output.len() <= 1;
                        for value in build.output.drain(..) {
                            build.rows.push((TypedKey::from_extracted(value)?, slot));
                        }
                    }
                    if !field_builds.is_empty() {
                        let mut array_paths = Vec::new();
                        record.visit_nodes(&mut |path, node| {
                            if matches!(node, LogicalNode::Array) {
                                array_paths.push(path.to_owned());
                            }
                            Ok(())
                        })?;
                        for build in &mut field_builds {
                            build.output.clear();
                            let present = record.path_present(&build.path)?;
                            let count = record.visit_path(&build.path, &mut |value| {
                                let typed = match value {
                                    ScalarValue::Null => TypedValue::Null,
                                    ScalarValue::Bool(value) => TypedValue::Bool(value),
                                    ScalarValue::Number(value) => TypedValue::Number(value.into()),
                                    ScalarValue::Text(value) => TypedValue::Text(value.into()),
                                };
                                build.output.push(TypedKey::from_extracted(typed)?);
                                Ok(())
                            })?;
                            if !present && count > 0 {
                                return Err(Error::Request(
                                    "decoder emitted values for a missing field".into(),
                                ));
                            }
                            if present {
                                build.present[slot / 64] |= 1 << (slot % 64);
                            } else {
                                build.rows.push((TypedKey::Missing, slot));
                            }
                            build.scalar_safe &= count <= 1
                                && build.pointer.as_ref().is_some_and(|pointer| {
                                    !array_paths.iter().any(|array| {
                                        pointer == array
                                            || pointer
                                                .strip_prefix(array)
                                                .is_some_and(|rest| rest.starts_with('/'))
                                    })
                                });
                            for key in build.output.drain(..) {
                                build.rows.push((key, slot));
                            }
                        }
                    }
                    Ok(())
                })?;
            if matches!(outcome, DecodeOutcome::Malformed(_)) {
                for build in &mut text_builds {
                    build.malformed_slots.push(slot as u32);
                }
                for build in &mut derived_builds {
                    build.rows.push((TypedKey::Missing, slot));
                }
                for build in &mut field_builds {
                    build.rows.push((TypedKey::Missing, slot));
                }
                continue;
            }
            for build in &mut text_builds {
                observe_bigrams(
                    &mut build.observed_bigrams,
                    build.content.as_bytes(),
                    encoding,
                );
                build
                    .main
                    .add_file_content(&build.skip, slot, build.content.as_bytes());
            }
        }
        if self.options.freshness != Freshness::MetadataOnOpen {
            let latest = self.source_descriptors(collection)?;
            self.metrics.enumerations += 1;
            if latest != *selected {
                return Err(Error::Unstable(selected_collection.path));
            }
        }
        for build in text_builds {
            let mut filter = build.main.compress(None);
            filter.set_skip_index(build.skip.compress(Some(SKIP_INDEX_MIN_DENSITY_PCT)));
            self.extension_text_indexes.insert(
                build.key,
                RepresentationIndex {
                    filter,
                    observed_bigrams: build.observed_bigrams,
                    base_to_current: None,
                    overlay_slots: Vec::new(),
                    malformed_slots: build.malformed_slots,
                    slots: selected.len(),
                    base_source_fingerprints: selected
                        .iter()
                        .map(base_source_fingerprint)
                        .collect(),
                },
            );
        }
        for update in text_reuses {
            let mut posting = self
                .extension_text_indexes
                .remove(&update.old_key)
                .expect("reusable text posting exists until publication");
            posting.base_to_current = Some(update.base_to_current);
            posting.overlay_slots = update.overlay_slots;
            posting.malformed_slots = update.malformed_slots;
            posting.slots = selected.len();
            self.extension_text_indexes.insert(update.new_key, posting);
        }
        for build in derived_builds {
            let posting = SelectedDerivedPosting::from_rows(
                build.binding.path,
                build.rows,
                build.present,
                build.scalar_safe,
                selected.len(),
            );
            self.selected_derived_indexes.insert(build.key, posting);
        }
        for build in field_builds {
            let posting = SelectedDerivedPosting::from_field_rows(
                build.path,
                build.rows,
                build.present,
                build.scalar_safe,
                selected.len(),
                build.implicit_equivalent,
            );
            self.selected_derived_indexes.insert(build.key, posting);
        }
        if self.options.freshness == Freshness::MetadataOnOpen
            && !self.custom_source_snapshot.contains_key(collection)
        {
            let snapshot = CustomSourceSnapshot::new(
                &selected_collection,
                selected.as_ref().clone(),
                self.extensions
                    .as_deref()
                    .expect("registered selected outputs"),
            )?;
            self.custom_source_snapshot
                .insert(collection.into(), snapshot);
        }
        Ok(selected.len())
    }
}
