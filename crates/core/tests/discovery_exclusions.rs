use std::path::{Path, PathBuf};

use objekte_search_core::discovery::{Discovery, SymlinkPolicy, discover, selects_file};
use objekte_search_core::source::{SourceFormat, registry::SourceRegistry};

/// Builds a private fixture tree for traversal-policy assertions.
struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "objekte-discovery-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }

    fn file(&self, relative: &str) {
        let path = self.0.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, b"{}\n").unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn excluded_directories_are_pruned_instead_of_only_filtering_files() {
    let fixture = Fixture::new();
    fixture.file("visible.json");
    fixture.file("nested/visible.json");
    fixture.file("nested/.git/hidden.json");
    fixture.file(".git/hidden.json");
    fixture.file(".objekte/snapshot.json");
    fixture.file("nested/cache/snapshot.json");
    fixture.file("nested/cache[1]/snapshot.json");
    fixture.file("nested/ignore.jsonl");
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let rules = Discovery {
        recursive: true,
        hidden: true,
        symlinks: SymlinkPolicy::Skip,
        exclude: vec![".git".into(), "**/.git".into(), ".objekte".into()],
        exclude_directories: vec![
            PathBuf::from("nested/cache"),
            PathBuf::from("nested/cache[1]"),
        ],
        ..Discovery::default()
    };

    let selected = discover(&fixture.0, Path::new(""), &rules, &source).unwrap();
    let relative: Vec<_> = selected
        .iter()
        .map(|path| path.strip_prefix(&fixture.0).unwrap().to_owned())
        .collect();
    assert_eq!(
        relative,
        vec![
            PathBuf::from("nested/visible.json"),
            PathBuf::from("visible.json")
        ]
    );
}

