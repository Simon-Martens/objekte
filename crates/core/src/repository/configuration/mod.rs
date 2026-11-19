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

/// Caller-supplied source, schema, identity, and sort configuration.
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
    /// Define a recursive folder collection with no declared fields or typed indexes.
    /// JSONL and registered readers use per-record generated identities because a file
    /// can contain several records. Callers may override any of these defaults.
    pub fn folder(
        name: impl Into<String>,
        source_path: impl Into<PathBuf>,
        format: SourceFormat,
    ) -> Self {
        let identity = if format.supports_relative_path_identity() {
            IdentityPolicy::RelativePath
        } else {
            IdentityPolicy::Generated
        };
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
