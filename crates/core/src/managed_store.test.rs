use super::*;
use crate::index_storage::{IndexStorageLocation, IndexLocationResolver};
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::segment_format::{REQUIRED_SECTIONS, SectionKind, SegmentWriter};
use crate::source::{SourceFormat, SourceRegistry};
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
    fs::write(&lock, b"").unwrap();
    fs::remove_file(&lock).unwrap();
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
                write: Box::new(|_| Err("injected write failure".into())),
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
fn supported_corruption_and_unsupported_version_have_different_kinds() {
    let fixture = TestStore::new();
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 1,
                bytes: Arc::from(complete_segment(3)),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let path = fixture.store.path().join(MANIFEST_NAME);
    let original = fs::read(&path).unwrap();
    let mut corrupt = original.clone();
    corrupt[20] ^= 1;
    fs::write(&path, corrupt).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Corrupt(_))
    ));
    let mut unsupported = original.clone();
    unsupported[8..12].copy_from_slice(&(MANIFEST_VERSION + 1).to_le_bytes());
    let end = unsupported.len();
    let checksum = Sha256::digest(&unsupported[..end - 32]);
    unsupported[end - 32..].copy_from_slice(&checksum);
    fs::write(&path, unsupported).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Incompatible(_))
    ));
}

#[test]
fn opening_validates_metadata_but_defers_posting_payload_integrity_on_demand() {
    let fixture = TestStore::new();
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 2,
                bytes: Arc::from(complete_segment(5)),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    assert!(!opened.segments[0].is_owned());
    let path = opened
        .generation_directory
        .join("segments")
        .join(&opened.segment_names[0]);
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
            .section(SectionKind::PostingPayloads)
            .is_err()
    );
}

impl Drop for TestStore {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.base).unwrap();
    }
}

fn complete_segment(marker: u8) -> Vec<u8> {
    let mut writer = SegmentWriter::new(Cursor::new(Vec::new()), REQUIRED_SECTIONS.len()).unwrap();
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

#[test]
fn streamed_segment_failure_keeps_previous_generation_and_allows_retry() {
    let fixture = TestStore::new();
    let first: Arc<[u8]> = Arc::from(complete_segment(1));
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 1,
                bytes: first,
                active: &fixture.active,
            }],
        )
        .unwrap();
    let failure = fixture.store.commit_generation(
        Some(1),
        2,
        &[SegmentInput::NewStream {
            origin: SegmentOrigin::Unvalidated,
            collection: CollectionId(0),
            ordinal: 2,
            active: &fixture.active,
            write: Box::new(|file| {
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
    let bytes = complete_segment(2);
    let committed = fixture
        .store
        .commit_generation(
            Some(1),
            2,
            &[SegmentInput::NewStream {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 2,
                active: &fixture.active,
                write: Box::new(|file| file.write_all(&bytes).map_err(|error| error.to_string())),
            }],
        )
        .unwrap();
    assert_eq!(committed.segment_bytes_written, bytes.len() as u64);
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        2
    );
}

#[test]
fn publishes_new_and_reused_segments_without_rewriting_old_bytes() {
    let fixture = TestStore::new();
    assert!(fixture.store.read_generation().unwrap().is_none());
    let first: Arc<[u8]> = Arc::from(complete_segment(1));
    let second: Arc<[u8]> = Arc::from(complete_segment(2));
    let third: Arc<[u8]> = Arc::from(complete_segment(3));
    let committed = fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 3,
                bytes: Arc::clone(&first),
                active: &fixture.active,
            }],
        )
        .unwrap();
    assert_eq!(committed.segment_bytes_written, first.len() as u64);
    let pinned = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(pinned.generation, 1);
    let old_name = pinned.segment_names[0].clone();
    let committed = fixture
        .store
        .commit_generation(
            Some(1),
            2,
            &[
                SegmentInput::Existing {
                    name: &old_name,
                    active: &fixture.active,
                },
                SegmentInput::New {
                    origin: SegmentOrigin::Unvalidated,
                    collection: CollectionId(0),
                    ordinal: 4,
                    bytes: Arc::clone(&second),
                    active: &fixture.active,
                },
            ],
        )
        .unwrap();
    assert_eq!(committed.segments_reused, 1);
    assert_eq!(committed.segment_bytes_written, second.len() as u64);
    let current = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(current.generation, 2);
    assert_eq!(current.segments.len(), 2);
    assert_eq!(
        current.identities,
        vec![(CollectionId(0), 3), (CollectionId(0), 4)]
    );
    let unknown = fixture.store.path().join("ordinary-user-file.txt");
    fs::write(&unknown, b"preserve").unwrap();
    let orphan = fixture
        .store
        .path()
        .join("segment-0000000000000001-00000000-0000000000000000.idx");
    fs::write(&orphan, complete_segment(8)).unwrap();
    let incomplete_manifest = fixture.store.path().join("manifest-0000000000000000.tmp");
    fs::write(&incomplete_manifest, b"OBJHMAN1partial").unwrap();
    let committed = fixture
        .store
        .commit_generation(
            Some(2),
            3,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 5,
                bytes: Arc::clone(&third),
                active: &fixture.active,
            }],
        )
        .unwrap();
    assert_eq!(committed.orphan_files_reclaimed, 0);
    assert!(committed.reclaim_errors.is_empty());
    assert!(unknown.is_file());
    assert!(orphan.exists());
    assert!(incomplete_manifest.exists());
    assert!(
        pinned
            .generation_directory
            .join("segments")
            .join(old_name)
            .exists()
    );
    assert_eq!(
        pinned.segments[0]
            .section(SectionKind::FieldMetadata)
            .unwrap(),
        Some(&[1][..])
    );
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        3
    );
}

