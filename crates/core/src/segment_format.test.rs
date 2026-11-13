use super::*;

fn sample_segment() -> (Arc<[u8]>, RoaringBitmap, PostingDescriptor) {
    let posting = RoaringBitmap::from_iter([1, 4, 9]);
    let mut payload = Vec::new();
    posting.serialize_into(&mut payload).unwrap();
    let descriptor = PostingDescriptor::for_bitmap(0, &posting, &payload).unwrap();
    let cursor = Cursor::new(Vec::new());
    let mut writer = SegmentWriter::new(cursor, 2).unwrap();
    writer
        .write_section(SectionKind::PostingPayloads, |target| {
            target
                .write_all(&payload)
                .map_err(|error| error.to_string())
        })
        .unwrap();
    writer
        .write_section(SectionKind::FieldMetadata, |target| {
            target
                .write_all(b"fields")
                .map_err(|error| error.to_string())
        })
        .unwrap();
    (
        Arc::from(writer.finish().unwrap().into_inner()),
        posting,
        descriptor,
    )
}

#[test]
fn owned_sections_and_postings_are_checked() {
    let (bytes, posting, descriptor) = sample_segment();
    let reader = SegmentReader::from_owned(bytes).unwrap();
    assert_eq!(
        reader.section(SectionKind::FieldMetadata).unwrap(),
        Some(b"fields".as_slice())
    );
    assert_eq!(
        reader
            .materialize_posting(descriptor, &RoaringBitmap::from_iter(0..10))
            .unwrap(),
        posting
    );
    assert_eq!(
        PostingDescriptor::decode(&descriptor.encode()).unwrap(),
        descriptor
    );
    assert!(
        reader
            .materialize_posting(descriptor, &RoaringBitmap::from_iter(0..9))
            .is_err()
    );
    assert!(
        reader
            .materialize_posting(
                PostingDescriptor {
                    cardinality: 2,
                    ..descriptor
                },
                &RoaringBitmap::from_iter(0..10)
            )
            .is_err()
    );
    assert!(
        reader
            .materialize_posting(
                PostingDescriptor {
                    checksum: descriptor.checksum ^ 1,
                    ..descriptor
                },
                &RoaringBitmap::from_iter(0..10)
            )
            .is_err()
    );
}

#[test]
fn corrupt_sections_cannot_be_used_for_negative_routing() {
    let (bytes, _, _) = sample_segment();
    let mut corrupt = bytes.to_vec();
    let offset = corrupt
        .windows(6)
        .position(|window| window == b"fields")
        .unwrap();
    corrupt[offset] ^= 0x10;
    let reader = SegmentReader::from_owned(Arc::from(corrupt)).unwrap();
    assert!(reader.section(SectionKind::FieldMetadata).is_err());
    assert!(reader.section(SectionKind::FieldMetadata).is_err());
}

#[test]
fn malformed_directory_is_rejected_before_exposure() {
    let (bytes, _, _) = sample_segment();
    assert!(SegmentReader::from_owned(Arc::from(bytes[..40].to_vec())).is_err());
    let mut overlap = bytes.to_vec();
    // Second entry's offset aliases the first entry's payload.
    let first_offset = overlap[HEADER_BYTES + 8..HEADER_BYTES + 16].to_vec();
    let second_offset = HEADER_BYTES + DIRECTORY_ENTRY_BYTES + 8;
    overlap[second_offset..second_offset + 8].copy_from_slice(&first_offset);
    assert!(SegmentReader::from_owned(Arc::from(overlap)).is_err());
}

#[test]
fn mapped_reader_uses_same_checked_path() {
    let (bytes, posting, descriptor) = sample_segment();
    let path = std::env::temp_dir().join(format!(
        "objekte-hybrid-segment-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    file.sync_all().unwrap();
    drop(file);
    let file = File::open(&path).unwrap();
    let reader = SegmentReader::from_index_file(&file).unwrap();
    assert_eq!(
        reader
            .materialize_posting(descriptor, &RoaringBitmap::from_iter(0..10))
            .unwrap(),
        posting
    );
    drop(reader);
    drop(file);
    std::fs::remove_file(path).unwrap();
}
