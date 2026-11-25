//! Caller-owned collection configuration and repository options.

pub mod identity;
pub mod loader;

use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::PathBuf;

use crate::index::bigram::BigramAlphabet;
use crate::index::sorting::{SortConfig, SortOptions};
use crate::inference::InferenceConfig;
use crate::inventory::discovery::Discovery;
use crate::model::FieldPath;
use crate::schema::Schema;
use crate::source::SourceFormat;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IdentityPolicy {
    Filename,
    /// Repository-native path relative to the collection directory, including its extension.
    RelativePath,
    Generated,
    Intrinsic(Vec<String>),
}

/// Source-neutral collection schema and query/index policy. The declaration
/// is fixed once an index is built; registrations supply records separately.
#[derive(Clone, Debug)]
pub struct CollectionDeclaration {
    pub name: String,
    pub schema: Schema,
    pub sort_fields: Vec<FieldPath>,
    pub sort_locale: String,
    pub sort_options: SortOptions,
    pub sort_overrides: HashMap<FieldPath, SortConfig>,
}

impl CollectionDeclaration {
    pub fn new(name: impl Into<String>, schema: Schema) -> Self {
        Self {
            name: name.into(),
            schema,
            sort_fields: Vec::new(),
            sort_locale: "und".into(),
            sort_options: SortOptions::default(),
            sort_overrides: HashMap::new(),
        }
    }
}

/// Configuration for a file importer, separate from collection semantics.
#[derive(Clone, Debug)]
pub struct FileSourceConfig {
    pub source_path: PathBuf,
    pub format: SourceFormat,
    pub identity: IdentityPolicy,
    pub discovery: Discovery,
}

/// Borrowed collection policy consumed by the shared segment builder.
pub(crate) trait CollectionPolicy {
    fn schema(&self) -> &Schema;
    fn sort_fields(&self) -> &[FieldPath];
    fn sort_locale(&self) -> &str;
    fn sort_options(&self) -> SortOptions;
    fn sort_overrides(&self) -> &HashMap<FieldPath, SortConfig>;
}

impl CollectionPolicy for CollectionDeclaration {
    fn schema(&self) -> &Schema {
        &self.schema
    }

    fn sort_fields(&self) -> &[FieldPath] {
        &self.sort_fields
    }

    fn sort_locale(&self) -> &str {
        &self.sort_locale
    }

    fn sort_options(&self) -> SortOptions {
        self.sort_options
    }

    fn sort_overrides(&self) -> &HashMap<FieldPath, SortConfig> {
        &self.sort_overrides
    }
}

impl CollectionPolicy for CollectionDefinition {
    fn schema(&self) -> &Schema {
        &self.schema
    }

    fn sort_fields(&self) -> &[FieldPath] {
        &self.sort_fields
    }

    fn sort_locale(&self) -> &str {
        &self.sort_locale
    }

    fn sort_options(&self) -> SortOptions {
        self.sort_options
    }

    fn sort_overrides(&self) -> &HashMap<FieldPath, SortConfig> {
        &self.sort_overrides
    }
}

/// Caller-supplied file configuration retained by the existing file-opening
/// facade. New registrars use [`CollectionDeclaration`] directly.
#[derive(Clone, Debug)]
pub struct CollectionDefinition {
    pub name: String,
    pub source_path: PathBuf,
    pub format: SourceFormat,
    pub identity: IdentityPolicy,
    pub discovery: Discovery,
    pub schema: Schema,
    pub sort_fields: Vec<FieldPath>,
    pub sort_locale: String,
    pub sort_options: SortOptions,
    pub sort_overrides: HashMap<FieldPath, SortConfig>,
}

impl CollectionDefinition {
    pub fn from_parts(declaration: CollectionDeclaration, source: FileSourceConfig) -> Self {
        Self {
            name: declaration.name,
            source_path: source.source_path,
            format: source.format,
            identity: source.identity,
            discovery: source.discovery,
            schema: declaration.schema,
            sort_fields: declaration.sort_fields,
            sort_locale: declaration.sort_locale,
            sort_options: declaration.sort_options,
            sort_overrides: declaration.sort_overrides,
        }
    }

    pub fn declaration(&self) -> CollectionDeclaration {
        CollectionDeclaration {
            name: self.name.clone(),
            schema: self.schema.clone(),
            sort_fields: self.sort_fields.clone(),
            sort_locale: self.sort_locale.clone(),
            sort_options: self.sort_options,
            sort_overrides: self.sort_overrides.clone(),
        }
    }

    pub fn file_source(&self) -> FileSourceConfig {
        FileSourceConfig {
            source_path: self.source_path.clone(),
            format: self.format.clone(),
            identity: self.identity.clone(),
            discovery: self.discovery.clone(),
        }
    }

    /// Define a recursive folder collection with no declared fields or typed indexes.
    /// Generic folder collections use per-record generated identities.
    pub fn folder(
        name: impl Into<String>,
        source_path: impl Into<PathBuf>,
        format: SourceFormat,
    ) -> Self {
        Self::folder_with_identity(name, source_path, format, IdentityPolicy::Generated)
    }

    /// Define a recursive folder collection with an identity selected by the caller.
    pub fn folder_with_identity(
        name: impl Into<String>,
        source_path: impl Into<PathBuf>,
        format: SourceFormat,
        identity: IdentityPolicy,
    ) -> Self {
        Self {
            name: name.into(),
            source_path: source_path.into(),
            format,
            identity,
            discovery: Discovery {
                recursive: true,
                ..Discovery::default()
            },
            schema: Schema::default(),
            sort_fields: Vec::new(),
            sort_locale: "und".into(),
            sort_options: SortOptions::default(),
            sort_overrides: HashMap::new(),
        }
    }
}

/// Index alphabet, verified cache, and worker settings.
#[derive(Clone, Copy, Debug)]
pub struct RepositoryOptions {
    pub bigram_alphabet: BigramAlphabet,
    pub verified_cache_bytes: usize,
    pub query_workers: usize,
    /// `None` uses the parallelism available to this process at build time.
    pub build_workers: Option<NonZeroUsize>,
    /// Shared soft record/file boundary for all native segment producers.
    pub segment_policy: crate::index::segment::SegmentPolicy,
    pub inference: InferenceConfig,
    /// Request detailed instrumentation in a build compiled with the
    /// `detailed-profiling` feature. Ignored by ordinary builds.
    pub profile_build: bool,
}

impl Default for RepositoryOptions {
    fn default() -> Self {
        Self {
            bigram_alphabet: BigramAlphabet::Ascii,
            verified_cache_bytes: 0,
            query_workers: std::thread::available_parallelism()
                .map_or(1, |workers| workers.get().min(32)),
            build_workers: None,
            segment_policy: crate::index::segment::SegmentPolicy::default(),
            inference: InferenceConfig::default(),
            profile_build: false,
        }
    }
}

impl RepositoryOptions {
    /// A compile-time false result removes detailed profiling from normal
    /// builds, including callers that accidentally set `profile_build`.
    #[inline(always)]
    pub(crate) const fn detailed_profiling_enabled(&self) -> bool {
        #[cfg(feature = "detailed-profiling")]
        {
            self.profile_build
        }

        #[cfg(not(feature = "detailed-profiling"))]
        {
            false
        }
    }
}