#[test]
fn catalog_sidecar_is_atomically_replaced_checked_and_reclaimed() {
    let fixture = TestStore::new();
    let first: Arc<[u8]> = Arc::from(&b"OBJHCAT1-first"[..]);
    let second: Arc<[u8]> = Arc::from(&b"OBJHCAT1-second"[..]);
    fixture
        .store
        .commit_generation_with_catalog(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 6,
                bytes: Arc::from(complete_segment(1)),
                active: &fixture.active,
            }],
            Arc::clone(&first),
        )
        .unwrap();
    let pinned = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(pinned.catalog.as_deref(), Some(first.as_ref()));
    let manifest = fixture.store.read_manifest().unwrap().unwrap();
    let old_catalog = pinned
        .generation_directory
        .join(&manifest.catalog.unwrap().name);
    let old_segment = pinned.segment_names[0].clone();
    let mut corrupted = fs::read(&old_catalog).unwrap();
    corrupted[8] ^= 1;
    fs::write(&old_catalog, corrupted).unwrap();
    assert!(
        fixture
            .store
            .read_generation()
            .err()
            .unwrap()
            .to_string()
            .contains("catalog checksum mismatch")
    );
    fs::write(&old_catalog, first.as_ref()).unwrap();

    let stats = fixture
        .store
        .commit_generation_with_catalog(
            Some(1),
            2,
            &[SegmentInput::Existing {
                name: &old_segment,
                active: &fixture.active,
            }],
            Arc::clone(&second),
        )
        .unwrap();
    assert_eq!(stats.segments_reused, 1);
    assert!(old_catalog.exists());
    assert_eq!(pinned.catalog.as_deref(), Some(first.as_ref()));
    let current = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(current.catalog.as_deref(), Some(second.as_ref()));
}

#[test]
fn truncated_manifest_is_an_error_and_interrupted_temp_is_ignored() {
    let fixture = TestStore::new();
    let bytes: Arc<[u8]> = Arc::from(complete_segment(7));
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 7,
                bytes: Arc::clone(&bytes),
                active: &fixture.active,
            }],
        )
        .unwrap();
    fs::write(
        fixture.store.path().join("manifest-interrupted.tmp"),
        b"partial",
    )
    .unwrap();
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        1
    );
    fs::write(fixture.store.path().join(MANIFEST_NAME), b"truncated").unwrap();
    assert!(fixture.store.read_generation().is_err());
}

#[test]
fn rejects_unknown_existing_segment_and_generation_rewind() {
    let fixture = TestStore::new();
    let bytes: Arc<[u8]> = Arc::from(complete_segment(1));
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 8,
                bytes: Arc::clone(&bytes),
                active: &fixture.active,
            }],
        )
        .unwrap();
    assert!(fixture.store.commit_generation(Some(1), 1, &[]).is_err());
    assert!(
        fixture
            .store
            .commit_generation(
                Some(1),
                2,
                &[SegmentInput::Existing {
                    name: "segment-evil.idx",
                    active: &fixture.active
                }]
            )
            .is_err()
    );
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        1
    );
}

#[test]
fn duplicate_segment_identity_cannot_replace_durable_generation() {
    let fixture = TestStore::new();
    let bytes: Arc<[u8]> = Arc::from(complete_segment(3));
    let result = fixture.store.commit_generation(
        None,
        1,
        &[
            SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                bytes: Arc::clone(&bytes),
                collection: CollectionId(0),
                ordinal: 7,
                active: &fixture.active,
            },
            SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                bytes,
                collection: CollectionId(0),
                ordinal: 7,
                active: &fixture.active,
            },
        ],
    );
    assert!(result.err().unwrap().contains("duplicate segment identity"));
    assert!(fixture.store.read_generation().unwrap().is_none());
}

