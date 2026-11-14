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
fn desktop_open_has_no_storage_target_and_query_has_no_range() {
    let opening = OpenDraft {
        paths: "/tmp/data".into(),
        ..OpenDraft::default()
    };
    let spec = open_spec(&opening, Path::new("/tmp")).unwrap();
    assert_eq!(spec.args.path, vec![std::path::PathBuf::from("/tmp/data")]);
    let spec = query_spec(&SearchDraft::default()).unwrap();
    assert_eq!(spec.offset, 0);
    assert_eq!(spec.limit, None);
    assert!(!spec.count);
}

#[test]
fn desktop_open_can_load_an_explicit_or_preset_index() {
    let mut draft = OpenDraft {
        paths: "/tmp/data".into(),
        index_source: crate::ui::model::IndexOpenSource::Load,
        check_after_load: true,
        ..OpenDraft::default()
    };
    let spec = open_spec(&draft, Path::new("/tmp")).unwrap();
    assert_eq!(
        spec.args.load_index,
        Some(std::path::PathBuf::from("cache"))
    );
    assert!(spec.args.check_index);
    draft.index_location = crate::ui::model::IndexStorageLocation::Repository;
    let spec = open_spec(&draft, Path::new("/tmp")).unwrap();
    assert_eq!(spec.args.load_index, Some(std::path::PathBuf::from("repo")));
    draft.index_location = crate::ui::model::IndexStorageLocation::Custom;
    draft.custom_index = String::new();
    assert!(open_spec(&draft, Path::new("/tmp")).is_err());
    draft.custom_index = "/mnt/indexes/archive".into();
    let spec = open_spec(&draft, Path::new("/tmp")).unwrap();
    assert_eq!(
        spec.args.load_index,
        Some(std::path::PathBuf::from("/mnt/indexes/archive"))
    );
}

#[test]
fn configured_repository_builds_from_its_source_root() {
    let opening = OpenDraft {
        mode: OpenMode::ConfiguredRepository,
        paths: "/tmp/repository".into(),
        ..OpenDraft::default()
    };
    let spec = open_spec(&opening, Path::new("/tmp")).unwrap();
    assert_eq!(
        spec.args.repo.as_deref(),
        Some(Path::new("/tmp/repository"))
    );
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
