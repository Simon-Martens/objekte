use clap::Parser;
use objekte_search_core::index_storage::{
    IndexResidency, IndexStorageLocation, IndexStoragePolicy,
};

use super::{Arguments, IndexStorageChoice};

#[test]
fn removed_index_flags_are_recognized_without_consuming_query() {
    let args = Arguments::try_parse_from(["obj", "--write-index", "title = \"A\""]).unwrap();
    assert_eq!(args.write_index.as_deref(), Some(""));
    assert_eq!(args.query.as_deref(), Some("title = \"A\""));
    let args = Arguments::try_parse_from(["obj", "--write-index=cache", "--fts", "A"]).unwrap();
    assert_eq!(args.write_index.as_deref(), Some("cache"));
    assert_eq!(args.fts.as_deref(), Some("A"));
    assert!(
        args.validate_input()
            .unwrap_err()
            .contains("--index-storage")
    );
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
fn removed_flags_are_rejected_before_path_resolution() {
    let cwd = tempfile::tempdir().unwrap();
    let args =
        Arguments::try_parse_from(["obj", "--load-index=one", "--write-index=two", "--fts", "A"])
            .unwrap();
    assert!(
        args.resolve_paths(cwd.path())
            .unwrap_err()
            .contains("were removed")
    );
    let args = Arguments::try_parse_from([
        "obj",
        "--load-index=one/../same",
        "--write-index=same",
        "--fts",
        "A",
    ])
    .unwrap();
    assert!(
        args.resolve_paths(cwd.path())
            .unwrap_err()
            .contains("were removed")
    );
}

#[test]
fn bare_command_does_not_open_repository() {
    let args = Arguments::try_parse_from(["obj"]).unwrap();
    assert!(args.validate_input().is_err());
    let args = Arguments::try_parse_from(["obj", "--write-index"]).unwrap();
    assert!(
        args.validate_input()
            .unwrap_err()
            .contains("--index-storage")
    );
}

#[test]
fn memory_is_default_for_plain_and_configured_repositories() {
    let default = Arguments::try_parse_from(["obj", "--fts", "A"]).unwrap();
    assert_eq!(default.index_storage, IndexStorageChoice::Memory);
    assert_eq!(
        default.index_storage_policy().unwrap(),
        IndexStoragePolicy::default()
    );
    let configured = Arguments::try_parse_from(["obj", "--repo", ".", "--fts", "A"]).unwrap();
    assert_eq!(
        configured.index_storage_policy().unwrap(),
        IndexStoragePolicy::default()
    );
}

#[test]
fn storage_flags_map_to_the_core_policy_and_reject_contradictions() {
    let cwd = tempfile::tempdir().unwrap();
    let temporary =
        Arguments::try_parse_from(["obj", "--index-storage", "temporary", "--fts", "A"]).unwrap();
    assert_eq!(
        temporary.index_storage_policy().unwrap().location,
        IndexStorageLocation::SystemTemporary
    );
    assert_eq!(
        temporary.index_storage_policy().unwrap().residency,
        IndexResidency::OnDemand
    );
    let repository = Arguments::try_parse_from([
        "obj",
        "--index-storage",
        "repository",
        "--index-residency",
        "full",
        "--fts",
        "A",
    ])
    .unwrap();
    assert_eq!(
        repository.index_storage_policy().unwrap().location,
        IndexStorageLocation::RepositoryDirectory
    );
    assert_eq!(
        repository.index_storage_policy().unwrap().residency,
        IndexResidency::FullyResident
    );
    let custom = Arguments::try_parse_from([
        "obj",
        "--index-storage",
        "custom",
        "--index-directory",
        ".",
        "--fts",
        "A",
    ])
    .unwrap();
    assert_eq!(
        custom.resolve_paths(cwd.path()).unwrap().index_directory,
        Some(cwd.path().to_owned())
    );
    assert_eq!(
        custom.index_storage_policy().unwrap().location,
        IndexStorageLocation::CustomDirectory(".".into())
    );
    assert_eq!(
        custom
            .resolved_index_storage_policy(&custom.resolve_paths(cwd.path()).unwrap())
            .unwrap()
            .location,
        IndexStorageLocation::CustomDirectory(cwd.path().to_owned())
    );
    assert!(
        Arguments::try_parse_from(["obj", "--index-storage", "custom", "--fts", "A"])
            .unwrap()
            .validate_input()
            .unwrap_err()
            .contains("requires --index-directory")
    );
    assert!(
        Arguments::try_parse_from([
            "obj",
            "--index-storage",
            "memory",
            "--index-directory",
            ".",
            "--fts",
            "A"
        ])
        .unwrap()
        .validate_input()
        .unwrap_err()
        .contains("requires --index-storage custom")
    );
    assert!(
        Arguments::try_parse_from([
            "obj",
            "--index-storage",
            "memory",
            "--index-residency",
            "on-demand",
            "--fts",
            "A"
        ])
        .unwrap()
        .validate_input()
        .is_err()
    );
    assert!(
        Arguments::try_parse_from([
            "obj",
            "--index-storage",
            "memory",
            "--index-residency",
            "full",
            "--fts",
            "A"
        ])
        .unwrap()
        .validate_input()
        .is_ok()
    );
    assert!(
        Arguments::try_parse_from(["obj", "--index-storage", "temporary"])
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
