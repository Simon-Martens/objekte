use std::path::Path;

use super::*;

#[test]
fn opening_modes_preserve_cli_validation() {
    let cwd = Path::new("/tmp");
    let mut draft = OpenDraft {
        mode: OpenMode::MultipleFolders,
        paths: "/tmp/one\n/tmp/two".into(),
        ..OpenDraft::default()
    };
    let spec = open_spec(&draft, cwd).unwrap();
    assert_eq!(spec.args.path.len(), 2);
    draft.schema = "schema.json".into();
    assert!(open_spec(&draft, cwd).unwrap_err().contains("--schema"));
    draft.mode = OpenMode::ConfiguredRepository;
    draft.paths = "/tmp/repo".into();
    draft.schema.clear();
    draft.recursive = false;
    assert!(open_spec(&draft, cwd).unwrap_err().contains("--no-recurse"));
}

#[test]
fn snapshot_is_explicit_and_range_is_not_viewport_size() {
    let mut opening = OpenDraft {
        snapshot_directory: "/tmp/index".into(),
        ..OpenDraft::default()
    };
    assert!(open_spec(&opening, Path::new("/tmp")).is_err());
    opening.snapshot = SnapshotMode::LoadAndWrite;
    let spec = open_spec(&opening, Path::new("/tmp")).unwrap();
    assert_eq!(spec.args.load_index.as_deref(), Some("/tmp/index"));
    assert_eq!(spec.args.write_index.as_deref(), Some("/tmp/index"));

    let query = SearchDraft {
        offset: "10".into(),
        limit: "0".into(),
        count: true,
        ..SearchDraft::default()
    };
    let spec = query_spec(&query).unwrap();
    assert_eq!(spec.offset, 10);
    assert_eq!(spec.limit, Some(0));
    assert!(spec.count);
    let unlimited = query_spec(&SearchDraft::default()).unwrap();
    assert_eq!(unlimited.limit, None);
}

#[test]
fn quoted_cli_literal_uses_json_string_decoding() {
    let draft = SearchDraft {
        structured: "\"A\\nB\"".into(),
        ..SearchDraft::default()
    };
    let spec = query_spec(&draft).unwrap();
    assert_eq!(spec.full_text, "A\nB");
    assert!(spec.filter.is_empty());
    let malformed = SearchDraft {
        structured: "\"unterminated".into(),
        ..SearchDraft::default()
    };
    assert!(
        query_spec(&malformed)
            .unwrap_err()
            .contains("invalid FTS JSON string literal")
    );
}
