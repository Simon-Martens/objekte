//! Removed snapshot CLI spellings are ordinary unsupported arguments.

use std::process::Command;

#[test]
fn old_snapshot_spellings_are_unsupported() {
    let root = tempfile::tempdir().unwrap();
    for flag in [
        "--load-index=old-cache",
        "--flush-index=repository",
        "--write-index=old-cache",
        "--index-storage=repository",
        "--index-directory=old-cache",
        "--index-residency=full",
    ] {
        let result = Command::new(env!("CARGO_BIN_EXE_obj"))
            .current_dir(root.path())
            .args([flag, "--fts", "needle"])
            .output()
            .unwrap();
        assert_eq!(result.status.code(), Some(2), "{flag}");
        assert!(result.stdout.is_empty(), "{flag}");
        let error = String::from_utf8_lossy(&result.stderr);
        assert!(error.contains("unexpected argument"), "{flag}: {error}");
        assert!(!root.path().join("old-cache").exists());
    }
}
