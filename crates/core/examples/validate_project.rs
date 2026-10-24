//! Validate every record using the same Rust schemas and references as editing.
use structured_editor_core::Editor;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: validate_project PATH")?;
    let editor = Editor::open(path)?;
    let mut failures = 0;
    for (name, collection) in &editor.config().collections {
        let mut count = 0;
        for file in std::fs::read_dir(editor.config().root.join(&collection.path))? {
            let path = file?.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                continue;
            }
            let value = serde_json::from_slice(&std::fs::read(&path)?)?;
            let issues = editor.validate(name, &value)?;
            if !issues.is_empty() {
                if failures < 10 {
                    eprintln!("{}: {:?}", path.display(), issues);
                }
                failures += 1;
            }
            count += 1;
        }
        println!("{name}: {count} records validated");
    }
    if failures > 0 {
        return Err(format!("{failures} invalid records").into());
    }
    Ok(())
}
