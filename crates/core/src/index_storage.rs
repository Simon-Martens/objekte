//! Shared index location and residency policy for every frontend.
//!
//! Resolving a memory-only policy does not inspect or create an index store.

use std::path::{Component, Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::source::SourceRegistry;
use crate::storage::configuration_hash;

/// Where immutable Objekte index generations are kept.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub enum IndexStorageLocation {
    #[default]
    MemoryOnly,
    SystemTemporary,
    RepositoryDirectory,
    /// A selected base directory, beneath which Objekte owns a namespace.
    CustomDirectory(PathBuf),
}

/// Whether file-backed index bytes are accessed on demand or owned in memory.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum IndexResidency {
    #[default]
    OnDemand,
    FullyResident,
}

/// One policy shared by direct library callers and all user interfaces.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexStoragePolicy {
    pub location: IndexStorageLocation,
    pub residency: IndexResidency,
}

impl Default for IndexStoragePolicy {
    fn default() -> Self {
        Self {
            location: IndexStorageLocation::MemoryOnly,
            residency: IndexResidency::FullyResident,
        }
    }
}

impl IndexStoragePolicy {
    pub fn validate(&self) -> Result<(), String> {
        if self.location == IndexStorageLocation::MemoryOnly
            && self.residency != IndexResidency::FullyResident
        {
            return Err("memory-only index storage requires full residency".into());
        }
        if let IndexStorageLocation::CustomDirectory(path) = &self.location {
            if path.as_os_str().is_empty() {
                return Err("custom index base directory is empty".into());
            }
        }
        Ok(())
    }

    /// Prepare the collection definitions used for both configuration
    /// identity and source discovery. A disk store beneath a selected source
    /// directory is pruned before traversal; memory-only leaves definitions
    /// untouched. This operation does not inspect or create index files.
    pub fn prepare_source_definitions(
        &self,
        root: &Path,
        definitions: &[CollectionDefinition],
    ) -> Result<Vec<CollectionDefinition>, String> {
        self.validate()?;
        let root = canonical_root(root)?;
        let mut prepared = definitions.to_vec();
        let Some(owned) = self.owned_source_namespace(&root)? else {
            return Ok(prepared);
        };
        for definition in &mut prepared {
            let selected = root.join(normalized_selection(&definition.source_path)?);
            let selected = match selected.canonicalize() {
                Ok(path) => path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => selected,
                Err(error) => {
                    return Err(format!(
                        "collection {} source selection {}: {error}",
                        definition.name,
                        selected.display()
                    ));
                }
            };
            if selected.starts_with(&owned) {
                return Err(format!(
                    "collection {} selects the managed index namespace as source data",
                    definition.name
                ));
            }
            if let Ok(relative) = owned.strip_prefix(&selected) {
                if relative.as_os_str().is_empty() {
                    return Err("managed index namespace is the collection source root".into());
                }
                if !definition
                    .discovery
                    .exclude_directories
                    .iter()
                    .any(|existing| {
                        !existing.as_os_str().is_empty()
                            && existing
                                .components()
                                .all(|part| matches!(part, Component::Normal(_)))
                            && relative.starts_with(existing)
                    })
                {
                    definition
                        .discovery
                        .exclude_directories
                        .push(relative.to_path_buf());
                }
            }
        }
        Ok(prepared)
    }

    /// Preferred construction entry point: keep the discovery definitions
    /// paired with the identity that was calculated from those definitions.
    /// Memory-only preparation never inspects an index store.
    pub fn prepare(
        &self,
        root: &Path,
        definitions: &[CollectionDefinition],
        options: RepositoryOptions,
        sources: &SourceRegistry,
    ) -> Result<PreparedIndexStorage, String> {
        let definitions = self.prepare_source_definitions(root, definitions)?;
        let resolved = self.resolve(root, &definitions, options, sources)?;
        Ok(PreparedIndexStorage {
            definitions,
            resolved,
        })
    }

    fn owned_source_namespace(&self, root: &Path) -> Result<Option<PathBuf>, String> {
        Ok(match &self.location {
            IndexStorageLocation::MemoryOnly => None,
            IndexStorageLocation::SystemTemporary => Some(
                std::env::temp_dir()
                    .canonicalize()
                    .map_err(|error| format!("system temporary directory: {error}"))?
                    .join(private_temporary_namespace())
                    .join("indexes"),
            ),
            IndexStorageLocation::RepositoryDirectory => {
                Some(root.join(".objekte").join("indexes"))
            }
            IndexStorageLocation::CustomDirectory(base) => {
                let base = base
                    .canonicalize()
                    .map_err(|error| format!("custom index base {}: {error}", base.display()))?;
                if !base.is_dir() {
                    return Err(format!(
                        "custom index base is not a directory: {}",
                        base.display()
                    ));
                }
                Some(base.join("objekte-indexes"))
            }
        })
    }

