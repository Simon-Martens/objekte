use std::io::Write;

use super::backing::{Backing, MappedObject};
use super::error::StorageError;
use super::format::*;

fn object_bytes() -> Vec<u8> {
    let mut builder = ObjectBuilder::new(ObjectKind::Catalogue);
    for kind in ObjectKind::Catalogue.required_sections() {
        let bytes = match kind {
            SectionKind::FileRows => vec![7u8; 150_000],
            SectionKind::PathDictionary => b"paths".to_vec(),
            SectionKind::Diagnostics => b"diagnostics".to_vec(),
            _ => Vec::new(),
        };
        builder.add_section(*kind, bytes).unwrap();
    }
    builder.finish().unwrap().into_bytes()
}

fn mapped(bytes: &[u8]) -> (tempfile::TempDir, std::path::PathBuf, Backing) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("object.obx");
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let object = MappedObject::open(&path, None).unwrap();
    (
        directory,
        path,
        Backing::mapped(std::sync::Arc::new(object)),
    )
}

#[test]
fn resident_and_mapped_reads_are_identical() {
    let bytes = object_bytes();
    let resident = Backing::resident(bytes.clone());
    let (_directory, _path, mapped) = mapped(&bytes);
    assert_eq!(resident.len(), mapped.len());
    assert_eq!(resident.read(0, 128).unwrap(), mapped.read(0, 128).unwrap());
    assert_eq!(
        resident.read(1024, 4096).unwrap(),
        mapped.read(1024, 4096).unwrap()
    );
    let view = ObjectView::new(&mapped).unwrap();
    assert_eq!(
        view.section_bytes(SectionKind::FileRows).unwrap().len(),
        150_000
    );
    view.verify_full().unwrap();
}

#[test]
fn mapped_reads_are_out_of_bounds_checked() {
    let bytes = object_bytes();
    let (_directory, _path, mapped) = mapped(&bytes);
    assert!(mapped.read(mapped.len(), 1).is_err());
    assert!(mapped.read(u64::MAX, 1).is_err());
}

#[test]
fn mapped_payload_corruption_is_detected_on_first_access() {
    let bytes = object_bytes();
    let (directory, path, mapped) = mapped(&bytes);
    let descriptor = ObjectView::new(&mapped)
        .unwrap()
        .section(SectionKind::FileRows)
        .unwrap();
    drop(mapped);
    let mut corrupt = std::fs::read(&path).unwrap();
    corrupt[descriptor.offset as usize + 10] ^= 0x80;
    std::fs::write(directory.path().join("corrupt.obx"), &corrupt).unwrap();
    let object = MappedObject::open(&directory.path().join("corrupt.obx"), None).unwrap();
    let backing = Backing::mapped(std::sync::Arc::new(object));
    let error = backing
        .read(descriptor.offset, descriptor.length)
        .unwrap_err();
    assert!(matches!(error, StorageError::ChecksumMismatch { .. }));
}

#[test]
fn invalid_magic_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("bad.obx");
    std::fs::write(&path, vec![0u8; 256]).unwrap();
    let error = MappedObject::open(&path, None).unwrap_err();
    assert!(matches!(error, StorageError::InvalidHeader(_)));
}
