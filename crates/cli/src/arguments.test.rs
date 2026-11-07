use clap::Parser;

use super::Arguments;

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
