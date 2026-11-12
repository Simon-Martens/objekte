use clap::Parser;

use super::{Arguments, IndexWait};

#[test]
fn optional_index_paths_require_equals_and_do_not_eat_query() {
    let args = Arguments::try_parse_from(["obj", "--write-index", "title = \"A\""]).unwrap();
    assert_eq!(args.write_index.as_deref(), Some(""));
    assert_eq!(args.query.as_deref(), Some("title = \"A\""));
    let args = Arguments::try_parse_from(["obj", "--write-index=cache", "--fts", "A"]).unwrap();
    assert_eq!(args.write_index.as_deref(), Some("cache"));
    assert_eq!(args.fts.as_deref(), Some("A"));
}

#[test]
fn input_modes_and_repeated_positionals_are_rejected() {
    assert!(Arguments::try_parse_from(["obj", "--fts", "A", "--query-file", "q.txt"]).is_err());
    assert!(Arguments::try_parse_from(["obj", "a", "b"]).is_err());
    assert_eq!(
        Arguments::try_parse_from(["obj", "--", "-a"])
            .unwrap()
            .query
            .as_deref(),
        Some("-a")
    );
}

#[test]
fn index_directory_mismatch_is_an_argument_error() {
    let cwd = tempfile::tempdir().unwrap();
    let args =
        Arguments::try_parse_from(["obj", "--load-index=one", "--write-index=two", "--fts", "A"])
            .unwrap();
    assert!(args.resolve_paths(cwd.path()).is_err());
    let args = Arguments::try_parse_from([
        "obj",
        "--load-index=one/../same",
        "--write-index=same",
        "--fts",
        "A",
    ])
    .unwrap();
    assert_eq!(
        args.resolve_paths(cwd.path()).unwrap().index_directory,
        Some(cwd.path().join("same"))
    );
}

#[test]
fn bare_command_does_not_open_repository() {
    let args = Arguments::try_parse_from(["obj"]).unwrap();
    assert!(args.validate_input().is_err());
    let args = Arguments::try_parse_from(["obj", "--write-index"]).unwrap();
    assert!(args.validate_input().is_ok());
}

#[test]
fn automatic_indexing_accepts_an_index_only_command() {
    let args = Arguments::try_parse_from(["obj", "--auto-index", "code = \"A\""]).unwrap();
    assert!(args.auto_index);
    assert!(args.validate_input().is_ok());
    let args = Arguments::try_parse_from(["obj", "--auto-index"]).unwrap();
    assert!(args.validate_input().is_ok());
    assert_eq!(args.effective_index_wait(), IndexWait::Complete);
    assert!(Arguments::try_parse_from(["obj", "--auto-index=true", "code = \"A\""]).is_err());
}

#[test]
fn index_wait_defaults_follow_command_intent() {
    let query = Arguments::try_parse_from(["obj", "--fts", "x"]).unwrap();
    assert_eq!(query.effective_index_wait(), IndexWait::Search);
    let tui = Arguments::try_parse_from(["obj", "--tui"]).unwrap();
    assert_eq!(tui.effective_index_wait(), IndexWait::Search);
    let save = Arguments::try_parse_from(["obj", "--write-index"]).unwrap();
    assert_eq!(save.effective_index_wait(), IndexWait::Complete);
    let override_wait = Arguments::try_parse_from([
        "obj", "--index-wait=complete", "--fts", "x",
    ])
    .unwrap();
    assert_eq!(override_wait.effective_index_wait(), IndexWait::Complete);
    assert!(Arguments::try_parse_from(["obj", "--index-wait=other", "--fts", "x"]).is_err());
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