    /// Resolve stable identity and a managed directory without touching index files.
    /// The selected custom base must already exist; store creation is separate.
    pub fn resolve(
        &self,
        root: &Path,
        definitions: &[CollectionDefinition],
        options: RepositoryOptions,
        sources: &SourceRegistry,
    ) -> Result<ResolvedIndexStorage, String> {
        self.validate()?;
        let root = canonical_root(root)?;
        let prepared = self.prepare_source_definitions(&root, definitions)?;
        let mut repository_hash = Sha256::new();
        feed(&mut repository_hash, &path_bytes(&root));
        for definition in definitions {
            feed(&mut repository_hash, definition.name.as_bytes());
            feed(
                &mut repository_hash,
                &path_bytes(&normalized_selection(&definition.source_path)?),
            );
        }
        let repository_identity: [u8; 32] = repository_hash.finalize().into();
        let configuration_identity = configuration_hash(&prepared, options, sources)?;
        let repository_id = hex(&repository_identity);
        let configuration_id = hex(&configuration_identity);
        let (managed_directory, owned_namespace_root, deprecated_directories) = match &self.location
        {
            IndexStorageLocation::MemoryOnly => (None, None, Vec::new()),
            IndexStorageLocation::SystemTemporary => {
                let base = std::env::temp_dir()
                    .canonicalize()
                    .map_err(|error| format!("system temporary directory: {error}"))?;
                let owned = base.join(private_temporary_namespace());
                (
                    Some(
                        owned
                            .join("indexes")
                            .join(&repository_id)
                            .join(&configuration_id),
                    ),
                    Some(owned.clone()),
                    vec![owned],
                )
            }
            IndexStorageLocation::RepositoryDirectory => {
                let owned = root.join(".objekte").join("indexes");
                (
                    Some(owned.join(&configuration_id)),
                    Some(owned),
                    vec![root.join(".objekte")],
                )
            }
            IndexStorageLocation::CustomDirectory(base) => {
                let base = base
                    .canonicalize()
                    .map_err(|error| format!("custom index base {}: {error}", base.display()))?;
                if !base.is_dir() {
                    return Err(format!(
                        "custom index base is not a directory: {}",
                        base.display()
                    ));
                }
                let owned = base.join("objekte-indexes");
                (
                    Some(owned.join(&repository_id).join(&configuration_id)),
                    Some(owned),
                    vec![base],
                )
            }
        };
        Ok(ResolvedIndexStorage {
            repository_root: root,
            repository_identity,
            configuration_identity,
            managed_directory,
            owned_namespace_root,
            deprecated_directories,
            private_namespace: matches!(&self.location, IndexStorageLocation::SystemTemporary),
            residency: self.residency,
        })
    }
}

fn canonical_root(root: &Path) -> Result<PathBuf, String> {
    let root = root
        .canonicalize()
        .map_err(|error| format!("repository root {}: {error}", root.display()))?;
    if !root.is_dir() {
        return Err(format!(
            "repository root is not a directory: {}",
            root.display()
        ));
    }
    Ok(root)
}

/// Identity checked against manifests before a generation can be opened.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedIndexStorage {
    pub repository_root: PathBuf,
    pub repository_identity: [u8; 32],
    pub configuration_identity: [u8; 32],
    pub managed_directory: Option<PathBuf>,
    /// First application-owned directory beneath a selected existing base.
    pub owned_namespace_root: Option<PathBuf>,
    /// Exact selected directories where the previous two snapshot names may exist.
    pub deprecated_directories: Vec<PathBuf>,
    pub private_namespace: bool,
    pub residency: IndexResidency,
}

