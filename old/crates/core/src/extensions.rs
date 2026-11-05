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
pub mod text_search;

pub use format::{
    DecodeOutcome, DecodedRecord, FormatDecoder, FormatDiagnostic, LogicalNode, ScalarValue,
};
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
pub use text_search::{
    TextComparison, TextCompileRequest, TextSearchDefinition, TextSearchOptions, TextSyntax,
};

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
    text_definitions: BTreeMap<String, TextSearchDefinition>,
    text_aliases: BTreeMap<String, String>,
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

    pub(crate) fn extractor_identities(&self) -> impl Iterator<Item = &String> {
        self.extractors.keys()
    }

    pub(crate) fn ensure_source_representations(&mut self) -> Result<()> {
        if self.sources.is_empty() {
            return Ok(());
        }
        if !self.representations.contains_key("objekte:stored") {
            self.register_representation(StoredRepresentation::default())?;
        }
        for format in [
            crate::TextFormat::Plain,
            crate::TextFormat::Markdown,
            crate::TextFormat::Html,
            crate::TextFormat::Xml,
        ] {
            let representation = RenderedRepresentation::new(format)?;
            if !self
                .representations
                .contains_key(&representation.descriptor().identity)
            {
                self.register_representation(representation)?;
            }
        }
        Ok(())
    }

    /// Built-ins use the same registry and graph validation as application
    /// definitions. They are installed after discovery so the default native
    /// repository does not acquire a second schema load at open.
    pub(crate) fn ensure_builtin_text_definitions(&mut self) -> Result<()> {
        if !self.representations.contains_key("objekte:stored") {
            self.register_representation(StoredRepresentation::default())?;
        }
        for format in [
            crate::TextFormat::Plain,
            crate::TextFormat::Markdown,
            crate::TextFormat::Html,
            crate::TextFormat::Xml,
        ] {
            let representation = RenderedRepresentation::new(format)?;
            if !self
                .representations
                .contains_key(&representation.descriptor().identity)
            {
                self.register_representation(representation)?;
            }
        }
        for mode in [
            crate::TextMode::Word,
            crate::TextMode::WordRaw,
            crate::TextMode::Regex,
        ] {
            let operator = NativeTextOperator::new(mode, false);
            if !self.operators.contains_key(&operator.descriptor().identity) {
                self.register_operator(operator)?;
            }
        }
        for (name, representation, operator) in [
            ("plain", "objekte:visible-plain", "objekte:word"),
            ("markdown", "objekte:visible-markdown", "objekte:word"),
            ("html", "objekte:visible-html", "objekte:word"),
            ("xml", "objekte:visible-xml", "objekte:word"),
            ("raw-text", "objekte:stored", "objekte:word-raw"),
        ] {
            let identity = format!("objekte:{name}");
            if self.text_definitions.contains_key(&identity) {
                continue;
            }
            let definition = TextSearchDefinition::new(
                Descriptor::new(
                    &identity,
                    SemanticVersion {
                        major: 1,
                        minor: 0,
                        patch: 0,
                    },
                    &(representation, operator),
                    DataKind::Text,
                    DataKind::Predicate,
                    vec![
                        Dependency::SourceBytes,
                        Dependency::Schema("x-text-search".into()),
                    ],
                ),
                representation,
            )
            .alias(if name == "raw-text" { "raw" } else { name })
            .operator(TextComparison::Contains, operator)
            .operator(TextComparison::Equal, operator)
            .regex_operator("objekte:regex");
            self.register_text_definition_inner(definition, true)?;
        }
        Ok(())
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
            && self.text_definitions.is_empty()
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
        values.extend(
            self.text_definitions
                .values()
                .map(TextSearchDefinition::descriptor),
        );
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
            definition_identity: None,
            index_path: None,
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

    /// Register a composed text definition. Components may be registered in
    /// any order; `validate_text_definitions` checks the graph before open.
    pub fn register_text_definition(&mut self, definition: TextSearchDefinition) -> Result<()> {
        self.register_text_definition_inner(definition, false)
    }

    fn register_text_definition_inner(
        &mut self,
        definition: TextSearchDefinition,
        builtin: bool,
    ) -> Result<()> {
        if !builtin && definition.descriptor().identity.starts_with("objekte:") {
            return Err(Error::Request(format!(
                "text definition {} uses the reserved objekte namespace",
                definition.descriptor().identity
            )));
        }
        self.check(definition.descriptor(), DataKind::Text, DataKind::Predicate)?;
        let mut aliases = std::collections::BTreeSet::new();
        for alias in definition.aliases() {
            let valid = alias.bytes().enumerate().all(|(index, byte)| {
                byte.is_ascii_alphanumeric() && (index > 0 || byte.is_ascii_alphabetic())
                    || index > 0 && byte == b'_'
            });
            if !valid
                || alias.is_empty()
                || matches!(alias.as_str(), "length" | "each" | "lower" | "fts" | "text")
            {
                return Err(Error::Request(format!(
                    "invalid or reserved text alias {alias}"
                )));
            }
            if !aliases.insert(alias)
                || self.text_aliases.contains_key(alias)
                || self.text_definitions.contains_key(alias)
            {
                return Err(Error::Request(format!("duplicate text alias {alias}")));
            }
        }
        let identity = definition.descriptor().identity.clone();
        for alias in definition.aliases() {
            self.text_aliases.insert(alias.clone(), identity.clone());
        }
        self.text_definitions.insert(identity, definition);
        Ok(())
    }

    pub(crate) fn text_definition(&self, name: &str) -> Result<&TextSearchDefinition> {
        let identity = self.text_aliases.get(name).map_or(name, String::as_str);
        self.text_definitions
            .get(identity)
            .ok_or_else(|| Error::Request(format!("unknown text definition {name}")))
    }

    /// Resolve an explicit query name or the field's configured default.
    /// Index availability is deliberately absent from this semantic choice.
    pub(crate) fn resolve_text_definition<'a>(
        &'a self,
        field: Option<&crate::Field>,
        explicit: Option<&str>,
    ) -> Result<&'a TextSearchDefinition> {
        if let Some(field) = field
            && field.kind != crate::ValueType::String
            && !(field.kind == crate::ValueType::Array
                && field
                    .items
                    .as_deref()
                    .is_some_and(|item| item.kind == crate::ValueType::String))
        {
            return Err(Error::Request(
                "text definition requires a string field".into(),
            ));
        }
        if let Some(explicit) = explicit {
            return self.text_definition(explicit);
        }
        let name = if let Some(config) = field.and_then(|field| field.text_search.as_ref()) {
            if let Some(default) = config.default.as_deref() {
                default
            } else if config.definitions.len() == 1 {
                config.definitions.keys().next().expect("one definition")
            } else {
                return Err(Error::Request(
                    "text field has multiple definitions and no default".into(),
                ));
            }
        } else {
            match field.and_then(|field| field.text_format) {
                Some(crate::TextFormat::Markdown) => "objekte:markdown",
                Some(crate::TextFormat::Html) => "objekte:html",
                Some(crate::TextFormat::Xml) => "objekte:xml",
                _ => "objekte:plain",
            }
        };
        self.text_definition(name)
    }

    /// Bind and compile one complete text interpretation for a field.
    pub(crate) fn bind_text_definition(
        &self,
        collection: &crate::config::Collection,
        path: &str,
        explicit: Option<&str>,
        request: &TextCompileRequest,
    ) -> Result<BoundTextQuery> {
        let parsed = crate::text::parse_text_path(path)?;
        let field = crate::schema::field_at_path(&collection.fields, &parsed);
        let definition = self.resolve_text_definition(field, explicit)?;
        let representation = self.representation_definition(definition.representation())?;
        let legacy_format = field
            .and_then(|field| field.text_format)
            .unwrap_or(crate::TextFormat::Plain);
        let format = match legacy_format {
            crate::TextFormat::Plain => "plain",
            crate::TextFormat::Markdown => "markdown",
            crate::TextFormat::Html => "html",
            crate::TextFormat::Xml => "xml",
        };
        if !representation.binds_explicit(&BindingContext {
            collection: &collection.name,
            path,
            format,
            schema: self.schema(&collection.name).map(Arc::as_ref),
        }) {
            return Err(Error::Request(format!(
                "text definition {} cannot bind to {}.{path}",
                definition.descriptor().identity,
                collection.name,
            )));
        }
        let operator_name = definition.operator_for(request).ok_or_else(|| {
            Error::Request(format!(
                "text definition {} does not support this comparison/syntax",
                definition.descriptor().identity,
            ))
        })?;
        let operator = self
            .operators
            .get(operator_name)
            .expect("validated definition graph");
        let compiled = operator.compile_text(request)?;
        let candidate = if request.syntax == TextSyntax::Regex
            && operator.descriptor().identity == "objekte:regex"
        {
            crate::file_picker::regex_plan::mandatory_ascii_literal(&request.pattern).map_or(
                CandidateConstraint::Unsupported,
                |literal| CandidateConstraint::BigramLiteral {
                    literal,
                    representation: representation.descriptor().identity.clone(),
                },
            )
        } else {
            compiled.candidate()
        };
        let identity = crate::config::fingerprint(
            "bound-text-definition/v1",
            &(
                definition.descriptor(),
                representation.descriptor(),
                operator.descriptor(),
                path,
                request,
            ),
        );
        Ok(BoundTextQuery {
            definition_identity: Some(definition.descriptor().identity.clone()),
            index_path: Some(crate::json_filter::format_path(&parsed)),
            preparation: representation.preparation(),
            native_index_projection: representation.native_index_projection(),
            representation,
            operator: Arc::from(compiled),
            identity,
            candidate,
            cacheable: definition.descriptor().cacheable()
                && operator.descriptor().cacheable()
                && self
                    .representation_descriptor(definition.representation())
                    .cacheable(),
        })
    }

    pub(crate) fn validate_text_definitions(&self) -> Result<()> {
        for definition in self.text_definitions.values() {
            let representation = self.representation_definition(definition.representation())?;
            if representation.descriptor().input != DataKind::Text
                || representation.descriptor().output != DataKind::Text
            {
                return Err(Error::Request(format!(
                    "text definition {} has an incompatible representation",
                    definition.descriptor().identity
                )));
            }
            if !definition.operators.contains_key(&TextComparison::Contains) {
                return Err(Error::Request(format!(
                    "text definition {} needs a contains operator",
                    definition.descriptor().identity
                )));
            }
            for operator in definition
                .operators
                .values()
                .chain(definition.regex_operator.iter())
            {
                if self
                    .operators
                    .get(operator)
                    .is_none_or(|item| item.descriptor().input != DataKind::Text)
                {
                    return Err(Error::Request(format!(
                        "text definition {} needs registered text operator {operator}",
                        definition.descriptor().identity
                    )));
                }
            }
        }
        Ok(())
    }

    pub(crate) fn validate_text_fields(&self, model: &RepositoryModel) -> Result<()> {
        for collection in model.collections.values() {
            for path in crate::schema::text_search_paths(&collection.fields) {
                let parsed = crate::JsonFilter::parse_path(&path)?;
                let field = crate::schema::field_at_path(&collection.fields, &parsed)
                    .expect("configured text path has a field");
                let config = field.text_search.as_ref().expect("configured text field");
                let mut resolved = std::collections::BTreeSet::new();
                for (name, options) in &config.definitions {
                    let definition = self.text_definition(name)?;
                    if !resolved.insert(&definition.descriptor().identity) {
                        return Err(Error::Request(format!(
                            "duplicate text definition on {}.{path}: {name}",
                            collection.name,
                        )));
                    }
                    if options.index && !definition.descriptor().cacheable() {
                        return Err(Error::Request(format!(
                            "volatile text definition {name} cannot be indexed",
                        )));
                    }
                    let representation =
                        self.representation_definition(definition.representation())?;
                    if options.index && !representation.descriptor().cacheable() {
                        return Err(Error::Request(format!(
                            "volatile text representation {} cannot be indexed",
                            definition.representation(),
                        )));
                    }
                    if !representation.binds_explicit(&BindingContext {
                        collection: &collection.name,
                        path: &path,
                        format: "plain",
                        schema: self.schema(&collection.name).map(Arc::as_ref),
                    }) {
                        return Err(Error::Request(format!(
                            "text definition {name} cannot bind to {}.{path}",
                            collection.name,
                        )));
                    }
                }
                if let Some(default) = &config.default {
                    let default = self.text_definition(default)?;
                    if !resolved.contains(&default.descriptor().identity) {
                        return Err(Error::Request(format!(
                            "text default on {}.{path} is not declared",
                            collection.name,
                        )));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn register_frontend(&mut self, item: impl QueryFrontend + 'static) -> Result<()> {
        self.check(item.descriptor(), DataKind::Query, DataKind::Query)?;
        self.frontends
            .insert(item.descriptor().identity.clone(), Arc::new(item));
        Ok(())
    }
}