#[test]
fn stale_writer_cannot_replace_a_newer_durable_generation() {
    let fixture = TestStore::new();
    let first: Arc<[u8]> = Arc::from(complete_segment(1));
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 9,
                bytes: first,
                active: &fixture.active,
            }],
        )
        .unwrap();
    let second: Arc<[u8]> = Arc::from(complete_segment(2));
    assert!(
        fixture
            .store
            .commit_generation(
                None,
                2,
                &[SegmentInput::New {
                    origin: SegmentOrigin::Unvalidated,
                    collection: CollectionId(0),
                    ordinal: 10,
                    bytes: Arc::clone(&second),
                    active: &fixture.active,
                }]
            )
            .unwrap_err()
            .contains("durable generation changed")
    );
    fixture
        .store
        .commit_generation(
            Some(1),
            2,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 11,
                bytes: second,
                active: &fixture.active,
            }],
        )
        .unwrap();
    let third: Arc<[u8]> = Arc::from(complete_segment(3));
    assert!(
        fixture
            .store
            .commit_generation(
                Some(1),
                3,
                &[SegmentInput::New {
                    origin: SegmentOrigin::Unvalidated,
                    collection: CollectionId(0),
                    ordinal: 12,
                    bytes: third,
                    active: &fixture.active,
                }]
            )
            .unwrap_err()
            .contains("durable generation changed")
    );
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        2
    );
}

#[test]
fn reused_segment_publishes_retirement_mask_without_rewriting_bytes() {
    let fixture = TestStore::new();
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 13,
                bytes: Arc::from(complete_segment(6)),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let pinned = fixture.store.read_generation().unwrap().unwrap();
    let name = pinned.segment_names[0].clone();
    let old_bytes = fs::read(pinned.generation_directory.join("segments").join(&name)).unwrap();
    let retired =
        ActiveRecords::from_bitmap(16, RoaringBitmap::from_iter([0, 1, 3, 8, 15])).unwrap();
    let stats = fixture
        .store
        .commit_generation(
            Some(1),
            2,
            &[SegmentInput::Existing {
                name: &name,
                active: &retired,
            }],
        )
        .unwrap();
    assert_eq!(stats.segment_bytes_written, 0);
    assert_eq!(stats.segments_reused, 1);
    assert_eq!(pinned.active[0].len(), 16);
    let current = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(current.active[0].slot_count(), 16);
    assert_eq!(current.active[0].bitmap(), retired.bitmap());
    assert_eq!(current.segment_names, vec![name.clone()]);
    assert_eq!(
        fs::read(current.generation_directory.join("segments").join(&name)).unwrap(),
        old_bytes
    );
    let wrong_slots = ActiveRecords::all(17).unwrap();
    assert!(
        fixture
            .store
            .commit_generation(
                Some(2),
                3,
                &[SegmentInput::Existing {
                    name: &name,
                    active: &wrong_slots,
                }]
            )
            .unwrap_err()
            .contains("active slot count changed")
    );
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        2
    );
}

#[test]
fn active_manifest_semantics_are_checked_after_checksum() {
    let fixture = TestStore::new();
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 14,
                bytes: Arc::from(complete_segment(6)),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let path = fixture.store.path().join(MANIFEST_NAME);
    let original = fs::read(&path).unwrap();
    let mut invalid = original.clone();
    let kind = invalid.len() - 32 - 1;
    invalid[kind] = 2;
    let digest = Sha256::digest(&invalid[..invalid.len() - 32]);
    let end = invalid.len();
    invalid[end - 32..].copy_from_slice(&digest);
    fs::write(&path, invalid).unwrap();
    assert!(
        fixture
            .store
            .read_generation()
            .err()
            .unwrap()
            .to_string()
            .contains("active mask kind invalid")
    );
    fs::write(&path, original).unwrap();
    assert!(fixture.store.read_generation().is_ok());
}

