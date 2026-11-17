use std::process::Command;

fn git_output(repository_root: &str, arguments: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(repository_root)
        .args(arguments)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

fn main() {
    let repository_root = format!("{}/../..", env!("CARGO_MANIFEST_DIR"));
    println!("cargo:rerun-if-changed=../../.git/HEAD");
    println!("cargo:rerun-if-changed=../../.git/index");
    if let Some(revision) = git_output(&repository_root, &["rev-parse", "HEAD"]) {
        println!("cargo:rustc-env=OBJEKTE_BUILD_REVISION={revision}");
    }
    if let Some(dirty) = git_output(&repository_root, &["status", "--porcelain"]) {
        println!(
            "cargo:rustc-env=OBJEKTE_BUILD_DIRTY={}",
            if dirty.is_empty() { "false" } else { "true" }
        );
    }
}
