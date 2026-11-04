//! Statically linked extension definitions and their stable semantic identities.
//!
//! Registration happens before a repository is opened. A descriptor identifies
//! executable semantics across process restarts; a pointer or registration
//! position never does. The engine retains ownership of indexes and caches.

use crate::{Error, JsonPath, JsonPathStep, RepositoryModel, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sonic_rs::{JsonValueTrait, Value as SonicValue, ValueRef};
use std::{collections::BTreeMap, fs, path::PathBuf, sync::Arc};

mod builtins;
pub use builtins::{
    JsonFormatDecoder, NativeTextOperator, RenderedRepresentation, StoredRepresentation,
};

#[cfg(test)]
#[path = "extensions.test.rs"]
mod tests;


/// The kind consumed or produced by an extension definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DataKind {
    Source,
    Record,
    Text,
    TypedValues,
    Predicate,
    Query,
}


/// Declared inputs that determine whether a saved result remains valid.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Dependency {
    SourceBytes,
    FileName,
    Metadata(String),
    Schema(String),
    Relation(String),
    /// A versioned input supplied by the application, including its version.
    ExternalVersion(String),
    /// Results may be used for this request only, never persisted or cached.
    Volatile,
}


/// Stable semantic version of one statically linked definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct SemanticVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}


/// Persistent identity. `configuration` is a canonical SHA-256 fingerprint.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Descriptor {
    pub identity: String,
    pub version: SemanticVersion,
    pub configuration: String,
    pub input: DataKind,
    pub output: DataKind,
    pub dependencies: Vec<Dependency>,
}


impl Descriptor {
    /// Hash serializable configuration with a fixed domain and field order.
    /// Maps should have stable key ordering, such as `BTreeMap` or JSON maps.
    pub fn new(
        identity: impl Into<String>,
        version: SemanticVersion,
        configuration: &impl Serialize,
        input: DataKind,
        output: DataKind,
        mut dependencies: Vec<Dependency>,
    ) -> Self {
        dependencies.sort();
        dependencies.dedup();
        Self {
            identity: identity.into(),
            version,
            configuration: crate::config::fingerprint("library-extension-config/v1", configuration),
            input,
            output,
            dependencies,
        }
    }

    pub fn cacheable(&self) -> bool {
        !self.dependencies.contains(&Dependency::Volatile)
    }

    fn validate(&self, input: DataKind, output: DataKind) -> Result<()> {
        let (namespace, name) = self.identity.split_once(':').unwrap_or(("", ""));
        if namespace.is_empty()
            || name.is_empty()
            || !self.identity.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b':' | b'-' | b'_' | b'.')
            })
        {
            return Err(Error::Request(format!(
                "extension identity {:?} must be namespaced as namespace:name",
                self.identity
            )));
        }
        if self.input != input || self.output != output {
            return Err(Error::Request(format!(
                "extension {} declares incompatible input/output kinds",
                self.identity
            )));
        }
        if self.configuration.len() != 64
            || !self
                .configuration
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err(Error::Request(format!(
                "extension {} has an invalid configuration fingerprint",
                self.identity
            )));
        }
        if self.dependencies.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(Error::Request(format!(
                "extension {} dependencies must be sorted and unique",
                self.identity
            )));
        }
        if self.dependencies.iter().any(|dependency| {
            matches!(dependency, Dependency::ExternalVersion(version) if version.is_empty())
        }) {
            return Err(Error::Request(format!(
                "extension {} has an unversioned external dependency",
                self.identity
            )));
        }
        if self.dependencies.iter().any(|dependency| {
            matches!(dependency, Dependency::Schema(keyword) if !keyword.starts_with("x-") || keyword.len() <= 2)
        }) {
            return Err(Error::Request(format!(
                "extension {} schema dependencies must name x- annotations",
                self.identity
            )));
        }
        Ok(())
    }
}


/// A source's stable identity is distinct from its locator and record ID.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SourceDescriptor {
    pub collection: String,
    pub provider: String,
    pub source_id: String,
    pub record_id: String,
    pub locator: PathBuf,
    pub format: String,
    pub revision: String,
    pub metadata: BTreeMap<String, String>,
}


/// Enumeration and bounded, revision-checked reads of selected sources.
/// Implementations own their secure-read policy; the engine owns inventories.
pub trait SourceProvider: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn enumerate(&self, collection: &str) -> Result<Vec<SourceDescriptor>>;
    fn read(&self, source: &SourceDescriptor, output: &mut Vec<u8>) -> Result<()>;
}


/// Borrowed logical scalar delivered during one decoder callback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScalarValue<'a> {
    Null,
    Bool(bool),
    Number(&'a str),
    Text(&'a str),
}


/// A decoded record lends both path presence and logical leaves for one call.
/// Implementations may borrow the source bytes or own a format-specific tree.
pub trait DecodedRecord: RecordAccess {
    /// Visit scalar leaves at JSON-pointer paths. Borrowed values may not be
    /// retained after the callback returns.
    fn visit_leaves(
        &self,
        visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
    ) -> Result<()>;
}


/// Decode one source and lend its logical record only during the callback.
pub trait FormatDecoder: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn accepts(&self, source: &SourceDescriptor) -> bool;
    fn decode(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&dyn DecodedRecord) -> Result<()>,
    ) -> Result<()>;

    fn visit_leaves(
        &self,
        bytes: &[u8],
        visit: &mut dyn FnMut(&str, ScalarValue<'_>) -> Result<()>,
    ) -> Result<()> {
        self.decode(bytes, &mut |record| record.visit_leaves(visit))
    }
}


