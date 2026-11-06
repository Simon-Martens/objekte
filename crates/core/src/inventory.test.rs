use super::*;

#[test]
fn file_record_mapping_in_both_directions() {
    let mut files = FileInventory::new(PathBuf::from("/tmp/repo"));
    let revision = SourceRevision {
        modified_ns: 5,
        size: 10,
    };
    let f0 = files
        .add("empty.json".into(), revision, None, CollectionId(0))
        .unwrap();
    let f1 = files
        .add("multi.jsonl".into(), revision, None, CollectionId(0))
        .unwrap();
    assert_eq!(
        files
            .add("multi.jsonl".into(), revision, None, CollectionId(1))
            .unwrap(),
        f1
    );
    let mut a = CollectionBuilder::new(CollectionId(0), "a".into(), 1, files.files.len());
    a.include_file(f0).unwrap();
    a.include_file(f1).unwrap();
    a.add_record(&files.files[1], RecordId::intrinsic("one").unwrap(), 0, 3)
        .unwrap();
    a.add_record(&files.files[1], RecordId::intrinsic("two").unwrap(), 4, 10)
        .unwrap();
    let generation = a.finish(&files).unwrap();
    assert_eq!(generation.file_records.records_for(f0).unwrap().len(), 0);
    assert_eq!(
        generation.file_records.records_for(f1).unwrap(),
        &[RecordSlot(0), RecordSlot(1)]
    );
    assert_eq!(generation.records[1].range, ByteRange { start: 4, end: 10 });
    let mut b = CollectionBuilder::new(CollectionId(1), "b".into(), 1, files.files.len());
    b.include_file(f1).unwrap();
    b.add_record(
        &files.files[1],
        RecordId::intrinsic("other").unwrap(),
        0,
        10,
    )
    .unwrap();
    assert_eq!(
        b.finish(&files)
            .unwrap()
            .file_records
            .records_for(f1)
            .unwrap(),
        &[RecordSlot(0)]
    );
    assert!(ByteRange::checked(9, 11, 10).is_err());
    assert!(ByteRange::checked(5, 4, 10).is_err());
}
