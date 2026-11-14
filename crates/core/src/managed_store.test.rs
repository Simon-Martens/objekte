use super::*;
use crate::index_storage::{IndexLocationResolver, IndexStorageLocation};
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::segment_format::{REQUIRED_SECTIONS, SectionKind, SegmentWriteMetadata, SegmentWriter};
use crate::source::{SourceFormat, SourceRegistry};
use roaring::RoaringBitmap;
use std::io::Cursor;

struct TestStore {
    base: PathBuf,
    store: ManagedStore,
    active: ActiveRecords,
}

impl TestStore {
    fn new() -> Self {
        let base = std::env::temp_dir().join(format!(
            "objekte-hybrid-store-test-{}-{}",
            std::process::id(),
            unique_nonce().unwrap()
        ));
        Self::create(base)
    }

    fn create(base: PathBuf) -> Self {
        fs::create_dir(&base).unwrap();
        let root = base.join("source");
        let destination = base.join("destination");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&destination).unwrap();
        let resolved = IndexLocationResolver {
            location: IndexStorageLocation::CustomDirectory(destination),
        }
        .resolve(
            &root,
            &[CollectionDefinition::folder(
                "files",
                "data",
                SourceFormat::Json,
            )],
            RepositoryOptions::default(),
            &SourceRegistry::builtin(),
        )
        .unwrap();
        let store = ManagedStore::open(&resolved).unwrap();
        Self {
            base,
            store,
            active: ActiveRecords::all(16).unwrap(),
        }
    }
}

impl Drop for TestStore {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).unwrap();
    }
}

fn complete_segment(metadata: &SegmentWriteMetadata<'_>, marker: u8) -> Vec<u8> {
    let mut writer = SegmentWriter::new_with_metadata(
        Cursor::new(Vec::new()),
        REQUIRED_SECTIONS.len(),
        metadata,
    )
    .unwrap();
    for kind in REQUIRED_SECTIONS {
        writer
            .write_section(kind, |target| {
                target
                    .write_all(&[marker])
                    .map_err(|error| error.to_string())
            })
            .unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn stream_segment<'a>(
    collection: CollectionId,
    ordinal: u64,
    active: &'a ActiveRecords,
    marker: u8,
) -> SegmentInput<'a> {
    SegmentInput::NewStream {
        collection,
        ordinal,
        active,
        origin: SegmentOrigin::Validated,
        write: Box::new(move |file, metadata| {
            file.write_all(&complete_segment(metadata, marker))
                .map_err(|error| error.to_string())
        }),
    }
}

fn commit_one(fixture: &TestStore, expected: Option<u64>, generation: u64) -> CommitStats {
    fixture
        .store
        .commit_generation(
            expected,
            generation,
            &[stream_segment(CollectionId(0), 1, &fixture.active, 1)],
        )
        .unwrap()
}

#[test]
fn lock_free_reader_ignores_writer_lock() {
    let fixture = TestStore::new();
    assert!(fixture.store.read_generation().unwrap().is_none());
    let lock = fixture.store.path().join(LOCK_NAME);
    fs::create_dir(lock).unwrap();
    assert!(fixture.store.read_generation().unwrap().is_none());
}

#[test]
fn typed_commit_distinguishes_lock_io_and_generation_conflict() {
    let fixture = TestStore::new();
    let catalog: Arc<[u8]> = Arc::from(&b"OBJHCAT1-typed"[..]);
    assert!(fixture.store.read_generation().unwrap().is_none());
    let lock = fixture.store.path().join(LOCK_NAME);
    fs::create_dir(&lock).unwrap();
    assert!(matches!(
        fixture
            .store
            .commit_generation_with_catalog_typed(None, 1, &[], Arc::clone(&catalog)),
        Err(StoreReadError::Io { .. })
    ));
    fs::remove_dir(&lock).unwrap();
    fixture
        .store
        .commit_generation_with_catalog_typed(None, 1, &[], Arc::clone(&catalog))
        .unwrap();
    assert!(matches!(
        fixture
            .store
            .commit_generation_with_catalog_typed(None, 2, &[], Arc::clone(&catalog)),
        Err(StoreReadError::Conflict(_))
    ));
    assert!(matches!(
        fixture.store.commit_generation_with_catalog_typed(
            Some(1),
            2,
            &[SegmentInput::NewStream {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 1,
                active: &fixture.active,
                write: Box::new(|_, _| Err("injected write failure".into())),
            }],
            catalog,
        ),
        Err(StoreReadError::Io { .. })
    ));
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        1
    );
}