/// Field-level binding inputs, resolved once from a schema and source rule.
pub struct BindingContext<'a> {
    pub collection: &'a str,
    pub path: &'a str,
    pub format: &'a str,
    pub schema: Option<&'a Value>,
}


/// A text view of one logical source value. The returned slice is borrowed
/// from `value` or reusable `output`; it cannot escape the current call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextPreparation {
    /// The value remains source text; an operator may prepare it for its mode.
    Stored,
    /// Visible text has already had markup, punctuation and spacing prepared.
    VisibleNormalized,
}


/// A promise that a representation is byte-for-byte equivalent to one of the
/// engine's existing text projections for candidate filtering. An incorrect
/// declaration can omit matches; implementations must verify the equivalence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NativeTextProjection {
    Stored,
    VisibleNormalized(crate::TextFormat),
}


/// Produce one logical text value, with its preparation contract stated once.
pub trait TextRepresentation: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn binds(&self, context: &BindingContext<'_>) -> bool;
    fn preparation(&self) -> TextPreparation {
        TextPreparation::Stored
    }

    /// Declare compatibility with an existing engine-owned index projection.
    /// The default keeps independent representations on exact scan fallback.
    fn native_index_projection(&self) -> Option<NativeTextProjection> {
        None
    }

    fn render<'a>(&self, value: &'a str, output: &'a mut String) -> Result<Option<&'a str>>;
}


/// Read-only access to logical leaves for a derived-value extractor.
pub trait RecordAccess {
    /// Whether the path reaches a value, including an empty array or object.
    fn path_present(&self, path: &JsonPath) -> Result<bool>;

    /// Visit scalar leaves in source order. A present empty container visits
    /// zero leaves; distinguish that case with `path_present`.
    fn visit_path(
        &self,
        path: &JsonPath,
        visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>,
    ) -> Result<usize>;
}


impl RecordAccess for Value {
    fn path_present(&self, path: &JsonPath) -> Result<bool> {
        Ok(!crate::json_filter::values(self, path, false).is_empty())
    }

    fn visit_path(
        &self,
        path: &JsonPath,
        visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>,
    ) -> Result<usize> {
        let mut count = 0;
        let mut error = None;
        crate::json_filter::visit_values(self, path, true, &mut |value| {
            let result = match value {
                Value::Null => visit(ScalarValue::Null),
                Value::Bool(value) => visit(ScalarValue::Bool(*value)),
                Value::Number(value) => {
                    // Sonic canonicalizes number spellings during the native
                    // projection. Use the same spelling for direct access.
                    sonic_rs::from_str::<SonicValue>(value.as_str())
                        .map_err(|_| Error::Request("invalid extracted JSON number".into()))
                        .and_then(|canonical| {
                            let number = canonical.as_raw_number().expect("parsed number");
                            visit(ScalarValue::Number(number.as_str()))
                        })
                }
                Value::String(value) => visit(ScalarValue::Text(value)),
                Value::Array(_) | Value::Object(_) => return true,
            };
            count += 1;
            if let Err(failure) = result {
                error = Some(failure);
                return false;
            }
            true
        });
        if let Some(error) = error {
            Err(error)
        } else {
            Ok(count)
        }
    }
}


impl RecordAccess for SonicValue {
    fn path_present(&self, path: &JsonPath) -> Result<bool> {
        Ok(!crate::json_filter::sonic_values(self, path, false).is_empty())
    }

    fn visit_path(
        &self,
        path: &JsonPath,
        visit: &mut dyn FnMut(ScalarValue<'_>) -> Result<()>,
    ) -> Result<usize> {
        let mut count = 0;
        for value in crate::json_filter::sonic_values(self, path, true) {
            if let Some(number) = value.as_raw_number() {
                // Match serde_json's arbitrary-precision spelling used by
                // direct verification before lending the temporary number.
                let canonical: serde_json::Number = number
                    .as_str()
                    .parse()
                    .map_err(|_| Error::Request("invalid extracted JSON number".into()))?;
                visit(ScalarValue::Number(canonical.as_str()))?;
                count += 1;
                continue;
            }
            let scalar = match value.as_ref() {
                ValueRef::Null => Some(ScalarValue::Null),
                ValueRef::Bool(value) => Some(ScalarValue::Bool(value)),
                ValueRef::String(value) => Some(ScalarValue::Text(value)),
                ValueRef::Number(_) => unreachable!("raw numbers enabled"),
                ValueRef::Array(_) | ValueRef::Object(_) => None,
            };
            if let Some(scalar) = scalar {
                visit(scalar)?;
                count += 1;
            }
        }
        Ok(count)
    }
}


/// Typed values preserve exact decimal spellings; `Missing` is represented by
/// `Extraction::Missing`, while a present null is `TypedValue::Null`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum TypedValue {
    Null,
    Bool(bool),
    Number(String),
    Text(String),
}


