use clap::Parser;
use super::Arguments;

#[test]
fn removed_storage_flags_are_unsupported_arguments() {
    for option in [
        "--flush-index",
        "--write-index",
        "--index-storage",
        "--index-directory",
        "--index-residency",
        "--load-index-directory",
        "--flush-index-directory",
    ] {
        assert!(
            Arguments::try_parse_from(["obj", option, "repository", "--fts", "A"]).is_err(),
            "{option}"
        );
    }
}

#[test]
fn input_modes_and_repeated_positionals_are_rejected() {
    assert!(Arguments::try_parse_from(["obj", "--fts", "A", "--query-file", "q.txt"]).is_err());
    assert!(Arguments::try_parse_from(["obj", "a", "b"]).is_err());
    assert_eq!(Arguments::try_parse_from(["obj", "--", "-a"]).unwrap().query.as_deref(), Some("-a"));
}

#[test]
fn storage_flags_allow_no_query_and_require_a_load_for_checking() {
    let args = Arguments::try_parse_from(["obj", "--load-index", "cache"]).unwrap();
    assert_eq!(args.load_index, Some(std::path::PathBuf::from("cache")));
    assert!(args.validate_input().is_ok());
    let args = Arguments::try_parse_from(["obj", "--save-index", "./index"]).unwrap();
    assert!(args.validate_input().is_ok());
    assert!(
        Arguments::try_parse_from(["obj", "--check-index", "--load-index", "repo"])
            .unwrap()
            .validate_input()
            .is_ok()
    );
    assert!(Arguments::try_parse_from(["obj", "--check-index"]).is_err());
}

#[test]
fn bare_command_requires_a_query_or_interactive_mode() {
    assert!(Arguments::try_parse_from(["obj"]).unwrap().validate_input().is_err());
    assert!(
        Arguments::try_parse_from(["obj", "--tui"])
            .unwrap()
            .validate_input()
            .is_ok()
    );
}

#[test]
fn automatic_indexing_requires_no_value_but_not_a_query() {
    let args = Arguments::try_parse_from(["obj", "--auto-index", "code = \"A\""]).unwrap();
    assert!(args.auto_index);
    assert!(args.validate_input().is_ok());
    let args = Arguments::try_parse_from(["obj", "--auto-index"]).unwrap();
    assert!(args.validate_input().is_err());
    assert!(Arguments::try_parse_from(["obj", "--auto-index=true", "code = \"A\""]).is_err());
}

#[test]
fn diagnostic_aliases_are_equivalent() {
    assert!(
        Arguments::try_parse_from(["obj", "--diagnostic", "--fts", "x"])
            .unwrap()
            .diagnostic
    );
    assert!(
        Arguments::try_parse_from(["obj", "--diagnostics", "--fts", "x"])
            .unwrap()
            .diagnostic
    );
}

#[test]
fn repository_mode_uses_its_root_and_rejects_single_folder_options() {
    let cwd = tempfile::tempdir().unwrap();
    let args = Arguments::try_parse_from(["obj", "--repo", ".", "--tui"]).unwrap();
    assert_eq!(args.resolve_paths(cwd.path()).unwrap().root, cwd.path());
    assert!(Arguments::try_parse_from(["obj", "--repo", ".", "--path", ".", "--tui"]).is_err());
    assert!(
        Arguments::try_parse_from(["obj", "--repo", ".", "--schema", "one.json", "--tui"]).is_err()
    );
    assert!(Arguments::try_parse_from(["obj", "--repo", ".", "--no-recurse", "--tui"]).is_err());
}

#[test]
fn directly_constructed_open_options_keep_cli_conflicts() {
    let cwd = tempfile::tempdir().unwrap();
    let mut args = Arguments::try_parse_from(["obj", "--tui"]).unwrap();
    args.repo = Some(cwd.path().to_path_buf());
    args.path.push(cwd.path().to_path_buf());
    assert!(
        args.resolve_paths(cwd.path())
            .unwrap_err()
            .contains("--repo")
    );
    args.path.clear();
    args.schema = Some(cwd.path().join("schema.json"));
    assert!(
        args.validate_open_options()
            .unwrap_err()
            .contains("--schema")
    );
    args.schema = None;
    args.no_recurse = true;
    assert!(
        args.validate_open_options()
            .unwrap_err()
            .contains("--no-recurse")
    );
}

#[test]
fn repeated_paths_resolve_under_one_root_and_select_multiple_collections() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("data/authors")).unwrap();
    std::fs::create_dir_all(cwd.path().join("data/books")).unwrap();
    let args = Arguments::try_parse_from([
        "obj",
        "--path",
        "data/authors",
        "--path",
        "data/books",
        "--fts",
        "Ada",
    ])
    .unwrap();
    assert!(args.multiple_collections());
    let paths = args.resolve_paths(cwd.path()).unwrap();
    assert_eq!(paths.root, cwd.path().join("data"));
    assert_eq!(
        paths.collection_paths,
        [
            cwd.path().join("data/authors"),
            cwd.path().join("data/books")
        ]
    );
    assert!(
        !Arguments::try_parse_from(["obj", "--path", "data/authors", "--fts", "Ada"])
            .unwrap()
            .multiple_collections()
    );
    let args = Arguments::try_parse_from([
        "obj",
        "--path",
        "data/authors",
        "--path",
        "data/books",
        "--schema",
        "schema.json",
        "--fts",
        "Ada",
    ])
    .unwrap();
    assert!(args.validate_input().is_err());
}

#[test]
fn repeated_paths_reject_overlap() {
    let cwd = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(cwd.path().join("data/authors")).unwrap();
    let args = Arguments::try_parse_from([
        "obj",
        "--path",
        "data",
        "--path",
        "data/authors",
        "--fts",
        "Ada",
    ])
    .unwrap();
    assert!(
        args.resolve_paths(cwd.path())
            .unwrap_err()
            .contains("non-overlapping")
    );
}
