//! Record-oriented search over local repository files.
//!
//! The generic core is developed here. Fixture-specific schema registration and
//! workload execution live in the CLI integration layer.

/// Version of the in-memory search model and future snapshot envelope.
pub const MODEL_VERSION: u32 = 3;

pub mod bigram;
pub mod executor;
pub mod inventory;
pub mod model;
pub mod path_repr;
pub mod posting;
mod prepared;
pub mod query;
pub mod reference;
pub mod repository;
pub mod run;
pub mod sorting;
pub mod source;
pub mod storage;
mod verified;
pub mod workload;

#[cfg(test)]
mod tests {
    #[test]
    fn root_fixture_is_available_without_archived_workspace_paths() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../examples/musenalm/.objekte/collections");
        assert!(root.join("agents.schema.json").is_file());
    }
}