#[test]
fn manifest_is_readable_toml_and_has_no_binary_manifest() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let text = fs::read_to_string(fixture.store.path().join(MANIFEST_NAME)).unwrap();
    assert!(text.contains(&format!("version = {}", crate::PROGRAM_VERSION)));
    assert!(text.contains("generation = 1"));
    assert!(text.contains("segments = \"generations/0000000000000001/segments\""));
    assert!(text.contains("repository = \""));
    assert!(text.contains("configuration = \""));
    assert!(!fixture.store.path().join(LEGACY_MANIFEST_NAME).exists());
    let parsed: Manifest = toml::from_str(&text).unwrap();
    assert_eq!(parsed.generation, 1);
    assert!(parsed.catalog.is_none());
    assert!(parsed.verify.is_empty());
}

#[test]
fn supported_corruption_and_unsupported_version_have_different_kinds() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let path = fixture.store.path().join(MANIFEST_NAME);
    let original = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{original}\nnot valid = [")).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Corrupt(_))
    ));
    let unsupported = original.replace(
        &format!("version = {}", crate::PROGRAM_VERSION),
        &format!("version = {}", crate::PROGRAM_VERSION + 1),
    );
    assert_ne!(unsupported, original);
    fs::write(&path, unsupported).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Incompatible(_))
    ));
}

#[test]
fn obsolete_binary_manifest_alone_reports_incompatibility() {
    let fixture = TestStore::new();
    fs::write(
        fixture.store.path().join(LEGACY_MANIFEST_NAME),
        b"OBJHMAN1 not decodable",
    )
    .unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Incompatible(_))
    ));
}

#[test]
fn manifest_toml_takes_precedence_over_obsolete_binary_manifest() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    fs::write(
        fixture.store.path().join(LEGACY_MANIFEST_NAME),
        b"OBJHMAN1 obsolete",
    )
    .unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(opened.generation, 1);
    commit_one(&fixture, Some(1), 2);
    assert!(!fixture.store.path().join(LEGACY_MANIFEST_NAME).exists());
}

#[test]
fn opening_validates_metadata_but_defers_posting_payload_integrity_on_demand() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let opened = fixture.store.read_generation().unwrap().unwrap();
    let path = opened
        .generation_directory
        .join("segments")
        .join(&opened.segments[0].name);
    drop(opened);
    let original = fs::read(&path).unwrap();
    let mut corrupted = original.clone();
    let field_offset = corrupted.len() - REQUIRED_SECTIONS.len();
    corrupted[field_offset] ^= 1;
    fs::write(&path, &corrupted).unwrap();
    assert!(fixture.store.read_generation().is_err());
    fs::write(&path, &original).unwrap();
    let mut corrupted = original.clone();
    let posting = SectionKind::PostingPayloads as usize - 1;
    corrupted[field_offset + posting] ^= 1;
    fs::write(&path, &corrupted).unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    assert!(
        opened.segments[0]
            .reader
            .section(SectionKind::PostingPayloads)
            .is_err()
    );
}

#[test]
fn streamed_segment_failure_keeps_previous_generation_and_allows_retry() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let failure = fixture.store.commit_generation(
        Some(1),
        2,
        &[SegmentInput::NewStream {
            origin: SegmentOrigin::Unvalidated,
            collection: CollectionId(0),
            ordinal: 2,
            active: &fixture.active,
            write: Box::new(|file, _| {
                file.write_all(b"partial")
                    .map_err(|error| error.to_string())?;
                Err("injected writer failure".into())
            }),
        }],
    );
    assert!(failure.unwrap_err().contains("injected writer failure"));
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        1
    );
    let committed = fixture
        .store
        .commit_generation(
            Some(1),
            2,
            &[stream_segment(CollectionId(0), 2, &fixture.active, 2)],
        )
        .unwrap();
    assert_eq!(committed.segment_names.len(), 1);
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        2
    );
}

