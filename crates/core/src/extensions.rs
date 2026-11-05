//! Statically linked extension definitions and their stable semantic identities.
//!
//! Registration happens before a repository is opened. A descriptor identifies
//! executable semantics across process restarts; a pointer or registration
//! position never does. The engine retains ownership of indexes and caches.

use crate::{Error, JsonPath, JsonPathStep, RepositoryModel, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::{
    collections::BTreeMap,
    fs,
    path::PathBuf,
    sync::{Arc, LazyLock},
};

pub mod format;
pub mod identity;
pub mod operator;
pub mod projection;
pub mod query;
pub mod source;

pub use format::{DecodedRecord, FormatDecoder, ScalarValue};
pub use identity::{DataKind, Dependency, Descriptor, SemanticVersion};
pub(crate) use operator::BoundTextQuery;
pub use operator::{CandidateConstraint, CompiledOperator, QueryOperator};
pub use projection::{
    BindingContext, Extraction, NativeTextProjection, RecordAccess, TextPreparation,
    TextRepresentation, TypedValue, ValueExtractor,
};
pub(crate) use projection::{BoundValueExtractor, DERIVED_PATH_PREFIX, is_derived_path};
pub use query::{ExtensionBindings, LogicalPlan, LogicalPredicate, QueryFrontend};
pub use source::{SourceDescriptor, SourceProvider};

mod builtins;
pub use builtins::{
    JsonFormatDecoder, NativeTextOperator, RenderedRepresentation, StoredRepresentation,
};

static JSON_DECODER: LazyLock<JsonFormatDecoder> = LazyLock::new(JsonFormatDecoder::default);

pub(crate) fn json_decoder() -> &'static JsonFormatDecoder {
    &JSON_DECODER
}

#[cfg(test)]
#[path = "extensions.test.rs"]
mod tests;


/// Registrations are sorted by namespaced identity. Cloning keeps definitions
/// immutable and shared; no registration lookup is required per default record.
#[derive(Clone, Default)]
pub struct Extensions {
    sources: BTreeMap<String, Arc<dyn SourceProvider>>,
    formats: BTreeMap<String, Arc<dyn FormatDecoder>>,
    representations: BTreeMap<String, Arc<dyn TextRepresentation>>,
    extractors: BTreeMap<String, Arc<dyn ValueExtractor>>,
    operators: BTreeMap<String, Arc<dyn QueryOperator>>,
    frontends: BTreeMap<String, Arc<dyn QueryFrontend>>,
    schemas: BTreeMap<String, Arc<Value>>,
}


impl Extensions {
    pub(crate) fn frontend(&self, identity: &str) -> Result<&dyn QueryFrontend> {
        self.frontends
            .get(identity)
            .map(|frontend| frontend.as_ref())
            .ok_or_else(|| Error::Request(format!("unknown query frontend {identity}")))
    }

    pub(crate) fn text_handle_identity(&self, handle: u32, operator: bool) -> Result<&str> {
        let identity = if operator {
            self.operators.keys().nth(handle as usize)
        } else {
            self.representations.keys().nth(handle as usize)
        };
        identity.map(String::as_str).ok_or_else(|| {
            Error::Request(format!(
                "invalid {} handle {handle}",
                if operator {
                    "operator"
                } else {
                    "representation"
                }
            ))
        })
    }

    pub(crate) fn extractor_handle_identity(&self, handle: u32) -> Result<&str> {
        self.extractors
            .keys()
            .nth(handle as usize)
            .map(String::as_str)
            .ok_or_else(|| Error::Request(format!("invalid extractor handle {handle}")))
    }

    pub(crate) fn source_providers(&self) -> impl Iterator<Item = &Arc<dyn SourceProvider>> {
        self.sources.values()
    }

    pub(crate) fn format_identities(&self) -> impl Iterator<Item = &String> {
        self.formats.keys()
    }

    pub(crate) fn decoder(&self, source: &SourceDescriptor) -> Result<&dyn FormatDecoder> {
        let mut accepted = self
            .formats
            .values()
            .filter(|format| format.accepts(source));
        let first = accepted.next();
        if accepted.next().is_some() {
            return Err(Error::Request(format!(
                "ambiguous format selection for source {}",
                source.source_id
            )));
        }
        match first {
            Some(format) if format.descriptor().identity == source.format => Ok(format.as_ref()),
            None if source.format == "objekte:json" => Ok(json_decoder()),
            Some(_) | None => Err(Error::Request(format!(
                "no registered decoder for source {} with format {}",
                source.source_id, source.format
            ))),
        }
    }

