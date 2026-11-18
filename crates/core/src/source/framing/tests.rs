#[test]
fn bounded_jsonl_chunks_preserve_whole_file_ranges_and_line_origins() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("target")
        .join(format!("bounded-jsonl-source-{}", std::process::id()));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("records.jsonl");
    let bytes = b"\n \t\r\n{\"id\":\"a\"}\r\n{\"id\":\"b\"}\n\x0c\n{\"id\":\"c\"}";
    std::fs::write(&path, bytes).unwrap();
    let expected = JsonLinesReader.frame(bytes).unwrap();
    let mut reader = BoundedJsonLines::open(&path).unwrap();
    let mut ranges = Vec::new();
    let mut seen_bytes = 0usize;
    let mut previous_lines = 0u64;
    while let Some(chunk) = reader.next_chunk(18, 2).unwrap() {
        assert_eq!(chunk.file_offset as usize, seen_bytes);
        assert_eq!(chunk.preceding_lines, previous_lines);
        assert_eq!(
            chunk.bytes,
            bytes[seen_bytes..seen_bytes + chunk.bytes.len()]
        );
        assert!(chunk.bytes.len() <= 18 || chunk.ranges.len() <= 1);
        for range in chunk.ranges {
            ranges.push(ByteRange {
                start: chunk.file_offset + range.start,
                end: chunk.file_offset + range.end,
            });
        }
        previous_lines += chunk.bytes.iter().filter(|byte| **byte == b'\n').count() as u64;
        seen_bytes += chunk.bytes.len();
    }
    assert_eq!(seen_bytes, bytes.len());
    assert_eq!(ranges, expected);
    assert!(reader.next_chunk(18, 2).unwrap().is_none());

    let oversized = root.join("oversized.jsonl");
    let one = format!("{{\"x\":\"{}\"}}\n", "a".repeat(100));
    std::fs::write(&oversized, &one).unwrap();
    let mut reader = BoundedJsonLines::open(&oversized).unwrap();
    let chunk = reader.next_chunk(16, 2).unwrap().unwrap();
    assert_eq!(chunk.bytes, one.as_bytes());
    assert_eq!(chunk.ranges.len(), 1);
    assert!(reader.next_chunk(16, 2).unwrap().is_none());

    let empty = root.join("empty.jsonl");
    std::fs::write(&empty, b"").unwrap();
    assert!(
        BoundedJsonLines::open(&empty)
            .unwrap()
            .next_chunk(16, 2)
            .unwrap()
            .is_none()
    );
    assert!(JsonLinesReader.supports_bounded_json_lines_build());
    assert!(!JsonReader.supports_bounded_json_lines_build());
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_jsonl_many_records_keep_offsets_and_chunk_limits() {
    let root = std::env::temp_dir().join(format!(
        "objekte-many-bounded-lines-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("records.jsonl");
    let mut bytes = Vec::new();
    for index in 0..600 {
        bytes.extend_from_slice(format!("{{\"id\":{index}}}\r\n").as_bytes());
        if index % 13 == 0 {
            bytes.extend_from_slice(b" \t\r\n");
        }
    }
    std::fs::write(&path, &bytes).unwrap();
    let expected = JsonLinesReader.frame(&bytes).unwrap();
    let mut framer = BoundedJsonLines::open(&path).unwrap();
    let mut actual = Vec::new();
    let mut chunk_count = 0;
    let mut read_bytes = 0;
    while let Some(chunk) = framer.next_chunk(256, 17).unwrap() {
        chunk_count += 1;
        assert!(chunk.bytes.len() <= 256);
        assert!(chunk.ranges.len() <= 17);
        assert_eq!(chunk.file_offset, read_bytes);
        for range in chunk.ranges {
            actual.push(ByteRange {
                start: range.start + chunk.file_offset,
                end: range.end + chunk.file_offset,
            });
        }
        read_bytes += chunk.bytes.len() as u64;
    }
    assert!(chunk_count > 30);
    assert_eq!(read_bytes, bytes.len() as u64);
    assert_eq!(actual, expected);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn bounded_jsonl_reports_read_errors_before_and_after_first_chunk() {
    struct FailingRead {
        bytes: std::io::Cursor<&'static [u8]>,
        limit: u64,
    }

    impl Read for FailingRead {
        fn read(&mut self, output: &mut [u8]) -> std::io::Result<usize> {
            if self.bytes.position() >= self.limit {
                return Err(std::io::Error::other("injected read failure"));
            }
            let available = (self.limit - self.bytes.position()) as usize;
            let count = output.len().min(available);
            self.bytes.read(&mut output[..count])
        }
    }

    const LINES: &[u8] = b"{\"a\":1}\n{\"a\":2}\n";
    let make = |limit| {
        BoundedJsonLines::from_reader(BufReader::with_capacity(
            1,
            FailingRead {
                bytes: std::io::Cursor::new(LINES),
                limit,
            },
        ))
    };
    assert!(make(0).next_chunk(8, 1).is_err());
    let mut after_first = make(8);
    assert_eq!(
        after_first.next_chunk(8, 1).unwrap().unwrap().ranges.len(),
        1
    );
    assert!(after_first.next_chunk(8, 1).is_err());
}

use std::io::{BufReader, Read};
use std::path::Path;

use crate::inventory::ByteRange;
use crate::source::SourceReader;
use crate::source::json::JsonReader;
use crate::source::json_lines::JsonLinesReader;

use super::BoundedJsonLines;
