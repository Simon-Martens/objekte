use super::*;
use crate::discovery::SymlinkPolicy;
use crate::lifecycle::{IndexRevision, PublishedIndex};
use crate::model::FileId;
use crate::repository::Repository;
use crate::source::SourceFormat;

fn fixture() -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "objekte-cache-guard-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/a.json"), br#"{"name":"A"}"#).unwrap();
    root
}

fn publication(root: &Path) -> PublishedIndex {
    let repository = Repository::open(
        root,
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
    )
    .unwrap();
    PublishedIndex::from_repository(1, IndexRevision::INITIAL, std::sync::Arc::new(repository))
        .unwrap()
}

fn ensure_cache_destination_safe(
    path: &Path,
    publication: &PublishedIndex,
    protected_files: &[PathBuf],
) -> Result<(), String> {
    CacheDestinationGuard::new(publication, protected_files).ensure_safe(path)
}

#[test]
fn active_source_and_protected_configuration_targets_are_refused() {
    let root = fixture();
    let view = publication(&root);
    let source = root.join("data/a.json");
    let config = root.join("schema.json");
    fs::write(&config, b"schema").unwrap();
    let protected = vec![config.canonicalize().unwrap()];

    assert!(
        ensure_cache_destination_safe(&source, &view, &protected)
            .unwrap_err()
            .contains("indexed source")
    );
    assert!(
        ensure_cache_destination_safe(&config, &view, &protected)
            .unwrap_err()
            .contains("configuration file")
    );
    assert!(ensure_cache_destination_safe(&root.join("cache.snapshot"), &view, &protected).is_ok());
    assert_eq!(fs::read(&source).unwrap(), br#"{"name":"A"}"#);
    assert_eq!(fs::read(&config).unwrap(), b"schema");
    fs::remove_file(&config).unwrap();
    assert!(ensure_cache_destination_safe(&config, &view, &[config.clone()]).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn retired_source_slot_is_not_guarded_as_active() {
    let root = fixture();
    let mut view = publication(&root);
    view.catalog = view.catalog.retired(FileId(0)).unwrap();
    assert!(ensure_cache_destination_safe(&root.join("data/a.json"), &view, &[]).is_ok());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn symlink_and_hardlink_aliases_are_refused() {
    use std::os::unix::fs::symlink;

    let root = fixture();
    let view = publication(&root);
    let source = root.join("data/a.json");
    let linked_source = root.join("linked-source.json");
    let hardlinked_source = root.join("hardlinked-source.json");
    symlink(&source, &linked_source).unwrap();
    fs::hard_link(&source, &hardlinked_source).unwrap();
    assert!(ensure_cache_destination_safe(&linked_source, &view, &[]).is_err());
    assert!(ensure_cache_destination_safe(&hardlinked_source, &view, &[]).is_err());

    let config = root.join("schema.json");
    let linked_config = root.join("linked-schema.json");
    let hardlinked_config = root.join("hardlinked-schema.json");
    fs::write(&config, b"schema").unwrap();
    symlink(&config, &linked_config).unwrap();
    fs::hard_link(&config, &hardlinked_config).unwrap();
    let protected = vec![config.canonicalize().unwrap()];
    assert!(ensure_cache_destination_safe(&linked_config, &view, &protected).is_err());
    assert!(ensure_cache_destination_safe(&hardlinked_config, &view, &protected).is_err());

    let later_alias = root.join("later-cache.json");
    let guard = CacheDestinationGuard::new(&view, &protected);
    assert!(guard.ensure_safe(&later_alias).is_ok());
    symlink(&source, &later_alias).unwrap();
    assert!(guard.ensure_safe(&later_alias).is_err());
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn missing_parent_resolution_follows_existing_symlink_ancestor() {
    use std::os::unix::fs::symlink;

    let root = fixture();
    symlink(root.join("data"), root.join("alias")).unwrap();
    let target = root.join("alias/new/nested/cache.snapshot");
    assert_eq!(
        resolved_destination(&target).unwrap(),
        root.canonicalize()
            .unwrap()
            .join("data/new/nested/cache.snapshot")
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn source_directory_alias_is_refused_and_cache_symlink_name_is_excluded() {
    use std::os::unix::fs::symlink;

    let root = fixture();
    symlink(root.join("data"), root.join("alias")).unwrap();
    let repository = Repository::open(
        &root,
        vec![CollectionDefinition::folder(
            "alias",
            "alias",
            SourceFormat::Json,
        )],
    )
    .unwrap();
    let view =
        PublishedIndex::from_repository(1, IndexRevision::INITIAL, std::sync::Arc::new(repository))
            .unwrap();
    assert!(ensure_cache_destination_safe(&root.join("data/a.json"), &view, &[]).is_err());

    let cache = root.join("data/cache.json");
    let cache_alias = root.join("data/cache-alias.json");
    fs::write(&cache, b"cache").unwrap();
    symlink(&cache, &cache_alias).unwrap();
    let mut definitions = vec![CollectionDefinition::folder(
        "data",
        "data",
        SourceFormat::Json,
    )];
    exclude_cache_artifacts(&root, &mut definitions, &cache_alias).unwrap();
    assert!(
        definitions[0]
            .discovery
            .exclude_files
            .contains(&PathBuf::from("cache.json"))
    );
    assert!(
        definitions[0]
            .discovery
            .exclude_files
            .contains(&PathBuf::from("cache-alias.json"))
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn exact_cache_exclusion_preserves_other_files_in_its_directory() {
    let root = fixture();
    fs::write(root.join("data/cache.json"), br#"{"name":"cache"}"#).unwrap();
    fs::write(root.join("data/sibling.json"), br#"{"name":"sibling"}"#).unwrap();
    let mut definitions = vec![CollectionDefinition::folder(
        "data",
        "data",
        SourceFormat::Json,
    )];
    definitions[0].discovery.recursive = true;
    definitions[0].discovery.symlinks = SymlinkPolicy::Skip;
    exclude_cache_artifacts(&root, &mut definitions, &root.join("data/cache.json")).unwrap();
    assert_eq!(
        definitions[0].discovery.exclude_files,
        vec![PathBuf::from("cache.json")]
    );
    assert_eq!(
        definitions[0].discovery.exclude,
        vec![".objekte-snapshot-*.tmp"]
    );
    let matcher = globset::Glob::new(&definitions[0].discovery.exclude[0])
        .unwrap()
        .compile_matcher();
    assert!(matcher.is_match(".objekte-snapshot-123-456.tmp"));
    assert!(!matcher.is_match("sibling.json"));
    let repository = Repository::open(&root, definitions).unwrap();
    assert_eq!(repository.files().files.len(), 2);
    assert!(
        repository
            .files()
            .id_for(Path::new("data/sibling.json"))
            .is_some()
    );
    assert!(
        repository
            .files()
            .id_for(Path::new("data/cache.json"))
            .is_none()
    );
    fs::remove_dir_all(root).unwrap();
}
