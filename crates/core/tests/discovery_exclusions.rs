use std::path::{Path, PathBuf};

use objekte_search_core::discovery::{Discovery, SymlinkPolicy, discover};
use objekte_search_core::source::{SourceFormat, SourceRegistry};

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