#[test]
fn removes_only_recognized_deprecated_snapshots_in_selected_scope() {
    let fixture = TestStore::new();
    let selected = fixture.base.join("destination");
    let elsewhere = fixture.base.join("source");
    let old_ascii = selected.join(DEPRECATED_NAMES[0]);
    let old_unicode = selected.join(DEPRECATED_NAMES[1]);
    let unrelated = selected.join("user-data.txt");
    let other_repository = elsewhere.join(DEPRECATED_NAMES[0]);
    for (path, version) in [
        (&old_ascii, 27u32),
        (&old_unicode, 28u32),
        (&other_repository, 27u32),
    ] {
        let mut bytes = Vec::from(DEPRECATED_MAGIC.as_slice());
        bytes.extend_from_slice(&version.to_le_bytes());
        bytes.extend_from_slice(b"old bytes");
        fs::write(path, bytes).unwrap();
    }
    fs::write(&unrelated, b"keep").unwrap();
    let removed = fixture.store.cleanup_deprecated_snapshots().unwrap();
    assert_eq!(removed, vec![old_ascii.clone(), old_unicode.clone()]);
    assert!(!old_ascii.exists() && !old_unicode.exists());
    assert_eq!(fs::read(unrelated).unwrap(), b"keep");
    assert!(other_repository.is_file());
    assert!(
        fixture
            .store
            .cleanup_deprecated_snapshots()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn unknown_candidate_blocks_cleanup_without_deleting_known_old_file() {
    let fixture = TestStore::new();
    let selected = fixture.base.join("destination");
    let old_ascii = selected.join(DEPRECATED_NAMES[0]);
    let newer_unicode = selected.join(DEPRECATED_NAMES[1]);
    let mut old = Vec::from(DEPRECATED_MAGIC.as_slice());
    old.extend_from_slice(&27u32.to_le_bytes());
    fs::write(&old_ascii, old).unwrap();
    let mut newer = Vec::from(DEPRECATED_MAGIC.as_slice());
    newer.extend_from_slice(&29u32.to_le_bytes());
    fs::write(&newer_unicode, newer).unwrap();
    assert!(
        fixture
            .store
            .cleanup_deprecated_snapshots()
            .unwrap_err()
            .contains("unsupported index version 29")
    );
    assert!(old_ascii.is_file() && newer_unicode.is_file());
    fs::write(&newer_unicode, b"user content").unwrap();
    assert!(
        fixture
            .store
            .cleanup_deprecated_snapshots()
            .unwrap_err()
            .contains("unknown index format")
    );
    assert!(old_ascii.is_file() && newer_unicode.is_file());
}

#[cfg(unix)]
#[test]
fn deprecated_snapshot_symlink_is_not_followed() {
    use std::os::unix::fs::symlink;
    let fixture = TestStore::new();
    let target = fixture.base.join("user-data.txt");
    fs::write(&target, b"keep").unwrap();
    let link = fixture.base.join("destination").join(DEPRECATED_NAMES[0]);
    symlink(&target, &link).unwrap();
    assert!(
        fixture
            .store
            .cleanup_deprecated_snapshots()
            .unwrap_err()
            .contains("unsafe deprecated index candidate")
    );
    assert_eq!(fs::read(target).unwrap(), b"keep");
    assert!(fs::symlink_metadata(link).unwrap().file_type().is_symlink());
}

#[cfg(unix)]
#[test]
fn obsolete_symlink_is_not_followed_or_removed() {
    use std::os::unix::fs::symlink;
    let fixture = TestStore::new();
    let first: Arc<[u8]> = Arc::from(complete_segment(1));
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 15,
                bytes: Arc::clone(&first),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    let name = opened.segment_names[0].clone();
    let path = opened.generation_directory.join("segments").join(&name);
    fs::remove_file(&path).unwrap();
    let target = fixture.base.join("user-data.txt");
    fs::write(&target, b"keep").unwrap();
    symlink(&target, &path).unwrap();
    let second: Arc<[u8]> = Arc::from(complete_segment(2));
    let stats = fixture
        .store
        .commit_generation(
            Some(1),
            2,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 16,
                bytes: second,
                active: &fixture.active,
            }],
        )
        .unwrap();
    assert!(stats.reclaim_errors.is_empty());
    assert!(
        fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read(target).unwrap(), b"keep");
}