#[cfg(unix)]
#[test]
fn exact_directory_exclusion_preserves_non_utf8_path_identity() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;

    let fixture = Fixture::new();
    let name = PathBuf::from(OsString::from_vec(vec![b'c', 0xff]));
    std::fs::create_dir(fixture.0.join(&name)).unwrap();
    std::fs::write(fixture.0.join(&name).join("inside.json"), b"{}\n").unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let rules = Discovery {
        recursive: true,
        hidden: true,
        exclude_directories: vec![name],
        ..Discovery::default()
    };
    assert!(
        discover(&fixture.0, Path::new(""), &rules, &source)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn generated_index_namespaces_are_pruned_without_reading_their_contents() {
    let fixture = Fixture::new();
    fixture.file("visible.json");
    fixture.file("nested/visible.json");
    fixture.file(".objekte/indexes/poison.json");
    fixture.file("nested/objekte-indexes/poison.json");
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let rules = Discovery {
        recursive: true,
        hidden: true,
        symlinks: SymlinkPolicy::Skip,
        ..Discovery::default()
    };
    let selected = discover(&fixture.0, Path::new(""), &rules, &source).unwrap();
    let relative: Vec<_> = selected
        .iter()
        .map(|path| path.strip_prefix(&fixture.0).unwrap().to_owned())
        .collect();
    assert_eq!(
        relative,
        vec![
            PathBuf::from("nested/visible.json"),
            PathBuf::from("visible.json")
        ]
    );
    let non_recursive = Discovery {
        recursive: false,
        ..rules
    };
    let shallow = discover(&fixture.0, Path::new(""), &non_recursive, &source).unwrap();
    assert_eq!(
        shallow
            .iter()
            .map(|path| path.strip_prefix(&fixture.0).unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec![PathBuf::from("visible.json")]
    );
}

#[test]
fn generated_namespaces_are_rejected_by_direct_selection_and_aliases() {
    let fixture = Fixture::new();
    fixture.file("data/a.json");
    fixture.file("data/nested/objekte-indexes/poison.json");
    fixture.file(".objekte/.indexes/poison.json");
    fixture.file(".objekte/indexes/poison.json");
    fixture.file("cache/objekte-indexes/poison.json");
    fixture.file("cache/source.json");
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let rules = Discovery {
        recursive: true,
        hidden: true,
        symlinks: SymlinkPolicy::Skip,
        ..Discovery::default()
    };
    let selected = discover(&fixture.0, Path::new(""), &rules, &source).unwrap();
    let relative: Vec<_> = selected
        .iter()
        .map(|path| path.strip_prefix(&fixture.0).unwrap().to_owned())
        .collect();
    assert_eq!(
        relative,
        vec![
            PathBuf::from("cache/source.json"),
            PathBuf::from("data/a.json")
        ]
    );
    for poison in [
        "data/nested/objekte-indexes/poison.json",
        ".objekte/.indexes/poison.json",
        ".objekte/indexes/poison.json",
        "cache/objekte-indexes/poison.json",
    ] {
        assert!(
            !selects_file(
                &fixture.0,
                Path::new(""),
                &rules,
                &source,
                Path::new(poison)
            )
            .unwrap(),
            "{poison}"
        );
    }
    assert!(
        !selects_file(
            &fixture.0,
            Path::new("data"),
            &rules,
            &source,
            Path::new("data/nested/objekte-indexes/poison.json")
        )
        .unwrap()
    );
    for reserved in [
        ".objekte",
        ".objekte/indexes",
        ".objekte/.indexes",
        "data/nested/objekte-indexes",
        "cache/objekte-indexes",
    ] {
        assert!(
            discover(&fixture.0, Path::new(reserved), &rules, &source).is_err(),
            "{reserved}"
        );
        assert!(
            selects_file(
                &fixture.0,
                Path::new(reserved),
                &rules,
                &source,
                Path::new("unused.json")
            )
            .is_err(),
            "{reserved}"
        );
    }
}

#[cfg(unix)]
#[test]
fn followed_aliases_into_generated_namespaces_are_not_sources() {
    let fixture = Fixture::new();
    fixture.file("data/a.json");
    fixture.file(".objekte/indexes/poison.json");
    std::os::unix::fs::symlink(fixture.0.join(".objekte/indexes"), fixture.0.join("alias"))
        .unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let rules = Discovery {
        recursive: true,
        hidden: true,
        symlinks: SymlinkPolicy::FollowWithinRoot,
        ..Discovery::default()
    };
    let selected = discover(&fixture.0, Path::new(""), &rules, &source).unwrap();
    let relative: Vec<_> = selected
        .iter()
        .map(|path| path.strip_prefix(&fixture.0).unwrap().to_owned())
        .collect();
    assert_eq!(relative, vec![PathBuf::from("data/a.json")]);
    assert!(
        !selects_file(
            &fixture.0,
            Path::new(""),
            &rules,
            &source,
            Path::new("alias/poison.json")
        )
        .unwrap()
    );
}

#[cfg(unix)]
#[test]
fn followed_discovery_ignores_reserved_dangling_and_looping_links() {
    let fixture = Fixture::new();
    fixture.file("data/a.json");
    std::fs::create_dir_all(fixture.0.join("data/nested")).unwrap();
    std::os::unix::fs::symlink(
        fixture.0.join("missing-target"),
        fixture.0.join("data/objekte-indexes"),
    )
    .unwrap();
    std::os::unix::fs::symlink(
        "objekte-indexes",
        fixture.0.join("data/nested/objekte-indexes"),
    )
    .unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let rules = Discovery {
        recursive: true,
        hidden: true,
        symlinks: SymlinkPolicy::FollowWithinRoot,
        ..Discovery::default()
    };
    let selected = discover(&fixture.0, Path::new("data"), &rules, &source).unwrap();
    assert_eq!(
        selected
            .iter()
            .map(|path| path.strip_prefix(&fixture.0).unwrap().to_owned())
            .collect::<Vec<_>>(),
        vec![PathBuf::from("data/a.json")]
    );
}

#[cfg(unix)]
#[test]
fn canonical_alias_source_roots_are_rejected_for_direct_selection() {
    let fixture = Fixture::new();
    fixture.file("data/a.json");
    fixture.file(".objekte/indexes/poison.json");
    std::os::unix::fs::symlink(fixture.0.join(".objekte/indexes"), fixture.0.join("alias"))
        .unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    for symlinks in [SymlinkPolicy::Skip, SymlinkPolicy::FollowWithinRoot] {
        let rules = Discovery {
            recursive: true,
            hidden: true,
            symlinks,
            ..Discovery::default()
        };
        for relative in ["alias/poison.json", "alias/missing.json"] {
            let error = selects_file(
                &fixture.0,
                Path::new("alias"),
                &rules,
                &source,
                Path::new(relative),
            )
            .unwrap_err();
            assert!(
                error.contains("resolves inside a generated index namespace"),
                "{symlinks:?} {relative}: {error}"
            );
        }
    }
}
