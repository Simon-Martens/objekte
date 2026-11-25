use super::*;

#[test]
fn inventory_without_catalog_has_no_root_or_file_identity() {
    let mut inventory = FileInventory::without_root();
    assert_eq!(inventory.root(), None);
    assert!(inventory.files.is_empty());
    assert_eq!(inventory.native_path(FileId(0)), None);
    assert!(
        inventory
            .add(
                "data/a.row".into(),
                SourceRevision {
                    modified_ns: 0,
                    size: 1,
                    object_id: None,
                },
                None,
                CollectionId(0),
            )
            .is_err()
    );
}

#[test]
fn restored_entries_build_lookup_once_with_add_equivalent_results() {
    let root = PathBuf::from("/tmp/restore-inventory");
    let empty = FileInventory::from_restored_entries(root.clone(), Vec::new()).unwrap();
    assert!(empty.files.is_empty());
    assert!(empty.relative_paths.is_empty());

    let revision = SourceRevision {
        modified_ns: 7,
        size: 12,
        object_id: None,
    };
    let mut built = FileInventory::new(root.clone());
    let first = built
        .add("data/a.row".into(), revision, Some(5), CollectionId(0))
        .unwrap();
    let second = built
        .add("data/b.row".into(), revision, None, CollectionId(0))
        .unwrap();
    built
        .add("data/b.row".into(), revision, None, CollectionId(1))
        .unwrap();

    let restored = FileInventory::from_restored_entries(root.clone(), built.files.clone()).unwrap();
    assert_eq!(restored.files, built.files);
    assert_eq!(restored.id_for(Path::new("data/a.row")), Some(first));
    assert_eq!(restored.id_for(Path::new("data/b.row")), Some(second));
    assert_eq!(
        restored.files[1].collections,
        [CollectionId(0), CollectionId(1)]
    );
    assert_eq!(restored.relative_paths, built.relative_paths);
    assert_eq!(
        restored.relative_path_ids_up_to(Operator::Eq, "data/b.row", 1),
        Some(vec![second])
    );

    let mut duplicate = built.files.clone();
    duplicate[1].relative_path = duplicate[0].relative_path.clone();
    assert!(FileInventory::from_restored_entries(root.clone(), duplicate).is_err());
    let mut bad_id = built.files.clone();
    bad_id[1].id = FileId(9);
    assert!(FileInventory::from_restored_entries(root.clone(), bad_id).is_err());
    let mut escaping = built.files.clone();
    escaping[0].relative_path = SharedPath::from("../escape.row");
    assert!(FileInventory::from_restored_entries(root.clone(), escaping).is_err());
    let mut bad_status = built.files.clone();
    bad_status[0].status = FileStatus::Broken;
    assert!(FileInventory::from_restored_entries(root, bad_status).is_err());
}

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
        object_id: None,
    };
    let f0 = files
        .add("empty.row".into(), revision, None, CollectionId(0))
        .unwrap();
    let f1 = files
        .add("multi.rows".into(), revision, None, CollectionId(0))
        .unwrap();
    assert_eq!(
        files.relative_path_ids_up_to(Operator::Eq, "empty.row", 1),
        Some(vec![f0])
    );
    assert_eq!(
        files.relative_path_ids_up_to(Operator::Ge, "empty.row", 1),
        None
    );
    assert!(
        files
            .relative_paths
            .keys()
            .all(|path| !path.starts_with("/tmp/repo/"))
    );
    let SharedPath::Plain(entry_path) = &files.files[f0.0 as usize].relative_path else {
        panic!("ordinary path should share its query key");
    };
    assert!(Arc::ptr_eq(
        entry_path,
        files.relative_paths.get_key_value("empty.row").unwrap().0
    ));
    let lookup_path = files
        .by_path
        .get_key_value(Path::new("empty.row"))
        .unwrap()
        .0;
    let SharedPath::Plain(lookup_path) = lookup_path else {
        panic!("ordinary lookup path should share its entry payload");
    };
    assert!(Arc::ptr_eq(entry_path, lookup_path));
    assert_eq!(
        files
            .add("multi.rows".into(), revision, None, CollectionId(1))
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
fn relative_path_index_preserves_native_escaping_and_rejects_absolute_literals() {
    let mut files = FileInventory::new(PathBuf::from("/tmp/repo"));
    let revision = SourceRevision {
        modified_ns: 1,
        size: 2,
        object_id: None,
    };
    for (name, id) in [("data/a.row", FileId(0)), ("other/a.row", FileId(1))] {
        assert_eq!(
            files
                .add(name.into(), revision, None, CollectionId(0))
                .unwrap(),
            id
        );
        assert_eq!(files.id_for(Path::new(name)), Some(id));
        assert_eq!(
            files.relative_path_ids_up_to(Operator::Eq, name, 2),
            Some(vec![id])
        );
    }
    assert_eq!(
        files.relative_path_ids_up_to(Operator::Eq, "/tmp/repo/data/a.row", 2),
        Some(vec![])
    );
    let escaped = PathBuf::from("data/percent%and\\backslash.row");
    let key = crate::model::path_representation::query_path(&escaped);
    let id = files
        .add(escaped.clone(), revision, None, CollectionId(0))
        .unwrap();
    assert!(matches!(
        files.files[id.0 as usize].relative_path,
        SharedPath::Native(_)
    ));
    assert_eq!(files.id_for(&escaped), Some(id));
    assert_eq!(
        files.relative_path_ids_up_to(Operator::Eq, &key, 2),
        Some(vec![id])
    );
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        let native = PathBuf::from(std::ffi::OsString::from_vec(
            b"data/invalid-\xff.row".to_vec(),
        ));
        let key = crate::model::path_representation::query_path(&native);
        let id = files
            .add(native.clone(), revision, None, CollectionId(0))
            .unwrap();
        assert_eq!(files.id_for(&native), Some(id));
        assert_eq!(
            files.relative_path_ids_up_to(Operator::Eq, &key, 2),
            Some(vec![id])
        );
    }
}

