#[test]
fn source_failure_classifier_keeps_missing_revision_and_range_distinct() {
    use crate::measurement::SourceUnavailableKind;
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("source-failure-classifier-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("record.json");
    std::fs::write(&path, b"{}").unwrap();
    let original = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    assert_eq!(
        classify_source_read_failure(&path, original, Some(ByteRange { start: 0, end: 3 })),
        SourceUnavailableKind::UnusableRange,
    );
    std::fs::write(&path, b"{\"id\":1}").unwrap();
    assert_eq!(
        classify_source_read_failure(&path, original, None),
        SourceUnavailableKind::RevisionMismatch,
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        classify_source_read_failure(&path, original, None),
        SourceUnavailableKind::Missing,
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn typed_source_change_requires_a_confirmed_revision_mismatch() {
    use crate::streaming::{ExecutionContext, ExecutionErrorKind, ExecutionLimits};
    let root = std::env::temp_dir().join(format!(
        "objekte-typed-source-change-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.json");
    std::fs::write(&path, b"old").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let range = ByteRange::checked(0, 3, revision.size).unwrap();
    let context = || {
        ExecutionContext::new(
            crate::query::clock::ClockContext::new(time::OffsetDateTime::now_utc()),
            ExecutionLimits::default(),
            None,
        )
    };

    std::fs::write(&path, b"newer content").unwrap();
    let mismatch = context();
    let error = read_record_range_with_execution(&path, revision, range, &mismatch).unwrap_err();
    assert_eq!(
        mismatch.recorded_error(error).kind,
        ExecutionErrorKind::SourceChanged
    );

    std::fs::remove_file(&path).unwrap();
    let missing = context();
    let error = read_record_range_with_execution(&path, revision, range, &missing).unwrap_err();
    assert_eq!(
        missing.recorded_error(error).kind,
        ExecutionErrorKind::Execution
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn nearby_multi_record_ranges_share_one_read_window() {
    let path = std::env::temp_dir().join(format!("objekte-batch-{}.jsonl", std::process::id()));
    std::fs::write(&path, b"{\"x\":1}\n{\"x\":2}\n").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let ranges = JsonLinesReader
        .frame(&std::fs::read(&path).unwrap())
        .unwrap();
    let (records, bytes_read) =
        read_record_ranges(&path, revision, &[(1, ranges[1]), (0, ranges[0])]).unwrap();
    assert_eq!(records.len(), 2);
    assert_eq!(records[0].0, 0);
    assert_eq!(records[1].1, b"{\"x\":2}");
    assert_eq!(bytes_read, 15);
    let (single, single_bytes) = read_record_ranges(&path, revision, &[(1, ranges[1])]).unwrap();
    assert_eq!(single, vec![(1, b"{\"x\":2}".to_vec())]);
    assert_eq!(single_bytes, 7);
    std::fs::remove_file(path).unwrap();
}

#[test]
fn positioned_reader_seeks_only_when_the_next_range_is_not_adjacent() {
    let root =
        std::env::temp_dir().join(format!("objekte-positioned-reader-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    std::fs::write(&path, b"0123456789abcdef").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let mut reader = RecordRangeReader::open(&path, revision).unwrap();
    let read = |reader: &mut RecordRangeReader, start, end| {
        reader
            .read(&[(0, ByteRange { start, end })])
            .unwrap()
            .0
            .remove(0)
            .1
    };
    assert_eq!(read(&mut reader, 0, 2), b"01");
    assert_eq!(reader.seeks, 0);
    assert_eq!(read(&mut reader, 2, 4), b"23");
    assert_eq!(reader.seeks, 0);
    assert_eq!(read(&mut reader, 0, 2), b"01");
    assert_eq!(reader.seeks, 1);
    assert_eq!(read(&mut reader, 8, 10), b"89");
    assert_eq!(reader.seeks, 2);
    assert_eq!(read(&mut reader, 10, 10), b"");
    assert_eq!(reader.seeks, 2);
    assert!(reader.read(&[]).unwrap().0.is_empty());
    assert_eq!(reader.seeks, 2);
    assert_eq!(
        reader.read(&[(0, ByteRange { start: 17, end: 18 })]),
        Err("invalid byte range 17..18 for file size 16".into())
    );
    assert_eq!(reader.seeks, 2);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn positioned_reader_tracks_coalesced_windows_and_failed_reads() {
    let root =
        std::env::temp_dir().join(format!("objekte-positioned-window-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    std::fs::write(&path, b"0123456789abcdef").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let mut reader = RecordRangeReader::open(&path, revision).unwrap();
    let (values, bytes_read) = reader
        .read(&[
            (2, ByteRange { start: 4, end: 6 }),
            (1, ByteRange { start: 0, end: 2 }),
        ])
        .unwrap();
    assert_eq!(values, vec![(1, b"01".to_vec()), (2, b"45".to_vec())]);
    assert_eq!(bytes_read, 6);
    assert_eq!(reader.seeks, 0);
    assert_eq!(reader.position, Some(6));
    assert_eq!(
        reader
            .read(&[(3, ByteRange { start: 6, end: 8 })])
            .unwrap()
            .0,
        vec![(3, b"67".to_vec())]
    );
    assert_eq!(reader.seeks, 0);
    assert_eq!(
        reader
            .read(&[(4, ByteRange { start: 1, end: 2 })])
            .unwrap()
            .0,
        vec![(4, b"1".to_vec())]
    );
    assert_eq!(reader.seeks, 1);
    let mut sparse = RecordRangeReader::open(&path, revision).unwrap();
    let (values, bytes_read) = sparse
        .read(&[
            (0, ByteRange { start: 0, end: 1 }),
            (1, ByteRange { start: 10, end: 11 }),
        ])
        .unwrap();
    assert_eq!(values, vec![(0, b"0".to_vec()), (1, b"a".to_vec())]);
    assert_eq!(bytes_read, 11);
    assert_eq!(sparse.seeks, 0);
    let mut failed = RecordRangeReader::open(&path, revision).unwrap();
    std::fs::write(&path, b"01").unwrap();
    assert!(failed.read(&[(0, ByteRange { start: 0, end: 4 })]).is_err());
    assert_eq!(failed.position, None);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn changed_source_rejects_old_ranges_and_incomplete_extraction() {
    let path = std::env::temp_dir().join(format!("objekte-stale-{}.jsonl", std::process::id()));
    std::fs::write(&path, b"{\"x\":1}\n{\"x\":2}\n").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let ranges = JsonLinesReader
        .frame(&std::fs::read(&path).unwrap())
        .unwrap();
    let changed = extract_file(&path, SourceFormat::JsonLines, revision, |_, _| {
        std::fs::write(&path, b"{\"x\":1}\n{\"x\":22}\n").map_err(|e| e.to_string())?;
        Ok(())
    });
    assert!(changed.unwrap_err().contains("stale source revision"));
    assert!(
        read_record_range(&path, revision, ranges[1])
            .unwrap_err()
            .contains("stale source revision")
    );
    let current = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    assert!(
        read_record_ranges(
            &path,
            current,
            &[(
                0,
                ByteRange {
                    start: 0,
                    end: current.size + 1
                }
            )]
        )
        .unwrap_err()
        .contains("invalid byte range")
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn same_size_edit_with_restored_mtime_is_outside_revision_detection() {
    let root = std::env::temp_dir().join(format!(
        "objekte-same-revision-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    std::fs::write(&path, b"original").unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let revision = SourceRevision::from_metadata(&metadata).unwrap();
    let mut reader = RecordRangeReader::open(&path, revision).unwrap();
    std::fs::write(&path, b"modified").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert_eq!(
        SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        revision
    );
    assert_eq!(
        reader
            .read(&[(0, ByteRange { start: 0, end: 8 })])
            .unwrap()
            .0[0]
            .1,
        b"modified"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn checked_range_reads_reject_replaced_path_with_spoofed_revision() {
    let root = std::env::temp_dir().join(format!(
        "objekte-source-incarnation-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    let moved = root.join("moved.txt");
    std::fs::write(&path, b"original").unwrap();
    let metadata = std::fs::metadata(&path).unwrap();
    let revision = SourceRevision::from_metadata(&metadata).unwrap();
    let created_ns = metadata
        .created()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos() as i128);
    let fingerprint: [u8; 32] = Sha256::digest(b"original").into();
    let range = ByteRange { start: 0, end: 8 };
    std::fs::rename(&path, &moved).unwrap();
    std::fs::write(&path, b"modified").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert_eq!(
        SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        revision
    );
    if created_ns.is_some() {
        assert!(
            read_record_range_checked(&path, revision, created_ns, Some(fingerprint), range)
                .is_err()
        );
        assert!(
            RecordRangeReader::open_checked(&path, revision, created_ns, Some(fingerprint))
                .is_err()
        );
        assert_eq!(
            probe_source_unavailable_checked(
                &path,
                revision,
                created_ns,
                Some(fingerprint),
                Some(range)
            ),
            Some(SourceUnavailableKind::RevisionMismatch)
        );
    }
    assert!(read_record_range_checked(&path, revision, None, Some(fingerprint), range).is_err());
    assert!(RecordRangeReader::open_checked(&path, revision, None, None).is_err());
    assert_eq!(
        classify_source_read_failure_checked(&path, revision, None, Some(fingerprint), Some(range)),
        SourceUnavailableKind::RevisionMismatch
    );
    assert_eq!(
        read_record_range_checked(&moved, revision, created_ns, Some(fingerprint), range).unwrap(),
        b"original"
    );
    assert_eq!(
        RecordRangeReader::open_checked(&moved, revision, None, Some(fingerprint))
            .unwrap()
            .read(&[(0, range)])
            .unwrap()
            .0[0]
            .1,
        b"original"
    );
    let mut pinned_reader =
        RecordRangeReader::open_checked(&moved, revision, None, Some(fingerprint)).unwrap();
    std::fs::write(&moved, b"modified").unwrap();
    std::fs::File::open(&moved)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(metadata.modified().unwrap()))
        .unwrap();
    assert!(pinned_reader.read(&[(0, range)]).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
use super::*;
use crate::source::json_lines::JsonLinesReader;
use crate::source::{SourceFormat, SourceReader, extract_file};
use sha2::{Digest, Sha256};