#[cfg(unix)]
#[test]
fn refuses_symlinked_managed_namespace() {
    use std::os::unix::fs::symlink;
    let base = std::env::temp_dir().join(format!(
        "objekte-hybrid-symlink-test-{}-{}",
        std::process::id(),
        unique_nonce().unwrap()
    ));
    fs::create_dir(&base).unwrap();
    let root = base.join("source");
    let destination = base.join("destination");
    let other = base.join("other");
    fs::create_dir(&root).unwrap();
    fs::create_dir(&destination).unwrap();
    fs::create_dir(&other).unwrap();
    symlink(&other, destination.join("objekte-indexes")).unwrap();
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
    assert!(ManagedStore::open(&resolved).is_err());
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn refuses_writable_owned_namespace_before_creating_generation_children() {
    use std::os::unix::fs::PermissionsExt;
    let base = std::env::temp_dir().join(format!(
        "objekte-hybrid-writable-namespace-{}-{}",
        std::process::id(),
        unique_nonce().unwrap()
    ));
    let root = base.join("source");
    let destination = base.join("destination");
    fs::create_dir_all(&root).unwrap();
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
    let owned = resolved.owned_namespace_root.as_ref().unwrap();
    fs::create_dir(owned).unwrap();
    fs::set_permissions(owned, fs::Permissions::from_mode(0o777)).unwrap();
    assert!(
        ManagedStore::open(&resolved)
            .err()
            .unwrap()
            .contains("unsafe or unowned managed store namespace")
    );
    assert!(!resolved.managed_directory.as_ref().unwrap().exists());
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn refuses_writable_lock_and_segment_files() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = TestStore::new();
    let lock = fixture.store.path().join(LOCK_NAME);
    fs::write(&lock, b"").unwrap();
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(fixture.store.read_generation().unwrap().is_none());
    assert!(fixture.store.commit_generation(None, 1, &[]).is_err());
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                origin: SegmentOrigin::Unvalidated,
                collection: CollectionId(0),
                ordinal: 17,
                bytes: Arc::from(complete_segment(1)),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    let segment = opened
        .generation_directory
        .join("segments")
        .join(&opened.segment_names[0]);
    fs::set_permissions(&segment, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(
        fixture
            .store
            .read_generation()
            .err()
            .unwrap()
            .to_string()
            .contains("owned regular file")
    );
}

#[test]
fn concurrent_first_open_converges_on_one_safe_namespace() {
    let base = std::env::temp_dir().join(format!(
        "objekte-hybrid-concurrent-open-{}-{}",
        std::process::id(),
        unique_nonce().unwrap()
    ));
    let root = base.join("source");
    let destination = base.join("destination");
    fs::create_dir_all(&root).unwrap();
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
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let workers = (0..8)
        .map(|_| {
            let resolved = resolved.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                ManagedStore::open(&resolved).map(|store| store.path().to_path_buf())
            })
        })
        .collect::<Vec<_>>();
    for worker in workers {
        assert_eq!(
            worker.join().unwrap().unwrap(),
            resolved.managed_directory.clone().unwrap()
        );
    }
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn reused_segment_is_hard_linked_without_rewriting_bytes() {
    use std::os::unix::fs::MetadataExt;
    let fixture = TestStore::new();
    let first = fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                bytes: Arc::from(complete_segment(8)),
                collection: CollectionId(0),
                ordinal: 1,
                active: &fixture.active,
                origin: SegmentOrigin::Validated,
            }],
        )
        .unwrap();
    let name = &first.segment_names[0];
    let second = fixture
        .store
        .commit_generation(
            Some(1),
            2,
            &[SegmentInput::Existing {
                name,
                active: &fixture.active,
            }],
        )
        .unwrap();
    let old = fixture
        .store
        .generation_directory(1)
        .join("segments")
        .join(name);
    let new = fixture
        .store
        .generation_directory(2)
        .join("segments")
        .join(name);
    assert_eq!(
        fs::metadata(&old).unwrap().ino(),
        fs::metadata(&new).unwrap().ino()
    );
    assert_eq!(fs::metadata(&new).unwrap().nlink(), 2);
    assert_eq!(second.segment_bytes_written, 0);
    assert_eq!(second.segment_links_created, 1);
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().validation,
        vec![SegmentTrust::Stamped]
    );
}

#[test]
fn grace_gc_keeps_eight_generations_and_skips_young_files() {
    let fixture = TestStore::new();
    for generation in 1..=13 {
        fixture
            .store
            .commit_generation((generation > 1).then_some(generation - 1), generation, &[])
            .unwrap();
    }
    for generation in 1..=13 {
        assert!(fixture.store.generation_directory(generation).exists());
        let path = fixture.store.generation_directory(generation);
        File::open(&path)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(SystemTime::now() - MIN_RETENTION_AGE - Duration::from_secs(1)),
            )
            .unwrap();
    }
    let stats = fixture.store.commit_generation(Some(13), 14, &[]).unwrap();
    assert_eq!(stats.generations_removed, 6);
    for generation in 1..=6 {
        assert!(!fixture.store.generation_directory(generation).exists());
    }
    for generation in 7..=14 {
        assert!(fixture.store.generation_directory(generation).exists());
    }
}

