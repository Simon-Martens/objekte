use super::*;
use crate::index_storage::{IndexResidency, IndexStorageLocation, IndexStoragePolicy};
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
        Self::with_residency(IndexResidency::OnDemand)
    }

    fn with_residency(residency: IndexResidency) -> Self {
        let base = std::env::temp_dir().join(format!(
            "objekte-hybrid-store-test-{}-{}",
            std::process::id(),
            unique_nonce().unwrap()
        ));
        fs::create_dir(&base).unwrap();
        let root = base.join("source");
        let destination = base.join("destination");
        fs::create_dir(&root).unwrap();
        fs::create_dir(&destination).unwrap();
        let resolved = IndexStoragePolicy {
            location: IndexStorageLocation::CustomDirectory(destination),
            residency,
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
fn store_read_errors_keep_io_distinct_from_corruption() {
    let fixture = TestStore::new();
    assert!(fixture.store.read_generation().unwrap().is_none());
    let lock = fixture.store.path().join(LOCK_NAME);
    fs::remove_file(&lock).unwrap();
    fs::create_dir(lock).unwrap();
    let error = fixture.store.read_generation().err().unwrap();
    assert!(matches!(&error, StoreReadError::Io { .. }));
    assert!(matches!(
        crate::lifecycle::IndexError::from(error),
        crate::lifecycle::IndexError::Io(_)
    ));
}

#[test]
fn supported_corruption_and_unsupported_version_have_different_kinds() {
    let fixture = TestStore::new();
    fixture.store.commit_generation(
        None, 1,
        &[SegmentInput::New {
            collection: CollectionId(0), ordinal: 1,
            bytes: Arc::from(complete_segment(3)), active: &fixture.active,
        }],
    ).unwrap();
    let path = fixture.store.path().join(MANIFEST_NAME);
    let original = fs::read(&path).unwrap();
    let mut corrupt = original.clone();
    corrupt[20] ^= 1;
    fs::write(&path, corrupt).unwrap();
    assert!(matches!(fixture.store.read_generation(), Err(StoreReadError::Corrupt(_))));
    let mut unsupported = original.clone();
    unsupported[8..12].copy_from_slice(&(FORMAT_VERSION + 1).to_le_bytes());
    let end = unsupported.len();
    let checksum = Sha256::digest(&unsupported[..end - 32]);
    unsupported[end - 32..].copy_from_slice(&checksum);
    fs::write(&path, unsupported).unwrap();
    assert!(matches!(fixture.store.read_generation(), Err(StoreReadError::Incompatible(_))));
}

#[test]
fn full_residency_owns_bytes_and_checks_all_sections() {
    let fixture = TestStore::with_residency(IndexResidency::FullyResident);
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                collection: CollectionId(0),
                ordinal: 1,
                bytes: Arc::from(complete_segment(4)),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    assert!(opened.segments[0].is_owned());
    let path = fixture.store.path().join(&opened.segment_names[0]);
    fs::remove_file(&path).unwrap();
    assert_eq!(
        opened.segments[0]
            .section(SectionKind::FieldMetadata)
            .unwrap(),
        Some(&[4][..])
    );
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
                collection: CollectionId(0),
                ordinal: 2,
                bytes: Arc::from(complete_segment(5)),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let opened = fixture.store.read_generation().unwrap().unwrap();
    assert!(!opened.segments[0].is_owned());
    let path = fixture.store.path().join(&opened.segment_names[0]);
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
                collection: CollectionId(0),
                ordinal: 5,
                bytes: Arc::clone(&third),
                active: &fixture.active,
            }],
        )
        .unwrap();
    assert_eq!(committed.obsolete_files_reclaimed, 2);
    assert_eq!(committed.orphan_files_reclaimed, 2);
    assert!(committed.reclaim_errors.is_empty());
    assert!(unknown.is_file());
    assert!(!orphan.exists());
    assert!(!incomplete_manifest.exists());
    assert!(!fixture.store.path().join(old_name).exists());
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
    let old_catalog = fixture.store.path().join(&manifest.catalog.unwrap().name);
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
    assert_eq!(stats.obsolete_files_reclaimed, 1);
    assert!(!old_catalog.exists());
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
                bytes: Arc::clone(&bytes),
                collection: CollectionId(0),
                ordinal: 7,
                active: &fixture.active,
            },
            SegmentInput::New {
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
                collection: CollectionId(0),
                ordinal: 13,
                bytes: Arc::from(complete_segment(6)),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let pinned = fixture.store.read_generation().unwrap().unwrap();
    let name = pinned.segment_names[0].clone();
    let old_bytes = fs::read(fixture.store.path().join(&name)).unwrap();
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
        fs::read(fixture.store.path().join(&name)).unwrap(),
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
                collection: CollectionId(0),
                ordinal: 15,
                bytes: Arc::clone(&first),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let name = fixture
        .store
        .read_generation()
        .unwrap()
        .unwrap()
        .segment_names
        .remove(0);
    let path = fixture.store.path().join(&name);
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
                collection: CollectionId(0),
                ordinal: 16,
                bytes: second,
                active: &fixture.active,
            }],
        )
        .unwrap();
    assert_eq!(stats.obsolete_files_reclaimed, 0);
    assert_eq!(stats.reclaim_errors.len(), 1);
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
    let resolved = IndexStoragePolicy {
        location: IndexStorageLocation::CustomDirectory(destination),
        residency: IndexResidency::OnDemand,
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
    let resolved = IndexStoragePolicy {
        location: IndexStorageLocation::CustomDirectory(destination),
        residency: IndexResidency::OnDemand,
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
    assert!(
        fixture
            .store
            .read_generation()
            .err()
            .unwrap()
            .to_string()
            .contains("safe owned regular file")
    );
    fs::set_permissions(&lock, fs::Permissions::from_mode(0o600)).unwrap();
    fixture
        .store
        .commit_generation(
            None,
            1,
            &[SegmentInput::New {
                collection: CollectionId(0),
                ordinal: 17,
                bytes: Arc::from(complete_segment(1)),
                active: &fixture.active,
            }],
        )
        .unwrap();
    let name = fixture
        .store
        .read_generation()
        .unwrap()
        .unwrap()
        .segment_names
        .remove(0);
    let segment = fixture.store.path().join(name);
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
    let resolved = IndexStoragePolicy {
        location: IndexStorageLocation::CustomDirectory(destination),
        residency: IndexResidency::OnDemand,
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
