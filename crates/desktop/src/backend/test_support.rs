//! Disposable source fixtures for desktop backend tests.

use std::path::Path;

use tempfile::TempDir;

use super::{OpenSpec, OpenedSession, open};
use objekte_cli::arguments::Arguments;
use objekte_search_core::streaming::CancellationToken;

/// Owned documents and their opened repository stay alive for a whole test.
pub(crate) struct Fixture {
    pub _root: TempDir,
    pub session: OpenedSession,
}

pub(super) fn write_records(root: &Path) {
    std::fs::write(root.join("a.json"), r#"{"title":"Alpha","year":2024}"#).unwrap();
    std::fs::write(root.join("b.json"), r#"{"title":"Beta","year":2023}"#).unwrap();
    std::fs::write(root.join("c.json"), r#"{"title":"alpha two","year":2022}"#).unwrap();
}

pub(crate) fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    write_records(root.path());
    let args = Arguments {
        path: vec![root.path().to_path_buf()],
        ..Default::default()
    };
    let session = open(
        &OpenSpec {
            storage_policy: None,
            args,
            cwd: root.path().to_path_buf(),
            configured_cache_policy: None,
        },
        &CancellationToken::new(),
    )
    .unwrap();
    Fixture {
        _root: root,
        session,
    }
}
