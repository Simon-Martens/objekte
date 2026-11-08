use super::*;
use crate::lifecycle::{IndexRevision, PublishedIndex};
use crate::model::{FieldPath, LogicalType};
use crate::query::{Query, QueryOptions, parse_filter};
use crate::reference::{CustomBinding, FieldDefinition, Schema, Transform};
use crate::repository::IdentityPolicy;
use crate::repository::{SnapshotAction, SnapshotMode, SnapshotOpenPolicy};
use crate::source::SourceFormat;
use crate::source::SourceRegistry;
use crate::workload::ReturnMode;

#[test]
fn one_pass_section_encoding_matches_the_existing_codec_bytes() {
    let value = vec!["alpha".to_owned(), "ßeta".to_owned(), "".to_owned()];
    let expected = codec().serialize(&value).unwrap();
    let mut encoded = Vec::new();
    codec().serialize_into(&mut encoded, &value).unwrap();
    assert_eq!(encoded, expected);
    let mut section = Vec::new();
    write_section(&mut section, 7, &value).unwrap();
    assert_eq!(&section[44..], expected);
}

fn lifecycle_fixture() -> (
    PathBuf,
    CollectionDefinition,
    RepositoryOptions,
    PublishedIndex,
) {
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root = std::env::temp_dir().join(format!(
        "objekte-lifecycle-cache-{}-{nonce}",
        std::process::id()
    ));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    let definition = CollectionDefinition::folder("files", "data", SourceFormat::Json);
    let options = RepositoryOptions {
        snapshot_mode: SnapshotMode::Disabled,
        ..Default::default()
    };
    let repository =
        Repository::open_with_options(&root, vec![definition.clone()], options).unwrap();
    let publication =
        PublishedIndex::from_repository(1, IndexRevision::INITIAL, std::sync::Arc::new(repository))
            .unwrap();
    (root, definition, options, publication)
}

