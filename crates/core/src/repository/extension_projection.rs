//! Fused explicit projection build over the selected provider source table.
//! A source is read and decoded once for every requested text/typed output.

use super::*;
use crate::{
    JsonFilter,
    bigram::{BigramIndexBuilder, SKIP_INDEX_MIN_DENSITY_PCT},
    extensions::{BindingContext, BoundValueExtractor, Extraction, TextRepresentation, TypedValue},
    file_picker::TypedKey,
    repository::{
        extension_derived_index::{SelectedDerivedKey, SelectedDerivedPosting},
        extension_index::{
            RepresentationIndex, RepresentationIndexKey, add_text, binding_path, field_format,
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
}


struct TextBuild {
    key: RepresentationIndexKey,
    path: JsonPath,
    definition: Arc<dyn TextRepresentation>,
    main: BigramIndexBuilder,
    skip: BigramIndexBuilder,
    rendered: String,
    content: String,
}


struct DerivedBuild {
    key: SelectedDerivedKey,
    binding: BoundValueExtractor,
    rows: Vec<(TypedKey, usize)>,
    present: Vec<u64>,
    scalar_safe: bool,
    output: Vec<TypedValue>,
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
        if requests.is_empty() {
            return Err(Error::Request(
                "selected index request list is empty".into(),
            ));
        }
        if requests.len() > 8 {
            return Err(Error::Request(
                "selected index build accepts at most eight outputs".into(),
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
        let selection = crate::config::fingerprint("selected-sources/v1", selected.as_ref());
        let encoding = selected_collection
            .bigram_encoding
            .unwrap_or(self.options.bigram_encoding);
        let mut text_builds = Vec::new();
        let mut derived_builds = Vec::new();
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
                    let parsed = JsonFilter::parse_path(path)?;
                    if !definition.binds(&BindingContext {
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
                    text_builds.push(TextBuild {
                        key,
                        path: parsed,
                        definition,
                        main: BigramIndexBuilder::new_with_encoding(selected.len(), encoding),
                        skip: BigramIndexBuilder::new_with_encoding(selected.len(), encoding),
                        rendered: String::new(),
                        content: String::new(),
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
                    derived_builds.push(DerivedBuild {
                        key,
                        binding,
                        rows: Vec::new(),
                        present: vec![0; selected.len().div_ceil(64)],
                        scalar_safe: true,
                        output: Vec::new(),
                    });
                }
            }
        }
        if text_builds.is_empty() && derived_builds.is_empty() {
            return Ok(selected.len());
        }
        let mut bytes = Vec::new();
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
            for build in &mut text_builds {
                build.content.clear();
            }
            let mut records = 0;
            self.metrics.parses += 1;
            decoder.decode(&bytes, &mut |record| {
                records += 1;
                if records > 1 {
                    return Err(Error::Request(
                        "format decoder emitted multiple records".into(),
                    ));
                }
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
                Ok(())
            })?;
            if records == 0 {
                return Err(Error::Request("format decoder emitted no record".into()));
            }
            for build in &text_builds {
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
            self.extension_text_indexes
                .insert(build.key, RepresentationIndex { filter });
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
        Ok(selected.len())
    }
}