    pub(crate) fn bind_schemas(&mut self, model: &RepositoryModel) -> Result<()> {
        self.schemas.clear();
        for (name, collection) in &model.collections {
            let (Some(path), Some(expected)) = (
                collection.schema_path.as_ref(),
                collection.schema_hash.as_ref(),
            ) else {
                continue;
            };
            let bytes = fs::read(path).map_err(|source| crate::error::io(path, source))?;
            let raw: Value = serde_json::from_slice(&bytes)
                .map_err(|error| crate::error::config(path, "", error))?;
            if crate::config::fingerprint("schema/v1", &raw) != *expected {
                return Err(Error::Unstable(path.clone()));
            }
            self.schemas.insert(name.clone(), Arc::new(raw));
        }
        Ok(())
    }

    pub(crate) fn schema(&self, collection: &str) -> Option<&Arc<Value>> {
        self.schemas.get(collection)
    }

    pub(crate) fn representation_definition(
        &self,
        identity: &str,
    ) -> Result<Arc<dyn TextRepresentation>> {
        self.representations
            .get(identity)
            .cloned()
            .ok_or_else(|| Error::Request(format!("unknown text representation {identity}")))
    }

    pub(crate) fn schema_annotations(&self) -> Vec<String> {
        self.descriptors()
            .into_iter()
            .flat_map(|descriptor| &descriptor.dependencies)
            .filter_map(|dependency| match dependency {
                Dependency::Schema(keyword) => Some(keyword.clone()),
                _ => None,
            })
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.sources.is_empty()
            && self.formats.is_empty()
            && self.representations.is_empty()
            && self.extractors.is_empty()
            && self.operators.is_empty()
            && self.frontends.is_empty()
    }

