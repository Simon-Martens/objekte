use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::inventory::SourceRevision;
use crate::source::test_support::DOCUMENT_READER;
use crate::source::test_support::LINE_READER;
use crate::source::test_support::OutcomeReader;
use crate::source::{
    ByteRange, DiagnosticCategory, ReaderDiagnostic, SourceOutcome, SourceReader,
    extraction::extract_file, extraction::extract_file_build_outcomes_hashed_with_buffer,
    extraction::trim_oversized_build_buffer,
};

use super::{
    FILE_COORDINATE_SCANNED_BYTES, FileCoordinateCursor, MAX_RETAINED_BUILD_BUFFER_BYTES,
    SourceBufferProfile, extract_file_build_outcomes, extract_file_outcomes,
    extract_framed_outcomes_impl, file_coordinates, frame_file_for_build,
};

#[test]
fn reusable_build_buffer_preserves_outcomes_and_releases_large_storage() {
    use sha2::{Digest, Sha256};

    let root = std::env::temp_dir().join(format!(
        "objekte-reused-build-buffer-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let mut buffer = Vec::new();
    let mut profile = SourceBufferProfile::default();
    let parses = Arc::new(AtomicUsize::new(0));
    let large = vec![b'x'; MAX_RETAINED_BUILD_BUFFER_BYTES + 1];
    for (name, bytes, malformed) in [
        ("large.outcome", large.as_slice(), false),
        ("small.outcome", b"ok".as_slice(), false),
        ("broken.outcome", b"bad".as_slice(), true),
        ("last.outcome", b"next".as_slice(), false),
    ] {
        let path = root.join(name);
        std::fs::write(&path, bytes).unwrap();
        let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
        let reader = OutcomeReader {
            ranges: vec![ByteRange {
                start: 0,
                end: bytes.len() as u64,
            }],
            parses: parses.clone(),
            fail_framing: malformed,
        };
        let mut outcome_kind = None;
        let (_, digest) = extract_file_build_outcomes_hashed_with_buffer(
            &path,
            &reader,
            revision,
            &mut buffer,
            None,
            Some(&mut profile),
            |outcome| {
                outcome_kind = Some(match outcome {
                    SourceOutcome::Logical { .. } => "logical",
                    SourceOutcome::Malformed { bytes: raw, .. } => {
                        assert_eq!(raw, bytes);
                        "malformed"
                    }
                    SourceOutcome::Unreadable { .. } => "unreadable",
                });
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(digest, Some(Sha256::digest(bytes).into()));
        assert_eq!(
            outcome_kind,
            Some(if malformed { "malformed" } else { "logical" })
        );
        if name == "large.outcome" {
            assert!(buffer.capacity() > MAX_RETAINED_BUILD_BUFFER_BYTES);
            assert!(trim_oversized_build_buffer(&mut buffer));
            assert_eq!(buffer.capacity(), 0);
        } else if name == "small.outcome" {
            assert!(buffer.capacity() >= bytes.len());
            assert!(!trim_oversized_build_buffer(&mut buffer));
        } else if malformed {
            assert_eq!(buffer.capacity(), 0);
        }
    }
    assert_eq!(profile.files_read, 4);
    #[cfg(feature = "detailed-profiling")]
    {
        assert!(profile.growth_files >= 3);
        assert!(profile.peak_capacity_bytes > MAX_RETAINED_BUILD_BUFFER_BYTES as u64);
    }
    assert_eq!(parses.load(Ordering::Relaxed), 3);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn unavailable_source_read_is_classified_without_fabricating_records() {
    let missing =
        std::env::temp_dir().join(format!("objekte-missing-source-{}.row", std::process::id()));
    let mut issues = Vec::new();
    let counts = extract_file_outcomes(
        &missing,
        &DOCUMENT_READER,
        SourceRevision {
            modified_ns: 0,
            size: 0,
            object_id: None,
        },
        |outcome| {
            match outcome {
                SourceOutcome::Unreadable { diagnostic } => issues.push(diagnostic),
                _ => panic!("unreadable source produced a record"),
            }
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(counts.framed_records, 0);
    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].category, DiagnosticCategory::Io);
    assert_eq!(issues[0].code, "source.open");
}

#[test]
fn malformed_lines_coordinate_work_grows_with_file_size() {
    fn scan_work(frames: usize) -> (usize, usize) {
        let bytes = b"{oops}\n".repeat(frames);
        let ranges = LINE_READER.frame(&bytes).unwrap();
        FILE_COORDINATE_SCANNED_BYTES.with(|scanned| scanned.set(0));
        let mut malformed = 0;
        let counters = extract_framed_outcomes_impl(
            Path::new("malformed.rows"),
            &LINE_READER,
            &bytes,
            &ranges,
            true,
            |outcome| {
                let SourceOutcome::Malformed { diagnostic, .. } = outcome else {
                    panic!("invalid line-framed source frame was accepted")
                };
                malformed += 1;
                assert_eq!(diagnostic.line, Some(malformed));
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(counters.failed_parses, frames as u64);
        assert_eq!(malformed, frames as u64);
        let scanned = FILE_COORDINATE_SCANNED_BYTES.with(|scanned| scanned.get());
        (scanned, bytes.len())
    }

    let (small_work, small_size) = scan_work(128);
    let (large_work, large_size) = scan_work(256);
    assert!(small_work <= small_size, "{small_work} > {small_size}");
    assert!(large_work <= large_size, "{large_work} > {large_size}");
    assert!(large_work <= small_work * 2 + 8);
}

#[test]
fn file_coordinates_preserve_custom_reader_line_and_column_meaning() {
    let file = b"aa\r\nbb_cc\nmm\nnn";
    let mut cursor = FileCoordinateCursor::default();
    let mut diagnostic = ReaderDiagnostic::new(DiagnosticCategory::Syntax, "custom", "bad");
    diagnostic.line = Some(1);
    diagnostic.column = Some(2);
    diagnostic.relative_byte = Some(1);
    file_coordinates(
        file,
        ByteRange { start: 6, end: 9 },
        &mut diagnostic,
        &mut cursor,
    );
    assert_eq!((diagnostic.line, diagnostic.column), (Some(2), Some(4)));
    assert_eq!(diagnostic.relative_byte, Some(1));

    let mut at_boundary = ReaderDiagnostic::new(DiagnosticCategory::Syntax, "custom", "bad");
    at_boundary.line = Some(1);
    at_boundary.column = Some(1);
    file_coordinates(
        file,
        ByteRange { start: 13, end: 15 },
        &mut at_boundary,
        &mut cursor,
    );
    assert_eq!((at_boundary.line, at_boundary.column), (Some(4), Some(1)));

    let mut without_line = ReaderDiagnostic::new(DiagnosticCategory::Syntax, "custom", "bad");
    without_line.column = Some(7);
    file_coordinates(
        file,
        ByteRange { start: 15, end: 15 },
        &mut without_line,
        &mut cursor,
    );
    assert_eq!((without_line.line, without_line.column), (None, Some(7)));

    let mut multiline = ReaderDiagnostic::new(DiagnosticCategory::Syntax, "custom", "bad");
    multiline.line = Some(2);
    multiline.column = Some(1);
    file_coordinates(
        b"prefix\n{\n}\n",
        ByteRange { start: 7, end: 10 },
        &mut multiline,
        &mut FileCoordinateCursor::default(),
    );
    assert_eq!((multiline.line, multiline.column), (Some(3), Some(1)));
}

#[test]
fn fused_extraction_parses_once_and_reports_bad_record() {
    let path =
        std::env::temp_dir().join(format!("objekte-source-{}-{}.rows", std::process::id(), 42));
    std::fs::write(&path, b"(\"name\"=\"A\")\n(\"name\"=\"B\")\n").unwrap();
    let mut seen = Vec::new();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let stats = extract_file(
        &path,
        crate::source::test_support::line_format(),
        revision,
        |range, record| {
            seen.push((range, record.text[0].text.clone()));
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(stats.record_parses, 2);
    assert_eq!(seen.len(), 2);
    std::fs::write(&path, b"(\"name\"=\"A\")\n(oops)\n").unwrap();
    assert!(
        extract_file(
            &path,
            crate::source::test_support::line_format(),
            SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
            |_, _| Ok(())
        )
        .unwrap_err()
        .contains("..")
    );
    std::fs::remove_file(path).unwrap();
}

#[test]
fn build_reads_use_actual_bytes_without_revision_probes() {
    let root =
        std::env::temp_dir().join(format!("objekte-build-stable-input-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("records.rows");
    std::fs::write(&path, b"(\"x\"=1)\n(\"x\"=2)\n").unwrap();
    let wrong_revision = SourceRevision {
        modified_ns: 0,
        size: 1,
        object_id: None,
    };
    let mut ranges = Vec::new();
    let counters = extract_file_build_outcomes(&path, &LINE_READER, wrong_revision, |outcome| {
        let SourceOutcome::Logical { range, .. } = outcome else {
            panic!("expected logical record")
        };
        ranges.push(range);
        Ok(())
    })
    .unwrap();
    assert_eq!(counters.record_parses, 2);
    assert_eq!(
        ranges,
        [
            ByteRange { start: 0, end: 7 },
            ByteRange { start: 8, end: 15 }
        ]
    );
    assert!(extract_file_outcomes(&path, &LINE_READER, wrong_revision, |_| Ok(())).is_err());
    let framed = frame_file_for_build(&path, &LINE_READER).unwrap().unwrap();
    assert_eq!(framed.0.len(), 16);
    assert_eq!(framed.1, ranges);

    let malformed = root.join("malformed.rows");
    std::fs::write(&malformed, b"{\n").unwrap();
    extract_file_build_outcomes(&malformed, &LINE_READER, wrong_revision, |outcome| {
        assert!(matches!(outcome, SourceOutcome::Malformed { .. }));
        Ok(())
    })
    .unwrap();
    let empty = root.join("empty.rows");
    std::fs::write(&empty, b"").unwrap();
    assert_eq!(
        extract_file_build_outcomes(&empty, &LINE_READER, wrong_revision, |_| panic!(
            "empty file must not emit a record"
        ))
        .unwrap()
        .framed_records,
        0
    );
    let missing = root.join("missing.rows");
    extract_file_build_outcomes(&missing, &LINE_READER, wrong_revision, |outcome| {
        assert!(matches!(outcome, SourceOutcome::Unreadable { .. }));
        Ok(())
    })
    .unwrap();
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn fresh_build_rejects_same_stamp_replacement_after_discovery() {
    let root = std::env::temp_dir().join(format!(
        "objekte-discovery-replacement-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::create_dir(&root).unwrap();
    let path = root.join("source.row");
    let substitute = root.join("replacement");
    std::fs::write(&path, br#"("id"="A")"#).unwrap();
    let discovery = std::fs::metadata(&path).unwrap();
    let expected = SourceRevision::from_metadata(&discovery).unwrap();
    std::fs::write(&substitute, br#"("id"="B")"#).unwrap();
    std::fs::File::open(&substitute)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(discovery.modified().unwrap()))
        .unwrap();
    std::fs::rename(&substitute, &path).unwrap();
    let current = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    assert_eq!(
        (expected.size, expected.modified_ns),
        (current.size, current.modified_ns)
    );
    assert_ne!(expected.object_id, current.object_id);
    let result = extract_file_build_outcomes_hashed_with_buffer(
        &path,
        &DOCUMENT_READER,
        expected,
        &mut Vec::new(),
        None,
        None,
        |_| panic!("replacement bytes must not reach a discovery-owned contribution"),
    );
    assert!(result.unwrap_err().contains("stale source revision"));
    std::fs::remove_dir_all(root).unwrap();
}
