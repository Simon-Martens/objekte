use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::*;
use crate::lifecycle::{IndexRevision, PublishedIndex};
use crate::repository::{CollectionDefinition, Repository, RepositoryOptions};
use crate::source::{DiagnosticCategory, SourceFormat};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "objekte-generation-catalog-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn publication() -> (TempDir, PublishedIndex) {
    let root = TempDir::new();
    std::fs::create_dir(root.path().join("data")).unwrap();
    std::fs::write(root.path().join("data/a.json"), br#"{"title":"A"}"#).unwrap();
    std::fs::write(root.path().join("data/b.json"), br#"{"title":"B"}"#).unwrap();
    let repository = Repository::open_with_options(
        root.path(),
        vec![CollectionDefinition::folder(
            "data",
            "data",
            SourceFormat::Json,
        )],
        RepositoryOptions::default(),
    )
    .unwrap();
    let published =
        PublishedIndex::from_repository(1, IndexRevision::INITIAL, Arc::new(repository)).unwrap();
    (root, published)
}

#[test]
fn current_and_historical_metadata_round_trip_without_source_files() {
    let (root, published) = publication();
    let current = published.catalog.entry(FileId(0)).unwrap().clone();
    let mut older = current.clone();
    older.revision.size += 1;
    let mut builder = GenerationCatalogBuilder::from_publication(&published).unwrap();
    assert_eq!(builder.intern(&current).unwrap(), 0);
    let old_ref = builder.intern(&older).unwrap();
    assert_eq!(old_ref, 2);
    assert_eq!(builder.intern(&older).unwrap(), old_ref);
    let bytes = builder.encode().unwrap();
    std::fs::remove_dir_all(root.path().join("data")).unwrap();
    let restored =
        RestoredGenerationCatalog::decode(&bytes, published.configuration_identity, 1).unwrap();
    assert_eq!(
        restored.configuration_identity,
        published.configuration_identity
    );
    assert_eq!(restored.revision, 1);
    assert_eq!(restored.next_segment_ordinal, 1);
    assert_eq!(restored.file_versions, vec![FileVersion::INITIAL; 2]);
    assert_eq!(restored.resolve_metadata(0), Some(&current));
    assert_eq!(restored.resolve_metadata(old_ref), Some(&older));
    assert!(restored.resolve_metadata(old_ref + 1).is_none());
    assert_eq!(restored.slots.len(), 2);
}

#[test]
fn retired_hole_cannot_resolve_but_its_history_can() {
    let (_, mut published) = publication();
    let old = published.catalog.entry(FileId(0)).unwrap().clone();
    published.catalog = published.catalog.retired(FileId(0)).unwrap();
    let mut builder = GenerationCatalogBuilder::from_publication(&published).unwrap();
    let old_ref = builder.intern(&old).unwrap();
    assert_eq!(old_ref, 2);
    let restored = RestoredGenerationCatalog::decode(
        &builder.encode().unwrap(),
        published.configuration_identity,
        1,
    )
    .unwrap();
    assert!(restored.slots[0].is_none());
    assert!(restored.resolve_metadata(0).is_none());
    assert_eq!(restored.resolve_metadata(old_ref), Some(&old));
    assert_eq!(restored.file_versions.len(), 2);
}

#[test]
fn checksum_identity_and_duplicate_current_paths_are_rejected() {
    let (_, published) = publication();
    let builder = GenerationCatalogBuilder::from_publication(&published).unwrap();
    let mut bytes = builder.encode().unwrap();
    assert!(RestoredGenerationCatalog::decode(&bytes, [9; 32], 1).is_err());
    bytes[HEADER_BYTES + 5] ^= 1;
    assert!(
        RestoredGenerationCatalog::decode(&bytes, published.configuration_identity, 1)
            .unwrap_err()
            .contains("checksum")
    );

    let mut bytes = GenerationCatalogBuilder::from_publication(&published)
        .unwrap()
        .encode()
        .unwrap();
    let old = path_bytes(Path::new("data/b.json"));
    let replacement = path_bytes(Path::new("data/a.json"));
    let start = bytes
        .windows(old.len())
        .position(|part| part == old)
        .unwrap();
    bytes[start..start + old.len()].copy_from_slice(&replacement);
    let digest = Sha256::digest(&bytes[HEADER_BYTES..]);
    bytes[20..52].copy_from_slice(&digest);
    assert!(
        RestoredGenerationCatalog::decode(&bytes, published.configuration_identity, 1)
            .unwrap_err()
            .contains("duplicate")
    );
}

#[test]
fn oversized_or_misowned_diagnostics_cannot_be_persisted() {
    let (_, published) = publication();
    let mut builder = GenerationCatalogBuilder::from_publication(&published).unwrap();
    let mut entry = published.catalog.entry(FileId(0)).unwrap().clone();
    entry.diagnostics.push(SourceDiagnostic {
        file: entry.id,
        collection: CollectionId(0),
        source_id: "json".into(),
        source_version: 1,
        record: None,
        range: None,
        category: DiagnosticCategory::Syntax,
        code: "bad".into(),
        field_path: None,
        byte_offset: None,
        line: None,
        column: None,
        message: "x".repeat(MAX_DIAGNOSTIC_MESSAGE_BYTES + 1),
    });
    assert!(builder.intern(&entry).unwrap_err().contains("diagnostic"));

    let mut builder = GenerationCatalogBuilder::from_publication(&published).unwrap();
    let mut entry = published.catalog.entry(FileId(0)).unwrap().clone();
    entry.diagnostics.push(SourceDiagnostic {
        file: FileId(1),
        collection: CollectionId(0),
        source_id: "json".into(),
        source_version: 1,
        record: None,
        range: None,
        category: DiagnosticCategory::Syntax,
        code: "bad".into(),
        field_path: None,
        byte_offset: None,
        line: None,
        column: None,
        message: "bad".into(),
    });
    assert!(builder.intern(&entry).unwrap_err().contains("diagnostic"));

    let mut builder = GenerationCatalogBuilder::from_publication(&published).unwrap();
    let mut entry = published.catalog.entry(FileId(0)).unwrap().clone();
    entry.status = FileStatus::Broken;
    assert!(builder.intern(&entry).unwrap_err().contains("status"));
}

#[cfg(unix)]
#[test]
fn native_non_utf8_paths_round_trip() {
    use std::os::unix::ffi::OsStringExt;
    let (_, mut published) = publication();
    let path = PathBuf::from(std::ffi::OsString::from_vec(
        b"data/native-\xff.json".to_vec(),
    ));
    let mut entry = published.catalog.entry(FileId(0)).unwrap().clone();
    entry.relative_path = path.clone().into();
    published.catalog = published.catalog.replaced(entry, None, None).unwrap();
    let builder = GenerationCatalogBuilder::from_publication(&published).unwrap();
    let restored = RestoredGenerationCatalog::decode(
        &builder.encode().unwrap(),
        published.configuration_identity,
        1,
    )
    .unwrap();
    assert_eq!(
        restored
            .resolve_metadata(0)
            .unwrap()
            .relative_path
            .as_path(),
        path
    );
}
