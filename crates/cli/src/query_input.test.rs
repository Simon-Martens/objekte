use clap::Parser;

use super::*;

#[test]
fn quoted_inputs_are_complete_json_string_literals() {
    let args = Arguments::try_parse_from(["obj", "\"Gr\\u00fcnde\""]).unwrap();
    let input = QuerySource::open(&args)
        .unwrap()
        .next_input()
        .unwrap()
        .unwrap();
    assert_eq!(input.kind, QueryKind::Fts);
    assert_eq!(input.parse(&args).unwrap().1.as_deref(), Some("Gründe"));
    for text in ["\"hello\" trailing", "\"unclosed", "\"x\\q\""] {
        let input = QueryInput {
            original: text.into(),
            line: None,
            kind: QueryKind::Fts,
        };
        assert!(input.parse(&args).is_err());
    }
}

#[test]
fn direct_fts_does_not_decode_backslash_escapes() {
    let args = Arguments::try_parse_from(["obj", "--fts", "\\n"]).unwrap();
    let input = QuerySource::open(&args)
        .unwrap()
        .next_input()
        .unwrap()
        .unwrap();
    assert_eq!(input.parse(&args).unwrap().1.as_deref(), Some("\\n"));
}

#[test]
fn file_lines_keep_physical_numbers_and_ignore_blank_lines() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("queries.txt");
    std::fs::write(&path, "\u{feff}\r\n \"A\" \r\n\r\ntitle ~ \"B\"\n").unwrap();
    let args = Arguments::try_parse_from(["obj", "--query-file", path.to_str().unwrap()]).unwrap();
    let mut source = QuerySource::open(&args).unwrap();
    let first = source.next_input().unwrap().unwrap();
    assert_eq!(first.line, Some(2));
    assert_eq!(first.original, "\"A\"");
    let second = source.next_input().unwrap().unwrap();
    assert_eq!(second.line, Some(4));
    assert_eq!(second.kind, QueryKind::Structured);
    assert!(source.next_input().unwrap().is_none());
    assert_eq!(source.yielded(), 2);
}

#[test]
fn invalid_utf8_in_query_file_is_fatal_input_error() {
    let folder = tempfile::tempdir().unwrap();
    let path = folder.path().join("queries.txt");
    std::fs::write(&path, b"\xff\n").unwrap();
    let args = Arguments::try_parse_from(["obj", "--query-file", path.to_str().unwrap()]).unwrap();
    let mut source = QuerySource::open(&args).unwrap();
    assert!(source.next_input().unwrap_err().contains("line 1"));
}