#[test]
fn thirty_two_updates_reclaim_aged_generations_with_bounded_work() {
    let fixture = TestStore::new();
    let mut removed = 0;
    for generation in 1..=33 {
        let stats = fixture
            .store
            .commit_generation((generation > 1).then_some(generation - 1), generation, &[])
            .unwrap();
        removed += stats.generations_removed;
        assert!(stats.reclaim_errors.is_empty());
        let path = fixture.store.generation_directory(generation);
        File::open(&path)
            .unwrap()
            .set_times(
                fs::FileTimes::new()
                    .set_modified(SystemTime::now() - MIN_RETENTION_AGE - Duration::from_secs(1)),
            )
            .unwrap();
    }
    assert_eq!(removed, 25);
    let retained = fs::read_dir(fixture.store.path().join(GENERATIONS_DIR))
        .unwrap()
        .count();
    assert_eq!(retained, RETAINED_GENERATIONS as usize);
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        33
    );
}

#[cfg(unix)]
#[test]
fn reader_rejects_symlinked_generations_root() {
    use std::os::unix::fs::symlink;

    let fixture = TestStore::new();
    fixture.store.commit_generation(None, 1, &[]).unwrap();
    let root = fixture.store.path().join(GENERATIONS_DIR);
    let original = fixture.store.path().join("held-generations");
    fs::rename(&root, &original).unwrap();
    symlink(&original, &root).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Corrupt(_))
    ));
}

#[test]
fn stamp_upgrade_is_metadata_only_and_cannot_overwrite_newer_generation() {
    let fixture = TestStore::new();
    let result = fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                bytes: Arc::from(complete_segment(2)),
                collection: CollectionId(0),
                ordinal: 1,
                active: &fixture.active,
                origin: SegmentOrigin::Unvalidated,
            }],
        )
        .unwrap();
    let name = result.segment_names[0].clone();
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().validation,
        vec![SegmentTrust::Untrusted]
    );
    let path = fixture
        .store
        .generation_directory(1)
        .join("segments")
        .join(&name);
    let before = fs::metadata(&path).unwrap().len();
    assert!(
        fixture
            .store
            .record_validation_stamps(1, &[(name.clone(), before, VALIDATOR_VERSION)])
            .unwrap()
    );
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().validation,
        vec![SegmentTrust::Stamped]
    );
    fixture
        .store
        .commit_generation(
            Some(1),
            2,
            &[SegmentInput::Existing {
                name: &name,
                active: &fixture.active,
            }],
        )
        .unwrap();
    assert!(
        !fixture
            .store
            .record_validation_stamps(1, &[(name, before, VALIDATOR_VERSION)])
            .unwrap()
    );
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        2
    );
}

#[test]
fn gc_leaves_unknown_and_symlinked_generation_entries_untouched() {
    let fixture = TestStore::new();
    fixture.store.commit_generation(None, 1, &[]).unwrap();
    let path = fixture.store.generation_directory(1).join("unknown.txt");
    fs::write(&path, b"user data").unwrap();
    File::open(fixture.store.generation_directory(1))
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(SystemTime::now() - MIN_RETENTION_AGE - Duration::from_secs(1)),
        )
        .unwrap();
    let mut last = 1;
    for generation in 2..=9 {
        let stats = fixture
            .store
            .commit_generation(Some(last), generation, &[])
            .unwrap();
        last = generation;
        if generation == 9 {
            assert!(!stats.reclaim_errors.is_empty());
        }
    }
    assert_eq!(fs::read(path).unwrap(), b"user data");
}

