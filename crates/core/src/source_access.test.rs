use super::*;
use crate::inventory::SourceRevision;
use crate::model::CollectionId;

#[test]
fn range_windows_are_bounded_and_handles_evict_at_budget() {
    let root = std::env::temp_dir().join(format!("objekte-source-session-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let mut files = FileInventory::new(root.clone());
    for n in 0..9 {
        let path = format!("{n}.txt");
        std::fs::write(root.join(&path), vec![b'a' + n as u8; 1024 * 1024]).unwrap();
        let metadata = std::fs::metadata(root.join(&path)).unwrap();
        let revision = SourceRevision::from_metadata(&metadata).unwrap();
        files
            .add(path.into(), revision, None, CollectionId(0))
            .unwrap();
    }
    let mut session = SourceAccessSession::new(&files);
    let first = session
        .read(FileId(0), &[(0, ByteRange { start: 0, end: 5 })])
        .unwrap();
    assert_eq!(first.opened, 1);
    assert_eq!(first.bytes_read, WINDOW_BYTES);
    assert_eq!(first.records[0].1, b"aaaaa");
    let nearby = session
        .read(FileId(0), &[(0, ByteRange { start: 10, end: 15 })])
        .unwrap();
    assert_eq!(nearby.opened, 0);
    assert_eq!(nearby.bytes_read, 0);
    assert_eq!(nearby.records[0].1, b"aaaaa");
    for n in 1..9 {
        let read = session
            .read(FileId(n), &[(0, ByteRange { start: 0, end: 1 })])
            .unwrap();
        assert_eq!(read.opened, 1);
        assert_eq!(read.bytes_read, WINDOW_BYTES);
        assert!(session.open.len() <= MAX_OPEN_HANDLES);
    }
    let reopened = session
        .read(FileId(0), &[(0, ByteRange { start: 20, end: 21 })])
        .unwrap();
    assert_eq!(reopened.opened, 1);
    assert_eq!(session.distinct_files(), 9);
    assert_eq!(reopened.records[0].1, b"a");
    assert!(
        session
            .read(
                FileId(0),
                &[(
                    0,
                    ByteRange {
                        start: 1_048_575,
                        end: 1_048_577
                    }
                )]
            )
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn window_hits_and_misses_keep_the_reader_at_the_actual_read_end() {
    let root = std::env::temp_dir().join(format!("objekte-source-window-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    let bytes: Vec<u8> = (0..150_000).map(|n| (n % 251) as u8).collect();
    std::fs::write(&path, &bytes).unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let mut files = FileInventory::new(root.clone());
    files
        .add("source.txt".into(), revision, None, CollectionId(0))
        .unwrap();
    let mut session = SourceAccessSession::new(&files);
    for (start, expected_read, expected_seeks) in [
        (0, WINDOW_BYTES, 0),
        (17, 0, 0),
        (65_536, WINDOW_BYTES, 0),
        (17, WINDOW_BYTES, 1),
        (149_999, 1, 2),
    ] {
        let read = session
            .read(
                FileId(0),
                &[(
                    0,
                    ByteRange {
                        start,
                        end: start + 1,
                    },
                )],
            )
            .unwrap();
        assert_eq!(read.records[0].1, bytes[start as usize..start as usize + 1]);
        assert_eq!(read.bytes_read, expected_read);
        assert_eq!(session.open[0].reader.seeks, expected_seeks);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_stream_reads_skip_lookahead_and_cap_spaced_range_windows() {
    let root = std::env::temp_dir().join(format!("objekte-bounded-source-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("spaced.jsonl");
    let mut bytes = vec![b' '; 100 * 2048];
    let mut ranges = Vec::new();
    for slot in 0..100u32 {
        let start = slot as usize * 2048;
        bytes[start..start + 80].fill(slot as u8);
        ranges.push((
            slot,
            ByteRange {
                start: start as u64,
                end: (start + 80) as u64,
            },
        ));
    }
    std::fs::write(&path, bytes).unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let mut files = FileInventory::new(root.clone());
    files
        .add("spaced.jsonl".into(), revision, None, CollectionId(0))
        .unwrap();
    let mut session = SourceAccessSession::new(&files);
    let single = session.read_bounded(FileId(0), &ranges[..1], 8192).unwrap();
    assert_eq!(single.bytes_read, 80);
    assert!(session.open[0].window.is_empty());
    assert_eq!(single.records[0].1, vec![0; 80]);
    let read = session.read_bounded(FileId(0), &ranges, 8192).unwrap();
    assert_eq!(read.records.len(), 100);
    assert_eq!(read.bytes_read, 8000, "spaced gaps are not read");
    assert_eq!(session.open[0].reader.max_window_read, 80);
    assert!(session.open[0].window.is_empty());
    for (slot, data) in read.records {
        assert_eq!(data, vec![slot as u8; 80]);
    }
    assert!(
        session
            .read_bounded(FileId(0), &ranges[..1], 40)
            .err()
            .unwrap()
            .starts_with("stream resource:")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn buffered_hits_reuse_bytes_but_a_new_read_checks_the_opened_revision() {
    use std::io::{Seek, SeekFrom, Write};
    use std::time::{Duration, UNIX_EPOCH};

    let root = std::env::temp_dir().join(format!(
        "objekte-source-freshness-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    std::fs::write(&path, vec![b'a'; 128 * 1024]).unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let mut files = FileInventory::new(root.clone());
    files
        .add("source.txt".into(), revision, None, CollectionId(0))
        .unwrap();
    let mut session = SourceAccessSession::new(&files);
    let range = |start| ByteRange {
        start,
        end: start + 1,
    };
    assert_eq!(
        session
            .read(FileId(0), &[(0, range(0))])
            .unwrap()
            .bytes_read,
        WINDOW_BYTES
    );

    // Same-size content replacement with a changed mtime is detectable at the
    // next actual source read, while an already-buffered window is reused.
    let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    file.seek(SeekFrom::Start(10)).unwrap();
    file.write_all(b"b").unwrap();
    file.set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(10)))
        .unwrap();
    assert_ne!(
        SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap(),
        revision
    );
    let cached = session.read(FileId(0), &[(0, range(10))]).unwrap();
    assert_eq!(cached.bytes_read, 0);
    assert_eq!(cached.records[0].1, b"a");
    assert!(
        session
            .read(FileId(0), &[(0, range(70_000))])
            .err()
            .unwrap()
            .contains("stale source revision")
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn renamed_source_keeps_opened_handle_but_new_open_checks_replacement() {
    use std::time::{Duration, UNIX_EPOCH};

    let root = std::env::temp_dir().join(format!(
        "objekte-source-replacement-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("source.txt");
    std::fs::write(&path, b"old value").unwrap();
    let revision = SourceRevision::from_metadata(&std::fs::metadata(&path).unwrap()).unwrap();
    let mut reader = RecordRangeReader::open(&path, revision).unwrap();
    std::fs::rename(&path, root.join("moved.txt")).unwrap();
    std::fs::write(&path, b"new value").unwrap();
    std::fs::File::open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(10)))
        .unwrap();
    let old = reader.read(&[(0, ByteRange { start: 0, end: 3 })]).unwrap();
    assert_eq!(old.0[0].1, b"old");
    let old_again = reader.read(&[(0, ByteRange { start: 4, end: 9 })]).unwrap();
    assert_eq!(old_again.0[0].1, b"value");
    assert!(
        RecordRangeReader::open(&path, revision)
            .err()
            .unwrap()
            .contains("stale source revision")
    );
    std::fs::remove_dir_all(root).unwrap();
}