#[test]
fn construction_membership_preserves_order_empty_files_and_record_ownership() {
    let mut inventory = FileInventory::new(PathBuf::from("/tmp/repo"));
    let revision = SourceRevision {
        modified_ns: 7,
        size: 10,
        object_id: None,
    };
    let empty = inventory
        .add("empty.row".into(), revision, None, CollectionId(0))
        .unwrap();
    let valid = inventory
        .add("valid.row".into(), revision, None, CollectionId(0))
        .unwrap();
    let broken = inventory
        .add("broken.row".into(), revision, None, CollectionId(0))
        .unwrap();
    let omitted = inventory
        .add("omitted.row".into(), revision, None, CollectionId(0))
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
                    "fixture-record",
                    1,
                    Path::new("broken.row"),
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
        source_id: "fixture-record".into(),
        source_version: 1,
        record: None,
        range: Some(ByteRange { start: 0, end: 10 }),
        category: DiagnosticCategory::Syntax,
        code: "fixture.parse".into(),
        field_path: None,
        byte_offset: Some(1),
        line: Some(1),
        column: Some(2),
        message: "invalid fixture".into(),
    };
    let reference = inventory.record_diagnostic(broken, diagnostic).unwrap();
    let fallback_slot = builder
        .add_raw_fallback(
            &inventory.files[broken.0 as usize],
            RecordId::fallback(
                CollectionId(0),
                "fixture-record",
                1,
                Path::new("broken.row"),
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
        object_id: None,
    };
    let files: Vec<_> = (0..4)
        .map(|i| {
            inventory
                .add(format!("{i}.rows").into(), revision, None, CollectionId(0))
                .unwrap()
        })
        .collect();
    let empty = CollectionBuilder::new(CollectionId(0), "empty".into(), 1, files.len())
        .finish(&inventory)
        .unwrap();
    assert_eq!(empty.file_records.file_count, 4);
    assert!(empty.file_records.offsets.is_empty());
    assert!(empty.file_records.slots.is_empty());
    for file in 0..4 {
        assert_eq!(
            empty.file_records.records_for(FileId(file)),
            Some([].as_slice())
        );
    }
    assert_eq!(empty.file_records.records_for(FileId(4)), None);

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
        assert_eq!(bincode::serialize(&status).unwrap(), vec![code]);
        assert_eq!(bincode::deserialize::<FileStatus>(&[code]).unwrap(), status);
    }
    assert!(bincode::deserialize::<FileStatus>(&[3]).is_err());
    assert!(bincode::deserialize::<RecordClass>(&[2]).is_err());
    assert_eq!(
        bincode::serialize(&RecordClass::RawFallback).unwrap(),
        vec![1]
    );

    let mut files = FileInventory::new(PathBuf::from("/tmp/repo"));
    let file = files
        .add(
            "broken.rows".into(),
            SourceRevision {
                modified_ns: 5,
                size: 50,
                object_id: None,
            },
            None,
            CollectionId(0),
        )
        .unwrap();
    let make = |category| SourceDiagnostic {
        file,
        collection: CollectionId(0),
        source_id: "fixture-lines".into(),
        source_version: 1,
        record: None,
        range: Some(ByteRange { start: 0, end: 3 }),
        category,
        code: "fixture.parse".into(),
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
fn restored_file_versions_keep_tokens_across_chunks() {
    let mut tokens = vec![FileVersion(1); 1026];
    tokens[0] = FileVersion(7);
    tokens[1024] = FileVersion(3);
    let restored = FileVersions::from_versions(tokens.clone()).unwrap();
    assert_eq!(restored.len(), tokens.len());
    assert_eq!(restored.get(FileId(0)), Some(FileVersion(7)));
    assert_eq!(restored.get(FileId(1024)), Some(FileVersion(3)));
    let (advanced, version) = restored.advanced(FileId(1024)).unwrap();
    assert_eq!(version, FileVersion(4));
    assert_eq!(restored.get(FileId(1024)), Some(FileVersion(3)));
    assert_eq!(advanced.get(FileId(1025)), Some(FileVersion(1)));
    tokens[1025] = FileVersion(0);
    assert!(FileVersions::from_versions(tokens).is_err());
}

#[test]
fn published_active_records_keep_old_view_and_cross_chunk_boundaries() {
    let first = MemoryLiveSlots::all(65_538).unwrap();
    assert_eq!(first.bitmap().len(), 65_538);
    assert_eq!(first.bitmap().max(), Some(65_537));
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

#[test]
fn restored_active_records_keep_sparse_domains_and_append_boundary() {
    let bitmap = RoaringBitmap::from_iter([0, 65_535, 65_537]);
    let restored = MemoryLiveSlots::from_bitmap(65_538, bitmap.clone()).unwrap();
    assert_eq!(restored.slot_count(), 65_538);
    assert_eq!(restored.len(), 3);
    assert_eq!(restored.bitmap(), bitmap);
    assert!(!restored.contains(RecordSlot(65_536)));
    let appended = restored.appended(2).unwrap();
    assert!(appended.contains(RecordSlot(65_538)));
    assert!(appended.contains(RecordSlot(65_539)));
    assert_eq!(restored.len(), 3);
    assert!(MemoryLiveSlots::from_bitmap(65_537, bitmap).is_err());
    assert!(MemoryLiveSlots::from_bitmap(0, RoaringBitmap::from_iter([0])).is_err());
    assert_eq!(
        MemoryLiveSlots::from_bitmap(0, RoaringBitmap::new())
            .unwrap()
            .len(),
        0
    );
}
