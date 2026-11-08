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
    draft.mode = OpenMode::Folder;
    draft.paths = "/tmp/one".into();
    draft.schema = "schema.json".into();
    draft.recursive = false;
    let single = open_spec(&draft, cwd).unwrap();
    assert_eq!(single.args.schema, Some("schema.json".into()));
    assert!(single.args.no_recurse);
    draft.mode = OpenMode::MultipleFolders;
    draft.paths = "/tmp/one\n/tmp/two".into();
    draft.schema = "schema.json".into();
    assert!(open_spec(&draft, cwd).unwrap().args.schema.is_none());
    draft.mode = OpenMode::ConfiguredRepository;
    draft.paths = "/tmp/repo".into();
    draft.recursive = false;
    let repo = open_spec(&draft, cwd).unwrap();
    assert!(!repo.args.no_recurse);
    assert!(repo.args.schema.is_none());
    assert_eq!(repo.args.repo, Some("/tmp/repo".into()));
}

#[test]
fn snapshot_is_explicit_and_desktop_query_has_no_range() {
    let mut opening = OpenDraft {
        snapshot_directory: "/tmp/index".into(),
        ..OpenDraft::default()
    };
    opening.paths = "/tmp/data".into();
    assert!(
        open_spec(&opening, Path::new("/tmp"))
            .unwrap()
            .args
            .write_index
            .is_none()
    );
    opening.snapshot = SnapshotMode::LoadAndWrite;
    let spec = open_spec(&opening, Path::new("/tmp")).unwrap();
    assert_eq!(spec.args.load_index.as_deref(), Some("/tmp/index"));
    assert_eq!(spec.args.write_index.as_deref(), Some("/tmp/index"));

    let spec = query_spec(&SearchDraft::default()).unwrap();
    assert_eq!(spec.offset, 0);
    assert_eq!(spec.limit, None);
    assert!(!spec.count);
    let unlimited = query_spec(&SearchDraft::default()).unwrap();
    assert_eq!(unlimited.limit, None);
}

#[test]
fn empty_path_never_defaults_to_process_working_directory() {
    assert!(
        open_spec(&OpenDraft::default(), Path::new("/tmp"))
            .unwrap_err()
            .contains("Choose a folder")
    );
}

#[test]
fn quoted_cli_literal_uses_json_string_decoding() {
    let draft = SearchDraft {
        structured: "\"A\\nB\"".into(),
        filter_enabled: true,
        ..SearchDraft::default()
    };
    let spec = query_spec(&draft).unwrap();
    assert_eq!(spec.full_text, "A\nB");
    assert!(spec.filter.is_empty());
    let malformed = SearchDraft {
        structured: "\"unterminated".into(),
        filter_enabled: true,
        ..SearchDraft::default()
    };
    assert!(
        query_spec(&malformed)
            .unwrap_err()
            .contains("invalid FTS JSON string literal")
    );
}

#[test]
fn disabled_filter_keeps_editor_text_out_of_execution() {
    let draft = SearchDraft {
        structured: "year >= 1900 SORT BY year DESC".into(),
        filter_enabled: false,
        ..SearchDraft::default()
    };
    let spec = query_spec(&draft).unwrap();
    assert!(spec.filter.is_empty());
    assert_eq!(draft.structured, "year >= 1900 SORT BY year DESC");
}
