use super::backing::Backing;
use super::error::StorageError;
use super::format::*;

fn sample_object() -> BuiltObject {
    let mut builder = ObjectBuilder::new(ObjectKind::Segment);
    for kind in ObjectKind::Segment.required_sections() {
        let bytes = match kind {
            SectionKind::Records => vec![1, 2, 3, 4],
            SectionKind::FileParts => vec![9; 70000],
            SectionKind::LogicalIds => b"logical".to_vec(),
            _ => Vec::new(),
        };
        builder.add_section(*kind, bytes).unwrap();
    }
    builder
        .add_optional_section(SectionKind::Compatibility, b"future".to_vec())
        .unwrap();
    builder.finish().unwrap()
}

#[test]
fn header_and_sections_round_trip() {
    let object = sample_object();
    let backing = Backing::resident(object.as_bytes().to_vec());
    let view = ObjectView::new(&backing).unwrap();
    assert_eq!(view.header().kind, ObjectKind::Segment);
    assert_eq!(view.header().total_len, object.as_bytes().len() as u64);
    assert_eq!(
        view.section_bytes(SectionKind::Records).unwrap(),
        &[1, 2, 3, 4]
    );
    assert_eq!(
        view.section_bytes(SectionKind::FileParts).unwrap().len(),
        70000
    );
    view.verify_full().unwrap();
}

#[test]
fn checksum_detects_payload_corruption() {
    let object = sample_object();
    let mut bytes = object.into_bytes();
    let records = {
        let backing = Backing::resident(bytes.clone());
        let view = ObjectView::new(&backing).unwrap();
        view.section(SectionKind::Records).unwrap().offset as usize
    };
    bytes[records] ^= 0xff;
    let backing = Backing::resident(bytes);
    let view = ObjectView::new(&backing).unwrap();
    let error = view.verify_full().unwrap_err();
    assert!(matches!(error, StorageError::ChecksumMismatch { .. }));
}

#[test]
fn truncated_object_is_rejected() {
    let object = sample_object();
    let mut bytes = object.into_bytes();
    bytes.truncate(HEADER_LEN + 3);
    let backing = Backing::resident(bytes);
    assert!(ObjectView::new(&backing).is_err());
}

#[test]
fn missing_required_section_is_rejected_by_builder() {
    let mut builder = ObjectBuilder::new(ObjectKind::Segment);
    builder
        .add_section(SectionKind::Records, Vec::new())
        .unwrap();
    assert!(builder.finish().is_err());
}

#[test]
fn duplicate_section_is_rejected() {
    let mut builder = ObjectBuilder::new(ObjectKind::Catalogue);
    builder
        .add_section(SectionKind::FileRows, Vec::new())
        .unwrap();
    assert!(
        builder
            .add_section(SectionKind::FileRows, Vec::new())
            .is_err()
    );
}

#[test]
fn header_checksum_catches_header_edits() {
    let object = sample_object();
    let mut bytes = object.into_bytes();
    bytes[32] ^= 1;
    let backing = Backing::resident(bytes);
    let error = ObjectView::new(&backing).unwrap_err();
    assert!(matches!(error, StorageError::InvalidHeader(_)));
}

#[test]
fn optional_section_is_preserved() {
    let object = sample_object();
    let backing = Backing::resident(object.as_bytes().to_vec());
    let view = ObjectView::new(&backing).unwrap();
    assert_eq!(view.sections().len(), 16);
    assert_eq!(
        view.section_bytes(SectionKind::Compatibility).unwrap(),
        b"future"
    );
    view.verify_full().unwrap();
}

fn refresh_checksums(bytes: &mut [u8]) {
    let header = ObjectHeader::parse(bytes).unwrap();
    for block in 0..header.checksum_count {
        let start = block as usize * CHECKSUM_BLOCK_SIZE as usize;
        let end = (start + CHECKSUM_BLOCK_SIZE as usize).min(header.checksum_table_offset as usize);
        let crc = crc32fast::hash(&bytes[start..end]);
        let at = header.checksum_table_offset as usize + block as usize * 4;
        bytes[at..at + 4].copy_from_slice(&crc.to_le_bytes());
    }
}
