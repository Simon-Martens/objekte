//! Optional engine-owned postings for a bound representation on selected sources.
//! The ordinary JSON picker remains the default concrete index path.

use super::*;
use crate::{
    JsonFilter, JsonPathStep, TextFormat,
    bigram::{BigramFilter, BigramIndexBuilder, SKIP_INDEX_MIN_DENSITY_PCT},
    extensions::{BindingContext, DecodeOutcome, ScalarValue, TextRepresentation},
    text::TextRules,
};
use sha2::{Digest as _, Sha256};
use std::sync::Arc;
use unicode_casefold::UnicodeCaseFold;


#[derive(Clone, Eq, Ord, PartialEq, PartialOrd, serde::Serialize, serde::Deserialize)]
pub(super) struct RepresentationIndexKey {
    pub collection: String,
    pub path: String,
    pub representation: String,
    pub selection: String,
    pub semantics: String,
}


impl RepresentationIndexKey {
    pub(super) fn new(
        collection: &Collection,
        path: &str,
        representation: &str,
        selection: &str,
        extensions: &crate::extensions::Extensions,
        encoding: crate::BigramEncoding,
    ) -> Self {
        Self {
            collection: collection.name.clone(),
            path: path.into(),
            representation: representation.into(),
            selection: selection.into(),
            semantics: crate::config::fingerprint(
                "representation-index/v2",
                &(
                    collection.mapping_hash.as_str(),
                    collection.schema_hash.as_deref(),
                    extensions.representation_descriptor(representation),
                    encoding,
                ),
            ),
        }
    }
}


pub(super) struct RepresentationIndex {
    pub filter: BigramFilter,
    /// Pairs seen before compression; distinguishes absent pairs from dense
    /// postings deliberately omitted by the concrete bigram builder.
    pub observed_bigrams: Vec<u64>,
    /// Base immutable posting slots mapped into the current generation.
    /// None is the identity map of a newly built posting.
    pub base_to_current: Option<Vec<Option<u32>>>,
    /// New or changed records are verified without consulting stale bits.
    pub overlay_slots: Vec<u32>,
    /// Sources with no logical record; unscoped raw fallback must verify them.
    pub malformed_slots: Vec<u32>,
    pub slots: usize,
    pub base_source_fingerprints: Vec<[u8; 16]>,
}


pub(super) fn base_source_fingerprint(source: &crate::extensions::SourceDescriptor) -> [u8; 16] {
    let mut digest = Sha256::new();
    digest.update(b"selected-base-source/v1\0");
    digest.update(serde_json::to_vec(source).expect("source descriptor serializes"));
    let complete = digest.finalize();
    let mut result = [0_u8; 16];
    result.copy_from_slice(&complete[..16]);
    result
}

pub(super) fn observe_bigrams(
    observed: &mut [u64],
    content: &[u8],
    encoding: crate::BigramEncoding,
) {
    for pair in content.windows(2) {
        if encoding.usable(pair[0]) && encoding.usable(pair[1]) {
            let key = ((pair[0].to_ascii_lowercase() as usize) << 8)
                | pair[1].to_ascii_lowercase() as usize;
            observed[key / 64] |= 1 << (key % 64);
        }
    }
}

pub(super) fn binding_path(path: &[JsonPathStep]) -> String {
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

pub(super) fn field_format(collection: &Collection, path: &[JsonPathStep]) -> &'static str {
    let formats = TextRules::new(&collection.fields)
        .declared_text_paths()
        .into_iter()
        .collect::<BTreeMap<_, _>>();
    match formats
        .get(&binding_path(path))
        .copied()
        .unwrap_or(TextFormat::Plain)
    {
        TextFormat::Plain => "plain",
        TextFormat::Markdown => "markdown",
        TextFormat::Html => "html",
        TextFormat::Xml => "xml",
    }
}

pub(super) fn add_text(
    record: &dyn crate::extensions::DecodedRecord,
    path: &crate::JsonPath,
    representation: &dyn TextRepresentation,
    rendered: &mut String,
    content: &mut String,
) -> Result<()> {
    record.visit_path(path, &mut |value| {
        let ScalarValue::Text(source) = value else {
            return Ok(());
        };
        rendered.clear();
        let Some(prepared) = representation.render(source, rendered)? else {
            return Ok(());
        };
        content.push_str(prepared);
        content.push('\u{1f}');
        if !prepared.is_ascii() {
            content.extend(prepared.case_fold());
            content.push('\u{1f}');
        }
        Ok(())
    })?;
    Ok(())
}