#[test]
fn segment_header_supplies_identity_slot_count_and_active_domain() {
    let fixture = TestStore::new();
    let mut sparse = RoaringBitmap::new();
    sparse.insert(0);
    sparse.insert(7);
    sparse.insert(15);
    let active = ActiveRecords::from_bitmap(16, sparse).unwrap();
    let expected = active.bitmap();
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::NewStream {
                origin: SegmentOrigin::Validated,
                collection: CollectionId(0),
                ordinal: 3,
                active: &active,
                write: Box::new(|file, metadata| {
                    file.write_all(&complete_segment(metadata, 7))
                        .map_err(|error| error.to_string())
                }),
            }],
        )
        .unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    let segment = &opened.segments[0];
    assert_eq!(segment.descriptor.generation, 1);
    assert_eq!(segment.descriptor.collection, CollectionId(0));
    assert_eq!(segment.descriptor.ordinal, 3);
    assert_eq!(segment.descriptor.slot_count, 16);
    assert_eq!(segment.descriptor.active.bitmap(), expected);
    assert!(segment.descriptor.write_validated);
    assert_eq!(segment.trust(), SegmentTrust::WriteValidated);
    assert!(segment.length > 0);
}

#[test]
fn unvalidated_writer_flag_roundtrips_and_requires_full_validation() {
    let fixture = TestStore::new();
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::NewStream {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 1,
                active: &fixture.active,
                write: Box::new(|file, metadata| {
                    file.write_all(&complete_segment(metadata, 3))
                        .map_err(|error| error.to_string())
                }),
            }],
        )
        .unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    assert!(!opened.segments[0].descriptor.write_validated);
    assert_eq!(opened.segments[0].trust(), SegmentTrust::Untrusted);
}

#[test]
fn segment_copied_from_another_generation_is_rejected() {
    let fixture = TestStore::new();
    let first = fixture
        .store
        .commit_generation(
            None,
            1,
            &[stream_segment(CollectionId(0), 1, &fixture.active, 1)],
        )
        .unwrap();
    let first_path = fixture
        .store
        .generation_directory(1)
        .join("segments")
        .join(&first.segment_names[0]);
    let copied = fs::read(&first_path).unwrap();
    fixture
        .store
        .commit_generation(
            Some(1),
            2,
            &[stream_segment(CollectionId(0), 1, &fixture.active, 1)],
        )
        .unwrap();
    let second_path = fixture.store.generation_directory(2).join("segments").join(
        fs::read_dir(fixture.store.generation_directory(2).join("segments"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .file_name(),
    );
    fs::write(&second_path, copied).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Corrupt(_))
    ));
}

#[test]
fn duplicate_segment_identity_cannot_be_published() {
    let fixture = TestStore::new();
    let error = fixture.store.commit_generation(
        None,
        1,
        &[
            stream_segment(CollectionId(0), 1, &fixture.active, 1),
            stream_segment(CollectionId(0), 1, &fixture.active, 2),
        ],
    );
    assert!(error.unwrap_err().contains("duplicate segment identity"));
    assert!(fixture.store.read_generation().unwrap().is_none());
}

#[test]
fn generation_must_advance_and_expected_generation_must_match() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    assert!(matches!(
        fixture.store.commit_generation(Some(1), 1, &[]),
        Err(error) if error.contains("monotonically")
    ));
    assert!(matches!(
        fixture.store.commit_generation(None, 2, &[]),
        Err(error) if error.contains("changed during publication")
    ));
    assert!(matches!(
        fixture.store.commit_generation(Some(1), 0, &[]),
        Err(error) if error.contains("invalid generation")
    ));
}

#[test]
fn catalog_is_optional_and_recorded_when_present() {
    let fixture = TestStore::new();
    let catalog: Arc<[u8]> = Arc::from(&b"OBJHCAT1-catalog"[..]);
    fixture
        .store
        .commit_generation_with_catalog(None, 1, &[], Arc::clone(&catalog))
        .unwrap();
    let text = fs::read_to_string(fixture.store.path().join(MANIFEST_NAME)).unwrap();
    assert!(text.contains("catalog = \"generations/0000000000000001/catalog.bin\""));
    let opened = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(opened.catalog.as_deref(), Some(&catalog[..]));
}