#[test]
fn lifecycle_snapshot_restores_pinned_liveness_without_source_enumeration() {
    let (root, definition, options, mut publication) = lifecycle_fixture();
    publication.revision = IndexRevision(7);
    publication.active[0] = publication.active[0].retired(&[RecordSlot(0)]).unwrap();
    let mut appended = publication.repository.files().files[0].clone();
    appended.id = FileId(1);
    appended.relative_path = PathBuf::from("data/b.json");
    publication.catalog = publication
        .catalog
        .appended(
            appended,
            Some([7; 32]),
            Some(publication.configuration_identity),
        )
        .unwrap();
    publication.file_versions = publication.file_versions.appended().unwrap();
    let cache = root.join("cache.snapshot");
    save_lifecycle_snapshot_with_stats(&publication, &cache, |_| unreachable!(), || Ok(()))
        .unwrap();
    assert!(matches!(
        load_snapshot_checked(
            root.clone(),
            vec![definition.clone()],
            options,
            &SourceRegistry::builtin(),
            &cache
        ),
        Err(SnapshotLoadError::Invalid(_))
    ));
    fs::remove_file(root.join("data/a.json")).unwrap();
    let (restored, state) = load_lifecycle_snapshot_checked(
        root.clone(),
        vec![definition],
        options,
        &SourceRegistry::builtin(),
        &cache,
        |_| unreachable!(),
    )
    .unwrap();
    assert_eq!(restored.files().files.len(), 1);
    assert_eq!(state.revision, 7);
    assert!(state.base_active[0].is_empty());
    assert_eq!(state.catalog.len(), 2);
    assert_eq!(
        state.catalog[0]
            .as_ref()
            .unwrap()
            .decode_entry()
            .unwrap()
            .relative_path,
        PathBuf::from("data/a.json")
    );
    assert_eq!(state.file_versions[0].0, 1);
    assert_eq!(
        state.catalog[1].as_ref().unwrap().fingerprint,
        Some([7; 32])
    );
    assert_eq!(
        state.catalog[1]
            .as_ref()
            .unwrap()
            .decode_entry()
            .unwrap()
            .relative_path,
        PathBuf::from("data/b.json")
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_snapshot_rejects_corrupt_sections_and_preserves_previous_cache() {
    let (root, definition, options, publication) = lifecycle_fixture();
    let cache = root.join("cache.snapshot");
    save_lifecycle_snapshot_with_stats(&publication, &cache, |_| unreachable!(), || Ok(()))
        .unwrap();
    let original = fs::read(&cache).unwrap();
    let error = save_lifecycle_snapshot_with_stats(
        &publication,
        &cache,
        |_| unreachable!(),
        || Err("injected pre-rename failure".into()),
    )
    .unwrap_err();
    assert_eq!(error, "injected pre-rename failure");
    assert_eq!(fs::read(&cache).unwrap(), original);
    let mut corrupted = original.clone();
    *corrupted.last_mut().unwrap() ^= 0x40;
    fs::write(&cache, corrupted).unwrap();
    assert!(matches!(
        load_lifecycle_snapshot_checked(root.clone(), vec![definition], options, &SourceRegistry::builtin(), &cache, |_| Ok(())),
        Err(SnapshotLoadError::Invalid(message)) if message.contains("checksum")
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn lifecycle_snapshot_validates_active_domains_and_requires_segment_decoder() {
    let (root, definition, options, publication) = lifecycle_fixture();
    let cache = root.join("cache.snapshot");
    let mut state =
        LifecycleCacheState::from_publication(&publication, |_| unreachable!()).unwrap();
    state.base_active[0].insert(9);
    publication
        .repository
        .save_snapshot_with_lifecycle_state(&cache, Some(&state), || Ok(()))
        .unwrap();
    assert!(matches!(
        load_lifecycle_snapshot_checked(root.clone(), vec![definition.clone()], options, &SourceRegistry::builtin(), &cache, |_| Ok(())),
        Err(SnapshotLoadError::Invalid(message)) if message.contains("active slot")
    ));
    state.base_active[0].remove(9);
    state.segments[0].push(LifecycleSegmentCache {
        ordinal: 1,
        collection: CollectionId(0),
        record_count: 1,
        active: RoaringBitmap::from_iter([0]),
        index: vec![1, 2, 3],
    });
    state.next_segment_ordinal = 2;
    publication
        .repository
        .save_snapshot_with_lifecycle_state(&cache, Some(&state), || Ok(()))
        .unwrap();
    assert!(matches!(
        load_lifecycle_snapshot_checked(root.clone(), vec![definition], options, &SourceRegistry::builtin(), &cache, |_| Err("invalid private segment".into())),
        Err(SnapshotLoadError::Invalid(message)) if message.contains("invalid private segment")
    ));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn default_open_never_persists_and_stale_snapshot_is_left_untouched() {
    let root =
        std::env::temp_dir().join(format!("objekte-no-implicit-save-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    let definition = CollectionDefinition::folder("files", "data", SourceFormat::Json);
    let snapshot = snapshot_path(&root, BigramAlphabet::Ascii);

    let first = Repository::open(&root, vec![definition.clone()]).unwrap();
    assert!(!first.startup().restored);
    assert_eq!(first.startup().snapshot_write_ns, 0);
    assert!(!snapshot.exists());
    drop(first);
    let second = Repository::open(&root, vec![definition.clone()]).unwrap();
    assert!(!second.startup().restored);
    assert!(!snapshot.exists());
    second.save_snapshot(&snapshot).unwrap();
    let saved = fs::read(&snapshot).unwrap();
    let modified = fs::metadata(&snapshot).unwrap().modified().unwrap();

    let restored = Repository::open(&root, vec![definition.clone()]).unwrap();
    assert!(restored.startup().restored);
    assert_eq!(restored.startup().snapshot_write_ns, 0);
    assert_eq!(restored.build_parses(), 0);
    drop(restored);

    fs::write(root.join("data/b.json"), br#"{"title":"B"}"#).unwrap();
    let rebuilt = Repository::open(&root, vec![definition]).unwrap();
    assert!(!rebuilt.startup().restored);
    assert_eq!(rebuilt.startup().snapshot_write_ns, 0);
    assert_eq!(fs::read(&snapshot).unwrap(), saved);
    assert_eq!(
        fs::metadata(&snapshot).unwrap().modified().unwrap(),
        modified
    );
    drop(rebuilt);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn failed_explicit_save_preserves_previous_snapshot_and_removes_owned_temp() {
    let root = std::env::temp_dir().join(format!(
        "objekte-save-before-publish-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    let definition = CollectionDefinition::folder("files", "data", SourceFormat::Json);
    let repository = Repository::open(&root, vec![definition]).unwrap();
    let snapshot = snapshot_path(&root, BigramAlphabet::Ascii);
    repository.save_snapshot(&snapshot).unwrap();
    let previous = fs::read(&snapshot).unwrap();
    let modified = fs::metadata(&snapshot).unwrap().modified().unwrap();

    let error = repository
        .save_snapshot_before_publish(&snapshot, || Err("injected publication failure".into()))
        .unwrap_err();
    assert_eq!(error, "injected publication failure");
    assert_eq!(fs::read(&snapshot).unwrap(), previous);
    assert_eq!(
        fs::metadata(&snapshot).unwrap().modified().unwrap(),
        modified
    );
    let temporary = fs::read_dir(snapshot.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with(".objekte-snapshot-") && name.ends_with(".tmp"))
        .collect::<Vec<_>>();
    assert!(
        temporary.is_empty(),
        "leftover temporary snapshots: {temporary:?}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_save_rejects_indexed_source_destination_and_path_aliases() {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "objekte-snapshot-source-guard-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join("data")).unwrap();
    let source = root.join("data/a.json");
    let original = br#"{"title":"A"}"#;
    fs::write(&source, original).unwrap();
    let repository = Repository::open(
        &root,
        vec![CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )],
    )
    .unwrap();

    let cache = snapshot_path(&root, BigramAlphabet::Ascii);
    repository.save_snapshot(&cache).unwrap();
    assert!(fs::metadata(&cache).unwrap().len() > 0);
    for destination in [source.clone(), root.join("data/../data/a.json")] {
        let error = repository.save_snapshot(&destination).unwrap_err();
        assert!(error.contains("aliases indexed source file"), "{error}");
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let symlink_path = root.join("source-symlink.snapshot");
        symlink(&source, &symlink_path).unwrap();
        let error = repository.save_snapshot(&symlink_path).unwrap_err();
        assert!(error.contains("aliases indexed source file"), "{error}");
        assert_eq!(fs::read(&source).unwrap(), original);

        fs::create_dir(root.join("data/nested")).unwrap();
        symlink(root.join("data/nested"), root.join("nested-alias")).unwrap();
        let parent_alias = root.join("nested-alias/../a.json");
        let error = repository.save_snapshot(&parent_alias).unwrap_err();
        assert!(error.contains("aliases indexed source file"), "{error}");
        assert_eq!(fs::read(&source).unwrap(), original);

        let hardlink_path = root.join("source-hardlink.snapshot");
        fs::hard_link(&source, &hardlink_path).unwrap();
        let error = repository.save_snapshot(&hardlink_path).unwrap_err();
        assert!(error.contains("aliases indexed source file"), "{error}");
        assert_eq!(fs::read(&source).unwrap(), original);
    }

    fs::remove_file(&source).unwrap();
    let error = repository.save_snapshot(&source).unwrap_err();
    assert!(error.contains("aliases indexed source file"), "{error}");
    assert!(!source.exists());
    #[cfg(unix)]
    {
        let error = repository
            .save_snapshot(&root.join("nested-alias/../a.json"))
            .unwrap_err();
        assert!(error.contains("aliases indexed source file"), "{error}");
        assert!(!source.exists());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn source_alias_introduced_during_save_is_rejected_and_temp_is_removed() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let root = std::env::temp_dir().join(format!(
            "objekte-snapshot-race-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(root.join("data")).unwrap();
        let source = root.join("data/a.json");
        let original = br#"{"title":"A"}"#;
        fs::write(&source, original).unwrap();
        let repository = Repository::open(
            &root,
            vec![CollectionDefinition::folder(
                "files",
                "data",
                SourceFormat::Json,
            )],
        )
        .unwrap();
        let destination = root.join("later-alias.snapshot");
        let error = repository
            .save_snapshot_before_publish(&destination, || {
                symlink(&source, &destination).map_err(|error| error.to_string())
            })
            .unwrap_err();
        assert!(error.contains("aliases indexed source file"), "{error}");
        assert_eq!(fs::read(&source).unwrap(), original);
        assert!(fs::symlink_metadata(&destination).is_ok());
        let temporary = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .filter(|name| name.starts_with(".objekte-snapshot-") && name.ends_with(".tmp"))
            .collect::<Vec<_>>();
        assert!(
            temporary.is_empty(),
            "leftover temporary snapshots: {temporary:?}"
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn ordinary_existing_cache_destination_does_not_scan_large_source_inventory() {
    let root = std::env::temp_dir().join(format!(
        "objekte-snapshot-guard-cost-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    let mut repository = Repository::open(
        &root,
        vec![CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )],
    )
    .unwrap();
    let cache = snapshot_path(&root, BigramAlphabet::Ascii);
    repository.save_snapshot(&cache).unwrap();
    let revision = repository.files.files[0].revision;
    for number in 0..8192 {
        repository
            .files
            .add(
                PathBuf::from(format!("data/synthetic-{number}.json")),
                revision,
                None,
                CollectionId(0),
            )
            .unwrap();
    }
    assert_eq!(repository.files.files.len(), 8193);
    assert_eq!(
        repository
            .ensure_snapshot_destination_is_not_source(&cache)
            .unwrap(),
        0,
        "ordinary cache save should not inspect each source path"
    );
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn symlinked_index_source_paths_trigger_alias_scan() {
    use std::os::unix::fs::symlink;

    let root = std::env::temp_dir().join(format!(
        "objekte-snapshot-source-symlinks-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    fs::create_dir_all(root.join("real")).unwrap();
    let physical_source = root.join("real/a.json");
    let original = br#"{"title":"A"}"#;
    fs::write(&physical_source, original).unwrap();
    symlink(root.join("real"), root.join("data")).unwrap();
    let repository = Repository::open(
        &root,
        vec![CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )],
    )
    .unwrap();
    let error = repository.save_snapshot(&physical_source).unwrap_err();
    assert!(error.contains("aliases indexed source file"), "{error}");
    assert_eq!(fs::read(&physical_source).unwrap(), original);
    fs::remove_dir_all(&root).unwrap();

    fs::create_dir_all(root.join("data")).unwrap();
    let physical_source = root.join("data/real.json");
    fs::write(&physical_source, original).unwrap();
    symlink(&physical_source, root.join("data/link.json")).unwrap();
    let mut definition = CollectionDefinition::folder("files", "data", SourceFormat::Json);
    definition.discovery.symlinks = SymlinkPolicy::FollowWithinRoot;
    let repository = Repository::open(&root, vec![definition]).unwrap();
    assert_eq!(
        repository.files.files[0].relative_path,
        Path::new("data/link.json")
    );
    let error = repository.save_snapshot(&physical_source).unwrap_err();
    assert!(error.contains("aliases indexed source file"), "{error}");
    assert_eq!(fs::read(&physical_source).unwrap(), original);
    fs::remove_file(&physical_source).unwrap();
    let error = repository.save_snapshot(&physical_source).unwrap_err();
    assert!(error.contains("aliases indexed source file"), "{error}");
    assert!(!physical_source.exists());
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn explicit_snapshot_policy_preserves_load_and_write_boundaries() {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let root = std::env::temp_dir().join(format!(
        "objekte-snapshot-policy-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    let directory = root.join("private-cache");
    let definition = CollectionDefinition::folder("files", "data", SourceFormat::Json);
    let open = |mode| {
        Repository::open_with_snapshot_policy(
            &root,
            vec![definition.clone()],
            RepositoryOptions::default(),
            SnapshotOpenPolicy::new(mode, Some(directory.clone())),
        )
    };
    let snapshot = snapshot_path_in(&directory, BigramAlphabet::Ascii);
    let built_memory = open(SnapshotMode::Disabled).unwrap();
    assert_eq!(
        built_memory.startup().snapshot_action,
        SnapshotAction::BuiltInMemory
    );
    assert!(!snapshot.exists());
    assert!(
        open(SnapshotMode::Required)
            .err()
            .unwrap()
            .contains("snapshot missing")
    );

    let written = open(SnapshotMode::Disabled).unwrap();
    assert_eq!(
        written.startup().snapshot_action,
        SnapshotAction::BuiltInMemory
    );
    assert_eq!(written.build_parses(), 1);
    assert!(!snapshot.exists());
    let save = written.save_snapshot_with_stats(&snapshot).unwrap();
    assert_eq!(save.sections.len(), 2);
    assert_eq!(save.bytes_written, fs::metadata(&snapshot).unwrap().len());
    assert!(save.total_ns >= save.sync_ns);
    assert_eq!(save.sections[0].id, 0);
    assert_eq!(save.sections[1].id, 1);
    let original = fs::read(&snapshot).unwrap();
    let restored = open(SnapshotMode::Required).unwrap();
    assert_eq!(restored.startup().snapshot_action, SnapshotAction::Restored);
    assert_eq!(restored.build_parses(), 0);
    assert_eq!(fs::read(&snapshot).unwrap(), original);
    let load_and_write = open(SnapshotMode::IfValid).unwrap();
    assert_eq!(
        load_and_write.startup().snapshot_action,
        SnapshotAction::Restored
    );
    assert_eq!(load_and_write.startup().snapshot_write_ns, 0);
    assert_eq!(fs::read(&snapshot).unwrap(), original);
    let write_only = open(SnapshotMode::Disabled).unwrap();
    assert_eq!(
        write_only.startup().snapshot_action,
        SnapshotAction::BuiltInMemory
    );
    assert_eq!(write_only.build_parses(), 1);
    assert_eq!(fs::read(&snapshot).unwrap(), original);
    let ignored = open(SnapshotMode::Disabled).unwrap();
    assert_eq!(ignored.build_parses(), 1);
    assert_eq!(fs::read(&snapshot).unwrap(), original);

    fs::write(root.join("data/b.json"), br#"{"title":"B"}"#).unwrap();
    assert!(
        open(SnapshotMode::Required)
            .err()
            .unwrap()
            .contains("stale")
    );
    assert_eq!(fs::read(&snapshot).unwrap(), original);
    let rebuilt = open(SnapshotMode::IfValid).unwrap();
    assert_eq!(
        rebuilt.startup().snapshot_action,
        SnapshotAction::BuiltInMemory
    );
    assert_eq!(rebuilt.build_parses(), 2);
    let phases = rebuilt.startup();
    assert!(phases.snapshot_decode_ns > 0);
    assert!(phases.reconciliation_ns > 0);
    assert!(
        phases.build_ns
            >= phases
                .build_discovery_ns
                .saturating_add(phases.build_extraction_index_ns)
                .saturating_add(phases.build_finalization_ns)
    );
    assert_eq!(fs::read(&snapshot).unwrap(), original);
    rebuilt.save_snapshot(&snapshot).unwrap();
    assert_ne!(fs::read(&snapshot).unwrap(), original);

    fs::write(&snapshot, b"corrupt").unwrap();
    assert!(open(SnapshotMode::Required).is_err());
    let repaired = open(SnapshotMode::IfValid).unwrap();
    assert_eq!(
        repaired.startup().snapshot_action,
        SnapshotAction::BuiltInMemory
    );
    assert!(repaired.startup().snapshot_restore_error.is_some());
    assert_eq!(fs::read(&snapshot).unwrap(), b"corrupt");
    fs::write(root.join("not-a-directory"), b"blocked").unwrap();
    let inaccessible = Repository::open_with_snapshot_policy(
        &root,
        vec![definition],
        RepositoryOptions::default(),
        SnapshotOpenPolicy::new(
            SnapshotMode::IfValid,
            Some(root.join("not-a-directory/child")),
        ),
    );
    assert!(inaccessible.err().unwrap().contains("snapshot access"));
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn round_trip_shared_multi_record_files_and_reject_corruption() {
    let root = std::env::temp_dir().join(format!("objekte-snapshot-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(
        root.join("data/multi.jsonl"),
        b"{\"x\":\"one\"}\n{\"x\":\"two\"}\n",
    )
    .unwrap();
    fs::write(root.join("data/empty.jsonl"), b"").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["x".into()],
            kind: LogicalType::String,
            required: false,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let definitions: Vec<_> = ["a", "b"]
        .into_iter()
        .map(|name| CollectionDefinition {
            name: name.into(),
            source_path: "data".into(),
            format: SourceFormat::JsonLines,
            identity: IdentityPolicy::Generated,
            discovery: Default::default(),
            schema: schema.clone(),
            sort_fields: vec![FieldPath::parse("x").unwrap()],
            sort_locale: "de".into(),
            sort_options: Default::default(),
            sort_overrides: Default::default(),
        })
        .collect();
    let query = Query {
        collections: vec!["a".into()],
        fts: None,
        filter: Some(parse_filter("x = 'one'").unwrap()),
        options: QueryOptions {
            sort: vec![],
            limit: 10,
            offset: 0,
            count: true,
            return_mode: ReturnMode::Locator,
            evidence: true,
        },
    };
    for alphabet in [BigramAlphabet::Ascii, BigramAlphabet::Unicode] {
        let options = RepositoryOptions {
            bigram_alphabet: alphabet,
            ..RepositoryOptions::default()
        };
        let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        assert_eq!(built.build_parses, 4);
        let path = root.join(format!("{alphabet:?}.snapshot"));
        built.save_snapshot(&path).unwrap();
        let restored = load_snapshot(
            root.clone(),
            definitions.clone(),
            options,
            &crate::source::SourceRegistry::builtin(),
            &path,
        )
        .unwrap();
        assert_eq!(restored.files.files.len(), 2);
        assert_eq!(restored.files.files[0].collections.len(), 2);
        assert!(
            restored
                .files
                .files
                .iter()
                .all(|file| file.status == crate::inventory::FileStatus::Valid
                    && file.diagnostics.is_empty())
        );
        assert_eq!(restored.collections[0].generation.records.len(), 2);
        assert!(
            restored.collections[0]
                .generation
                .records
                .iter()
                .all(
                    |record| record.class == crate::inventory::RecordClass::Logical
                        && record.diagnostic_ref == crate::inventory::DiagnosticRef(0)
                )
        );
        assert_eq!(
            restored.collections[0].generation.file_records.slots.len(),
            2
        );
        assert_eq!(
            built.search(&query).unwrap().count.unwrap(),
            restored.search(&query).unwrap().count.unwrap()
        );
        let other = RepositoryOptions {
            bigram_alphabet: if alphabet == BigramAlphabet::Ascii {
                BigramAlphabet::Unicode
            } else {
                BigramAlphabet::Ascii
            },
            ..options
        };
        assert!(
            load_snapshot(
                root.clone(),
                definitions.clone(),
                other,
                &crate::source::SourceRegistry::builtin(),
                &path
            )
            .err()
            .unwrap()
            .contains("incompatible snapshot configuration")
        );
        let mut bytes = fs::read(&path).unwrap();
        let mut corrupted = bytes.clone();
        let tail = corrupted.len() - 1;
        corrupted[tail] ^= 0x01;
        fs::write(&path, &corrupted).unwrap();
        assert!(
            load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &crate::source::SourceRegistry::builtin(),
                &path
            )
            .err()
            .unwrap()
            .contains("snapshot checksum mismatch")
        );
        bytes.truncate(bytes.len() / 2);
        fs::write(&path, &bytes).unwrap();
        assert!(
            load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &crate::source::SourceRegistry::builtin(),
                &path
            )
            .is_err()
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn seeded_snapshot_mutations_and_truncations_are_rejected_without_panics() {
    const SEED: u64 = 0x5A9A_2026_1106;
    let root = std::env::temp_dir().join(format!("objekte-snapshot-fuzz-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/one.json"), br#"{"x":"one"}"#).unwrap();
    let definitions = vec![CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema: Schema::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }];
    let options = RepositoryOptions {
        snapshot_mode: crate::repository::SnapshotMode::Disabled,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    let snapshot = root.join("snapshot.bin");
    built.save_snapshot(&snapshot).unwrap();
    let original = fs::read(&snapshot).unwrap();
    let mut state = SEED;
    for case in 0..64 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let index = state as usize % original.len();
        let mut bytes = original.clone();
        bytes[index] ^= 1 << (case % 8);
        fs::write(&snapshot, bytes).unwrap();
        let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &crate::source::SourceRegistry::builtin(),
                &snapshot,
            )
        }));
        assert!(
            loaded.is_ok() && loaded.unwrap().is_err(),
            "seed={SEED:#x} case={case} index={index}"
        );
    }
    for size in [0, 1, 8, 16, original.len() / 2, original.len() - 1] {
        fs::write(&snapshot, &original[..size]).unwrap();
        let loaded = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &crate::source::SourceRegistry::builtin(),
                &snapshot,
            )
        }));
        assert!(
            loaded.is_ok() && loaded.unwrap().is_err(),
            "seed={SEED:#x} truncated_size={size}"
        );
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn checksummed_snapshot_with_invalid_diagnostic_reference_is_rejected() {
    use std::io::Cursor;
    let root = std::env::temp_dir().join(format!("objekte-snapshot-ref-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    fs::write(root.join("data/bad.json"), br#"{"title":"bad""#).unwrap();
    let definitions = vec![CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::Json,
        identity: IdentityPolicy::Filename,
        discovery: Default::default(),
        schema: Schema::default(),
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }];
    let options = RepositoryOptions {
        snapshot_mode: crate::repository::SnapshotMode::Disabled,
        ..Default::default()
    };
    let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    let snapshot = root.join("snapshot.bin");
    built.save_snapshot(&snapshot).unwrap();
    let bytes = fs::read(&snapshot).unwrap();
    let header_size = MAGIC.len() + 4 + 4 + 32 + 4;
    let mut reader = Cursor::new(&bytes);
    reader.set_position(header_size as u64);
    let files: Vec<EncodedFile> = read_section(&mut reader, 0).unwrap();
    let mut collection: EncodedCollection = read_section(&mut reader, 1).unwrap();
    assert_eq!(collection.generation.records.len(), 1);
    collection.generation.records[0].diagnostic_ref = crate::inventory::DiagnosticRef(42);
    let mut rewritten = bytes[..header_size].to_vec();
    write_section(&mut rewritten, 0, &files).unwrap();
    write_section(&mut rewritten, 1, &collection).unwrap();
    fs::write(&snapshot, rewritten).unwrap();
    let error = load_snapshot(
        root.clone(),
        definitions,
        options,
        &crate::source::SourceRegistry::builtin(),
        &snapshot,
    )
    .err()
    .unwrap();
    assert!(
        error.contains("record range or revision mismatch"),
        "{error}"
    );
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn eligibility_fast_paths_restore_and_reject_checksums_of_invalid_domains() {
    use std::io::Cursor;
    let root = std::env::temp_dir().join(format!("objekte-domains-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    let source = root.join("data/records.jsonl");
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["x".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    schema
        .register_custom(
            vec!["x".into()],
            CustomBinding {
                name: "folded".into(),
                version: 1,
                transform: Transform::CaseFold,
            },
        )
        .unwrap();
    let definitions = vec![CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }];
    let options = RepositoryOptions {
        snapshot_mode: crate::repository::SnapshotMode::Disabled,
        ..Default::default()
    };
    let snapshot = root.join("snapshot.bin");
    for (name, content, logical, fallback) in [
        ("empty", b"".as_slice(), 0, 0),
        (
            "healthy",
            b"{\"x\":\"one\"}\n{\"x\":\"two\"}\n".as_slice(),
            2,
            0,
        ),
        ("fallback", b"{\"x\":\n".as_slice(), 0, 1),
        (
            "mixed",
            b"{\"x\":\"one\"}\n{\"x\":\n{\"x\":\"two\"}\n".as_slice(),
            2,
            1,
        ),
    ] {
        fs::write(&source, content).unwrap();
        let built = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
        let (inventory_bytes, diagnostic_bytes) = built.files.estimated_heap_bytes();
        assert!(inventory_bytes > 0, "{name}");
        assert_eq!(diagnostic_bytes > 0, fallback > 0, "{name}");
        assert_eq!(
            built.collections[0].logical_records.len(),
            logical,
            "{name}"
        );
        assert_eq!(
            built.collections[0].generation.records.len() as u64,
            logical + fallback,
            "{name}"
        );
        built.save_snapshot(&snapshot).unwrap();
        let restored = load_snapshot(
            root.clone(),
            definitions.clone(),
            options,
            &crate::source::SourceRegistry::builtin(),
            &snapshot,
        )
        .unwrap();
        assert_eq!(
            restored.collections[0].logical_records,
            built.collections[0].logical_records
        );
        assert_eq!(
            restored.collections[0].empty_array_records,
            built.collections[0].empty_array_records
        );
        assert_eq!(
            restored.collections[0].generation.records.len() as u64,
            logical + fallback
        );
    }
    let original = fs::read(&snapshot).unwrap();
    let header_size = MAGIC.len() + 4 + 4 + 32 + 4;
    let mut previous_version = original.clone();
    previous_version[MAGIC.len()..MAGIC.len() + 4].copy_from_slice(&12u32.to_le_bytes());
    fs::write(&snapshot, previous_version).unwrap();
    assert!(
        load_snapshot(
            root.clone(),
            definitions.clone(),
            options,
            &crate::source::SourceRegistry::builtin(),
            &snapshot,
        )
        .err()
        .unwrap()
        .contains("incompatible snapshot version")
    );
    let mut previous_model = original.clone();
    previous_model[MAGIC.len() + 4..MAGIC.len() + 8].copy_from_slice(&8u32.to_le_bytes());
    fs::write(&snapshot, previous_model).unwrap();
    assert!(
        load_snapshot(
            root.clone(),
            definitions.clone(),
            options,
            &crate::source::SourceRegistry::builtin(),
            &snapshot,
        )
        .err()
        .unwrap()
        .contains("incompatible snapshot version")
    );
    for case in [
        "eligibility_missing",
        "eligibility_wrong",
        "posting_fallback",
        "posting_out_of_bounds",
        "raw_fallback",
        "fallback_logical",
        "custom_text_fallback",
        "posting_statistics",
        "posting_coverage",
        "diagnostic_owner",
        "structural_fallback",
        "structural_out_of_bounds",
    ] {
        let mut reader = Cursor::new(&original);
        reader.set_position(header_size as u64);
        let mut files: Vec<EncodedFile> = read_section(&mut reader, 0).unwrap();
        let mut collection: EncodedCollection = read_section(&mut reader, 1).unwrap();
        let posting = collection
            .postings
            .fields
            .get_mut(&vec!["x".into()])
            .unwrap();
        match case {
            "eligibility_missing" => {
                collection.logical_records.remove(2);
            }
            "eligibility_wrong" => {
                collection.logical_records.insert(1);
            }
            "posting_fallback" => {
                posting.presence.insert(1);
            }
            "posting_out_of_bounds" => {
                posting.presence.insert(99);
            }
            "raw_fallback" => {
                collection.raw.normal.entry(1).or_default().insert(1);
            }
            "fallback_logical" => {
                collection
                    .fallback_text
                    .normal
                    .entry(1)
                    .or_default()
                    .insert(0);
            }
            "custom_text_fallback" => {
                collection
                    .custom_text
                    .values_mut()
                    .next()
                    .unwrap()
                    .normal
                    .entry(1)
                    .or_default()
                    .insert(1);
            }
            "posting_statistics" => {
                posting.stats.present_count += 1;
            }
            "posting_coverage" => {
                posting.covered_records -= 1;
            }
            "diagnostic_owner" => {
                files[0].diagnostics[0].file = FileId(99);
            }
            "structural_fallback" => {
                collection.empty_array_records.insert(1);
            }
            "structural_out_of_bounds" => {
                collection.empty_array_records.insert(99);
            }
            _ => unreachable!(),
        }
        let mut rewritten = original[..header_size].to_vec();
        write_section(&mut rewritten, 0, &files).unwrap();
        write_section(&mut rewritten, 1, &collection).unwrap();
        fs::write(&snapshot, rewritten).unwrap();
        assert!(
            load_snapshot(
                root.clone(),
                definitions.clone(),
                options,
                &crate::source::SourceRegistry::builtin(),
                &snapshot,
            )
            .is_err(),
            "checksummed corruption accepted: {case}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn startup_restores_unchanged_and_rebuilds_after_complete_file_changes() {
    let root = std::env::temp_dir().join(format!("objekte-reopen-{}", std::process::id()));
    fs::create_dir_all(root.join("data")).unwrap();
    let multi = root.join("data/multi.jsonl");
    fs::write(&multi, b"{\"x\":\"a\"}\n{\"x\":\"b\"}\n").unwrap();
    let mut schema = Schema::default();
    schema
        .register_field(FieldDefinition {
            path: vec!["x".into()],
            kind: LogicalType::String,
            required: true,
            nullable: false,
            indexed: true,
            multi: false,
            element_nullable: Vec::new(),
        })
        .unwrap();
    let definitions = vec![CollectionDefinition {
        name: "data".into(),
        source_path: "data".into(),
        format: SourceFormat::JsonLines,
        identity: IdentityPolicy::Generated,
        discovery: Default::default(),
        schema,
        sort_fields: vec![FieldPath::parse("x").unwrap()],
        sort_locale: "de".into(),
        sort_options: Default::default(),
        sort_overrides: Default::default(),
    }];
    let options = RepositoryOptions::default();
    let fresh = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert!(!fresh.startup.restored);
    assert_eq!(fresh.build_parses, 2);
    fresh.save_default_snapshot_for_test();
    let restored = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert!(restored.startup.restored);
    assert_eq!(restored.build_parses, 0);
    assert_eq!(restored.collections[0].generation.records.len(), 2);
    fs::write(&multi, b"{\"x\":\"a\"}\n{\"x\":\"b\"}\n{\"x\":\"c\"}\n").unwrap();
    let changed = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert!(!changed.startup.restored);
    assert_eq!(changed.collections[0].generation.generation, 2);
    assert_eq!(changed.collections[0].generation.records.len(), 3);
    assert_eq!(
        changed.collections[0].generation.file_records.slots.len(),
        3
    );
    changed.save_default_snapshot_for_test();
    assert!(
        Repository::open_with_options(&root, definitions.clone(), options)
            .unwrap()
            .startup
            .restored
    );
    fs::write(root.join("data/extra.jsonl"), b"{\"x\":\"d\"}\n").unwrap();
    let added = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert_eq!(added.collections[0].generation.generation, 3);
    assert_eq!(added.collections[0].generation.records.len(), 4);
    added.save_default_snapshot_for_test();
    fs::remove_file(multi).unwrap();
    let deleted = Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert_eq!(deleted.collections[0].generation.generation, 4);
    assert_eq!(deleted.collections[0].generation.records.len(), 1);
    assert_eq!(deleted.files.files.len(), 1);
    deleted.save_default_snapshot_for_test();
    let snapshot = snapshot_path(&root, BigramAlphabet::Ascii);
    let mut bytes = fs::read(&snapshot).unwrap();
    // A previous bigram representation has a different configuration hash.
    bytes[16] ^= 1;
    fs::write(&snapshot, &bytes).unwrap();
    let old_representation =
        Repository::open_with_options(&root, definitions.clone(), options).unwrap();
    assert!(!old_representation.startup.restored);
    assert_eq!(old_representation.build_parses, 1);
    let mut bytes = fs::read(&snapshot).unwrap();
    bytes[0] ^= 1;
    fs::write(&snapshot, bytes).unwrap();
    let rebuilt = Repository::open_with_options(&root, definitions, options).unwrap();
    assert!(!rebuilt.startup.restored);
    assert_eq!(rebuilt.collections[0].generation.records.len(), 1);
    fs::remove_dir_all(root).unwrap();
}