impl Repository {
    /// Explicitly build a reusable bigram posting over one registered text
    /// representation and logical path. Source bytes and rendered text are
    /// discarded after each record; partial builds are never published.
    pub fn build_representation_index(
        &mut self,
        collection: &str,
        representation: &str,
        path: &str,
    ) -> Result<usize> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return Err(Error::Request(
                "representation indexes require memory cache and non-hash freshness".into(),
            ));
        }
        let selected_collection = self.model.collection(collection)?.clone();
        let extensions = self
            .extensions
            .as_ref()
            .ok_or_else(|| Error::Request("no text representations are registered".into()))?;
        let definition = extensions.representation_definition(representation)?;
        if !definition.descriptor().cacheable() {
            return Err(Error::Request(format!(
                "volatile representation {representation} cannot be indexed"
            )));
        }
        let parsed = JsonFilter::parse_path(path)?;
        let canonical_path = binding_path(&parsed);
        if !definition.binds(&BindingContext {
            collection,
            path: &canonical_path,
            format: field_format(&selected_collection, &parsed),
            schema: extensions.schema(collection).map(Arc::as_ref),
        }) {
            return Err(Error::Request(format!(
                "representation {representation} does not bind to {collection}.{path}"
            )));
        }
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
                "representation index source table exceeds 2^32 slots".into(),
            ));
        }
        let selection = extensions
            .selection_dependencies(&selected_collection, selected.as_ref())?
            .0;
        let encoding = selected_collection
            .bigram_encoding
            .unwrap_or(self.options.bigram_encoding);
        let key = RepresentationIndexKey::new(
            &selected_collection,
            path,
            representation,
            &selection,
            extensions,
            encoding,
        );
        if self.extension_text_indexes.contains_key(&key) {
            return Ok(selected.len());
        }
        let main = BigramIndexBuilder::new_with_encoding(selected.len(), encoding);
        let skip = BigramIndexBuilder::new_with_encoding(selected.len(), encoding);
        let mut bytes = Vec::new();
        let mut rendered = String::new();
        let mut content = String::new();
        let mut observed_bigrams = vec![0_u64; 1024];
        let mut malformed_slots = Vec::new();
        for (slot, source) in selected.iter().enumerate() {
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
            content.clear();
            self.metrics.parses += 1;
            let outcome =
                crate::extensions::format::decode_checked(decoder, &bytes, &mut |record| {
                    add_text(
                        record,
                        &parsed,
                        definition.as_ref(),
                        &mut rendered,
                        &mut content,
                    )
                })?;
            if matches!(outcome, DecodeOutcome::Malformed(_)) {
                malformed_slots.push(slot as u32);
                continue;
            }
            observe_bigrams(&mut observed_bigrams, content.as_bytes(), encoding);
            main.add_file_content(&skip, slot, content.as_bytes());
        }
        if self.options.freshness != Freshness::MetadataOnOpen {
            let latest = self.source_descriptors(collection)?;
            self.metrics.enumerations += 1;
            if latest != *selected {
                return Err(Error::Unstable(selected_collection.path));
            }
        }
        let mut filter = main.compress(None);
        filter.set_skip_index(skip.compress(Some(SKIP_INDEX_MIN_DENSITY_PCT)));
        self.extension_text_indexes.insert(
            key,
            RepresentationIndex {
                filter,
                observed_bigrams,
                base_to_current: None,
                overlay_slots: Vec::new(),
                malformed_slots,
                slots: selected.len(),
                base_source_fingerprints: selected.iter().map(base_source_fingerprint).collect(),
            },
        );
        Ok(selected.len())
    }

    pub(super) fn representation_index_candidates(
        &self,
        key: &RepresentationIndexKey,
        literal: &str,
    ) -> Option<Vec<u64>> {
        if self.options.cache != CacheMode::Memory
            || self.options.freshness == Freshness::HashOnRequest
        {
            return None;
        }
        let posting = self.extension_text_indexes.get(key)?;
        let base = posting.filter.query(literal.as_bytes()).or_else(|| {
            let encoding = posting.filter.encoding();
            let definitely_absent = literal.as_bytes().windows(2).any(|pair| {
                if !encoding.usable(pair[0]) || !encoding.usable(pair[1]) {
                    return false;
                }
                let key = ((pair[0].to_ascii_lowercase() as usize) << 8)
                    | pair[1].to_ascii_lowercase() as usize;
                posting.observed_bigrams[key / 64] & (1 << (key % 64)) == 0
            });
            definitely_absent.then(|| vec![0; posting.filter.file_count().div_ceil(64)])
        })?;
        if posting.base_to_current.is_none() && posting.overlay_slots.is_empty() {
            let mut base = base;
            for &slot in &posting.malformed_slots {
                base[slot as usize / 64] |= 1 << (slot % 64);
            }
            return Some(base);
        }
        let mut current = vec![0_u64; posting.slots.div_ceil(64)];
        for (word_index, &value) in base.iter().enumerate() {
            let mut word = value;
            while word != 0 {
                let base_slot = word_index * 64 + word.trailing_zeros() as usize;
                let mapped = posting.base_to_current.as_ref().map_or_else(
                    || u32::try_from(base_slot).ok(),
                    |map| map.get(base_slot).copied().flatten(),
                );
                if let Some(slot) = mapped.filter(|slot| (*slot as usize) < posting.slots) {
                    current[slot as usize / 64] |= 1 << (slot % 64);
                }
                word &= word - 1;
            }
        }
        for &slot in &posting.overlay_slots {
            current[slot as usize / 64] |= 1 << (slot % 64);
        }
        for &slot in &posting.malformed_slots {
            current[slot as usize / 64] |= 1 << (slot % 64);
        }
        Some(current)
    }

    pub(super) fn invalidate_representation_indexes(&mut self, collection: &str) {
        self.extension_text_indexes
            .retain(|key, _| key.collection != collection);
        self.selected_derived_indexes
            .retain(|key, _| key.collection != collection);
    }
}
