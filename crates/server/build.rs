use std::{env, fs, path::Path};

fn collect(root: &Path, directory: &Path, sources: &mut Vec<String>) {
    let mut entries: Vec<_> = fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect(root, &path, sources);
        } else if path.extension().is_some_and(|ext| ext == "html") {
            let name = path
                .strip_prefix(root)
                .unwrap()
                .to_str()
                .unwrap()
                .replace('\\', "/");
            sources.push(format!(
                "({name:?}, include_str!({:?}))",
                path.to_str().unwrap()
            ));
        }
    }
}

fn main() {
    println!("cargo:rerun-if-changed=templates");
    let root = Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join("templates");
    let mut sources = Vec::new();
    collect(&root, &root, &mut sources);
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("templates.rs"),
        format!(
            "const BUNDLED: &[(&str, &str)] = &[{}];",
            sources.join(",\n")
        ),
    )
    .unwrap();
}