#[test]
fn explicit_publication_replaces_an_incompatible_binary_manifest() {
    let fixture = TestStore::new();
    fs::write(
        fixture.store.path().join(LEGACY_MANIFEST_NAME),
        b"OBJHMAN1 obsolete",
    )
    .unwrap();
    commit_one(&fixture, None, 1);
    assert!(!fixture.store.path().join(LEGACY_MANIFEST_NAME).exists());
    assert!(fixture.store.path().join(MANIFEST_NAME).exists());
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        1
    );
}

#[test]
fn manifest_accepts_comments_and_reordered_fields() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let path = fixture.store.path().join(MANIFEST_NAME);
    let text = fs::read_to_string(&path).unwrap();
    let mut lines = text.lines().map(str::to_owned).collect::<Vec<_>>();
    let version = lines.remove(0);
    let generation = lines.remove(0);
    lines.insert(0, "# hand-edited manifest".into());
    lines.insert(1, generation);
    lines.insert(2, version);
    fs::write(&path, lines.join("\n") + "\n").unwrap();
    assert!(fixture.store.read_generation().unwrap().is_some());
}

#[test]
fn manifest_rejects_unknown_fields() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let path = fixture.store.path().join(MANIFEST_NAME);
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{text}\nmisspelled = true\n")).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Corrupt(_))
    ));
}

#[test]
fn manifest_cannot_verify_itself() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let mut manifest = fixture.store.read_manifest().unwrap().unwrap();
    manifest.verify.insert(
        MANIFEST_NAME.into(),
        VerifyRecord {
            length: 1,
            sha256: "00".repeat(32),
        },
    );
    fs::write(
        fixture.store.path().join(MANIFEST_NAME),
        toml::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Corrupt(_))
    ));
}

#[test]
fn optional_verification_accepts_matching_length_and_digest() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let segment_path = {
        let opened = fixture.store.read_generation().unwrap().unwrap();
        opened
            .generation_directory
            .join("segments")
            .join(&opened.segments[0].name)
    };
    let relative = segment_path
        .strip_prefix(fixture.store.path())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let bytes = fs::read(&segment_path).unwrap();
    let mut manifest = fixture.store.read_manifest().unwrap().unwrap();
    manifest.verify.insert(
        relative,
        VerifyRecord {
            length: bytes.len() as u64,
            sha256: encode_hex(&Sha256::digest(&bytes)),
        },
    );
    fs::write(
        fixture.store.path().join(MANIFEST_NAME),
        toml::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    assert!(fixture.store.read_generation().unwrap().is_some());
}

#[test]
fn optional_verification_rejects_wrong_length_and_digest() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let segment_path = {
        let opened = fixture.store.read_generation().unwrap().unwrap();
        opened
            .generation_directory
            .join("segments")
            .join(&opened.segments[0].name)
    };
    let relative = segment_path
        .strip_prefix(fixture.store.path())
        .unwrap()
        .to_string_lossy()
        .replace('\\', "/");
    let bytes = fs::read(&segment_path).unwrap();
    let mut manifest = fixture.store.read_manifest().unwrap().unwrap();
    manifest.verify.insert(
        relative.clone(),
        VerifyRecord {
            length: bytes.len() as u64 + 1,
            sha256: encode_hex(&Sha256::digest(&bytes)),
        },
    );
    fs::write(
        fixture.store.path().join(MANIFEST_NAME),
        toml::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Corrupt(_))
    ));

    let mut manifest = fixture.store.read_manifest().unwrap().unwrap();
    manifest.verify.insert(
        relative,
        VerifyRecord {
            length: bytes.len() as u64,
            sha256: "00".repeat(32),
        },
    );
    fs::write(
        fixture.store.path().join(MANIFEST_NAME),
        toml::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Corrupt(_))
    ));
}