#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Extraction {
    Missing,
    Present,
}


/// Append zero or more values to a worker-reused buffer. Errors are distinct
/// from missing and from a present field with no emitted values.
pub trait ValueExtractor: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn binds(&self, context: &BindingContext<'_>) -> bool;
    fn extract(
        &self,
        record: &dyn RecordAccess,
        output: &mut Vec<TypedValue>,
    ) -> Result<Extraction>;
}


/// A necessary index condition. The engine supplies the existing postings.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CandidateConstraint {
    Unsupported,
    /// `representation` identifies the indexed text semantics, not merely a
    /// similar-looking rendering. An incompatible index must be ignored.
    BigramLiteral {
        literal: String,
        representation: String,
    },
    TypedRange {
        path: String,
        lower: Option<TypedValue>,
        upper: Option<TypedValue>,
    },
}


/// One compiled, immutable exact predicate. Mutable scratch belongs to the
/// worker; implementations must not retain the borrowed value or scratch.
pub trait CompiledOperator: Send + Sync {
    fn matches(&self, value: ScalarValue<'_>, scratch: &mut String) -> Result<bool>;
    /// Consume a representation that has already prepared visible text.
    fn matches_prepared(&self, value: ScalarValue<'_>, scratch: &mut String) -> Result<bool> {
        self.matches(value, scratch)
    }

    fn candidate(&self) -> CandidateConstraint {
        CandidateConstraint::Unsupported
    }
}


/// Compile parameters once, then evaluate with the bound representation.
pub trait QueryOperator: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn compile(&self, parameters: &Value) -> Result<Box<dyn CompiledOperator>>;
}


/// A frontend produces only a logical request; execution remains in core.
pub trait QueryFrontend: Send + Sync {
    fn descriptor(&self) -> &Descriptor;
    fn compile(&self, request: &Value, bindings: &ExtensionBindings<'_>) -> Result<LogicalPlan>;
}


/// Stable numeric handles are assigned after sorting identities at binding.
pub struct ExtensionBindings<'a> {
    registry: &'a Extensions,
}


impl ExtensionBindings<'_> {
    pub fn operator(&self, identity: &str) -> Option<u32> {
        self.registry
            .operators
            .keys()
            .position(|name| name == identity)
            .and_then(|position| u32::try_from(position).ok())
    }

    pub fn representation(&self, identity: &str) -> Option<u32> {
        self.registry
            .representations
            .keys()
            .position(|name| name == identity)
            .and_then(|position| u32::try_from(position).ok())
    }
}


/// Boolean plan whose calls refer to immutable, bound extension definitions.
#[derive(Clone, Debug)]
pub enum LogicalPredicate {
    Call {
        operator: u32,
        representation: u32,
        path: String,
        parameters: Value,
    },
    And(Vec<Self>),
    Or(Vec<Self>),
    Not(Box<Self>),
}


#[derive(Clone, Debug)]
pub struct LogicalPlan {
    pub collections: Vec<String>,
    pub predicate: LogicalPredicate,
    pub sort: Vec<crate::Sort>,
    pub columns: Vec<String>,
    pub offset: usize,
    pub limit: usize,
    pub count: bool,
}


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


pub(crate) struct BoundTextQuery {
    pub representation: Arc<dyn TextRepresentation>,
    pub preparation: TextPreparation,
    pub native_index_projection: Option<NativeTextProjection>,
    pub operator: Arc<dyn CompiledOperator>,
    pub identity: String,
    pub candidate: CandidateConstraint,
    pub cacheable: bool,
}


/// Internal posting paths cannot alias a stored JSON field in public queries.
pub(crate) const DERIVED_PATH_PREFIX: &str = "\0objekte:derived";

pub(crate) fn is_derived_path(path: &JsonPath) -> bool {
    matches!(path.first(), Some(JsonPathStep::Key(name)) if name == DERIVED_PATH_PREFIX)
}


#[derive(Clone)]
pub(crate) struct BoundValueExtractor {
    pub extractor: Arc<dyn ValueExtractor>,
    pub path: JsonPath,
    pub identity: String,
    pub cacheable: bool,
}


impl Extensions {
    pub(crate) fn source_providers(&self) -> impl Iterator<Item = &Arc<dyn SourceProvider>> {
        self.sources.values()
    }

    pub(crate) fn decoder(&self, source: &SourceDescriptor) -> Result<Arc<dyn FormatDecoder>> {
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
            Some(format) if format.descriptor().identity == source.format => Ok(Arc::clone(format)),
            None if source.format == "objekte:json" => Ok(Arc::new(JsonFormatDecoder::default())),
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
        self.check(item.descriptor(), DataKind::Source, DataKind::Source)?;
        self.sources
            .insert(item.descriptor().identity.clone(), Arc::new(item));
        Ok(())
    }

    pub fn register_format(&mut self, item: impl FormatDecoder + 'static) -> Result<()> {
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
