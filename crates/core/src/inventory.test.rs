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

#[test]
fn status_codes_and_diagnostic_bounds_are_stable() {
    for (status, code) in [(FileStatus::Valid, 0), (FileStatus::Broken, 1), (FileStatus::Unreadable, 2)] {
        assert_eq!(status.code(), code);
        assert_eq!(serde_json::to_string(&status).unwrap(), code.to_string());
        assert_eq!(serde_json::from_str::<FileStatus>(&code.to_string()).unwrap(), status);
    }
    assert!(serde_json::from_str::<FileStatus>("3").is_err());
    assert!(serde_json::from_str::<RecordClass>("2").is_err());
    assert_eq!(serde_json::to_string(&RecordClass::RawFallback).unwrap(), "1");

    let mut files = FileInventory::new(PathBuf::from("/tmp/repo"));
    let file = files.add("broken.jsonl".into(), SourceRevision { modified_ns: 5, size: 50 }, None, CollectionId(0)).unwrap();
    let make = |category| SourceDiagnostic {
        file, collection: CollectionId(0), source_id: "jsonl".into(), source_version: 1,
        record: None, range: Some(ByteRange { start: 0, end: 3 }), category,
        code: "json.parse".into(), field_path: None, byte_offset: Some(2),
        line: Some(1), column: Some(3), message: "é".repeat(200),
    };
    let first = files.record_diagnostic(file, make(DiagnosticCategory::Syntax)).unwrap();
    assert_eq!(files.files[0].status, FileStatus::Broken);
    assert!(files.diagnostic(file, first).unwrap().message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
    for _ in 1..MAX_DIAGNOSTICS_PER_FILE { files.record_diagnostic(file, make(DiagnosticCategory::Syntax)).unwrap(); }
    let omitted = files.record_diagnostic(file, make(DiagnosticCategory::Io)).unwrap();
    assert!(omitted.omitted());
    assert_eq!(files.files[0].status, FileStatus::Unreadable);
    assert_eq!(files.files[0].omitted_diagnostics, 1);
    assert!(files.diagnostic(file, omitted).is_none());
    files.validate_diagnostics().unwrap();
    files.files[0].diagnostics[0].collection = CollectionId(1);
    assert!(files.validate_diagnostics().is_err());
}