fn prepare_flat_v1_store(fixture: &TestStore, omit_second: bool) -> (Vec<String>, Vec<u8>) {
    let names = vec![
        "segment-0000000000000001-00000000-0000000000000001.idx".to_string(),
        "segment-0000000000000001-00000001-0000000000000001.idx".to_string(),
    ];
    let segments = [complete_segment(1), complete_segment(2)];
    for (index, (name, bytes)) in names.iter().zip(&segments).enumerate() {
        if index != 1 || !omit_second {
            fs::write(fixture.store.path().join(name), bytes).unwrap();
        }
    }
    let catalog_name = "catalog-0000000000000001-0000000000000001.bin";
    let catalog = b"OBJHCAT1-flat".to_vec();
    fs::write(fixture.store.path().join(catalog_name), &catalog).unwrap();
    let store = &fixture.store;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(MANIFEST_MAGIC);
    bytes.extend_from_slice(&1u32.to_le_bytes());
    bytes.extend_from_slice(&crate::MODEL_VERSION.to_le_bytes());
    bytes.extend_from_slice(&store.repository_identity);
    bytes.extend_from_slice(&store.configuration_identity);
    bytes.extend_from_slice(&(store.root_bytes.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&store.root_bytes);
    bytes.extend_from_slice(&1u64.to_le_bytes());
    bytes.push(1);
    bytes.extend_from_slice(&(catalog_name.len() as u16).to_le_bytes());
    bytes.extend_from_slice(catalog_name.as_bytes());
    bytes.extend_from_slice(&(catalog.len() as u64).to_le_bytes());
    bytes.extend_from_slice(&Sha256::digest(&catalog));
    bytes.extend_from_slice(&2u32.to_le_bytes());
    for (index, (name, segment)) in names.iter().zip(&segments).enumerate() {
        bytes.extend_from_slice(&(name.len() as u16).to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&(segment.len() as u64).to_le_bytes());
        bytes.extend_from_slice(&CollectionId(0).0.to_le_bytes());
        bytes.extend_from_slice(&((index + 1) as u64).to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.push(0);
    }
    let digest = Sha256::digest(&bytes);
    bytes.extend_from_slice(&digest);
    fs::write(store.path().join(MANIFEST_NAME), bytes).unwrap();
    (names, catalog)
}

#[test]
fn flat_v1_migrates_two_segments_and_catalog_before_removing_flat_files() {
    let fixture = TestStore::new();
    let (names, catalog) = prepare_flat_v1_store(&fixture, false);
    assert_eq!(fixture.store.read_manifest().unwrap().unwrap().version, 1);
    let opened = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(opened.generation, 1);
    assert_eq!(opened.catalog.as_deref(), Some(catalog.as_slice()));
    assert_eq!(opened.segment_names, names);
    assert_eq!(opened.validation, vec![SegmentTrust::Untrusted; 2]);
    assert_eq!(
        fixture.store.read_manifest().unwrap().unwrap().version,
        MANIFEST_VERSION
    );
    for name in &names {
        assert!(!fixture.store.path().join(name).exists());
        assert!(
            opened
                .generation_directory
                .join("segments")
                .join(name)
                .exists()
        );
    }
}

#[test]
fn missing_flat_v1_file_is_incompatible_and_keeps_old_manifest() {
    let fixture = TestStore::new();
    prepare_flat_v1_store(&fixture, true);
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Incompatible(_))
    ));
    assert_eq!(fixture.store.read_manifest().unwrap().unwrap().version, 1);
}

#[test]
fn stale_stamp_requires_validation_and_can_be_upgraded() {
    let fixture = TestStore::new();
    let stats = fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                bytes: Arc::from(complete_segment(7)),
                collection: CollectionId(0),
                ordinal: 1,
                active: &fixture.active,
                origin: SegmentOrigin::Validated,
            }],
        )
        .unwrap();
    let mut manifest = fixture.store.read_manifest().unwrap().unwrap();
    manifest.segments[0].validation_version = VALIDATOR_VERSION + 1;
    fs::write(
        fixture.store.path().join(MANIFEST_NAME),
        fixture.store.encode_manifest(&manifest).unwrap(),
    )
    .unwrap();
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().validation,
        vec![SegmentTrust::Untrusted]
    );
    let name = stats.segment_names[0].clone();
    let length = manifest.segments[0].length;
    assert!(
        fixture
            .store
            .record_validation_stamps(1, &[(name, length, VALIDATOR_VERSION)])
            .unwrap()
    );
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().validation,
        vec![SegmentTrust::Stamped]
    );
}

#[test]
fn reader_opens_while_exclusive_writer_lock_is_held() {
    let fixture = TestStore::new();
    fixture.store.commit_generation(None, 1, &[]).unwrap();
    let lock = fixture.store.open_lock_read().unwrap();
    lock.try_lock().unwrap();
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        1
    );
    assert!(matches!(
        fixture.store.commit_generation_inner(Some(1), 2, &[], None),
        Err(StoreReadError::Conflict(_))
    ));
}

#[test]
fn manifest_rejects_unknown_segment_format_version() {
    let fixture = TestStore::new();
    fixture.store.commit_generation(None, 1, &[]).unwrap();
    let path = fixture.store.path().join(MANIFEST_NAME);
    let mut bytes = fs::read(&path).unwrap();
    bytes[12..16].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    let end = bytes.len();
    let checksum = Sha256::digest(&bytes[..end - 32]);
    bytes[end - 32..].copy_from_slice(&checksum);
    fs::write(path, bytes).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Incompatible(_))
    ));
}

