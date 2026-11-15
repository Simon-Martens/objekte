use std::sync::Arc;

use super::backing::{Backing, MappedObject};
use super::catalogue::{
    CatalogueView, DiagnosticInput, FILE_ROW_LEN, FileRowInput, build_catalogue,
    build_catalogue_with_high_water,
};
use super::format::{ObjectBuilder, ObjectKind, SectionKind};

fn rows() -> Vec<FileRowInput> {
    let mut rows = Vec::new();
    for file in 0..150u32 {
        let tombstone = file % 17 == 0;
        rows.push(FileRowInput {
            file_id: file,
            status: if file % 11 == 0 { 2 } else { 0 },
            tombstone,
            file_version: file as u64 + 1,
            content_generation: file as u64 * 3 + 1,
            size: 100 + file as u64,
            modified_ns: 1_700_000_000_000_000_000 + file as i128,
            created_ns: (file % 5 == 0).then_some(1_600_000_000_000_000_000 + file as i128),
            native_path: {
                let mut path = format!("dir{}\u{0}file{}.txt", file % 7, file).into_bytes();
                if file == 0 {
                    path.extend_from_slice(&[0xff, 0xfe]);
                }
                path
            },
            query_path: format!("dir{}/file{}.txt", file % 7, file).into_bytes(),
            collections: vec![0, (file % 3) as u32],
            diagnostics: if file % 13 == 0 {
                vec![DiagnosticInput {
                    category: 2,
                    code: "parse".into(),
                    field_path: vec!["title".into()],
                    byte_offset: Some(file as u64 * 10),
                    line: Some(file as u64),
                    column: Some(3),
                    message: "malformed value".into(),
                }]
            } else {
                Vec::new()
            },
            omitted_diagnostics: u32::from(file % 13 == 0),
            fingerprint: (file % 4 == 0).then_some([file as u8; 32]),
        });
    }
    rows
}

fn catalogue_bytes() -> Vec<u8> {
    let sections = build_catalogue(&rows()).unwrap();
    let mut builder = ObjectBuilder::new(ObjectKind::Catalogue);
    builder
        .add_section(SectionKind::FileRows, sections.file_rows)
        .unwrap();
    builder
        .add_section(SectionKind::PathDictionary, sections.path_dictionary)
        .unwrap();
    builder
        .add_section(SectionKind::Diagnostics, sections.diagnostics)
        .unwrap();
    builder
        .add_section(SectionKind::Statistics, sections.statistics)
        .unwrap();
    builder
        .add_section(SectionKind::ActiveFiles, sections.active_files)
        .unwrap();
    builder
        .add_section(SectionKind::Directories, sections.directories)
        .unwrap();
    builder.finish().unwrap().into_bytes()
}

fn open(bytes: Vec<u8>) -> CatalogueView {
    CatalogueView::open(Backing::resident(bytes)).unwrap()
}