    pub fn bindings(&self) -> ExtensionBindings<'_> {
        ExtensionBindings { registry: self }
    }

    pub fn descriptors(&self) -> Vec<&Descriptor> {
        let mut values = Vec::new();
        values.extend(self.sources.values().map(|item| item.descriptor()));
        values.extend(self.formats.values().map(|item| item.descriptor()));
        values.extend(self.representations.values().map(|item| item.descriptor()));
        values.extend(self.extractors.values().map(|item| item.descriptor()));
        values.extend(self.operators.values().map(|item| item.descriptor()));
        values.extend(self.frontends.values().map(|item| item.descriptor()));
        values.sort_by(|a, b| a.identity.cmp(&b.identity));
        values
    }

    pub fn fingerprint(&self) -> String {
        crate::config::fingerprint("library-extensions/v1", &self.descriptors())
    }

    /// Source rules are dependencies even when an applicable provider currently
    /// returns no records. Decoder definitions are retained only for formats
    /// actually selected by this inventory.
    pub(crate) fn selection_dependencies(
        &self,
        collection: &crate::config::Collection,
        sources: &[SourceDescriptor],
    ) -> Result<(String, bool, Vec<Descriptor>)> {
        let filesystem = crate::source::FilesystemJsonProvider::new(
            &collection.path,
            collection.recursive,
            &collection.name,
        );
        let mut definitions = BTreeMap::new();
        definitions.insert(
            filesystem.descriptor().identity.clone(),
            filesystem.descriptor().clone(),
        );
        definitions.insert(
            json_decoder().descriptor().identity.clone(),
            json_decoder().descriptor().clone(),
        );
        let mut custom_selection = false;
        for provider in self.source_providers() {
            if provider.may_select(&collection.name) {
                custom_selection = true;
                definitions.insert(
                    provider.descriptor().identity.clone(),
                    provider.descriptor().clone(),
                );
            }
        }
        // A decoder may select by locator or metadata, and the format trait
        // does not expose an invariant applicability predicate. When a custom
        // provider can select this collection, retain decoder precedence even
        // if that provider currently contributes no records.
        if custom_selection {
            for decoder in self.formats.values() {
                definitions.insert(
                    decoder.descriptor().identity.clone(),
                    decoder.descriptor().clone(),
                );
            }
        }
        for source in sources {
            let descriptor = self.decoder(source)?.descriptor();
            definitions.insert(descriptor.identity.clone(), descriptor.clone());
        }
        let definitions = definitions.into_values().collect::<Vec<_>>();
        let cacheable = definitions.iter().all(Descriptor::cacheable);
        let fingerprint =
            crate::config::fingerprint("selected-source-dependencies/v2", &(sources, &definitions));
        Ok((fingerprint, cacheable, definitions))
    }

    pub(crate) fn representation_descriptor(&self, identity: &str) -> &Descriptor {
        self.representations[identity].descriptor()
    }

    pub(crate) fn bind_text(
        &self,
        representation: &str,
        operator: &str,
        parameters: &Value,
    ) -> Result<BoundTextQuery> {
        let representation = self.representations.get(representation).ok_or_else(|| {
            Error::Request(format!("unknown text representation {representation}"))
        })?;
        let operator = self
            .operators
            .get(operator)
            .ok_or_else(|| Error::Request(format!("unknown query operator {operator}")))?;
        if operator.descriptor().input != DataKind::Text {
            return Err(Error::Request(format!(
                "operator {} does not accept text",
                operator.descriptor().identity
            )));
        }
        let compiled = operator.compile(parameters)?;
        let candidate = compiled.candidate();
        let identity = crate::config::fingerprint(
            "bound-text-query/v1",
            &(
                representation.descriptor(),
                operator.descriptor(),
                parameters,
            ),
        );
        Ok(BoundTextQuery {
            preparation: representation.preparation(),
            native_index_projection: representation.native_index_projection(),
            representation: Arc::clone(representation),
            operator: Arc::from(compiled),
            identity,
            candidate,
            cacheable: representation.descriptor().cacheable() && operator.descriptor().cacheable(),
        })
    }

    pub(crate) fn bind_value(
        &self,
        identity: &str,
        collection: &str,
        schema: Option<&Value>,
    ) -> Result<BoundValueExtractor> {
        let extractor = self
            .extractors
            .get(identity)
            .ok_or_else(|| Error::Request(format!("unknown value extractor {identity}")))?;
        let path = format!("@derived:{identity}");
        if !extractor.binds(&BindingContext {
            collection,
            path: &path,
            format: "derived",
            schema,
        }) {
            return Err(Error::Request(format!(
                "value extractor {identity} is not applicable to collection {collection}"
            )));
        }
        let descriptor = extractor.descriptor();
        let fingerprint = crate::config::fingerprint(
            "bound-value-extractor/v1",
            &(descriptor, collection, schema),
        );
        // The sidecar persists complete descriptor/configuration bytes in its
        // path map. Registration order and process addresses never enter it.
        let stored_identity = serde_json::to_string(&(descriptor, collection, &fingerprint))
            .expect("descriptor identity serializes");
        Ok(BoundValueExtractor {
            extractor: Arc::clone(extractor),
            path: vec![
                JsonPathStep::Key(DERIVED_PATH_PREFIX.into()),
                JsonPathStep::Key(stored_identity),
            ],
            identity: identity.into(),
            cacheable: descriptor.cacheable(),
        })
    }

    fn check(&self, descriptor: &Descriptor, input: DataKind, output: DataKind) -> Result<()> {
        descriptor.validate(input, output)?;
        if let Some(previous) = self
            .descriptors()
            .into_iter()
            .find(|previous| previous.identity == descriptor.identity)
        {
            return Err(Error::Request(format!(
                "duplicate extension {} (existing version {}.{}.{}, new version {}.{}.{})",
                descriptor.identity,
                previous.version.major,
                previous.version.minor,
                previous.version.patch,
                descriptor.version.major,
                descriptor.version.minor,
                descriptor.version.patch
            )));
        }
        Ok(())
    }

    pub fn register_source(&mut self, item: impl SourceProvider + 'static) -> Result<()> {
        if item.descriptor().identity == crate::source::FILESYSTEM_JSON_IDENTITY {
            return Err(Error::Request(
                "source provider identity objekte:filesystem is reserved for built-in JSON".into(),
            ));
        }
        self.check(item.descriptor(), DataKind::Source, DataKind::Source)?;
        self.sources
            .insert(item.descriptor().identity.clone(), Arc::new(item));
        Ok(())
    }

    pub fn register_format(&mut self, item: impl FormatDecoder + 'static) -> Result<()> {
        if item.descriptor().identity == "objekte:json" {
            return Err(Error::Request(
                "format identity objekte:json is reserved for the built-in JSON decoder".into(),
            ));
        }
        self.check(item.descriptor(), DataKind::Source, DataKind::Record)?;
        self.formats
            .insert(item.descriptor().identity.clone(), Arc::new(item));
        Ok(())
    }

    pub fn register_representation(
        &mut self,
        item: impl TextRepresentation + 'static,
    ) -> Result<()> {
        self.check(item.descriptor(), DataKind::Text, DataKind::Text)?;
        self.representations
            .insert(item.descriptor().identity.clone(), Arc::new(item));
        Ok(())
    }

    pub fn register_extractor(&mut self, item: impl ValueExtractor + 'static) -> Result<()> {
        self.check(item.descriptor(), DataKind::Record, DataKind::TypedValues)?;
        self.extractors
            .insert(item.descriptor().identity.clone(), Arc::new(item));
        Ok(())
    }

    pub fn register_operator(&mut self, item: impl QueryOperator + 'static) -> Result<()> {
        let input = item.descriptor().input;
        if !matches!(input, DataKind::Text | DataKind::TypedValues) {
            return Err(Error::Request(format!(
                "operator {} must consume text or typed values",
                item.descriptor().identity
            )));
        }
        self.check(item.descriptor(), input, DataKind::Predicate)?;
        self.operators
            .insert(item.descriptor().identity.clone(), Arc::new(item));
        Ok(())
    }

    pub fn register_frontend(&mut self, item: impl QueryFrontend + 'static) -> Result<()> {
        self.check(item.descriptor(), DataKind::Query, DataKind::Query)?;
        self.frontends
            .insert(item.descriptor().identity.clone(), Arc::new(item));
        Ok(())
    }
}
