//! Narrow compatibility check for removed snapshot CLI spellings.

use std::process::Command;

#[test]
fn old_snapshot_spellings_explain_the_explicit_operations() {
    let root = tempfile::tempdir().unwrap();
    for flag in ["--load-index=old-cache", "--write-index=old-cache"] {
        let result = Command::new(env!("CARGO_BIN_EXE_obj"))
            .current_dir(root.path())
            .args([flag, "--fts", "needle"])
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2));
        assert!(result.stdout.is_empty());
        let error = String::from_utf8_lossy(&result.stderr);
        if flag.starts_with("--load-index") {
            assert!(error.contains("temporary, repository, custom"), "{error}");
        } else {
            assert!(error.contains("--flush-index"), "{error}");
        }
        assert!(!root.path().join("old-cache").exists());
    }
}