#[test]
fn paged_rows_paths_and_statistics_round_trip() {
    let view = open(catalogue_bytes());
    let stats = view.stats();
    assert_eq!(stats.high_water, 150);
    assert_eq!(stats.page_count, 3);
    assert_eq!(stats.file_count, 150);
    assert_eq!(
        stats.tombstone_count,
        rows().iter().filter(|row| row.tombstone).count() as u32
    );
    assert_eq!(stats.collection_count, 3);
    for file in 0..150u32 {
        let row = match view.file(file) {
            Ok(Some(row)) => row,
            other => panic!("file {file}: {other:?}"),
        };
        assert_eq!(row.file_id, file);
        assert_eq!(row.tombstone, file % 17 == 0);
        assert_eq!(row.size, 100 + file as u64);
        assert_eq!(
            row.query_path,
            format!("dir{}/file{}.txt", file % 7, file).as_bytes()
        );
        assert!(row.native_path.contains(&0));
        if file == 0 {
            assert_eq!(&row.native_path[row.native_path.len() - 2..], &[0xff, 0xfe]);
        }
        assert_eq!(row.collections, vec![0, file % 3]);
        if file % 4 == 0 {
            assert_eq!(row.fingerprint, Some([file as u8; 32]));
        }
        if file % 13 == 0 {
            let diagnostics = view.diagnostics(&row).unwrap();
            assert_eq!(diagnostics.len(), 1);
            assert_eq!(diagnostics[0].code, "parse");
            assert_eq!(diagnostics[0].field_path, vec!["title"]);
            assert_eq!(diagnostics[0].line, Some(file as u64));
        }
    }
    assert_eq!(view.file(999).unwrap(), None);
    assert_eq!(
        view.path(b"dir0/file0.txt").unwrap(),
        None,
        "tombstoned path is suppressed"
    );
    assert_eq!(view.path(b"dir1/file1.txt").unwrap(), Some(1));
    assert_eq!(view.path(b"missing").unwrap(), None);
    let mut expected = (0..150u32)
        .filter(|file| file % 7 == 3 && file % 17 != 0)
        .collect::<Vec<_>>();
    expected.sort_by_key(|file| format!("dir3/file{file}.txt"));
    assert_eq!(view.paths_with_prefix(b"dir3/", 100).unwrap(), expected);
    let directories = view
        .directories()
        .range(std::ops::Bound::Unbounded, std::ops::Bound::Unbounded)
        .map(|entry| entry.map(|(key, _)| String::from_utf8(key).unwrap()))
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    assert_eq!(
        directories,
        (0..7)
            .map(|number| format!("dir{number}"))
            .collect::<Vec<_>>()
    );
    view.verify_full().unwrap();
}

#[test]
fn resident_and_mapped_catalogues_agree() {
    let bytes = catalogue_bytes();
    let resident = open(bytes.clone());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalogue.obx");
    std::fs::write(&path, &bytes).unwrap();
    let mapped = CatalogueView::open(Backing::mapped(Arc::new(
        MappedObject::open(&path, None).unwrap(),
    )))
    .unwrap();
    for file in [0u32, 1, 63, 64, 127, 149] {
        assert_eq!(resident.file(file).unwrap(), mapped.file(file).unwrap());
    }
    assert_eq!(
        resident.path(b"dir5/file5.txt").unwrap(),
        mapped.path(b"dir5/file5.txt").unwrap()
    );
    mapped.verify_full().unwrap();
}

#[test]
fn corrupt_rows_and_truncated_objects_are_rejected() {
    let bytes = catalogue_bytes();
    let mut truncated = bytes.clone();
    truncated.truncate(200);
    assert!(CatalogueView::open(Backing::resident(truncated)).is_err());

    let mut corrupt = bytes;
    let last = corrupt.len() - 10;
    corrupt[last] ^= 0xff;
    // The object header remains valid; full verification catches the payload
    // checksum mismatch.
    let view = CatalogueView::open(Backing::resident(corrupt)).unwrap();
    assert!(view.verify_full().is_err());
}

#[test]
fn allocated_high_water_survives_deleted_tail_slots() {
    let sections = build_catalogue_with_high_water(&rows()[..2], 25).unwrap();
    let mut builder = ObjectBuilder::new(ObjectKind::Catalogue);
    builder
        .add_section(SectionKind::FileRows, sections.file_rows)
        .unwrap();
    builder
        .add_section(SectionKind::PathDictionary, sections.path_dictionary)
        .unwrap();
    builder
        .add_section(SectionKind::Diagnostics, sections.diagnostics)
        .unwrap();
    builder
        .add_section(SectionKind::Statistics, sections.statistics)
        .unwrap();
    builder
        .add_section(SectionKind::ActiveFiles, sections.active_files)
        .unwrap();
    builder
        .add_section(SectionKind::Directories, sections.directories)
        .unwrap();
    let view = open(builder.finish().unwrap().into_bytes());
    assert_eq!(view.high_water(), 25);
    assert!(view.file(2).unwrap().is_none());
    assert!(view.file(24).unwrap().is_none());
    assert!(view.file(1).unwrap().is_some());
}

#[test]
fn every_file_row_is_fixed_width() {
    assert_eq!(FILE_ROW_LEN, 192);
}