#[test]
fn reader_pins_complete_generation_during_concurrent_publications() {
    let fixture = TestStore::new();
    let first = fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                bytes: Arc::from(complete_segment(9)),
                collection: CollectionId(0),
                ordinal: 1,
                active: &fixture.active,
                origin: SegmentOrigin::Validated,
            }],
        )
        .unwrap();
    let name = first.segment_names[0].clone();
    std::thread::scope(|scope| {
        scope.spawn(|| {
            for generation in 2..=11 {
                fixture
                    .store
                    .commit_generation(
                        Some(generation - 1),
                        generation,
                        &[SegmentInput::Existing {
                            name: &name,
                            active: &fixture.active,
                        }],
                    )
                    .unwrap();
            }
        });
        for _ in 0..100 {
            let opened = fixture.store.read_generation().unwrap().unwrap();
            assert!((1..=11).contains(&opened.generation));
            assert_eq!(opened.segments.len(), 1);
            assert_eq!(
                opened.segments[0]
                    .section(SectionKind::FieldMetadata)
                    .unwrap(),
                Some(&[9][..])
            );
        }
    });
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        11
    );
}

#[test]
fn abandoned_generation_directory_is_reclaimed_after_grace() {
    let fixture = TestStore::new();
    fixture.store.commit_generation(None, 1, &[]).unwrap();
    let orphan = fixture.store.generation_directory(20);
    create_private_directory_new(&orphan).unwrap();
    create_private_directory_new(&orphan.join("segments")).unwrap();
    File::open(&orphan)
        .unwrap()
        .set_times(
            fs::FileTimes::new()
                .set_modified(SystemTime::now() - MIN_RETENTION_AGE - Duration::from_secs(1)),
        )
        .unwrap();
    let stats = fixture.store.commit_generation(Some(1), 2, &[]).unwrap();
    assert_eq!(stats.orphan_generations_removed, 1);
    assert!(!orphan.exists());
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        2
    );
}

#[test]
fn retry_replaces_only_a_safe_unpublished_directory_with_same_number() {
    let fixture = TestStore::new();
    fixture.store.commit_generation(None, 1, &[]).unwrap();
    let abandoned = fixture.store.generation_directory(2);
    create_private_directory_new(&abandoned).unwrap();
    create_private_directory_new(&abandoned.join("segments")).unwrap();
    let stats = fixture.store.commit_generation(Some(1), 2, &[]).unwrap();
    assert_eq!(stats.orphan_generations_removed, 1);
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        2
    );
}

#[test]
fn obsolete_format_manifest_rebuild_retires_pinned_generation() {
    let fixture = TestStore::new();
    fixture.store.commit_generation(None, 1, &[]).unwrap();
    let path = fixture.store.path().join(MANIFEST_NAME);
    let mut bytes = fs::read(&path).unwrap();
    bytes[12..16].copy_from_slice(&(FORMAT_VERSION - 1).to_le_bytes());
    let end = bytes.len();
    let digest = Sha256::digest(&bytes[..end - 32]);
    bytes[end - 32..].copy_from_slice(&digest);
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        fixture.store.read_generation(),
        Err(StoreReadError::Incompatible(_))
    ));
    assert!(fixture.store.retire_obsolete_format_for_rebuild().unwrap());
    assert!(!path.exists());
    let stats = fixture.store.commit_generation(None, 1, &[]).unwrap();
    assert_eq!(stats.orphan_generations_removed, 1);
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        1
    );
    let generations = fixture.store.path().join(GENERATIONS_DIR);
    assert!(fs::read_dir(generations).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-0000000000000001-")
    }));
}

#[test]
fn obsolete_format_retirement_preserves_unknown_generation_entries() {
    let fixture = TestStore::new();
    fixture.store.commit_generation(None, 1, &[]).unwrap();
    let path = fixture.store.path().join(MANIFEST_NAME);
    let mut bytes = fs::read(&path).unwrap();
    bytes[12..16].copy_from_slice(&(FORMAT_VERSION - 1).to_le_bytes());
    let end = bytes.len();
    let digest = Sha256::digest(&bytes[..end - 32]);
    bytes[end - 32..].copy_from_slice(&digest);
    fs::write(&path, bytes).unwrap();
    let unknown = fixture.store.generation_directory(1).join("user-note.txt");
    fs::write(&unknown, b"keep").unwrap();
    assert!(matches!(
        fixture.store.retire_obsolete_format_for_rebuild(),
        Err(StoreReadError::Corrupt(_))
    ));
    assert!(path.exists());
    assert_eq!(fs::read(unknown).unwrap(), b"keep");
}