#[test]
fn manifest_rejects_absolute_and_escaping_paths() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    for replacement in ["/tmp/segments", "../segments", "generations/../segments"] {
        let text = fs::read_to_string(fixture.store.path().join(MANIFEST_NAME)).unwrap();
        let start = text.find("segments = \"").unwrap();
        let end = text[start..].find('"').unwrap() + start;
        let mut changed = text.clone();
        changed.replace_range(start..=end, &format!("segments = \"{replacement}\""));
        fs::write(fixture.store.path().join(MANIFEST_NAME), changed).unwrap();
        assert!(matches!(
            fixture.store.read_generation(),
            Err(StoreReadError::Corrupt(_))
        ));
    }
}

#[test]
fn directory_order_does_not_affect_discovered_identity() {
    let fixture = TestStore::new();
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[
                stream_segment(CollectionId(0), 2, &fixture.active, 2),
                stream_segment(CollectionId(0), 1, &fixture.active, 1),
            ],
        )
        .unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(
        opened
            .segments
            .iter()
            .map(|segment| segment.descriptor.ordinal)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn two_generations_write_independent_files() {
    let fixture = TestStore::new();
    let first = commit_one(&fixture, None, 1);
    let second = commit_one(&fixture, Some(1), 2);
    assert_ne!(first.segment_names, second.segment_names);
    let first_path = fixture
        .store
        .generation_directory(1)
        .join("segments")
        .join(&first.segment_names[0]);
    let second_path = fixture
        .store
        .generation_directory(2)
        .join("segments")
        .join(&second.segment_names[0]);
    assert_ne!(first_path, second_path);
    assert_ne!(fs::metadata(&first_path).unwrap().len(), 0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        assert_ne!(
            fs::metadata(&first_path).unwrap().ino(),
            fs::metadata(&second_path).unwrap().ino()
        );
    }
}

#[test]
fn interrupted_temporary_manifest_is_ignored_and_reclaimed() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let temporary = fixture.store.path().join("manifest-00000000deadbeef.tmp");
    fs::write(&temporary, b"version = 1\ngeneration = 9\n").unwrap();
    assert!(fixture.store.read_generation().unwrap().is_some());
    commit_one(&fixture, Some(1), 2);
    assert!(!temporary.exists());
}

#[test]
fn rejects_unknown_existing_generation_and_generation_rewind() {
    let fixture = TestStore::new();
    let error = fixture
        .store
        .commit_generation(Some(7), 8, &[])
        .unwrap_err();
    assert!(error.contains("changed during publication"));
    assert!(fixture.store.read_generation().unwrap().is_none());
}

#[test]
fn stale_writer_cannot_replace_a_newer_durable_generation() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    commit_one(&fixture, Some(1), 2);
    assert!(matches!(
        fixture.store.commit_generation(Some(1), 3, &[]),
        Err(error) if error.contains("changed during publication")
    ));
    assert!(matches!(
        fixture.store.commit_generation(Some(2), 2, &[]),
        Err(error) if error.contains("monotonically")
    ));
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        2
    );
}

#[test]
fn removes_only_recognized_deprecated_snapshots_in_selected_scope() {
    let fixture = TestStore::new();
    let selected = &fixture.store.deprecated_directories[0];
    let old_name = selected.join(DEPRECATED_NAMES[0]);
    let mut old = Vec::new();
    old.extend_from_slice(DEPRECATED_MAGIC);
    old.extend_from_slice(&DEPRECATED_VERSIONS[0].to_le_bytes());
    fs::write(&old_name, old).unwrap();
    let removed = fixture.store.cleanup_deprecated_snapshots().unwrap();
    assert_eq!(removed, vec![old_name]);
}

#[test]
fn unknown_candidate_blocks_cleanup_without_deleting_known_old_file() {
    let fixture = TestStore::new();
    let selected = &fixture.store.deprecated_directories[0];
    let known = selected.join(DEPRECATED_NAMES[0]);
    let mut old = Vec::new();
    old.extend_from_slice(DEPRECATED_MAGIC);
    old.extend_from_slice(&DEPRECATED_VERSIONS[0].to_le_bytes());
    fs::write(&known, old).unwrap();
    let unknown = selected.join(DEPRECATED_NAMES[1]);
    fs::write(&unknown, b"UNKNOWN!").unwrap();
    assert!(fixture.store.cleanup_deprecated_snapshots().is_err());
    assert!(known.exists());
    assert!(unknown.exists());
}

