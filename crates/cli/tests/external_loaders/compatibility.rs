use objekte_cli::loader::installed_sources;
use objekte_search_core::files::FileCatalog;
use objekte_search_core::index::bigram::BigramAlphabet;
use objekte_search_core::inference::OccurrenceCountStatus;
use objekte_search_core::lifecycle::Index;
use objekte_search_core::measurement::WorkCounters;
use objekte_search_core::query::options::ReturnMode;
use objekte_search_core::repository::configuration::identity::configuration_hash;
use objekte_search_core::repository::configuration::{
    CollectionDefinition, IdentityPolicy, RepositoryOptions,
};
use objekte_search_core::source::SourceFormat;

#[test]
fn live_configuration_hash_matches_the_pre_restore_fixture() {
    // Expected bytes were produced at the pre-restoration execution base
    // with the same definitions/options. They prove the live identity did
    // not change while disk format versions were removed.
    let sources = installed_sources();
    let folder = vec![objekte_loader_json::decoded_scalar_folder("files", "")];
    let configured = vec![
        objekte_loader_json::folder("authors", "data/authors"),
        CollectionDefinition::folder("books", "data/books", objekte_loader_jsonl::JSON_LINES),
    ];
    let cases = [
        (
            &folder,
            RepositoryOptions::default(),
            "3d6c9a95513505be8fa182c40bd88c655dca362932233b1aee53d697586590ec",
        ),
        (
            &folder,
            RepositoryOptions {
                bigram_alphabet: BigramAlphabet::Unicode,
                ..RepositoryOptions::default()
            },
            "8fed0f48d81a0838e8c264b9c5c98157fc219ea5c480b83d34257723262e2093",
        ),
        (
            &configured,
            RepositoryOptions::default(),
            "9451bad09cf9cca9287b5452ef98be53cdab8e26579d4f0334cf3531078a0f04",
        ),
        (
            &configured,
            RepositoryOptions {
                bigram_alphabet: BigramAlphabet::Unicode,
                ..RepositoryOptions::default()
            },
            "bb15855855053c61c268094a65258f7568ae57283e282ee5154db4fbb892d4fb",
        ),
    ];
    for (definitions, options, expected) in cases {
        let hash = configuration_hash(definitions, options, &sources).unwrap();
        let actual = hash
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        assert_eq!(actual, expected);
    }
}

#[test]
fn configuration_hashes_preserve_legacy_and_generic_selectors() {
    let sources = installed_sources();
    let opts = RepositoryOptions::default();
    let cases = [
        (
            objekte_loader_json::JSON_DOCUMENTS,
            "89c6e93f51b1209faebe7fe5986cdf81854b72d0e7e16ae27e07d411dbb6f5cb",
            "6dd8c8c302360ab03d2f6f80351577d319ab6e54f5e748b811653f1acf1793e2",
        ),
        (
            SourceFormat::registered("json"),
            "0701cb2a88a98e231401e22c77a2f9c3633231e1563413dbb4b420b3d2d315fb",
            "42cf0614adbbbb0e1e8b58995e3c4d52e9d114720a19b7c7072c8f94e599571d",
        ),
    ];
    for (format, expected_file, expected_programmatic) in cases {
        let definition = CollectionDefinition::folder_with_identity(
            "items",
            "data",
            format,
            IdentityPolicy::RelativePath,
        );
        let file_hash =
            configuration_hash(std::slice::from_ref(&definition), opts, &sources).unwrap();
        let root = tempfile::tempdir().unwrap();
        let mut builder = Index::builder()
            .with_options(opts)
            .with_source_registry(sources.clone());
        let collection = builder
            .declare_collection(definition.declaration())
            .unwrap();
        builder
            .register_file_catalog(FileCatalog::new(root.path()).unwrap())
            .unwrap();
        builder
            .register_file_source(collection, definition.file_source())
            .unwrap();
        let programmatic_hash = builder
            .build()
            .unwrap()
            .view()
            .unwrap()
            .configuration_identity();
        assert_eq!(
            format!("{file_hash:02x?}").replace(['[', ']', ',', ' '], ""),
            expected_file
        );
        assert_eq!(
            format!("{programmatic_hash:02x?}").replace(['[', ']', ',', ' '], ""),
            expected_programmatic
        );
    }
}

#[test]
fn root_fixture_is_available_without_archived_workspace_paths() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/musenalm/.objekte/collections");
    assert!(root.join("agents.schema.json").is_file());
}

#[test]
fn old_prepared_hit_counter_is_ignored_when_reading_archived_json() {
    let mut archived = serde_json::to_value(WorkCounters::default()).unwrap();
    let retired_key = ["prepared", "plan", "hit"].join("_");
    archived[retired_key.as_str()] = serde_json::Value::Bool(true);
    let counters: WorkCounters = serde_json::from_value(archived).unwrap();
    assert!(
        serde_json::to_value(counters)
            .unwrap()
            .get(&retired_key)
            .is_none()
    );
}

#[test]
fn prepared_folder_configuration_identity_is_stable() {
    let root = tempfile::tempdir().unwrap();
    let index = Index::start_build(
        root.path(),
        vec![objekte_loader_json::decoded_scalar_folder("files", "")],
        RepositoryOptions::default(),
        installed_sources(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let hash = index.view().unwrap().configuration_identity();
    assert_eq!(
        format!("{hash:02x?}").replace(['[', ']', ',', ' '], ""),
        "d025ae2e90ad25b5c5fb46e60f4247bbce8bf9338613cfd17f43185db7016a93"
    );
}

#[test]
fn archived_field_without_occurrence_status_defaults_to_incomplete() {
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("record.json"), r#"{"x":"a"}"#).unwrap();
    let mut options = RepositoryOptions::default();
    options.inference.mode = objekte_search_core::inference::InferenceMode::ReportOnly;
    let index = Index::start_build(
        root.path(),
        vec![objekte_loader_json::folder("items", "")],
        options,
        installed_sources(),
    )
    .unwrap()
    .wait_complete()
    .unwrap();
    let view = index.view().unwrap();
    let field = &view.inference_report("items").unwrap().fields[0];
    let mut serialized = serde_json::to_value(field).unwrap();
    serialized
        .as_object_mut()
        .unwrap()
        .remove("occurrence_count_status");
    let restored: objekte_search_core::inference::InferredFieldReport =
        serde_json::from_value(serialized).unwrap();
    assert_eq!(
        restored.occurrence_count_status,
        OccurrenceCountStatus::IncompleteObservation
    );
}

#[test]
fn workload_option_serialization_matches_the_runtime_types() {
    assert_eq!(
        serde_json::to_string(&ReturnMode::Logical).unwrap(),
        r#""logical""#
    );
    assert_eq!(
        serde_json::to_string(&BigramAlphabet::Unicode).unwrap(),
        r#""unicode""#
    );
}
