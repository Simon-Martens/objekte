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
        files.add(path.into(), revision, None, CollectionId(0)).unwrap();
    }
    let mut session = SourceAccessSession::new(&files);
    let first = session.read(FileId(0), &[(0, ByteRange { start: 0, end: 5 })]).unwrap();
    assert_eq!(first.opened, 1);
    assert_eq!(first.bytes_read, WINDOW_BYTES);
    assert_eq!(first.records[0].1, b"aaaaa");
    let nearby = session.read(FileId(0), &[(0, ByteRange { start: 10, end: 15 })]).unwrap();
    assert_eq!(nearby.opened, 0);
    assert_eq!(nearby.bytes_read, 0);
    assert_eq!(nearby.records[0].1, b"aaaaa");
    for n in 1..9 {
        let read = session.read(FileId(n), &[(0, ByteRange { start: 0, end: 1 })]).unwrap();
        assert_eq!(read.opened, 1);
        assert_eq!(read.bytes_read, WINDOW_BYTES);
        assert!(session.open.len() <= MAX_OPEN_HANDLES);
    }
    let reopened = session.read(FileId(0), &[(0, ByteRange { start: 20, end: 21 })]).unwrap();
    assert_eq!(reopened.opened, 1);
    assert_eq!(session.distinct_files(), 9);
    assert_eq!(reopened.records[0].1, b"a");
    assert!(session.read(FileId(0), &[(0, ByteRange { start: 1_048_575, end: 1_048_577 })]).is_err());
    std::fs::remove_dir_all(root).unwrap();
}