#[test]
fn refuses_symlinked_managed_namespace() {
    let base = std::env::temp_dir().join(format!(
        "objekte-hybrid-store-symlink-{}-{}",
        std::process::id(),
        unique_nonce().unwrap()
    ));
    fs::create_dir(&base).unwrap();
    let root = base.join("source");
    let destination = base.join("destination");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&destination).unwrap();
    let resolved = IndexLocationResolver {
        location: IndexStorageLocation::CustomDirectory(destination.clone()),
    }
    .resolve(
        &root,
        &[CollectionDefinition::folder(
            "files",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
        &SourceRegistry::builtin(),
    )
    .unwrap();
    let directory = resolved.managed_directory.clone().unwrap();
    let parent = directory.parent().unwrap();
    fs::create_dir_all(parent).unwrap();
    let elsewhere = base.join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&elsewhere, &directory).unwrap();
    #[cfg(not(unix))]
    fs::create_dir(&directory).unwrap();
    assert!(ManagedStore::open(&resolved).is_err());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn reader_rejects_symlinked_generations_root() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let generations = fixture.store.path().join(GENERATIONS_DIR);
    let moved = fixture.base.join("moved-generations");
    fs::rename(&generations, &moved).unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&moved, &generations).unwrap();
    #[cfg(not(unix))]
    fs::create_dir(&generations).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Corrupt(_))
    ));
}

#[test]
fn gc_leaves_unknown_and_symlinked_generation_entries_untouched() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let root = fixture.store.path().join(GENERATIONS_DIR);
    let unknown = root.join("unknown-entry");
    fs::create_dir(&unknown).unwrap();
    fs::write(unknown.join("keep"), b"keep").unwrap();
    let old = SystemTime::now() - Duration::from_secs(3600);
    filetime_set(&unknown, old);
    commit_one(&fixture, Some(1), 2);
    assert!(unknown.exists());
    assert!(unknown.join("keep").exists());
}

fn filetime_set(path: &Path, time: SystemTime) {
    let file = File::open(path).unwrap();
    file.set_times(fs::FileTimes::new().set_modified(time))
        .unwrap();
}

#[test]
fn grace_gc_keeps_eight_generations_and_skips_young_files() {
    let fixture = TestStore::new();
    for generation in 1u64..=10 {
        let expected = generation.checked_sub(1).filter(|value| *value != 0);
        fixture
            .store
            .commit_generation(expected, generation, &[])
            .unwrap();
    }
    for generation in 1..=10 {
        assert!(fixture.store.generation_directory(generation).exists());
    }
    let oldest = fixture.store.generation_directory(1);
    let old = SystemTime::now() - Duration::from_secs(600);
    filetime_set(&oldest, old);
    commit_one(&fixture, Some(10), 11);
    assert!(!oldest.exists());
    assert!(fixture.store.generation_directory(10).exists());
}

#[test]
fn reader_pins_complete_generation_during_concurrent_publications() {
    let fixture = TestStore::new();
    commit_one(&fixture, None, 1);
    let pinned = fixture.store.read_generation().unwrap().unwrap();
    let pinned_path = pinned
        .generation_directory
        .join("segments")
        .join(&pinned.segments[0].name);
    assert!(pinned_path.exists());
    commit_one(&fixture, Some(1), 2);
    assert!(pinned_path.exists());
    assert_eq!(pinned.segments.len(), 1);
    assert!(
        pinned.segments[0]
            .reader
            .section(SectionKind::PostingPayloads)
            .is_ok()
    );
}

#[test]
fn abandoned_generation_directory_is_reclaimed_after_grace() {
    let fixture = TestStore::new();
    let root = fixture.store.path().join(GENERATIONS_DIR);
    let abandoned = root.join(format!(".tmp-{:016x}-{:016x}", 3, 4));
    fs::create_dir_all(abandoned.join("segments")).unwrap();
    let old = SystemTime::now() - Duration::from_secs(600);
    filetime_set(&abandoned, old);
    commit_one(&fixture, None, 1);
    assert!(!abandoned.exists());
}
