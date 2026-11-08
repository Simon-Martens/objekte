use std::collections::HashSet;
use std::path::Path;

use super::{Discovery, discover, discover_bounded, selects_file};
use crate::source::{SourceFormat, SourceRegistry};

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
