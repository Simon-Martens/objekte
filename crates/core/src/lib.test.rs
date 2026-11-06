#[test]
fn root_fixture_is_available_without_archived_workspace_paths() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/musenalm/.objekte/collections");
    assert!(root.join("agents.schema.json").is_file());
}