/// One immutable identity/discovery pair for building, opening and updating.
/// Callers must pass `definitions` to the repository constructor and use the
/// associated `resolved` store path; the two must not be recomputed separately.
#[derive(Clone, Debug)]
pub struct PreparedIndexStorage {
    pub definitions: Vec<CollectionDefinition>,
    pub resolved: ResolvedIndexStorage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexReadiness {
    Building,
    Searchable,
    PreparingFullResidency,
    FullyResidentReady,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexSynchronization {
    MemoryOnly,
    Pending,
    Synchronizing,
    Synchronized,
    Failed,
}

/// Separate searchable coverage, source freshness and durable state.
#[derive(Clone, Debug)]
pub struct IndexStorageStatus {
    pub readiness: IndexReadiness,
    pub source_reconciliation_complete: bool,
    pub current_generation: u64,
    pub durable_generation: Option<u64>,
    pub synchronization: IndexSynchronization,
    pub last_error: Option<String>,
    pub managed_directory: Option<PathBuf>,
}

fn normalized_selection(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        return Err(format!(
            "source selection must be relative: {}",
            path.display()
        ));
    }
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => result.push(part),
            Component::ParentDir => {
                if !result.pop() {
                    return Err(format!("source selection escapes root: {}", path.display()));
                }
            }
            _ => return Err(format!("invalid source selection: {}", path.display())),
        }
    }
    Ok(result)
}

fn feed(hash: &mut Sha256, bytes: &[u8]) {
    hash.update((bytes.len() as u64).to_le_bytes());
    hash.update(bytes);
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(windows)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::windows::ffi::OsStrExt;
    path.as_os_str()
        .encode_wide()
        .flat_map(u16::to_le_bytes)
        .collect()
}

#[cfg(unix)]
fn private_temporary_namespace() -> String {
    // SAFETY: geteuid has no pointers or preconditions.
    let user = unsafe { libc::geteuid() };
    format!("objekte-{user}")
}

