use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use sha2::{Digest, Sha256};

use super::*;
use crate::inventory::FileVersions;
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
    assert_eq!(old_ref, HISTORICAL_TAG);
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
    assert_eq!(old_ref, HISTORICAL_TAG);
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
fn tagged_history_survives_retirement_and_new_file_ids() {
    let (_, mut published) = publication();
    let old = published.catalog.entry(FileId(0)).unwrap().clone();
    let mut old_revision = old.clone();
    old_revision.revision.size += 1;
    let mut first = GenerationCatalogBuilder::from_publication(&published).unwrap();
    let old_ref = first.intern(&old_revision).unwrap();
    let first = RestoredGenerationCatalog::decode(
        &first.encode().unwrap(),
        published.configuration_identity,
        1,
    )
    .unwrap();
    assert_eq!(old_ref, HISTORICAL_TAG);

    published.catalog = published.catalog.retired(FileId(0)).unwrap();
    let mut added = old.clone();
    added.id = FileId(2);
    added.relative_path = PathBuf::from("data/c.json").into();
    published.catalog = published
        .catalog
        .appended(added.clone(), None, None)
        .unwrap();
    published.file_versions = FileVersions::from_versions(vec![FileVersion::INITIAL; 3]).unwrap();
    let mut next = GenerationCatalogBuilder::from_publication_with_history(
        &published,
        first.historical_entries(),
    )
    .unwrap();
    assert_eq!(next.intern(&old_revision).unwrap(), old_ref);
    assert_eq!(next.intern(&added).unwrap(), 2);
    let mut changed_added = added.clone();
    changed_added.revision.size += 1;
    let added_ref = next.intern(&changed_added).unwrap();
    assert_eq!(added_ref, HISTORICAL_TAG | 1);
    let next = RestoredGenerationCatalog::decode(
        &next.encode().unwrap(),
        published.configuration_identity,
        1,
    )
    .unwrap();
    assert!(next.resolve_metadata(0).is_none());
    assert_eq!(next.resolve_metadata(2), Some(&added));
    assert_eq!(next.resolve_metadata(old_ref), Some(&old_revision));
    assert_eq!(next.resolve_metadata(added_ref), Some(&changed_added));
}

#[test]
fn historical_reference_remains_valid_when_entry_becomes_current_again() {
    let (_, mut published) = publication();
    let current = published.catalog.entry(FileId(0)).unwrap().clone();
    let mut former = current.clone();
    former.revision.size += 1;
    let mut first = GenerationCatalogBuilder::from_publication(&published).unwrap();
    let historical_ref = first.intern(&former).unwrap();
    let first = RestoredGenerationCatalog::decode(
        &first.encode().unwrap(),
        published.configuration_identity,
        1,
    )
    .unwrap();
    published.catalog = published
        .catalog
        .replaced(former.clone(), None, None)
        .unwrap();
    let mut next = GenerationCatalogBuilder::from_publication_with_history(
        &published,
        first.historical_entries(),
    )
    .unwrap();
    assert_eq!(next.intern(&former).unwrap(), 0);
    let next = RestoredGenerationCatalog::decode(
        &next.encode().unwrap(),
        published.configuration_identity,
        1,
    )
    .unwrap();
    assert_eq!(next.resolve_metadata(0), Some(&former));
    assert_eq!(next.resolve_metadata(historical_ref), Some(&former));
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
    let replacement = path_bytes(Path::new("data/a.json"));
    let body = &bytes[HEADER_BYTES..];
    let metadata_start = u32::from_le_bytes(body[80..84].try_into().unwrap()) as usize;
    let slot_start = u32::from_le_bytes(body[64..68].try_into().unwrap()) as usize;
    let second_slot = slot_start + 16;
    let second_offset = u32::from_le_bytes(body[second_slot + 8..second_slot + 12].try_into().unwrap()) as usize;
    let start = HEADER_BYTES + metadata_start + second_offset + 8;
    bytes[start..start + replacement.len()].copy_from_slice(&replacement);
    let digest = Sha256::digest(&bytes[HEADER_BYTES..]);
    bytes[20..52].copy_from_slice(&digest);
    assert!(RestoredGenerationCatalog::decode(&bytes, published.configuration_identity, 1).is_err());
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
