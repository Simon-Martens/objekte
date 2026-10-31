//! One-shot field inference and optional schema writeback.

use objekte_core::{InferOptions, InferenceReport, Repository};

pub(super) fn run(
    repo: &mut Repository,
    collection: Option<&str>,
    auto_index: bool,
    write_schema: bool,
) -> Result<InferenceReport, Box<dyn std::error::Error>> {
    if write_schema {
        let has_schema = if let Some(name) = collection {
            repo.model().collection(name)?.schema_path.is_some()
        } else {
            repo.model()
                .collections
                .values()
                .any(|collection| collection.schema_path.is_some())
        };
        if !has_schema {
            return Err("writeschema requires an existing schema in the selected scope".into());
        }
    }
    Ok(repo.infer_and_auto_index(
        collection,
        InferOptions {
            auto_index,
            write_schema,
        },
    )?)
}
