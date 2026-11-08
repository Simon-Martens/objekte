use super::*;

#[test]
fn shifted_batch_bitmaps_cross_container_boundaries_and_reject_overlap() {
    let mut destination: RoaringBitmap = [0, 1, 65_534].into_iter().collect();
    let source: RoaringBitmap = [0, 1, 63, 64, 65].into_iter().collect();
    append_shifted_bitmap(&mut destination, &source, 65_535).unwrap();
    assert_eq!(
        destination.iter().collect::<Vec<_>>(),
        [0, 1, 65_534, 65_535, 65_536, 65_598, 65_599, 65_600]
    );
    assert!(append_shifted_bitmap(&mut destination, &source, 65_535).is_err());
    assert!(append_shifted_bitmap(&mut destination, &source, u32::MAX).is_err());
}

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
fn construction_membership_preserves_order_empty_files_and_record_ownership() {
    let mut inventory = FileInventory::new(PathBuf::from("/tmp/repo"));
    let revision = SourceRevision {
        modified_ns: 7,
        size: 10,
    };
    let empty = inventory
        .add("empty.json".into(), revision, None, CollectionId(0))
        .unwrap();
    let valid = inventory
        .add("valid.json".into(), revision, None, CollectionId(0))
        .unwrap();
    let broken = inventory
        .add("broken.json".into(), revision, None, CollectionId(0))
        .unwrap();
    let omitted = inventory
        .add("omitted.json".into(), revision, None, CollectionId(0))
        .unwrap();
    let mut builder = CollectionBuilder::new(CollectionId(0), "data".into(), 1, 4);
    assert!(builder.include_file(FileId(4)).is_err());
    assert!(
        builder
            .add_record(
                &inventory.files[valid.0 as usize],
                RecordId::intrinsic("v").unwrap(),
                0,
                10
            )
            .is_err()
    );
    assert!(
        builder
            .add_raw_fallback(
                &inventory.files[broken.0 as usize],
                RecordId::fallback(
                    CollectionId(0),
                    "json",
                    1,
                    Path::new("broken.json"),
                    broken,
                    7,
                    0,
                    10
                ),
                ByteRange { start: 0, end: 10 },
                DiagnosticRef(1),
            )
            .is_err()
    );
    for file in [empty, valid, broken, empty, valid] {
        builder.include_file(file).unwrap();
    }
    let valid_slot = builder
        .add_record(
            &inventory.files[valid.0 as usize],
            RecordId::intrinsic("v").unwrap(),
            0,
            10,
        )
        .unwrap();
    assert!(
        builder
            .add_record(
                &inventory.files[valid.0 as usize],
                RecordId::intrinsic("v").unwrap(),
                0,
                10
            )
            .is_err()
    );
    assert!(
        builder
            .add_record(
                &inventory.files[omitted.0 as usize],
                RecordId::intrinsic("other").unwrap(),
                0,
                10
            )
            .is_err()
    );
    let diagnostic = SourceDiagnostic {
        file: broken,
        collection: CollectionId(0),
        source_id: "json".into(),
        source_version: 1,
        record: None,
        range: Some(ByteRange { start: 0, end: 10 }),
        category: DiagnosticCategory::Syntax,
        code: "json.parse".into(),
        field_path: None,
        byte_offset: Some(1),
        line: Some(1),
        column: Some(2),
        message: "invalid JSON".into(),
    };
    let reference = inventory.record_diagnostic(broken, diagnostic).unwrap();
    let fallback_slot = builder
        .add_raw_fallback(
            &inventory.files[broken.0 as usize],
            RecordId::fallback(
                CollectionId(0),
                "json",
                1,
                Path::new("broken.json"),
                broken,
                7,
                0,
                10,
            ),
            ByteRange { start: 0, end: 10 },
            reference,
        )
        .unwrap();
    inventory.files[broken.0 as usize].diagnostics[0].record = Some(fallback_slot);
    let generation = builder.finish(&inventory).unwrap();
    assert_eq!(generation.files, [empty, valid, broken]);
    assert_eq!(
        generation.file_records.records_for(empty),
        Some([].as_slice())
    );
    assert_eq!(
        generation.file_records.records_for(valid),
        Some([valid_slot].as_slice())
    );
    assert_eq!(
        generation.file_records.records_for(broken),
        Some([fallback_slot].as_slice())
    );
    assert_eq!(
        generation.file_records.records_for(omitted),
        Some([].as_slice())
    );
    assert_eq!(generation.records[0].class, RecordClass::Logical);
    assert_eq!(generation.records[1].class, RecordClass::RawFallback);
}

