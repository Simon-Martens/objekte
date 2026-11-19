use std::collections::HashSet;
use std::path::Path;

use super::{
    Discovery, PreparedScanSelection, SymlinkPolicy, discover, discover_bounded, discover_stream,
    selects_file,
};
use crate::source::{SourceFormat, registry::SourceRegistry};

#[test]
fn direct_selection_matches_full_discovery_without_enumerating_siblings() {
    let root = std::env::temp_dir().join(format!(
        "objekte-direct-discovery-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    for directory in ["data", "data/nested", "data/blocked", "other"] {
        std::fs::create_dir_all(root.join(directory)).unwrap();
    }
    for file in [
        "data/a.json",
        "data/nested/b.json",
        "data/blocked/c.json",
        "data/.hidden.json",
        "data/literal[1].json",
        "data/wrong.txt",
        "other/outside.json",
    ] {
        std::fs::write(root.join(file), b"{}").unwrap();
    }
    let root = root.canonicalize().unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    let rules = Discovery {
        recursive: true,
        exclude_files: vec!["literal[1].json".into()],
        exclude_directories: vec!["blocked".into()],
        ..Default::default()
    };
    let full = discover(&root, Path::new("data"), &rules, &source)
        .unwrap()
        .into_iter()
        .map(|path| path.strip_prefix(&root).unwrap().to_owned())
        .collect::<HashSet<_>>();
    assert_eq!(full.len(), 2);
    assert!(
        discover_bounded(&root, Path::new("data"), &rules, &source, 1)
            .unwrap_err()
            .contains("budget exceeded")
    );
    for file in [
        "data/a.json",
        "data/nested/b.json",
        "data/blocked/c.json",
        "data/.hidden.json",
        "data/literal[1].json",
        "data/wrong.txt",
        "other/outside.json",
        "data/missing.json",
    ] {
        assert_eq!(
            selects_file(&root, Path::new("data"), &rules, &source, Path::new(file)).unwrap(),
            full.contains(Path::new(file)),
            "{file}"
        );
    }
    let shallow = Discovery {
        recursive: false,
        ..Default::default()
    };
    assert!(
        !selects_file(
            &root,
            Path::new("data"),
            &shallow,
            &source,
            Path::new("data/nested/b.json")
        )
        .unwrap()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn streaming_discovery_preserves_selection_and_symlink_alias_policy() {
    let root = std::env::temp_dir().join(format!(
        "objekte-stream-discovery-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("data/nested")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{}").unwrap();
    std::fs::write(root.join("data/nested/b.json"), b"{}").unwrap();
    std::os::unix::fs::symlink("a.json", root.join("data/alias.json")).unwrap();
    let root = root.canonicalize().unwrap();
    let source = SourceRegistry::builtin()
        .resolve(&SourceFormat::Json)
        .unwrap();
    for policy in [SymlinkPolicy::Skip, SymlinkPolicy::FollowWithinRoot] {
        let rules = Discovery {
            recursive: true,
            symlinks: policy,
            ..Discovery::default()
        };
        let expected = discover(&root, Path::new("data"), &rules, &source).unwrap();
        let mut observed = Vec::new();
        discover_stream(
            &root,
            Path::new("data"),
            &rules,
            &source,
            || false,
            |path| {
                observed.push(path);
                Ok(())
            },
        )
        .unwrap();
        observed.sort();
        assert_eq!(observed, expected);
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn prepared_scan_selection_matches_generated_namespace_eligibility() {
    let root = std::env::temp_dir().join(format!(
        "objekte-prepared-generated-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data/nested/objekte-indexes")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{}").unwrap();
    std::fs::write(root.join("data/nested/objekte-indexes/poison.json"), b"{}").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(
        root.join("data/nested/objekte-indexes"),
        root.join("data/alias"),
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
    let selection = PreparedScanSelection::new(&root, Path::new("data"), &rules, &source).unwrap();
    assert!(selection.selects_lexical(Path::new("data/a.json")));
    assert!(!selection.selects_lexical(Path::new("data/nested/objekte-indexes/poison.json")));
    assert!(
        !selection
            .selects_file(&root, Path::new("data/nested/objekte-indexes/poison.json"))
            .unwrap()
    );
    #[cfg(unix)]
    assert!(
        !selection
            .selects_file(&root, Path::new("data/alias/poison.json"))
            .unwrap()
    );
    assert!(
        PreparedScanSelection::new(
            &root,
            Path::new("data/nested/objekte-indexes"),
            &rules,
            &source
        )
        .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn stream_discovery_ignores_reserved_dangling_and_looping_links() {
    let root = std::env::temp_dir().join(format!(
        "objekte-stream-reserved-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(root.join("data/nested")).unwrap();
    std::fs::write(root.join("data/a.json"), b"{}").unwrap();
    std::os::unix::fs::symlink(root.join("missing"), root.join("data/objekte-indexes")).unwrap();
    std::os::unix::fs::symlink("objekte-indexes", root.join("data/nested/objekte-indexes"))
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
    let mut observed = Vec::new();
    discover_stream(
        &root,
        Path::new("data"),
        &rules,
        &source,
        || false,
        |path| {
            observed.push(path);
            Ok(())
        },
    )
    .unwrap();
    assert_eq!(observed, vec![root.join("data/a.json")]);
    std::fs::remove_dir_all(root).unwrap();
}