#[cfg(not(unix))]
fn private_temporary_namespace() -> &'static str {
    // Platform temp APIs normally resolve inside a private per-user location.
    "objekte"
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for &byte in bytes {
        text.push(DIGITS[(byte >> 4) as usize] as char);
        text.push(DIGITS[(byte & 15) as usize] as char);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::discover;
    use crate::source::SourceFormat;
    use crate::workload::BigramAlphabet;

    #[test]
    fn memory_policy_never_resolves_a_store() {
        let root = std::env::current_dir().unwrap();
        let definitions = [CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )];
        let resolved = IndexStoragePolicy::default()
            .resolve(
                &root,
                &definitions,
                RepositoryOptions::default(),
                &SourceRegistry::builtin(),
            )
            .unwrap();
        assert_eq!(resolved.managed_directory, None);
    }

    #[test]
    fn memory_preparation_does_not_create_index_directories() {
        let root = std::env::temp_dir().join(format!(
            "objekte-hybrid-memory-preparation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let definitions = [CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )];
        let prepared = IndexStoragePolicy::default()
            .prepare(
                &root,
                &definitions,
                RepositoryOptions::default(),
                &SourceRegistry::builtin(),
            )
            .unwrap();
        assert!(
            prepared.definitions[0]
                .discovery
                .exclude_directories
                .is_empty()
        );
        assert!(prepared.resolved.managed_directory.is_none());
        assert!(!root.join(".objekte").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn disk_source_preparation_reports_invalid_path_components() {
        let root = std::env::temp_dir().join(format!(
            "objekte-hybrid-invalid-source-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("data"), b"not a directory").unwrap();
        let definitions = [CollectionDefinition::folder(
            "files",
            "data/nested",
            SourceFormat::Json,
        )];
        let policy = IndexStoragePolicy {
            location: IndexStorageLocation::RepositoryDirectory,
            residency: IndexResidency::OnDemand,
        };
        assert!(policy
            .prepare_source_definitions(&root, &definitions)
            .unwrap_err()
            .contains("source selection"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn memory_policy_rejects_on_demand() {
        assert!(
            IndexStoragePolicy {
                location: IndexStorageLocation::MemoryOnly,
                residency: IndexResidency::OnDemand,
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn source_selection_normalizes_without_escaping() {
        assert_eq!(
            normalized_selection(Path::new("a/./b/../c")).unwrap(),
            Path::new("a/c")
        );
        assert!(normalized_selection(Path::new("../a")).is_err());
    }

    #[test]
    fn temporary_location_is_stable_and_distinguishes_configuration() {
        let root = std::env::current_dir().unwrap();
        let definitions = [CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )];
        let policy = IndexStoragePolicy {
            location: IndexStorageLocation::SystemTemporary,
            residency: IndexResidency::OnDemand,
        };
        let sources = SourceRegistry::builtin();
        let first = policy
            .resolve(&root, &definitions, RepositoryOptions::default(), &sources)
            .unwrap();
        let again = policy
            .resolve(&root, &definitions, RepositoryOptions::default(), &sources)
            .unwrap();
        assert_eq!(first, again);
        let changed = policy
            .resolve(
                &root,
                &definitions,
                RepositoryOptions {
                    bigram_alphabet: BigramAlphabet::Unicode,
                    ..RepositoryOptions::default()
                },
                &sources,
            )
            .unwrap();
        assert_ne!(first.configuration_identity, changed.configuration_identity);
        assert_ne!(first.managed_directory, changed.managed_directory);
    }

    #[test]
    fn same_basename_at_different_roots_has_distinct_identity() {
        let base = std::env::temp_dir().join(format!(
            "objekte-hybrid-identity-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&base).unwrap();
        let a = base.join("a").join("same");
        let b = base.join("b").join("same");
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let definitions = [CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )];
        let policy = IndexStoragePolicy {
            location: IndexStorageLocation::SystemTemporary,
            residency: IndexResidency::OnDemand,
        };
        let sources = SourceRegistry::builtin();
        let a = policy
            .resolve(&a, &definitions, RepositoryOptions::default(), &sources)
            .unwrap();
        let b = policy
            .resolve(&b, &definitions, RepositoryOptions::default(), &sources)
            .unwrap();
        assert_ne!(a.repository_identity, b.repository_identity);
        assert_ne!(a.managed_directory, b.managed_directory);
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn nested_custom_store_is_pruned_and_hashed_as_effective_discovery() {
        let base = std::env::temp_dir().join(format!(
            "objekte-hybrid-source-exclusion-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let source_dir = base.join("data");
        let custom = source_dir.join("index-cache");
        let managed = custom.join("objekte-indexes").join("other-repository");
        std::fs::create_dir_all(&managed).unwrap();
        std::fs::write(source_dir.join("record.json"), b"{}").unwrap();
        std::fs::write(managed.join("must-not-be-source.json"), b"{}").unwrap();
        let definitions = [CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )];
        let policy = IndexStoragePolicy {
            location: IndexStorageLocation::CustomDirectory(custom.clone()),
            residency: IndexResidency::OnDemand,
        };
        let prepared = policy
            .prepare_source_definitions(&base, &definitions)
            .unwrap();
        assert_eq!(
            prepared[0].discovery.exclude_directories,
            vec![PathBuf::from("index-cache/objekte-indexes")]
        );
        assert_eq!(
            policy.prepare_source_definitions(&base, &prepared).unwrap()[0]
                .discovery
                .exclude_directories,
            prepared[0].discovery.exclude_directories
        );
        let sources = SourceRegistry::builtin();
        let resolved = policy
            .resolve(&base, &definitions, RepositoryOptions::default(), &sources)
            .unwrap();
        let paired = policy
            .prepare(&base, &definitions, RepositoryOptions::default(), &sources)
            .unwrap();
        assert_eq!(
            paired.definitions[0].discovery.exclude_directories,
            prepared[0].discovery.exclude_directories
        );
        assert_eq!(paired.resolved, resolved);
        assert_eq!(
            resolved.configuration_identity,
            configuration_hash(&prepared, RepositoryOptions::default(), &sources).unwrap()
        );
        let prepared_source = sources.resolve(&SourceFormat::Json).unwrap();
        let found = discover(
            &base,
            Path::new("data"),
            &prepared[0].discovery,
            &prepared_source,
        )
        .unwrap();
        assert_eq!(found, vec![source_dir.join("record.json")]);
        let memory = IndexStoragePolicy::default()
            .prepare_source_definitions(&base, &definitions)
            .unwrap();
        assert!(memory[0].discovery.exclude_directories.is_empty());
        let found_memory = discover(
            &base,
            Path::new("data"),
            &memory[0].discovery,
            &prepared_source,
        )
        .unwrap();
        assert_eq!(found_memory.len(), 2);
        assert!(
            !custom
                .join("objekte-indexes")
                .join("other-repository")
                .join("other-configuration")
                .exists()
        );
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn source_selection_inside_managed_namespace_is_rejected() {
        let base = std::env::temp_dir().join(format!(
            "objekte-hybrid-source-conflict-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let custom = base.join("cache");
        let source = custom.join("objekte-indexes").join("data");
        std::fs::create_dir_all(&source).unwrap();
        let policy = IndexStoragePolicy {
            location: IndexStorageLocation::CustomDirectory(custom),
            residency: IndexResidency::OnDemand,
        };
        let definitions = [CollectionDefinition::folder(
            "files",
            "cache/objekte-indexes/data",
            SourceFormat::Json,
        )];
        assert!(
            policy
                .prepare_source_definitions(&base, &definitions)
                .unwrap_err()
                .contains("managed index namespace")
        );
        std::fs::remove_dir_all(base).unwrap();
    }
}
