use super::*;
use crate::index_storage::{IndexResidency, IndexStorageLocation, IndexStoragePolicy};
use crate::repository::{CollectionDefinition, RepositoryOptions};
use crate::segment_format::{REQUIRED_SECTIONS, SectionKind, SegmentWriter};
use crate::source::{SourceFormat, SourceRegistry};
use std::io::Cursor;

struct TestStore {
    base: PathBuf,
    store: ManagedStore,
}

impl TestStore {
    fn new() -> Self {
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
        let store = ManagedStore::open(&resolved).unwrap();
        Self { base, store }
    }
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
fn publishes_new_and_reused_segments_without_rewriting_old_bytes() {
    let fixture = TestStore::new();
    assert!(fixture.store.read_generation().unwrap().is_none());
    let first: Arc<[u8]> = Arc::from(complete_segment(1));
    let second: Arc<[u8]> = Arc::from(complete_segment(2));
    let third: Arc<[u8]> = Arc::from(complete_segment(3));
    let committed = fixture
        .store
        .commit_generation(1, &[SegmentInput::New(Arc::clone(&first))])
        .unwrap();
    assert_eq!(committed.segment_bytes_written, first.len() as u64);
    let pinned = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(pinned.generation, 1);
    let old_name = pinned.segment_names[0].clone();
    let committed = fixture
        .store
        .commit_generation(
            2,
            &[
                SegmentInput::Existing(&old_name),
                SegmentInput::New(Arc::clone(&second)),
            ],
        )
        .unwrap();
    assert_eq!(committed.segments_reused, 1);
    assert_eq!(committed.segment_bytes_written, second.len() as u64);
    let current = fixture.store.read_generation().unwrap().unwrap();
    assert_eq!(current.generation, 2);
    assert_eq!(current.segments.len(), 2);
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
        .commit_generation(3, &[SegmentInput::New(Arc::clone(&third))])
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
fn truncated_manifest_is_an_error_and_interrupted_temp_is_ignored() {
    let fixture = TestStore::new();
    let bytes: Arc<[u8]> = Arc::from(complete_segment(7));
    fixture
        .store
        .commit_generation(1, &[SegmentInput::New(Arc::clone(&bytes))])
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
        .commit_generation(1, &[SegmentInput::New(Arc::clone(&bytes))])
        .unwrap();
    assert!(fixture.store.commit_generation(1, &[]).is_err());
    assert!(
        fixture
            .store
            .commit_generation(2, &[SegmentInput::Existing("segment-evil.idx")])
            .is_err()
    );
    assert_eq!(
        fixture.store.read_generation().unwrap().unwrap().generation,
        1
    );
}

#[cfg(unix)]
#[test]
fn obsolete_symlink_is_not_followed_or_removed() {
    use std::os::unix::fs::symlink;
    let fixture = TestStore::new();
    let first: Arc<[u8]> = Arc::from(complete_segment(1));
    fixture
        .store
        .commit_generation(1, &[SegmentInput::New(Arc::clone(&first))])
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
        .commit_generation(2, &[SegmentInput::New(second)])
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
