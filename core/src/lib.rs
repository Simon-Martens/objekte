//! Record-oriented search over local repository files.
//!
//! The generic core is developed here. Fixture-specific schema registration and
//! workload execution live in the CLI integration layer.

/// Version of the in-memory search model and future snapshot envelope.
pub const MODEL_VERSION: u32 = 1;

pub mod inventory;
pub mod model;
pub mod workload;

#[cfg(test)]
mod tests {
    #[test]
    fn root_fixture_is_available_without_archived_workspace_paths() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("examples/musenalm/.objekte/collections");
        assert!(root.join("agents.schema.json").is_file());
    }
}