#[test]
fn flat_mapping_matches_grouped_reference_for_interleaved_files() {
    let mut inventory = FileInventory::new(PathBuf::from("/tmp/repo"));
    let revision = SourceRevision {
        modified_ns: 9,
        size: 20,
    };
    let files: Vec<_> = (0..4)
        .map(|i| {
            inventory
                .add(format!("{i}.jsonl").into(), revision, None, CollectionId(0))
                .unwrap()
        })
        .collect();
    let empty = CollectionBuilder::new(CollectionId(0), "empty".into(), 1, files.len())
        .finish(&inventory)
        .unwrap();
    assert_eq!(empty.file_records.offsets, [0, 0, 0, 0, 0]);
    assert!(empty.file_records.slots.is_empty());

    let mut builder = CollectionBuilder::new(CollectionId(0), "data".into(), 1, files.len());
    for &file in &files {
        builder.include_file(file).unwrap();
    }
    for (n, &file) in [files[2], files[0], files[2], files[1], files[0]]
        .iter()
        .enumerate()
    {
        builder
            .add_record(
                &inventory.files[file.0 as usize],
                RecordId::intrinsic(&format!("r{n}")).unwrap(),
                0,
                10,
            )
            .unwrap();
    }
    let generation = builder.finish(&inventory).unwrap();
    let mut grouped = vec![Vec::new(); files.len()];
    for record in &generation.records {
        grouped[record.file.0 as usize].push(record.slot);
    }
    let mut expected_offsets = vec![0];
    let mut expected_slots = Vec::new();
    for group in grouped {
        expected_slots.extend(group);
        expected_offsets.push(expected_slots.len() as u32);
    }
    assert_eq!(generation.file_records.offsets, expected_offsets);
    assert_eq!(generation.file_records.slots, expected_slots);
    assert_eq!(generation.file_records.offsets, [0, 2, 3, 5, 5]);
    assert_eq!(
        generation.file_records.slots,
        [
            RecordSlot(1),
            RecordSlot(4),
            RecordSlot(3),
            RecordSlot(0),
            RecordSlot(2)
        ]
    );
}

#[test]
fn status_codes_and_diagnostic_bounds_are_stable() {
    for (status, code) in [
        (FileStatus::Valid, 0),
        (FileStatus::Broken, 1),
        (FileStatus::Unreadable, 2),
    ] {
        assert_eq!(status.code(), code);
        assert_eq!(serde_json::to_string(&status).unwrap(), code.to_string());
        assert_eq!(
            serde_json::from_str::<FileStatus>(&code.to_string()).unwrap(),
            status
        );
    }
    assert!(serde_json::from_str::<FileStatus>("3").is_err());
    assert!(serde_json::from_str::<RecordClass>("2").is_err());
    assert_eq!(
        serde_json::to_string(&RecordClass::RawFallback).unwrap(),
        "1"
    );

    let mut files = FileInventory::new(PathBuf::from("/tmp/repo"));
    let file = files
        .add(
            "broken.jsonl".into(),
            SourceRevision {
                modified_ns: 5,
                size: 50,
            },
            None,
            CollectionId(0),
        )
        .unwrap();
    let make = |category| SourceDiagnostic {
        file,
        collection: CollectionId(0),
        source_id: "jsonl".into(),
        source_version: 1,
        record: None,
        range: Some(ByteRange { start: 0, end: 3 }),
        category,
        code: "json.parse".into(),
        field_path: None,
        byte_offset: Some(2),
        line: Some(1),
        column: Some(3),
        message: "é".repeat(200),
    };
    let first = files
        .record_diagnostic(file, make(DiagnosticCategory::Syntax))
        .unwrap();
    assert_eq!(files.files[0].status, FileStatus::Broken);
    assert!(files.diagnostic(file, first).unwrap().message.len() <= MAX_DIAGNOSTIC_MESSAGE_BYTES);
    for _ in 1..MAX_DIAGNOSTICS_PER_FILE {
        files
            .record_diagnostic(file, make(DiagnosticCategory::Syntax))
            .unwrap();
    }
    let omitted = files
        .record_diagnostic(file, make(DiagnosticCategory::Io))
        .unwrap();
    assert!(omitted.omitted());
    assert_eq!(files.files[0].status, FileStatus::Unreadable);
    assert_eq!(files.files[0].omitted_diagnostics, 1);
    assert!(files.diagnostic(file, omitted).is_none());
    files.validate_diagnostics().unwrap();
    files.files[0].diagnostics[0].collection = CollectionId(1);
    assert!(files.validate_diagnostics().is_err());
}

#[test]
fn published_file_versions_change_only_the_selected_chunk() {
    let first = FileVersions::initial(1025);
    let (second, version) = first.advanced(FileId(1024)).unwrap();
    assert_eq!(version, FileVersion(2));
    assert_eq!(first.get(FileId(1024)), Some(FileVersion::INITIAL));
    assert_eq!(second.get(FileId(1024)), Some(FileVersion(2)));
    assert_eq!(second.get(FileId(0)), Some(FileVersion::INITIAL));
    assert_eq!(second.get(FileId(1025)), None);
    let third = second.appended().unwrap();
    assert_eq!(third.get(FileId(1025)), Some(FileVersion::INITIAL));
    assert_eq!(second.get(FileId(1025)), None);
}

#[test]
fn published_active_records_keep_old_view_and_cross_chunk_boundaries() {
    let first = ActiveRecords::all(65_538).unwrap();
    let second = first
        .retired(&[RecordSlot(0), RecordSlot(65_536), RecordSlot(65_536)])
        .unwrap();
    assert_eq!(first.len(), 65_538);
    assert_eq!(second.len(), 65_536);
    assert!(first.contains(RecordSlot(0)));
    assert!(first.contains(RecordSlot(65_536)));
    assert!(!second.contains(RecordSlot(0)));
    assert!(!second.contains(RecordSlot(65_536)));
    assert_eq!(second.slot_count(), 65_538);
    let third = second.appended(2).unwrap();
    assert_eq!(third.slot_count(), 65_540);
    assert_eq!(third.len(), 65_538);
    assert!(third.contains(RecordSlot(65_538)));
    assert!(third.contains(RecordSlot(65_539)));
    assert!(!third.contains(RecordSlot(65_536)));
    assert_eq!(third.bitmap().len(), third.len());
    assert!(second.retired(&[RecordSlot(65_538)]).is_err());
}
