//! Stable descriptors and declared dependencies.

use super::*;


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

    pub(super) fn validate(&self, input: DataKind, output: DataKind) -> Result<()> {
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
